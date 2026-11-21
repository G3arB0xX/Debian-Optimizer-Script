use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::sys::{self, CmdOutput, CommandRunner};

pub const MODULE_ID: &str = "podman";

pub const PODMAN_APPS_USER: &str = "apps";
pub const PODMAN_APPS_HOME_ABS: &str = "/home/apps";
pub const PODMAN_DATA_DIR_ABS: &str = "/home/apps/srv";
pub const PODMAN_BACKUPS_DIR_ABS: &str = "/home/apps/backups";
pub const PODMAN_GRAPHROOT_ABS: &str = "/home/apps/.local/share/containers/storage";
pub const PODMAN_LEGACY_STORAGE_ABS: &str = "/home/apps/srv/storage";

const DIAG_TARGET: &str = "debopty::apps::podman";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];

const PODMAN_BASE_PKGS: &[&str] = &[
    "podman",
    "buildah",
    "skopeo",
    "uidmap",
    "systemd-container",
    "acl",
];

const PODMAN_OPT_PKGS: &[&str] = &["fuse-overlayfs", "passt"];

const PODMAN_FISH_BASE_TEMPLATES: &[&str] = &["podman_env", "podman_abbr"];
const PODMAN_FISH_MAINTAINER_TEMPLATES: &[&str] = &["apppod", "appctl", "applog", "appshell"];

const CN_MIRROR_ENTRIES: &str = concat!(
    "[[registry.mirror]]\n",
    "location = \"docker.m.daocloud.io\"\n\n",
    "[[registry.mirror]]\n",
    "location = \"mirror.baidubce.com\"\n\n",
    "[[registry.mirror]]\n",
    "location = \"docker.nju.edu.cn\""
);

/// Options governing destructive cleanup during `uninstall_podman`.
/// Both flags default to `false` to preserve user data and the `apps` user (`U8`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PodmanUninstallOptions {
    pub delete_data: bool,
    pub delete_user: bool,
}

fn podman_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: MODULE_ID.to_string(),
        message: message.into(),
    }
}

fn maintainer_user(ctx: &AppContext) -> String {
    let initial = ctx.initial_user();
    if initial.is_empty() || initial == "root" {
        ctx.sot_user()
    } else {
        initial
    }
}

fn get_apps_uid(ctx: &AppContext, runner: &dyn CommandRunner) -> Option<String> {
    if ctx.no_sys {
        return ctx
            .passwd_entries()
            .into_iter()
            .find(|e| e.name == PODMAN_APPS_USER)
            .map(|e| e.uid.to_string());
    }
    runner
        .run("id", &["-u", PODMAN_APPS_USER], Duration::from_secs(10))
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

fn bootstrap_apps_session(ctx: &AppContext, runner: &dyn CommandRunner) -> bool {
    let Some(uid) = get_apps_uid(ctx, runner) else {
        return false;
    };
    let runtime_abs = format!("/run/user/{uid}");
    let runtime = ctx.path(&runtime_abs);

    if ctx.no_sys {
        let _ = fs::create_dir_all(&runtime);
        let _ = fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700));
        return runtime.is_dir();
    }

    let _ = runner.run(
        "loginctl",
        &["enable-linger", PODMAN_APPS_USER],
        Duration::from_secs(15),
    );
    let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    let user_unit = format!("user@{uid}.service");
    let _ = runner.run("systemctl", &["start", &user_unit], Duration::from_secs(30));

    if !runtime.is_dir() && !ctx.is_custom_root() {
        for _ in 0..3 {
            let _ = runner.run("systemctl", &["start", &user_unit], Duration::from_secs(15));
            if runtime.is_dir() {
                break;
            }
        }
    }

    if !runtime.is_dir() {
        let _ = fs::create_dir_all(ctx.path("/run/user"));
        let runtime_str = runtime.to_string_lossy().into_owned();
        let install_ok = runner
            .run(
                "install",
                &[
                    "-d",
                    "-m",
                    "0700",
                    "-o",
                    PODMAN_APPS_USER,
                    "-g",
                    PODMAN_APPS_USER,
                    &runtime_str,
                ],
                Duration::from_secs(15),
            )
            .map(|o| o.success())
            .unwrap_or(false);
        if !install_ok || !runtime.is_dir() {
            let _ = fs::create_dir_all(&runtime);
            let _ = fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700));
            if !ctx.is_custom_root() {
                let owner = format!("{PODMAN_APPS_USER}:{PODMAN_APPS_USER}");
                let _ = runner.run("chown", &[&owner, &runtime_str], Duration::from_secs(10));
            }
        }
    }

    let bus_path = runtime.join("bus");
    if runtime.is_dir() && !bus_path.exists() {
        let _ = sys::safe_apt_install(&["dbus-user-session"], runner);
        let addr_arg = format!("--address=unix:path={}", bus_path.to_string_lossy());
        let _ = runner.run(
            "sudo",
            &[
                "-u",
                PODMAN_APPS_USER,
                "dbus-daemon",
                "--session",
                &addr_arg,
                "--fork",
            ],
            Duration::from_secs(15),
        );
    }

    runtime.is_dir()
}

fn apps_exec(ctx: &AppContext, args: &[&str], runner: &dyn CommandRunner) -> Result<CmdOutput> {
    let uid = get_apps_uid(ctx, runner)
        .ok_or_else(|| podman_err("apps user does not exist; install podman first"))?;
    let runtime_abs = format!("/run/user/{uid}");
    let _ = bootstrap_apps_session(ctx, runner);
    let xdg_arg = format!("XDG_RUNTIME_DIR={runtime_abs}");

    let mut sudo_args: Vec<&str> = Vec::with_capacity(3 + args.len());
    sudo_args.push("-u");
    sudo_args.push(PODMAN_APPS_USER);
    sudo_args.push(&xdg_arg);
    sudo_args.extend_from_slice(args);
    runner.run("sudo", &sudo_args, Duration::from_secs(30))
}

/// Detect whether the installed Podman major version is `>= 5` (required for `database_backend = "sqlite"`).
pub fn supports_sqlite_backend(runner: &dyn CommandRunner) -> bool {
    let ver = runner
        .run(
            "podman",
            &["version", "--format", "{{.Version}}"],
            Duration::from_secs(10),
        )
        .ok()
        .filter(|o| o.success() && !o.stdout.trim().is_empty())
        .map(|o| o.stdout.trim().to_string())
        .or_else(|| {
            runner
                .run("podman", &["--version"], Duration::from_secs(10))
                .ok()
                .filter(|o| o.success())
                .and_then(|o| o.stdout.split_whitespace().nth(2).map(ToString::to_string))
        })
        .unwrap_or_default();

    shared::compare_dot_version_ge(&ver, "5")
}

fn remove_path_if_exists(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || meta.is_file() {
            let _ = fs::remove_file(path);
        } else if meta.is_dir() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn remove_conflicting_docker(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) {
    let docker_cmd = ctx.is_cmd_present("docker", runner);
    let docker_pkg = !ctx.no_sys
        && (sys::is_dpkg_installed("docker-ce", runner)
            || runner
                .run("dpkg", &["-s", "docker-ce"], Duration::from_secs(10))
                .map(|o| o.success())
                .unwrap_or(false));

    if !docker_cmd && !docker_pkg {
        return;
    }

    diag::log_event(
        LogLevel::Warn,
        DIAG_TARGET,
        None,
        "detected Docker installation; removing to avoid conflict with Podman",
        None,
        &[],
    );
    warnings.push("检测到 Docker 环境，已自动移除以避免与 Podman 冲突。".to_string());

    if !ctx.no_sys {
        let _ = runner.run(
            "systemctl",
            &["stop", "docker", "docker.socket", "containerd"],
            Duration::from_secs(30),
        );
        let _ = runner.run_with_env(
            "apt-get",
            &[
                "purge",
                "-yq",
                "docker-ce",
                "docker-ce-cli",
                "containerd.io",
                "docker-buildx-plugin",
                "docker-compose-plugin",
                "docker.io",
                "docker-doc",
            ],
            APT_ENV,
            Duration::from_secs(180),
        );
        let _ = runner.run_with_env(
            "apt-get",
            &["autoremove", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
    }

    remove_path_if_exists(&ctx.path("/etc/docker"));
    remove_path_if_exists(&ctx.path("/var/run/docker.sock"));
    remove_path_if_exists(&ctx.path("/usr/local/bin/docker-compose"));
}

fn apt_pkg_available(pkg: &str, runner: &dyn CommandRunner) -> bool {
    runner
        .run("apt-cache", &["show", pkg], Duration::from_secs(15))
        .map(|o| o.success())
        .unwrap_or(false)
}

fn install_podman_packages(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<()> {
    if ctx.no_sys {
        return Ok(());
    }

    let _ = runner.run_with_env(
        "apt-get",
        &["update", "-yq"],
        APT_ENV,
        Duration::from_secs(120),
    );

    let mut pkgs: Vec<&str> = PODMAN_BASE_PKGS.to_vec();
    if apt_pkg_available("podman-docker", runner) {
        pkgs.push("podman-docker");
    } else {
        warnings
            .push("当前 Debian 版本无 podman-docker 包，将手动创建 docker 兼容软链。".to_string());
    }
    if apt_pkg_available("podman-compose", runner) {
        pkgs.push("podman-compose");
    }

    if let Err(first_err) = sys::safe_apt_install(&pkgs, runner) {
        diag::log_event(
            LogLevel::Warn,
            DIAG_TARGET,
            None,
            "APT install failed; retrying with --fix-missing",
            None,
            &[("error", &first_err.to_string())],
        );
        let _ = runner.run_with_env(
            "apt-get",
            &["update", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
        let mut retry_args = vec!["install", "-yq", "--fix-missing"];
        retry_args.extend_from_slice(&pkgs);
        let out = runner.run_with_env("apt-get", &retry_args, APT_ENV, Duration::from_secs(300))?;
        if !out.success() {
            return Err(podman_err(format!(
                "apt-get install --fix-missing failed (code {}): {}",
                out.code,
                out.stderr.trim()
            )));
        }
    }

    if !ctx.is_cmd_present("docker", runner) && ctx.is_cmd_present("podman", runner) {
        let podman_bin = resolve_cmd_bin_path(ctx, "podman");
        let docker_link = ctx.path("/usr/local/bin/docker");
        if let Some(parent) = docker_link.parent() {
            let _ = fs::create_dir_all(parent);
        }
        remove_path_if_exists(&docker_link);
        let _ = symlink(&podman_bin, &docker_link);
    }

    if !ctx.is_cmd_present("podman-compose", runner) {
        let _ = sys::safe_apt_install(&["python3-pip", "python3-yaml"], runner);
        let pip_ok = runner
            .run(
                "pip3",
                &["install", "--break-system-packages", "podman-compose"],
                Duration::from_secs(180),
            )
            .map(|o| o.success())
            .unwrap_or(false)
            || runner
                .run(
                    "pip3",
                    &["install", "podman-compose"],
                    Duration::from_secs(180),
                )
                .map(|o| o.success())
                .unwrap_or(false);
        if !pip_ok {
            warnings.push("podman-compose 未安装，Compose 编排需手动补齐。".to_string());
        }
    }

    for &opt_pkg in PODMAN_OPT_PKGS {
        if apt_pkg_available(opt_pkg, runner) && sys::safe_apt_install(&[opt_pkg], runner).is_err()
        {
            warnings.push(format!("{opt_pkg} 安装失败，已跳过。"));
        }
    }

    Ok(())
}

fn resolve_cmd_bin_path(ctx: &AppContext, cmd: &str) -> PathBuf {
    for dir in ["/usr/local/bin", "/usr/bin", "/usr/sbin", "/bin"] {
        let abs = format!("{dir}/{cmd}");
        let candidate = ctx.path(&abs);
        if candidate.is_file() {
            return if ctx.is_custom_root() {
                candidate
            } else {
                PathBuf::from(abs)
            };
        }
    }
    PathBuf::from(format!("/usr/bin/{cmd}"))
}

fn ensure_layout(ctx: &AppContext) -> Result<()> {
    let apps_home = ctx.path(PODMAN_APPS_HOME_ABS);
    fs::create_dir_all(ctx.path(PODMAN_DATA_DIR_ABS))?;
    fs::create_dir_all(ctx.path(PODMAN_BACKUPS_DIR_ABS))?;
    fs::create_dir_all(apps_home.join(".config/containers/registries.conf.d"))?;
    fs::create_dir_all(apps_home.join(".config/containers/systemd"))?;
    fs::create_dir_all(apps_home.join(".config/systemd/user"))?;
    fs::create_dir_all(apps_home.join(".local/bin"))?;
    if let Some(parent) = ctx.path(PODMAN_GRAPHROOT_ABS).parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn file_has_user_prefix(path: &Path, user: &str) -> bool {
    let prefix = format!("{user}:");
    fs::read_to_string(path)
        .map(|txt| txt.lines().any(|l| l.starts_with(&prefix)))
        .unwrap_or(false)
}

fn remove_user_prefix_lines(path: &Path, user: &str) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let existing = fs::read_to_string(path)?;
    let prefix = format!("{user}:");
    let filtered: Vec<&str> = existing
        .lines()
        .filter(|line| !line.starts_with(&prefix))
        .collect();
    let mut rendered = filtered.join("\n");
    if !rendered.is_empty() {
        rendered.push('\n');
    }
    sys::write_file_atomic(path, rendered.as_bytes(), None)
}

fn create_apps_user(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<()> {
    let apps_shell = if ctx.is_cmd_present("fish", runner) {
        if ctx.path("/usr/bin/fish").is_file() {
            "/usr/bin/fish"
        } else if ctx.path("/usr/local/bin/fish").is_file() {
            "/usr/local/bin/fish"
        } else {
            "/usr/bin/fish"
        }
    } else {
        "/bin/bash"
    };

    if !ctx.no_sys {
        let exists = get_apps_uid(ctx, runner).is_some();
        if !exists {
            diag::log_event(
                LogLevel::Info,
                DIAG_TARGET,
                None,
                "creating dedicated rootless apps user",
                None,
                &[("user", PODMAN_APPS_USER), ("shell", apps_shell)],
            );
            let out = runner.run(
                "useradd",
                &[
                    "-m",
                    "-s",
                    apps_shell,
                    "-d",
                    PODMAN_APPS_HOME_ABS,
                    PODMAN_APPS_USER,
                ],
                Duration::from_secs(30),
            )?;
            if !out.success() {
                return Err(podman_err(format!(
                    "useradd failed for '{PODMAN_APPS_USER}' (code {}): {}",
                    out.code,
                    out.stderr.trim()
                )));
            }
        } else {
            let _ = runner.run(
                "usermod",
                &["-s", apps_shell, PODMAN_APPS_USER],
                Duration::from_secs(15),
            );
        }

        let _ = runner.run("passwd", &["-l", PODMAN_APPS_USER], Duration::from_secs(15));

        if !file_has_user_prefix(&ctx.path("/etc/subuid"), PODMAN_APPS_USER) {
            let out = runner.run(
                "usermod",
                &["--add-subuids", "100000-165535", PODMAN_APPS_USER],
                Duration::from_secs(15),
            )?;
            if !out.success() {
                return Err(podman_err(format!(
                    "usermod --add-subuids failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                )));
            }
        }
        if !file_has_user_prefix(&ctx.path("/etc/subgid"), PODMAN_APPS_USER) {
            let out = runner.run(
                "usermod",
                &["--add-subgids", "100000-165535", PODMAN_APPS_USER],
                Duration::from_secs(15),
            )?;
            if !out.success() {
                return Err(podman_err(format!(
                    "usermod --add-subgids failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                )));
            }
        }

        let _ = runner.run(
            "loginctl",
            &["enable-linger", PODMAN_APPS_USER],
            Duration::from_secs(15),
        );
        let _ = bootstrap_apps_session(ctx, runner);
    }

    ensure_layout(ctx)?;

    if !ctx.no_sys && !ctx.is_custom_root() {
        let owner = format!("{PODMAN_APPS_USER}:{PODMAN_APPS_USER}");
        let _ = runner.run(
            "chown",
            &["-R", &owner, PODMAN_APPS_HOME_ABS],
            Duration::from_secs(30),
        );
    }

    if let Ok(true) = shared::add_user_to_certs_group(ctx, PODMAN_APPS_USER, runner)
        && !ctx.no_sys
    {
        let _ = runner.run(
            "loginctl",
            &["terminate-user", PODMAN_APPS_USER],
            Duration::from_secs(15),
        );
        let _ = bootstrap_apps_session(ctx, runner);
    }

    Ok(())
}

/// Scrub ACLs on Podman graphroot and legacy storage directories (`20 §5` red-line).
pub fn scrub_graphroot_acl(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) {
    let graphroot = ctx.path(PODMAN_GRAPHROOT_ABS);
    let legacy = ctx.path(PODMAN_LEGACY_STORAGE_ABS);

    for dir in [&graphroot, &legacy] {
        if !dir.is_dir() {
            continue;
        }
        let dir_str = dir.to_string_lossy().into_owned();
        if !ctx.no_sys {
            let _ = runner.run("setfacl", &["-R", "-b", &dir_str], Duration::from_secs(30));
            let _ = runner.run("setfacl", &["-k", &dir_str], Duration::from_secs(30));
        }
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        if !ctx.no_sys && !ctx.is_custom_root() {
            let owner = format!("{PODMAN_APPS_USER}:{PODMAN_APPS_USER}");
            let _ = runner.run("chown", &["-R", &owner, &dir_str], Duration::from_secs(30));
        }
    }

    if legacy.is_dir() {
        warnings.push(format!(
            "检测到旧版 graphroot 目录 {}，已清除 ACL；若已迁移至默认路径可手动删除该目录。",
            PODMAN_LEGACY_STORAGE_ABS
        ));
    }
}

/// Layered ACL red-line (`20 §5`): scrub graphroot, grant `--x` on `/home/apps`,
/// and recursive `rwx` ONLY on `srv`, `backups`, and `.config`.
pub fn apply_maintainer_acl(
    ctx: &AppContext,
    maintainer: &str,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) {
    scrub_graphroot_acl(ctx, runner, warnings);

    if maintainer.is_empty() || maintainer == "root" || ctx.no_sys {
        return;
    }

    let _ = runner.run(
        "usermod",
        &["-aG", PODMAN_APPS_USER, maintainer],
        Duration::from_secs(15),
    );

    let apps_home = ctx.path(PODMAN_APPS_HOME_ABS);
    let traverse_spec = format!("u:{maintainer}:--x");
    let _ = runner.run(
        "setfacl",
        &["-m", &traverse_spec, &apps_home.to_string_lossy()],
        Duration::from_secs(15),
    );

    let rwx_spec = format!("u:{maintainer}:rwx");
    for scoped_dir in [
        ctx.path(PODMAN_DATA_DIR_ABS),
        ctx.path(PODMAN_BACKUPS_DIR_ABS),
        apps_home.join(".config"),
    ] {
        if !scoped_dir.is_dir() {
            continue;
        }
        let d = scoped_dir.to_string_lossy().into_owned();
        let _ = runner.run(
            "setfacl",
            &["-R", "-m", &rwx_spec, &d],
            Duration::from_secs(30),
        );
        let _ = runner.run(
            "setfacl",
            &["-R", "-d", "-m", &rwx_spec, &d],
            Duration::from_secs(30),
        );
    }
}

fn configure_apps(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    let engine = ctx.template_engine();
    let apps_home = ctx.path(PODMAN_APPS_HOME_ABS);
    let empty_vars = BTreeMap::new();

    let sqlite_supported = supports_sqlite_backend(runner);
    let sqlite_backend = if sqlite_supported {
        "database_backend = \"sqlite\""
    } else {
        warnings.push("当前 Podman 版本不支持 database_backend=sqlite，已跳过该项。".to_string());
        "# database_backend 需 Podman 5.x+，当前版本已跳过"
    };

    let fuse_section = if ctx.is_cmd_present("fuse-overlayfs", runner) {
        "[storage.options.overlay]\nmount_program = \"/usr/bin/fuse-overlayfs\""
    } else {
        "# fuse-overlayfs 未安装，使用内核原生 overlay"
    };

    let mut containers_vars = BTreeMap::new();
    containers_vars.insert("SQLITE_BACKEND".to_string(), sqlite_backend.to_string());
    engine.render_to_file(
        "templates/apps/podman/containers.conf",
        &apps_home.join(".config/containers/containers.conf"),
        &containers_vars,
        Some(0o644),
    )?;

    let mut storage_vars = BTreeMap::new();
    storage_vars.insert("FUSE_OVERLAY_SECTION".to_string(), fuse_section.to_string());
    engine.render_to_file(
        "templates/apps/podman/storage.conf",
        &apps_home.join(".config/containers/storage.conf"),
        &storage_vars,
        Some(0o644),
    )?;

    engine.render_to_file(
        "templates/apps/podman/restic-excludes.txt",
        &apps_home.join(".config/restic-excludes"),
        &empty_vars,
        Some(0o644),
    )?;

    engine.render_to_file(
        "templates/apps/podman/registries.conf.d/000-unqualified-search.conf",
        &apps_home.join(".config/containers/registries.conf.d/000-unqualified-search.conf"),
        &empty_vars,
        Some(0o644),
    )?;

    let mirrors_path = apps_home.join(".config/containers/registries.conf.d/001-mirrors.conf");
    if ctx.effective_is_cn() {
        let mut mirror_vars = BTreeMap::new();
        mirror_vars.insert("MIRROR_ENTRIES".to_string(), CN_MIRROR_ENTRIES.to_string());
        engine.render_to_file(
            "templates/apps/podman/registries.conf.d/001-mirrors.conf",
            &mirrors_path,
            &mirror_vars,
            Some(0o644),
        )?;
    } else {
        remove_path_if_exists(&mirrors_path);
    }

    engine.render_to_file(
        "templates/apps/podman/quadlet/hello.container.example",
        &apps_home.join(".config/containers/systemd/hello.container.example"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/podman/quadlet/tls-ferron-certs.container.example",
        &apps_home.join(".config/containers/systemd/tls-ferron-certs.container.example"),
        &empty_vars,
        Some(0o644),
    )?;

    engine.render_to_file(
        "templates/apps/podman/systemd-user/podman-gc.sh",
        &apps_home.join(".local/bin/podman-gc.sh"),
        &empty_vars,
        Some(0o755),
    )?;
    engine.render_to_file(
        "templates/apps/podman/systemd-user/podman-gc.service",
        &apps_home.join(".config/systemd/user/podman-gc.service"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/podman/systemd-user/podman-gc.timer",
        &apps_home.join(".config/systemd/user/podman-gc.timer"),
        &empty_vars,
        Some(0o644),
    )?;

    if !ctx.no_sys && !ctx.is_custom_root() {
        let owner = format!("{PODMAN_APPS_USER}:{PODMAN_APPS_USER}");
        let cfg_dir = apps_home.join(".config").to_string_lossy().into_owned();
        let local_dir = apps_home.join(".local").to_string_lossy().into_owned();
        let _ = runner.run(
            "chown",
            &["-R", &owner, &cfg_dir, &local_dir],
            Duration::from_secs(30),
        );
    }

    let maintainer = maintainer_user(ctx);
    let sudoers_path = ctx.path("/etc/sudoers.d/debopti-podman");
    let mut sudoers_vars = BTreeMap::new();
    sudoers_vars.insert("MAINTAINER_USER".to_string(), maintainer.clone());
    engine.render_to_file(
        "templates/apps/podman/sudoers.d/debopti-podman",
        &sudoers_path,
        &sudoers_vars,
        Some(0o440),
    )?;

    if !ctx.no_sys {
        let sudoers_str = sudoers_path.to_string_lossy().into_owned();
        let visudo_res = runner.run("visudo", &["-cf", &sudoers_str], Duration::from_secs(15));
        let valid = matches!(visudo_res, Ok(ref o) if o.success());
        if !valid {
            remove_path_if_exists(&sudoers_path);
            let detail = match visudo_res {
                Ok(o) => format!("visudo -cf failed (code {}): {}", o.code, o.stderr.trim()),
                Err(e) => format!("visudo execution failed: {e}"),
            };
            diag::log_event(
                LogLevel::Error,
                DIAG_TARGET,
                Some("E4004"),
                "sudoers validation failed; rolled back /etc/sudoers.d/debopti-podman",
                None,
                &[("detail", &detail)],
            );
            return Err(podman_err(format!(
                "sudoers validation failed and rolled back: {detail}"
            )));
        }
    }

    let sysctl_path = ctx.path("/etc/sysctl.d/99-debopti-podman.conf");
    engine.render_to_file(
        "templates/apps/podman/sysctl/99-debopti-podman.conf",
        &sysctl_path,
        &empty_vars,
        Some(0o644),
    )?;
    if !ctx.no_sys {
        let sysctl_ok = runner
            .run("sysctl", &["--system"], Duration::from_secs(30))
            .map(|o| o.success())
            .unwrap_or(false);
        if !sysctl_ok {
            let p = sysctl_path.to_string_lossy().into_owned();
            let _ = runner.run("sysctl", &["-p", &p], Duration::from_secs(15));
        }
    }

    apply_maintainer_acl(ctx, &maintainer, runner, warnings);
    Ok(sqlite_supported)
}

fn deploy_shell_helpers(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<()> {
    let engine = ctx.template_engine();
    let sot_user = ctx.sot_user();
    let mut profile_vars = BTreeMap::new();
    profile_vars.insert("SOT_USER".to_string(), sot_user.clone());

    engine.render_to_file(
        "templates/apps/podman/profile.d/debopti-podman.sh",
        &ctx.path("/etc/profile.d/debopti-podman.sh"),
        &profile_vars,
        Some(0o644),
    )?;

    if !ctx.is_cmd_present("fish", runner) {
        return Ok(());
    }

    let sot_home = ctx.user_home(&sot_user);
    let fish_conf_d = sot_home.join(".config/fish/conf.d");
    fs::create_dir_all(&fish_conf_d)?;
    let empty_vars = BTreeMap::new();

    for tpl in PODMAN_FISH_BASE_TEMPLATES
        .iter()
        .chain(PODMAN_FISH_MAINTAINER_TEMPLATES.iter())
    {
        let rel = format!("templates/apps/podman/fish/{tpl}.fish");
        let dest = fish_conf_d.join(format!("debopti_{tpl}.fish"));
        engine.render_to_file(&rel, &dest, &empty_vars, Some(0o644))?;
    }

    let _ = shared::sync_devops_sot_links(ctx, true, runner)?;
    Ok(())
}

fn remove_shell_helpers(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<()> {
    remove_path_if_exists(&ctx.path("/etc/profile.d/debopti-podman.sh"));

    let sot_user = ctx.sot_user();
    let fish_conf_d = ctx.user_home(&sot_user).join(".config/fish/conf.d");
    for tpl in PODMAN_FISH_BASE_TEMPLATES
        .iter()
        .chain(PODMAN_FISH_MAINTAINER_TEMPLATES.iter())
    {
        remove_path_if_exists(&fish_conf_d.join(format!("debopti_{tpl}.fish")));
    }
    let _ = shared::sync_devops_sot_links(ctx, true, runner)?;
    Ok(())
}

fn enable_user_services(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if ctx.no_sys {
        ctx.config_store().set("PODMAN_GC_ENABLED", "false")?;
        return Ok(false);
    }

    if !bootstrap_apps_session(ctx, runner) {
        warnings.push("apps 用户 systemd 会话引导失败，用户级服务可能无法启动。".to_string());
        ctx.config_store().set("PODMAN_GC_ENABLED", "false")?;
        return Ok(false);
    }

    let reload_ok = apps_exec(ctx, &["systemctl", "--user", "daemon-reload"], runner)
        .map(|o| o.success())
        .unwrap_or(false);
    if !reload_ok {
        warnings.push(
            "systemctl --user 当前不可用（需完整 systemd 登录会话），定时器单元已部署。"
                .to_string(),
        );
        ctx.config_store().set("PODMAN_GC_ENABLED", "false")?;
        return Ok(false);
    }

    let _ = apps_exec(
        ctx,
        &[
            "systemctl",
            "--user",
            "enable",
            "--now",
            "podman-auto-update.timer",
        ],
        runner,
    );
    let _ = apps_exec(
        ctx,
        &["systemctl", "--user", "enable", "--now", "podman-gc.timer"],
        runner,
    );
    ctx.config_store().set("PODMAN_GC_ENABLED", "true")?;
    Ok(true)
}

/// Probe whether `podman-gc.timer` is enabled for the `apps` user.
pub fn probe_podman_gc_enabled(ctx: &AppContext, runner: &dyn CommandRunner) -> bool {
    if get_apps_uid(ctx, runner).is_none() {
        return false;
    }
    if ctx.no_sys {
        return ctx
            .config_store()
            .get("PODMAN_GC_ENABLED")
            .ok()
            .flatten()
            .is_some_and(|v| v.trim() == "true");
    }
    apps_exec(
        ctx,
        &["systemctl", "--user", "is-enabled", "podman-gc.timer"],
        runner,
    )
    .map(|o| o.success())
    .unwrap_or(false)
}

/// Deploy the rootless Podman environment under the dedicated `apps` user.
pub fn install_podman(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "deploying rootless Podman environment",
        None,
        &[("apps_user", PODMAN_APPS_USER)],
    );

    let mut warnings = Vec::new();
    remove_conflicting_docker(ctx, runner, &mut warnings);
    install_podman_packages(ctx, runner, &mut warnings)?;
    create_apps_user(ctx, runner)?;
    let sqlite_enabled = configure_apps(ctx, runner, &mut warnings)?;
    deploy_shell_helpers(ctx, runner)?;
    let gc_enabled = enable_user_services(ctx, runner, &mut warnings)?;

    if !ctx.no_sys && !ctx.is_cmd_present("podman", runner) {
        return Err(podman_err(
            "podman binary not found after installation; check APT repositories",
        ));
    }

    let mut report = AppReport::new(
        MODULE_ID,
        "install",
        true,
        "Podman rootless container environment deployed",
    )
    .with_detail("apps_user", PODMAN_APPS_USER)
    .with_detail("sqlite_backend", sqlite_enabled.to_string())
    .with_detail("gc_timer_enabled", gc_enabled.to_string())
    .with_detail("cn_mirrors", ctx.effective_is_cn().to_string());

    for w in warnings {
        report = report.with_warning(w);
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "Podman rootless deployment completed",
        None,
        &[
            ("sqlite", &sqlite_enabled.to_string()),
            ("gc_enabled", &gc_enabled.to_string()),
        ],
    );
    Ok(report)
}

/// Uninstall Podman while strictly preserving `/home/apps/srv` and the `apps` user by default (`U8`).
pub fn uninstall_podman(
    ctx: &AppContext,
    opts: &PodmanUninstallOptions,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Podman environment",
        None,
        &[
            ("delete_data", &opts.delete_data.to_string()),
            ("delete_user", &opts.delete_user.to_string()),
        ],
    );

    if !ctx.no_sys && get_apps_uid(ctx, runner).is_some() {
        let _ = apps_exec(
            ctx,
            &[
                "systemctl",
                "--user",
                "stop",
                "podman-gc.timer",
                "podman-auto-update.timer",
            ],
            runner,
        );
        let _ = apps_exec(
            ctx,
            &[
                "systemctl",
                "--user",
                "disable",
                "podman-gc.timer",
                "podman-auto-update.timer",
            ],
            runner,
        );
        let _ = runner.run(
            "loginctl",
            &["disable-linger", PODMAN_APPS_USER],
            Duration::from_secs(15),
        );
    }

    remove_shell_helpers(ctx, runner)?;

    let maintainer = maintainer_user(ctx);
    if !ctx.no_sys && !maintainer.is_empty() && maintainer != "root" {
        let _ = runner.run(
            "gpasswd",
            &["-d", &maintainer, PODMAN_APPS_USER],
            Duration::from_secs(15),
        );
    }

    remove_path_if_exists(&ctx.path("/etc/sudoers.d/debopti-podman"));
    remove_path_if_exists(&ctx.path("/etc/sysctl.d/99-debopti-podman.conf"));

    if !ctx.no_sys {
        let _ = runner.run("sysctl", &["--system"], Duration::from_secs(30));
        let _ = runner.run_with_env(
            "apt-get",
            &[
                "purge",
                "-yq",
                "podman",
                "podman-docker",
                "podman-compose",
                "buildah",
                "skopeo",
                "uidmap",
                "fuse-overlayfs",
                "passt",
            ],
            APT_ENV,
            Duration::from_secs(180),
        );
        let _ = runner.run_with_env(
            "apt-get",
            &["autoremove", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
    }

    let mut report = AppReport::new(MODULE_ID, "uninstall", true, "Podman uninstalled");

    let data_dir = ctx.path(PODMAN_DATA_DIR_ABS);
    if opts.delete_data {
        remove_path_if_exists(&data_dir);
        report = report.with_detail("data_deleted", "true");
    } else {
        report = report
            .with_detail("data_deleted", "false")
            .with_preserved(format!(
                "preserved Podman data directory {PODMAN_DATA_DIR_ABS}"
            ));
    }

    if opts.delete_user {
        if !ctx.no_sys {
            let del_r = runner
                .run(
                    "userdel",
                    &["-r", PODMAN_APPS_USER],
                    Duration::from_secs(30),
                )
                .map(|o| o.success())
                .unwrap_or(false);
            if !del_r {
                let _ = runner.run("userdel", &[PODMAN_APPS_USER], Duration::from_secs(30));
            }
        }
        remove_user_prefix_lines(&ctx.path("/etc/subuid"), PODMAN_APPS_USER)?;
        remove_user_prefix_lines(&ctx.path("/etc/subgid"), PODMAN_APPS_USER)?;
        report = report.with_detail("user_deleted", "true");
    } else {
        report = report
            .with_detail("user_deleted", "false")
            .with_preserved(format!("preserved dedicated user '{PODMAN_APPS_USER}'"));
    }

    ctx.config_store().set("PODMAN_GC_ENABLED", "false")?;
    Ok(report)
}

/// Toggle the weekly `podman-gc.timer` user service for `apps` and persist `PODMAN_GC_ENABLED`.
pub fn toggle_podman_gc(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    if get_apps_uid(ctx, runner).is_none() {
        return Err(podman_err(
            "Podman apps user is not present; install Podman before toggling GC timer",
        ));
    }

    let currently_enabled = probe_podman_gc_enabled(ctx, runner);
    let next_enabled = !currently_enabled;

    if !ctx.no_sys {
        let action = if next_enabled { "enable" } else { "disable" };
        let out = apps_exec(
            ctx,
            &["systemctl", "--user", action, "--now", "podman-gc.timer"],
            runner,
        )?;
        if !out.success() {
            return Err(podman_err(format!(
                "systemctl --user {action} --now podman-gc.timer failed (code {}): {}",
                out.code,
                out.stderr.trim()
            )));
        }
    }

    let state_str = if next_enabled { "true" } else { "false" };
    ctx.config_store().set("PODMAN_GC_ENABLED", state_str)?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "toggled podman-gc.timer",
        None,
        &[("enabled", state_str)],
    );

    Ok(AppReport::new(
        MODULE_ID,
        "toggle-gc",
        true,
        if next_enabled {
            "Podman GC timer enabled"
        } else {
            "Podman GC timer disabled"
        },
    )
    .with_detail("podman_gc_enabled", state_str))
}
