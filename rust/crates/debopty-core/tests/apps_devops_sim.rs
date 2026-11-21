use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use debopty_core::apps::devops::cli_deps::{
    ensure_0xproto_nerd_font, ensure_bat, ensure_fd, ensure_fzf, ensure_resvg,
};
use debopty_core::apps::devops::fish::{clean_non_sot_fish_dir, install_fish, uninstall_fish};
use debopty_core::apps::devops::lego::{
    LegoDomainSpec, LegoFlag, add_lego_domain, build_lego_troubleshoot_cmd, install_lego,
    list_lego_domains, parse_openssl_enddate_epoch, remove_lego_domain, toggle_lego_flag,
    validate_primary_domain, validate_san_domain,
};
use debopty_core::apps::devops::micro::{
    install_micro, patch_micro_omni_session_lua, uninstall_micro,
};
use debopty_core::apps::devops::yazi::{
    YaziInstallOptions, inject_yazi_bashrc, install_yazi, remove_yazi_bashrc, uninstall_yazi,
};
use debopty_core::apps::podman::{
    PODMAN_APPS_HOME_ABS, PODMAN_BACKUPS_DIR_ABS, PODMAN_DATA_DIR_ABS, PODMAN_GRAPHROOT_ABS,
    PODMAN_LEGACY_STORAGE_ABS, PodmanUninstallOptions, apply_maintainer_acl, install_podman,
    probe_podman_gc_enabled, toggle_podman_gc, uninstall_podman,
};
use debopty_core::apps::shared::{self, AppContext};
use debopty_core::error::{CoreError, Result};
use debopty_core::network::{HttpResponse, MockHttpClient};
use debopty_core::sys::{self, CmdOutput, CommandRunner, MockCommandRunner};
use tempfile::TempDir;

fn ok_out(stdout: &str) -> CmdOutput {
    CmdOutput {
        code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn err_out(code: i32, stderr: &str) -> CmdOutput {
    CmdOutput {
        code,
        stdout: String::new(),
        stderr: stderr.to_string(),
        timed_out: false,
    }
}

fn make_executable(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn setup_passwd(ctx: &AppContext, users: &[(&str, u32, &str, &str)]) {
    let mut content = String::new();
    for &(name, uid, home, shell) in users {
        content.push_str(&format!("{name}:x:{uid}:{uid}::{home}:{shell}\n"));
        fs::create_dir_all(ctx.path(home)).unwrap();
    }
    sys::write_file_atomic(&ctx.path("/etc/passwd"), content.as_bytes(), Some(0o644)).unwrap();
}

type ExtractedFileList = Vec<(String, Vec<u8>)>;

/// Test runner wrapping `MockCommandRunner` that materializes dummy extracted files
/// whenever `tar` or `unzip` extraction is invoked, and supports dynamic per-call handlers.
#[derive(Default)]
struct ArchiveMockRunner {
    inner: MockCommandRunner,
    extract_files: Mutex<HashMap<String, ExtractedFileList>>,
    fail_first_apt_install: Mutex<bool>,
}

impl ArchiveMockRunner {
    fn new() -> Self {
        Self::default()
    }

    fn on_extract_contains(&self, needle: &str, files: Vec<(&str, &[u8])>) {
        self.extract_files.lock().unwrap().insert(
            needle.to_string(),
            files
                .into_iter()
                .map(|(p, b)| (p.to_string(), b.to_vec()))
                .collect(),
        );
    }

    fn fail_first_apt(&self) {
        *self.fail_first_apt_install.lock().unwrap() = true;
    }

    fn materialize_for(&self, archive_arg: &str, dest_dir: &str) {
        let map = self.extract_files.lock().unwrap();
        for (needle, files) in map.iter() {
            if archive_arg.contains(needle) {
                for (rel, content) in files {
                    let full = PathBuf::from(dest_dir).join(rel);
                    if let Some(parent) = full.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = fs::write(&full, content);
                    let _ = fs::set_permissions(&full, fs::Permissions::from_mode(0o755));
                }
            }
        }
    }
}

impl CommandRunner for ArchiveMockRunner {
    fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput> {
        if (prog == "tar" && args.len() >= 4 && args[2] == "-C")
            || (prog == "unzip" && args.len() >= 4 && args[2] == "-d")
        {
            self.materialize_for(args[1], args[3]);
        } else if prog == "apt-get"
            && args.first().copied() == Some("install")
            && !args.contains(&"--fix-missing")
        {
            let mut flag = self.fail_first_apt_install.lock().unwrap();
            if *flag {
                *flag = false;
                let _ = self.inner.run(prog, args, timeout);
                return Ok(err_out(100, "E: Failed to fetch package"));
            }
        }
        self.inner.run(prog, args, timeout)
    }

    fn run_with_env(
        &self,
        prog: &str,
        args: &[&str],
        env: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<CmdOutput> {
        if prog == "apt-get"
            && args.first().copied() == Some("install")
            && !args.contains(&"--fix-missing")
        {
            let mut flag = self.fail_first_apt_install.lock().unwrap();
            if *flag {
                *flag = false;
                let _ = self.inner.run_with_env(prog, args, env, timeout);
                return Ok(err_out(100, "E: Failed to fetch package"));
            }
        }
        self.inner.run_with_env(prog, args, env, timeout)
    }
}

fn wire_podman_happy_runner(runner: &MockCommandRunner, podman_ver: &str) {
    runner.on_prog("apt-get", ok_out(""));
    runner.on_prog("apt-cache", ok_out("Package: mock\n"));
    runner.on_prog("dpkg-query", ok_out("install ok installed\n"));
    runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "docker-ce"],
        err_out(1, ""),
    );
    runner.on_exact("dpkg", &["-s", "docker-ce"], err_out(1, ""));
    runner.on_exact("id", &["-u", "apps"], ok_out("1001\n"));
    runner.on_exact("id", &["-nG", "apps"], ok_out("apps\n"));
    runner.on_prog("getent", ok_out("debopti-certs:x:999:\n"));
    runner.on_prog("useradd", ok_out(""));
    runner.on_prog("usermod", ok_out(""));
    runner.on_prog("passwd", ok_out(""));
    runner.on_prog("loginctl", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("install", ok_out(""));
    runner.on_prog("sudo", ok_out(""));
    runner.on_prog("visudo", ok_out("parsed OK\n"));
    runner.on_prog("sysctl", ok_out(""));
    runner.on_prog("setfacl", ok_out(""));
    runner.on_prog("gpasswd", ok_out(""));
    runner.on_prog("userdel", ok_out(""));
    runner.on_exact(
        "podman",
        &["version", "--format", "{{.Version}}"],
        ok_out(&format!("{podman_ver}\n")),
    );
}

// ==================== PODMAN TESTS (1..9) ====================

#[test]
fn podman_install_5x_enables_sqlite_and_never_sets_active_service() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.sudo_user_override = Some("alice".to_string());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/usr/bin/fish"),
        ],
    );
    make_executable(&ctx.path("/usr/bin/podman"));
    make_executable(&ctx.path("/usr/bin/podman-compose"));
    make_executable(&ctx.path("/usr/bin/fuse-overlayfs"));
    make_executable(&ctx.path("/usr/bin/fish"));
    fs::create_dir_all(ctx.path("/home/alice/.config/fish/conf.d")).unwrap();

    let runner = MockCommandRunner::new();
    wire_podman_happy_runner(&runner, "5.2.0");

    let report = install_podman(&ctx, &runner).unwrap();
    assert_eq!(report.app, "podman");
    assert_eq!(
        report.details.get("sqlite_backend").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        report.details.get("gc_timer_enabled").map(String::as_str),
        Some("true")
    );

    let containers_conf =
        fs::read_to_string(ctx.path("/home/apps/.config/containers/containers.conf")).unwrap();
    assert!(containers_conf.contains("database_backend = \"sqlite\""));
    assert!(!containers_conf.contains("active_service=true"));
    assert!(!containers_conf.contains("active_service = true"));

    let storage_conf =
        fs::read_to_string(ctx.path("/home/apps/.config/containers/storage.conf")).unwrap();
    assert!(storage_conf.contains("mount_program = \"/usr/bin/fuse-overlayfs\""));

    assert!(
        !ctx.path("/home/apps/.config/containers/registries.conf.d/001-mirrors.conf")
            .exists()
    );
    assert!(
        ctx.path("/home/apps/.config/containers/registries.conf.d/000-unqualified-search.conf")
            .is_file()
    );
    assert!(ctx.path("/home/apps/.config/restic-excludes").is_file());
    assert!(
        ctx.path("/home/apps/.config/containers/systemd/hello.container.example")
            .is_file()
    );
    assert!(
        ctx.path("/home/apps/.config/containers/systemd/tls-ferron-certs.container.example")
            .is_file()
    );

    let gc_sh = ctx.path("/home/apps/.local/bin/podman-gc.sh");
    assert_eq!(
        fs::metadata(&gc_sh).unwrap().permissions().mode() & 0o777,
        0o755
    );

    let sudoers = ctx.path("/etc/sudoers.d/debopti-podman");
    assert_eq!(
        fs::metadata(&sudoers).unwrap().permissions().mode() & 0o777,
        0o440
    );
    assert!(
        fs::read_to_string(&sudoers)
            .unwrap()
            .contains("alice ALL=(apps)")
    );

    assert!(ctx.path("/etc/profile.d/debopti-podman.sh").is_file());
    assert!(
        ctx.path("/home/alice/.config/fish/conf.d/debopti_apppod.fish")
            .is_file()
    );
    assert_eq!(
        ctx.config_store()
            .get("PODMAN_GC_ENABLED")
            .unwrap()
            .as_deref(),
        Some("true")
    );
}

#[test]
fn podman_install_4x_skips_sqlite_and_enables_cn_mirrors_when_cn() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;
    ctx.sudo_user_override = Some("alice".to_string());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/bin/bash"),
        ],
    );
    make_executable(&ctx.path("/usr/bin/podman"));
    make_executable(&ctx.path("/usr/bin/podman-compose"));

    let runner = MockCommandRunner::new();
    wire_podman_happy_runner(&runner, "4.3.1");

    let report = install_podman(&ctx, &runner).unwrap();
    assert_eq!(
        report.details.get("sqlite_backend").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        report.details.get("cn_mirrors").map(String::as_str),
        Some("true")
    );

    let containers_conf =
        fs::read_to_string(ctx.path("/home/apps/.config/containers/containers.conf")).unwrap();
    assert!(!containers_conf.contains("database_backend = \"sqlite\""));

    let mirrors = fs::read_to_string(
        ctx.path("/home/apps/.config/containers/registries.conf.d/001-mirrors.conf"),
    )
    .unwrap();
    assert!(mirrors.contains("docker.m.daocloud.io"));
    assert!(mirrors.contains("mirror.baidubce.com"));
    assert!(mirrors.contains("docker.nju.edu.cn"));
}

#[test]
fn podman_install_visudo_failure_rolls_back_sudoers_with_e4004() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.sudo_user_override = Some("alice".to_string());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/bin/bash"),
        ],
    );
    make_executable(&ctx.path("/usr/bin/podman"));
    make_executable(&ctx.path("/usr/bin/podman-compose"));

    let runner = MockCommandRunner::new();
    wire_podman_happy_runner(&runner, "5.1.0");
    runner.on_prog("visudo", err_out(1, "syntax error near line 4"));

    let err = install_podman(&ctx, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
    assert!(matches!(err, CoreError::App { ref app, .. } if app == "podman"));
    assert!(
        !ctx.path("/etc/sudoers.d/debopti-podman").exists(),
        "broken sudoers file MUST be deleted immediately on visudo -cf failure"
    );
}

#[test]
fn podman_acl_scrubs_graphroot_and_never_sets_recursive_acl_on_home() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    let apps_home = ctx.path(PODMAN_APPS_HOME_ABS);
    let srv = ctx.path(PODMAN_DATA_DIR_ABS);
    let backups = ctx.path(PODMAN_BACKUPS_DIR_ABS);
    let config = apps_home.join(".config");
    let graphroot = ctx.path(PODMAN_GRAPHROOT_ABS);
    let legacy = ctx.path(PODMAN_LEGACY_STORAGE_ABS);

    for d in [&srv, &backups, &config, &graphroot, &legacy] {
        fs::create_dir_all(d).unwrap();
    }
    fs::set_permissions(&graphroot, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o755)).unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("usermod", ok_out(""));
    runner.on_prog("setfacl", ok_out(""));

    let mut warnings = Vec::new();
    apply_maintainer_acl(&ctx, "alice", &runner, &mut warnings);

    assert_eq!(
        fs::metadata(&graphroot).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&legacy).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains(PODMAN_LEGACY_STORAGE_ABS))
    );

    let setfacl_calls: Vec<Vec<String>> = runner
        .invocations()
        .into_iter()
        .filter(|(p, _)| p == "setfacl")
        .map(|(_, a)| a)
        .collect();

    let gr_s = graphroot.to_string_lossy().into_owned();
    let leg_s = legacy.to_string_lossy().into_owned();
    let home_s = apps_home.to_string_lossy().into_owned();

    // Scrubbed both graphroot and legacy storage
    assert!(setfacl_calls.iter().any(|a| a == &["-R", "-b", &gr_s]));
    assert!(setfacl_calls.iter().any(|a| a == &["-k", &gr_s]));
    assert!(setfacl_calls.iter().any(|a| a == &["-R", "-b", &leg_s]));
    assert!(setfacl_calls.iter().any(|a| a == &["-k", &leg_s]));

    // /home/apps gets ONLY non-recursive --x traverse ACL (NEVER -R!)
    assert!(
        setfacl_calls
            .iter()
            .any(|a| a == &["-m", "u:alice:--x", &home_s])
    );
    assert!(
        !setfacl_calls
            .iter()
            .any(|a| a.contains(&"-R".to_string()) && a.contains(&home_s)),
        "RED-LINE VIOLATION: must never run recursive setfacl on /home/apps"
    );

    // Scoped rwx ACL applied to srv, backups, .config
    for d in [&srv, &backups, &config] {
        let ds = d.to_string_lossy().into_owned();
        assert!(
            setfacl_calls
                .iter()
                .any(|a| a == &["-R", "-m", "u:alice:rwx", &ds])
        );
        assert!(
            setfacl_calls
                .iter()
                .any(|a| a == &["-R", "-d", "-m", "u:alice:rwx", &ds])
        );
    }
}

#[test]
fn podman_install_removes_conflicting_docker_artifacts() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_passwd(&ctx, &[("root", 0, "/root", "/bin/bash")]);
    make_executable(&ctx.path("/usr/bin/docker"));
    make_executable(&ctx.path("/usr/bin/podman"));
    make_executable(&ctx.path("/usr/bin/podman-compose"));
    fs::create_dir_all(ctx.path("/etc/docker")).unwrap();
    fs::write(ctx.path("/etc/docker/daemon.json"), "{}").unwrap();
    fs::create_dir_all(ctx.path("/var/run")).unwrap();
    fs::write(ctx.path("/var/run/docker.sock"), "").unwrap();

    let runner = MockCommandRunner::new();
    wire_podman_happy_runner(&runner, "5.0.0");

    let report = install_podman(&ctx, &runner).unwrap();
    assert!(!ctx.path("/etc/docker").exists());
    assert!(!ctx.path("/var/run/docker.sock").exists());
    assert!(report.warnings.iter().any(|w| w.contains("Docker")));
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "systemctl" && a.contains(&"docker".to_string()))
    );
}

#[test]
fn podman_uninstall_default_preserves_data_and_apps_user_u8() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.sudo_user_override = Some("alice".to_string());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/bin/bash"),
        ],
    );

    let db_file = ctx.path("/home/apps/srv/postgres/data.db");
    fs::create_dir_all(db_file.parent().unwrap()).unwrap();
    fs::write(&db_file, b"precious-user-data").unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/subuid"),
        b"alice:100000:65536\napps:165536:65536\n",
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/sudoers.d/debopti-podman"),
        b"rule\n",
        Some(0o440),
    )
    .unwrap();

    let runner = MockCommandRunner::new();
    wire_podman_happy_runner(&runner, "5.0.0");

    let report = uninstall_podman(&ctx, &PodmanUninstallOptions::default(), &runner).unwrap();
    assert!(
        db_file.is_file(),
        "U8: /home/apps/srv MUST be preserved by default"
    );
    assert_eq!(fs::read(&db_file).unwrap(), b"precious-user-data");
    assert!(!ctx.path("/etc/sudoers.d/debopti-podman").exists());
    assert!(
        !runner.invocations().iter().any(|(p, _)| p == "userdel"),
        "U8: apps user MUST NOT be deleted unless opts.delete_user == true"
    );
    assert!(
        fs::read_to_string(ctx.path("/etc/subuid"))
            .unwrap()
            .contains("apps:")
    );
    assert_eq!(report.preserved_user_custom.len(), 2);
    assert_eq!(
        ctx.config_store()
            .get("PODMAN_GC_ENABLED")
            .unwrap()
            .as_deref(),
        Some("false")
    );
}

#[test]
fn podman_uninstall_explicit_delete_removes_data_user_and_subids() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_passwd(&ctx, &[("root", 0, "/root", "/bin/bash")]);

    let data_dir = ctx.path(PODMAN_DATA_DIR_ABS);
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("app.dat"), b"x").unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/subuid"),
        b"alice:100000:65536\napps:165536:65536\n",
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/subgid"),
        b"alice:100000:65536\napps:165536:65536\n",
        Some(0o644),
    )
    .unwrap();

    let runner = MockCommandRunner::new();
    wire_podman_happy_runner(&runner, "5.0.0");

    let opts = PodmanUninstallOptions {
        delete_data: true,
        delete_user: true,
    };
    let report = uninstall_podman(&ctx, &opts, &runner).unwrap();
    assert!(!data_dir.exists());
    assert!(runner.invocations().iter().any(|(p, _)| p == "userdel"));
    let subuid = fs::read_to_string(ctx.path("/etc/subuid")).unwrap();
    assert!(!subuid.contains("apps:"));
    assert!(subuid.contains("alice:100000:65536"));
    assert_eq!(
        report.details.get("data_deleted").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        report.details.get("user_deleted").map(String::as_str),
        Some("true")
    );
}

#[test]
fn podman_toggle_gc_persists_state_and_fails_when_apps_missing() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());

    // 1. Missing apps user -> E4004
    let missing_runner = MockCommandRunner::new();
    missing_runner.on_exact("id", &["-u", "apps"], err_out(1, "no such user"));
    let err = toggle_podman_gc(&ctx, &missing_runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
    assert!(!probe_podman_gc_enabled(&ctx, &missing_runner));

    // 2. Currently disabled -> enables and persists true
    let runner = MockCommandRunner::new();
    wire_podman_happy_runner(&runner, "5.0.0");
    runner.on_exact(
        "sudo",
        &[
            "-u",
            "apps",
            "XDG_RUNTIME_DIR=/run/user/1001",
            "systemctl",
            "--user",
            "is-enabled",
            "podman-gc.timer",
        ],
        err_out(1, "disabled"),
    );
    let r1 = toggle_podman_gc(&ctx, &runner).unwrap();
    assert_eq!(
        r1.details.get("podman_gc_enabled").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        ctx.config_store()
            .get("PODMAN_GC_ENABLED")
            .unwrap()
            .as_deref(),
        Some("true")
    );

    // 3. Currently enabled -> disables and persists false
    runner.on_exact(
        "sudo",
        &[
            "-u",
            "apps",
            "XDG_RUNTIME_DIR=/run/user/1001",
            "systemctl",
            "--user",
            "is-enabled",
            "podman-gc.timer",
        ],
        ok_out("enabled\n"),
    );
    assert!(probe_podman_gc_enabled(&ctx, &runner));
    let r2 = toggle_podman_gc(&ctx, &runner).unwrap();
    assert_eq!(
        r2.details.get("podman_gc_enabled").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        ctx.config_store()
            .get("PODMAN_GC_ENABLED")
            .unwrap()
            .as_deref(),
        Some("false")
    );
}

#[test]
fn podman_install_apt_fix_missing_retry_and_docker_symlink_fallback() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_passwd(&ctx, &[("root", 0, "/root", "/bin/bash")]);
    make_executable(&ctx.path("/usr/bin/podman"));
    make_executable(&ctx.path("/usr/bin/podman-compose"));

    let runner = ArchiveMockRunner::new();
    wire_podman_happy_runner(&runner.inner, "5.0.0");
    // Make podman dpkg-query say not installed so safe_apt_install runs apt-get install
    runner.inner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "podman"],
        err_out(1, ""),
    );
    // Debian 11 style: no podman-docker package
    runner.inner.on_exact(
        "apt-cache",
        &["show", "podman-docker"],
        err_out(100, "No packages found"),
    );
    runner.fail_first_apt();

    let report = install_podman(&ctx, &runner).unwrap();
    assert!(report.changed);
    assert!(
        runner
            .inner
            .invocations()
            .iter()
            .any(|(p, a)| p == "apt-get" && a.contains(&"--fix-missing".to_string()))
    );
    assert!(
        fs::symlink_metadata(ctx.path("/usr/local/bin/docker"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

// ==================== DEVOPS CLI DEPS TESTS (10..13) ====================

#[test]
fn fzf_version_0_44_triggers_github_upgrade_while_0_55_skips() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());
    make_executable(&ctx.path("/usr/bin/fzf"));

    // Case A: fzf 0.55.0 -> skip download completely
    let runner_ok = ArchiveMockRunner::new();
    runner_ok
        .inner
        .on_exact("fzf", &["--version"], ok_out("0.55.0 (debian)\n"));
    let http_ok = MockHttpClient::new();
    let mut w1 = Vec::new();
    let upgraded_ok = ensure_fzf(&ctx, &http_ok, &runner_ok, &mut w1).unwrap();
    assert!(!upgraded_ok);
    assert!(http_ok.requested_urls().is_empty());

    // Case B: fzf 0.44.1 -> upgrades via GitHub release
    let runner_old = ArchiveMockRunner::new();
    runner_old
        .inner
        .on_exact("fzf", &["--version"], ok_out("0.44.1 (debian)\n"));
    runner_old.inner.on_prog("apt-cache", err_out(100, ""));
    runner_old.inner.on_prog("tar", ok_out(""));
    runner_old.on_extract_contains("fzf_", vec![("fzf", b"#!/bin/sh\necho 0.73.1\n")]);

    let http_old = MockHttpClient::new();
    http_old.on_get(
        "https://api.github.com/repos/junegunn/fzf/releases/latest",
        HttpResponse {
            status: 200,
            body: r#"{"tag_name":"v0.60.0"}"#.to_string(),
        },
    );
    http_old.on_download(
        "https://github.com/junegunn/fzf/releases/download/v0.60.0/fzf-0.60.0-linux_amd64.tar.gz",
        200,
        b"FAKE_TAR_GZ".to_vec(),
    );

    let mut w2 = Vec::new();
    let upgraded = ensure_fzf(&ctx, &http_old, &runner_old, &mut w2).unwrap();
    assert!(upgraded);
    let installed_fzf = ctx.path("/usr/local/bin/fzf");
    assert!(installed_fzf.is_file());
    assert_eq!(
        fs::metadata(&installed_fzf).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn fd_and_bat_create_debian_symlinks_from_fdfind_and_batcat() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    make_executable(&ctx.path("/usr/bin/fdfind"));
    make_executable(&ctx.path("/usr/bin/batcat"));

    let runner = MockCommandRunner::new();
    runner.on_prog("apt-cache", err_out(100, ""));
    let http = MockHttpClient::new();

    let mut warnings = Vec::new();
    assert!(ensure_fd(&ctx, &http, &runner, &mut warnings).unwrap());
    assert!(ensure_bat(&ctx, &runner, &mut warnings).unwrap());

    assert!(
        fs::symlink_metadata(ctx.path("/usr/local/bin/fd"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(ctx.path("/usr/local/bin/bat"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(http.requested_urls().is_empty());
}

#[test]
fn resvg_skips_github_on_aarch64_and_downloads_on_x86_64() {
    let tmp = TempDir::new().unwrap();
    let mut ctx_arm = AppContext::with_root(tmp.path());
    ctx_arm.arch_override = Some("aarch64".to_string());

    let runner = ArchiveMockRunner::new();
    runner.inner.on_prog("apt-cache", err_out(100, ""));
    runner.inner.on_prog("tar", ok_out(""));
    runner.on_extract_contains("resvg.tar.gz", vec![("resvg", b"#!/bin/sh\n")]);

    let http = MockHttpClient::new();
    let mut w_arm = Vec::new();
    assert!(!ensure_resvg(&ctx_arm, &http, &runner, &mut w_arm).unwrap());
    assert!(w_arm.iter().any(|w| w.contains("aarch64")));
    assert!(http.requested_urls().is_empty());

    let mut ctx_x64 = AppContext::with_root(tmp.path());
    ctx_x64.arch_override = Some("x86_64".to_string());
    http.on_download(
        "https://github.com/linebender/resvg/releases/download/v0.47.0/resvg-linux-x86_64.tar.gz",
        200,
        b"FAKE_RESVG_TAR".to_vec(),
    );
    let mut w_x64 = Vec::new();
    assert!(ensure_resvg(&ctx_x64, &http, &runner, &mut w_x64).unwrap());
    assert!(ctx_x64.path("/usr/local/bin/resvg").is_file());
}

#[test]
fn nerd_font_skips_when_marker_and_fc_list_present() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    let marker = ctx.path("/usr/local/share/fonts/0xProto-Nerd-Font/.debopti-installed");
    sys::write_file_atomic(&marker, b"", Some(0o644)).unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", ok_out("install ok installed\n"));
    runner.on_prog(
        "fc-list",
        ok_out("/usr/local/share/fonts/0xProtoNerdFont-Regular.ttf\n"),
    );
    let http = MockHttpClient::new();
    let mut warnings = Vec::new();

    assert!(!ensure_0xproto_nerd_font(&ctx, &http, &runner, &mut warnings).unwrap());
    assert!(http.requested_urls().is_empty());
}

// ==================== FISH & MICRO TESTS (14..18) ====================

#[test]
fn fish_hybrid_sot_uninstall_preserves_conf_d_local_and_never_deletes_sot_via_symlink() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/usr/bin/fish"),
            ("alice", 1000, "/home/alice", "/usr/bin/fish"),
            ("bob", 1001, "/home/bob", "/usr/bin/fish"),
            ("charlie", 1002, "/home/charlie", "/usr/bin/fish"),
        ],
    );
    make_executable(&ctx.path("/usr/bin/fish"));

    let alice_fish = ctx.path("/home/alice/.config/fish");
    let alice_zoxide = alice_fish.join("conf.d/zoxide.fish");
    sys::write_file_atomic(&alice_zoxide, b"# SOT zoxide\n", Some(0o644)).unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/starship.toml"),
        b"format = '$all'\n",
        Some(0o644),
    )
    .unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("chsh", ok_out(""));
    runner.on_prog("apt-get", ok_out(""));

    // Sync hybrid SOT links for bob & root
    shared::sync_devops_sot_links(&ctx, true, &runner).unwrap();

    // Bob adds custom user files in conf.d and conf.d.local (U8)
    let bob_custom_conf = ctx.path("/home/bob/.config/fish/conf.d/my_prompt.fish");
    let bob_local = ctx.path("/home/bob/.config/fish/conf.d.local/my_alias.fish");
    sys::write_file_atomic(&bob_custom_conf, b"alias ll 'ls -la'\n", Some(0o644)).unwrap();
    sys::write_file_atomic(&bob_local, b"set -g MY_KEY 1\n", Some(0o644)).unwrap();

    // Charlie has a legacy whole-directory symlink pointing to alice's SOT fish dir
    let charlie_fish = ctx.path("/home/charlie/.config/fish");
    fs::remove_dir_all(&charlie_fish).unwrap();
    symlink(&alice_fish, &charlie_fish).unwrap();

    // Cleaning charlie & bob MUST NOT delete alice's SOT directory via symlinks!
    let _ = clean_non_sot_fish_dir(&charlie_fish).unwrap();
    assert!(
        alice_zoxide.is_file(),
        "removing charlie symlink must NEVER delete SOT target"
    );
    assert!(fs::symlink_metadata(&charlie_fish).is_err());

    let preserved = clean_non_sot_fish_dir(&ctx.path("/home/bob/.config/fish")).unwrap();
    assert!(
        alice_zoxide.is_file(),
        "removing bob conf.d symlinks must NEVER delete SOT target"
    );
    assert!(
        bob_custom_conf.is_file(),
        "U8: bob non-symlink conf.d file preserved"
    );
    assert!(bob_local.is_file(), "U8: bob conf.d.local preserved");
    assert!(
        !ctx.path("/home/bob/.config/fish/conf.d/zoxide.fish")
            .exists()
    );
    assert!(
        !ctx.path("/home/bob/.config/fish/conf.d/00-debopti-sot-bootstrap.fish")
            .exists()
    );
    assert!(!preserved.is_empty());

    // Full uninstall_fish restores /bin/bash and cleans global files while preserving bob's U8 files
    let report = uninstall_fish(&ctx, &runner).unwrap();
    assert!(bob_custom_conf.is_file());
    assert!(bob_local.is_file());
    assert!(!ctx.path("/etc/starship.toml").exists());
    assert!(!report.preserved_user_custom.is_empty());
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "chsh" && a == &["-s", "/bin/bash", "bob"])
    );
}

#[test]
fn fish_install_renders_templates_sets_shells_and_syncs_sot() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/bin/bash"),
            ("bob", 1001, "/home/bob", "/bin/bash"),
        ],
    );
    for cmd in [
        "fish", "fzf", "fd", "rg", "zoxide", "xclip", "bat", "starship",
    ] {
        make_executable(&ctx.path(&format!("/usr/bin/{cmd}")));
    }
    sys::write_file_atomic(
        &ctx.path("/usr/local/share/fonts/0xProto-Nerd-Font/.debopti-installed"),
        b"",
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/starship.toml"),
        b"format = '$all'\n",
        Some(0o644),
    )
    .unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", ok_out("install ok installed\n"));
    runner.on_prog("apt-get", ok_out(""));
    runner.on_exact("fzf", &["--version"], ok_out("0.58.0\n"));
    runner.on_prog("fc-list", ok_out("0xProtoNerdFont\n"));
    runner.on_prog("sudo", ok_out(""));
    runner.on_prog("starship", ok_out(""));
    runner.on_prog("chsh", ok_out(""));

    let http = MockHttpClient::new();
    http.on_download(
        "https://raw.githubusercontent.com/jorgebucaran/fisher/main/functions/fisher.fish",
        200,
        b"function fisher; end\n".to_vec(),
    );

    let report = install_fish(&ctx, &http, &runner).unwrap();
    assert_eq!(report.app, "fish");
    assert!(
        ctx.path("/home/alice/.config/fish/functions/_puffer_fish_expand_dot.fish")
            .is_file()
    );
    assert!(
        ctx.path("/home/alice/.config/fish/conf.d/zoxide.fish")
            .is_file()
    );
    assert!(
        ctx.path("/home/alice/.config/fish/conf.d/starship.fish")
            .is_file()
    );
    assert!(
        ctx.path("/home/alice/.config/fish/conf.d/abbrs.fish")
            .is_file()
    );
    assert!(
        ctx.path("/home/bob/.config/fish/conf.d/00-debopti-sot-bootstrap.fish")
            .is_file()
    );
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "chsh" && a == &["-s", "/usr/bin/fish", "alice"])
    );
}

#[test]
fn micro_omni_session_lua_patch_is_idempotent_and_repairs_gogoos() {
    // 1. Upstream bug -> patched
    let upstream = "local goos = import(\"os\")\nos.MkdirAll(sessionsPath, os.ModePerm)\nlocal h = goos.Getenv(\"HOME\")\n";
    let (p1, c1) = patch_micro_omni_session_lua(upstream);
    assert!(c1);
    assert!(p1.contains("goos.MkdirAll(sessionsPath, goos.ModePerm)"));
    assert!(p1.contains("goos.Getenv(\"HOME\")"));
    assert!(!p1.contains("gogoos"));

    // 2. Second run is strictly idempotent
    let (p2, c2) = patch_micro_omni_session_lua(&p1);
    assert!(!c2);
    assert_eq!(p2, p1);

    // 3. Corrupted broad-sed artifact (`gogoos` / `gogogoos`) -> repaired cleanly
    let corrupted =
        "gogoos.MkdirAll(sessionsPath, gogogoos.ModePerm)\nlocal x = gogoos.Getenv(\"HOME\")\n";
    let (repaired, c3) = patch_micro_omni_session_lua(corrupted);
    assert!(c3);
    assert_eq!(
        repaired,
        "goos.MkdirAll(sessionsPath, goos.ModePerm)\nlocal x = goos.Getenv(\"HOME\")\n"
    );
    let (repaired2, c4) = patch_micro_omni_session_lua(&repaired);
    assert!(!c4);
    assert_eq!(repaired2, repaired);
}

#[test]
fn micro_install_deploys_configs_osc52_patches_session_and_sets_env() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/usr/bin/fish"),
        ],
    );
    for cmd in ["micro", "fish", "fzf", "fd", "rg", "zoxide", "xclip", "bat"] {
        make_executable(&ctx.path(&format!("/usr/bin/{cmd}")));
    }
    sys::write_file_atomic(
        &ctx.path("/usr/local/share/fonts/0xProto-Nerd-Font/.debopti-installed"),
        b"",
        Some(0o644),
    )
    .unwrap();

    // Custom runner that populates MicroOmni/Session.lua during git clone
    struct GitMockRunner(MockCommandRunner);
    impl CommandRunner for GitMockRunner {
        fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput> {
            if prog == "git"
                && args.contains(&"clone")
                && let Some(target) = args.last()
            {
                let p = Path::new(target);
                fs::create_dir_all(p).unwrap();
                if target.ends_with("MicroOmni") {
                    fs::write(
                        p.join("Session.lua"),
                        "os.MkdirAll(sessionsPath, os.ModePerm)\n",
                    )
                    .unwrap();
                } else if target.ends_with("micro_updated_plugins") {
                    let sub = p.join("micro-snippets-plugin");
                    fs::create_dir_all(&sub).unwrap();
                    fs::write(sub.join("snippets.lua"), "-- ok\n").unwrap();
                }
            }
            self.0.run(prog, args, timeout)
        }
    }

    let inner = MockCommandRunner::new();
    inner.on_prog("dpkg-query", ok_out("install ok installed\n"));
    inner.on_prog("apt-get", ok_out(""));
    inner.on_exact("fzf", &["--version"], ok_out("0.60.0\n"));
    inner.on_prog("fc-list", ok_out("0xProtoNerdFont\n"));
    inner.on_prog("git", ok_out(""));
    inner.on_prog("update-alternatives", ok_out(""));
    let runner = GitMockRunner(inner);
    let http = MockHttpClient::new();

    let report = install_micro(&ctx, &http, &runner).unwrap();
    assert_eq!(
        report.details.get("micro_omni_patched").map(String::as_str),
        Some("true")
    );

    let session_txt =
        fs::read_to_string(ctx.path("/home/alice/.config/micro/plug/MicroOmni/Session.lua"))
            .unwrap();
    assert!(session_txt.contains("goos.MkdirAll(sessionsPath, goos.ModePerm)"));
    assert!(
        ctx.path("/home/alice/.config/micro/plug/snippets/snippets.lua")
            .is_file()
    );
    assert_eq!(
        fs::metadata(ctx.path("/usr/local/bin/debopti-osc52"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(
        fs::symlink_metadata(ctx.path("/usr/local/bin/micro-clip"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let env_txt = fs::read_to_string(ctx.path("/etc/environment")).unwrap();
    assert!(env_txt.contains("EDITOR=\"micro\""));
    assert!(env_txt.contains("VISUAL=\"micro\""));
}

#[test]
fn micro_uninstall_cleans_env_alternatives_and_preserves_unrelated_env() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/bin/bash"),
        ],
    );
    sys::write_file_atomic(
        &ctx.path("/etc/environment"),
        b"USER_CUSTOM=\"keep\"\nEDITOR=\"micro\"\nVISUAL=\"micro\"\n",
        Some(0o644),
    )
    .unwrap();
    make_executable(&ctx.path("/usr/local/bin/debopti-osc52"));

    let runner = MockCommandRunner::new();
    runner.on_prog("update-alternatives", ok_out(""));
    runner.on_prog("apt-get", ok_out(""));

    uninstall_micro(&ctx, &runner).unwrap();
    let env_after = fs::read_to_string(ctx.path("/etc/environment")).unwrap();
    assert!(env_after.contains("USER_CUSTOM=\"keep\""));
    assert!(!env_after.contains("EDITOR="));
    assert!(!env_after.contains("VISUAL="));
    assert!(!ctx.path("/usr/local/bin/debopti-osc52").exists());
}

// ==================== YAZI TESTS (19..21) ====================

#[test]
fn yazi_bashrc_injection_idempotent_and_surgical_removal_preserves_user_lines_u8() {
    let original = "# user header\nalias ll='ls -alF'\nexport MY_TOKEN=keep\n";
    let snippet = "\n# Yazi CWD Sync Wrapper\nif [ -f /etc/profile.d/yazi_wrapper.sh ]; then\n    . /etc/profile.d/yazi_wrapper.sh\nfi";

    let (injected1, changed1) = inject_yazi_bashrc(original, snippet);
    assert!(changed1);
    assert!(injected1.contains("# Yazi CWD Sync Wrapper"));
    assert!(injected1.contains("alias ll='ls -alF'"));
    assert!(injected1.contains("export MY_TOKEN=keep"));

    // Idempotent second inject
    let (injected2, changed2) = inject_yazi_bashrc(&injected1, snippet);
    assert!(!changed2);
    assert_eq!(injected2, injected1);

    // Append more user content AFTER the wrapper block, then surgically remove wrapper
    let with_trailing_user = format!("{injected1}export AFTER_YAZI=still_here\n");
    let (cleaned, removed) = remove_yazi_bashrc(&with_trailing_user);
    assert!(removed);
    assert!(!cleaned.contains("# Yazi CWD Sync Wrapper"));
    assert!(!cleaned.contains("yazi_wrapper.sh"));
    assert!(cleaned.contains("# user header"));
    assert!(cleaned.contains("alias ll='ls -alF'"));
    assert!(cleaned.contains("export MY_TOKEN=keep"));
    assert!(cleaned.contains("export AFTER_YAZI=still_here"));
}

#[test]
fn yazi_install_persists_media_preview_preference_and_installs_wrappers() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());
    setup_passwd(
        &ctx,
        &[
            ("root", 0, "/root", "/bin/bash"),
            ("alice", 1000, "/home/alice", "/usr/bin/fish"),
        ],
    );
    for cmd in ["fish", "fzf", "fd", "rg", "zoxide", "xclip", "bat", "resvg"] {
        make_executable(&ctx.path(&format!("/usr/bin/{cmd}")));
    }
    fs::create_dir_all(ctx.path("/home/alice/.config/fish/functions")).unwrap();
    sys::write_file_atomic(
        &ctx.path("/usr/local/share/fonts/0xProto-Nerd-Font/.debopti-installed"),
        b"",
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/bash.bashrc"),
        b"# user custom bashrc\nalias gs='git status'\n",
        Some(0o644),
    )
    .unwrap();

    let runner = ArchiveMockRunner::new();
    runner
        .inner
        .on_prog("dpkg-query", ok_out("install ok installed\n"));
    runner.inner.on_prog("apt-get", ok_out(""));
    runner
        .inner
        .on_exact("fzf", &["--version"], ok_out("0.60.0\n"));
    runner.inner.on_prog("fc-list", ok_out("0xProtoNerdFont\n"));
    runner.inner.on_prog("unzip", ok_out(""));
    runner.inner.on_prog("sudo", ok_out(""));
    runner.on_extract_contains(
        "yazi_",
        vec![
            ("yazi-x86_64/yazi", b"#!/bin/sh\necho yazi\n"),
            ("yazi-x86_64/ya", b"#!/bin/sh\necho ya\n"),
        ],
    );

    let http = MockHttpClient::new();
    http.on_get(
        "https://api.github.com/repos/sxyazi/yazi/releases/latest",
        HttpResponse {
            status: 200,
            body: r#"{"tag_name":"v0.4.2"}"#.to_string(),
        },
    );
    http.on_download(
        "https://github.com/sxyazi/yazi/releases/download/v0.4.2/yazi-x86_64-unknown-linux-musl.zip",
        200,
        b"FAKE_YAZI_ZIP".to_vec(),
    );

    // 1. Explicit media_preview = Some(true) persists "true"
    let r1 = install_yazi(
        &ctx,
        &YaziInstallOptions {
            media_preview: Some(true),
        },
        &http,
        &runner,
    )
    .unwrap();
    assert_eq!(
        r1.details.get("media_preview").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        ctx.config_store()
            .get("YAZI_MEDIA_PREVIEW")
            .unwrap()
            .as_deref(),
        Some("true")
    );
    assert!(ctx.path("/usr/local/bin/yazi").is_file());
    assert!(ctx.path("/usr/local/bin/ya").is_file());
    assert!(ctx.path("/home/alice/.config/yazi/yazi.toml").is_file());
    assert!(ctx.path("/etc/fish/conf.d/yazi.fish").is_file());
    assert!(
        ctx.path("/home/alice/.config/fish/functions/y.fish")
            .is_file()
    );
    assert!(
        fs::read_to_string(ctx.path("/etc/bash.bashrc"))
            .unwrap()
            .contains("# Yazi CWD Sync Wrapper")
    );

    // 2. Subsequent call with None reuses persisted "true"
    let r2 = install_yazi(
        &ctx,
        &YaziInstallOptions {
            media_preview: None,
        },
        &http,
        &runner,
    )
    .unwrap();
    assert_eq!(
        r2.details.get("media_preview").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        r2.details.get("bashrc_injected").map(String::as_str),
        Some("false")
    );

    // 3. Uninstall surgically removes bashrc snippet while preserving user custom lines
    let ru = uninstall_yazi(&ctx, &runner).unwrap();
    let bashrc_after = fs::read_to_string(ctx.path("/etc/bash.bashrc")).unwrap();
    assert!(bashrc_after.contains("alias gs='git status'"));
    assert!(!bashrc_after.contains("# Yazi CWD Sync Wrapper"));
    assert!(!ru.preserved_user_custom.is_empty());
}

#[test]
fn yazi_install_rejects_unsupported_arch_with_e4004() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.no_sys = true;
    ctx.arch_override = Some("riscv64".to_string());

    let runner = MockCommandRunner::new();
    let http = MockHttpClient::new();
    let err = install_yazi(&ctx, &YaziInstallOptions::default(), &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
}

// ==================== LEGO TESTS (22..25) ====================

#[test]
fn lego_domain_validators_reject_injection_and_accept_valid_wildcards() {
    assert!(validate_primary_domain("example.com").is_ok());
    assert!(validate_primary_domain("sub.example.co.uk").is_ok());
    for bad in [
        "",
        "*.example.com",
        "../evil",
        "a/b",
        "a;id",
        "a b.com",
        "a\nb.com",
        "a\"b.com",
        "a'b.com",
        "-evil.com",
        "$(whoami).com",
    ] {
        assert!(
            validate_primary_domain(bad).is_err(),
            "primary validator must reject '{bad}'"
        );
    }

    assert!(validate_san_domain("*.example.com").is_ok());
    assert!(validate_san_domain("api.example.com").is_ok());
    for bad_san in [
        "",
        "../evil",
        "a;id",
        "a*b.example.com",
        "*.*.example.com",
        "*. ",
        "a/b",
    ] {
        assert!(
            validate_san_domain(bad_san).is_err(),
            "SAN validator must reject '{bad_san}'"
        );
    }
}

#[test]
fn lego_add_domain_enforces_0600_ferron_autodetect_rejects_dups_and_redacts_token() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    // Simulate Ferron installed
    fs::create_dir_all(ctx.path("/etc/ferron")).unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("getent", ok_out("debopti-certs:x:999:\n"));
    runner.on_exact("id", &["-u", "ferron"], err_out(1, ""));

    let secret_token = "cf_secret_super_sensitive_token_98765";
    let spec = LegoDomainSpec {
        primary_domain: "example.com".to_string(),
        san_domains: vec!["*.example.com".to_string()],
        email: "ops@example.com".to_string(),
        cf_token: secret_token.to_string(),
        provider: "cloudflare".to_string(),
        issue_immediately: false,
    };

    let report = add_lego_domain(&ctx, &spec, &runner).unwrap();
    assert_eq!(
        report.details.get("ferron_push").map(String::as_str),
        Some("true")
    );

    // U10: secret token must NEVER appear in report or troubleshoot command
    let report_json = serde_json::to_string(&report).unwrap();
    assert!(!report_json.contains(secret_token));
    let troubleshoot = build_lego_troubleshoot_cmd(&ctx, "example.com").unwrap();
    assert!(!troubleshoot.contains(secret_token));
    assert!(troubleshoot.contains("--domains=\"example.com\""));
    assert!(troubleshoot.contains("--domains=\"*.example.com\""));

    let env_file = ctx.path("/etc/lego/envs/example.com.env");
    assert_eq!(
        fs::metadata(&env_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(ctx.path("/etc/lego/envs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    // Duplicate domain refused with E4004
    let dup_err = add_lego_domain(&ctx, &spec, &runner).unwrap_err();
    assert_eq!(dup_err.error_code(), "E4004");
}

#[test]
fn lego_list_toggle_and_u8_remove_preserves_certificates() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.euid_override = Some(0);

    let runner = MockCommandRunner::new();
    runner.on_prog("openssl", ok_out("notAfter=May 20 12:00:00 2035 GMT\n"));
    let hook_path = ctx.path("/usr/local/bin/debopti-lego-hook.sh");
    make_executable(&hook_path);
    runner.on_prog(&hook_path.to_string_lossy(), ok_out("pushed\n"));

    let spec = LegoDomainSpec {
        primary_domain: "demo.example.org".to_string(),
        san_domains: vec!["*.demo.example.org".to_string()],
        email: "admin@example.org".to_string(),
        cf_token: "tok_abc123".to_string(),
        provider: "cloudflare".to_string(),
        issue_immediately: false,
    };
    add_lego_domain(&ctx, &spec, &runner).unwrap();

    // Place existing certificate & key
    let crt = ctx.path("/var/lib/lego/certificates/demo.example.org.crt");
    let key = ctx.path("/var/lib/lego/certificates/demo.example.org.key");
    sys::write_file_atomic(&crt, b"CERT_PEM", Some(0o600)).unwrap();
    sys::write_file_atomic(&key, b"KEY_PEM", Some(0o600)).unwrap();

    let list = list_lego_domains(&ctx, &runner).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].primary_domain, "demo.example.org");
    assert!(list[0].auto_renew);
    assert!(!list[0].ferron_push);
    assert!(list[0].cert_exists);
    assert!(list[0].days_left.unwrap() > 100);
    assert!(parse_openssl_enddate_epoch("notAfter=Jan  1 00:00:00 1970 GMT") == Some(0));

    // Toggle AutoRenew -> false, DnsSkipPropagation -> true, FerronPush -> true (triggers hook!)
    toggle_lego_flag(&ctx, "demo.example.org", LegoFlag::AutoRenew, &runner).unwrap();
    toggle_lego_flag(
        &ctx,
        "demo.example.org",
        LegoFlag::DnsSkipPropagation,
        &runner,
    )
    .unwrap();
    let ferron_rep =
        toggle_lego_flag(&ctx, "demo.example.org", LegoFlag::FerronPush, &runner).unwrap();
    assert_eq!(
        ferron_rep
            .details
            .get("ferron_hook_triggered")
            .map(String::as_str),
        Some("true")
    );

    let env_path = ctx.path("/etc/lego/envs/demo.example.org.env");
    assert_eq!(
        fs::metadata(&env_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let list2 = list_lego_domains(&ctx, &runner).unwrap();
    assert!(!list2[0].auto_renew);
    assert!(list2[0].dns_skip_propagation);
    assert!(list2[0].ferron_push);

    // U8: remove_lego_domain removes ONLY .env and preserves .crt & .key!
    let rem = remove_lego_domain(&ctx, "demo.example.org").unwrap();
    assert!(!env_path.exists());
    assert!(
        crt.is_file(),
        "U8: certificate file MUST be preserved on remove_lego_domain"
    );
    assert!(
        key.is_file(),
        "U8: private key file MUST be preserved on remove_lego_domain"
    );
    assert!(!rem.preserved_user_custom.is_empty());
}

#[test]
fn lego_install_deploys_binary_scripts_systemd_units_and_migrates() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("aarch64".to_string());

    let runner = ArchiveMockRunner::new();
    runner.inner.on_prog("tar", ok_out(""));
    runner.inner.on_prog("systemctl", ok_out(""));
    let lego_bin = ctx.path("/usr/local/bin/lego");
    runner
        .inner
        .on_prog(&lego_bin.to_string_lossy(), ok_out(""));
    runner.on_extract_contains(
        "lego.tar.gz",
        vec![("lego", b"#!/bin/sh\necho lego v5.2.2\n")],
    );

    let http = MockHttpClient::new();
    http.on_download(
        "https://github.com/go-acme/lego/releases/download/v5.2.2/lego_v5.2.2_linux_arm64.tar.gz",
        200,
        b"FAKE_LEGO_TAR".to_vec(),
    );

    let report = install_lego(&ctx, &http, &runner).unwrap();
    assert_eq!(
        report.details.get("version").map(String::as_str),
        Some("v5.2.2")
    );
    assert_eq!(
        fs::metadata(&lego_bin).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(ctx.path("/usr/local/bin/debopti-lego-lib.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(
        fs::metadata(ctx.path("/usr/local/bin/debopti-lego-run-once.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(
        ctx.path("/etc/systemd/system/debopti-lego-renew.service")
            .is_file()
    );
    assert!(
        ctx.path("/etc/systemd/system/debopti-lego-renew.timer")
            .is_file()
    );
    assert!(runner.inner.invocations().iter().any(
        |(p, a)| p == &lego_bin.to_string_lossy() && a == &["migrate", "--path=/var/lib/lego"]
    ));
}
