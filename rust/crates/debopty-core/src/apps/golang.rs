use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::{NamedTempFile, TempDir};

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, DEFAULT_USER_AGENT, HttpClient, HttpRequest, IpPreference};
use crate::security::firewall;
use crate::sys::{self, CommandRunner};

pub const MODULE_ID: &str = "golang";
const DIAG_TARGET: &str = "debopty::apps::golang";
const GO_FALLBACK_VERSION: &str = "go1.22.1";
const CN_GOPROXY: &str = "https://goproxy.cn,direct";
const TAILSCALE_DERPER_TAG: &str = "v1.94.2";
const TAILSCALE_GIT_URL: &str = "https://github.com/tailscale/tailscale.git";

const DERPER_CLOSE_CONN_DEF: &str = "func closeConn(w http.ResponseWriter) { if hj, ok := w.(http.Hijacker); ok { if conn, _, err := hj.Hijack(); err == nil { conn.Close() } } }";
const DERPER_MAIN_ANCHOR: &str = "func main()";
const DERPER_GEN204_ORIG: &str = r#"mux.HandleFunc("/generate_204", derphttp.ServeNoContent)"#;
const DERPER_GEN204_PATCHED: &str = r#"mux.HandleFunc("/generate_204", func(w http.ResponseWriter, r *http.Request) { if r.UserAgent() == "Go-http-client/1.1" { derphttp.ServeNoContent(w, r); return }; closeConn(w) })"#;
const DERPER_ROOT_ORIG: &str = r#"fmt.Fprintf(w, "DERP\n")"#;

fn map_go_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => Err(CoreError::App {
            app: MODULE_ID.to_string(),
            message: format!("unsupported architecture for Go: '{other}'"),
        }),
    }
}

fn build_go_path_env(target_home: &std::path::Path) -> String {
    let base = env::var("PATH").unwrap_or_else(|_| {
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()
    });
    format!("{base}:/usr/local/go/bin:{}/go/bin", target_home.display())
}

fn ensure_go_installed(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<()> {
    if ctx.path("/usr/local/go/bin/go").is_file() || ctx.is_cmd_present("go", runner) {
        return Ok(());
    }
    let _ = install_go(ctx, http, runner)?;
    Ok(())
}

/// Deploy arch-aware Go toolchain (`amd64` / `arm64`) with CN mirror support and Fish PATH/GOPROXY sync.
pub fn install_go(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let raw_arch = ctx.arch();
    let go_arch = map_go_arch(&raw_arch)?;
    let is_cn = ctx.effective_is_cn();
    let domain = if is_cn { "golang.google.cn" } else { "go.dev" };

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting Go toolchain installation",
        None,
        &[("arch", go_arch), ("domain", domain)],
    );

    let version_url = format!("https://{domain}/VERSION?m=text");
    let req = HttpRequest {
        url: &version_url,
        user_agent: Some(DEFAULT_USER_AGENT),
        connect_timeout: Duration::from_secs(5),
        max_timeout: Duration::from_secs(10),
        ip_preference: IpPreference::Any,
    };

    let mut warnings = Vec::new();
    let version = match http.get_text(&req) {
        Ok(resp) if (200..=299).contains(&resp.status) => resp
            .body
            .lines()
            .next()
            .map(str::trim)
            .filter(|line| line.starts_with("go"))
            .map(String::from)
            .unwrap_or_else(|| {
                warnings.push(format!(
                    "unexpected VERSION response from {domain}; fell back to {GO_FALLBACK_VERSION}"
                ));
                GO_FALLBACK_VERSION.to_string()
            }),
        _ => {
            warnings.push(format!(
                "failed to fetch Go VERSION from {domain}; fell back to {GO_FALLBACK_VERSION}"
            ));
            GO_FALLBACK_VERSION.to_string()
        }
    };

    let download_url = format!("https://{domain}/dl/{version}.linux-{go_arch}.tar.gz");
    let tmp_go = NamedTempFile::new()?;
    network::download_with_fallback(tmp_go.path(), &download_url, is_cn, http)?;

    let go_root = ctx.path("/usr/local/go");
    if go_root.exists() {
        fs::remove_dir_all(&go_root)?;
    }
    shared::extract_tar_gz(tmp_go.path(), &ctx.path("/usr/local"), runner)?;

    let target_user = ctx.initial_user();
    let target_home = ctx.user_home(&target_user);

    if is_cn {
        shared::update_fish_env(ctx, "GOPROXY", CN_GOPROXY, runner)?;
    }
    shared::update_fish_path(ctx, "/usr/local/go/bin", runner)?;
    shared::update_fish_path(ctx, &format!("{}/go/bin", target_home.display()), runner)?;

    let mut report = AppReport::new(
        MODULE_ID,
        "install_go",
        true,
        format!("Go {version} ({go_arch}) installed"),
    )
    .with_detail("version", &version)
    .with_detail("arch", go_arch)
    .with_detail("domain", domain);

    for w in warnings {
        report = report.with_warning(w);
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "Go toolchain installation complete",
        None,
        &[("version", &version), ("arch", go_arch)],
    );
    Ok(report)
}

/// Remove Go toolchain directories and Fish PATH/GOPROXY entries.
pub fn uninstall_go(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    let target_user = ctx.initial_user();
    let target_home = ctx.user_home(&target_user);

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting Go toolchain uninstallation",
        None,
        &[],
    );

    let go_root = ctx.path("/usr/local/go");
    if go_root.exists() {
        fs::remove_dir_all(&go_root)?;
    }
    let user_gopath = target_home.join("go");
    if user_gopath.exists() {
        fs::remove_dir_all(&user_gopath)?;
    }

    shared::remove_fish_path(ctx, "/usr/local/go/bin", runner)?;
    shared::remove_fish_path(ctx, &format!("{}/go/bin", target_home.display()), runner)?;
    shared::remove_fish_env(ctx, "GOPROXY", runner)?;

    Ok(AppReport::new(
        MODULE_ID,
        "uninstall_go",
        true,
        "Go toolchain removed from system",
    ))
}

/// Self-compile Caddy with `caddy-l4`, `caddy-dns/cloudflare`, and `forwardproxy@naive`,
/// preserving existing `/etc/caddy/Caddyfile` and `/usr/share/caddy/index.html` (U8).
pub fn install_caddy(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting custom Caddy build & deployment",
        None,
        &[],
    );

    ensure_go_installed(ctx, http, runner)?;

    let is_cn = ctx.effective_is_cn();
    let target_user = ctx.initial_user();
    let target_home = ctx.user_home(&target_user);
    let path_env = build_go_path_env(&target_home);
    let home_str = target_home.to_string_lossy().into_owned();

    let mut build_env: Vec<(&str, &str)> = vec![("PATH", &path_env), ("HOME", &home_str)];
    if is_cn {
        build_env.push(("GOPROXY", CN_GOPROXY));
    }

    let build_dir = TempDir::new()?;
    let built_caddy = build_dir.path().join("caddy");
    let built_caddy_str = built_caddy.to_string_lossy().into_owned();

    if !ctx.no_sys {
        let install_xcaddy = shared::run_as_user(
            ctx,
            &target_user,
            "/usr/local/go/bin/go",
            &["install", "github.com/caddyserver/xcaddy/cmd/xcaddy@latest"],
            &build_env,
            Duration::from_secs(300),
            runner,
        )?;
        if !install_xcaddy.success() {
            return Err(CoreError::App {
                app: "caddy".to_string(),
                message: format!(
                    "xcaddy installation failed (code {}): {}",
                    install_xcaddy.code,
                    install_xcaddy.stderr.trim()
                ),
            });
        }

        let xcaddy_bin = format!("{}/go/bin/xcaddy", target_home.display());
        let build_out = shared::run_as_user(
            ctx,
            &target_user,
            &xcaddy_bin,
            &[
                "build",
                "--output",
                &built_caddy_str,
                "--with",
                "github.com/mholt/caddy-l4",
                "--with",
                "github.com/caddy-dns/cloudflare",
                "--with",
                "github.com/caddyserver/forwardproxy@caddy2=github.com/klzgrad/forwardproxy@naive",
            ],
            &build_env,
            Duration::from_secs(900),
            runner,
        )?;
        if !build_out.success() {
            return Err(CoreError::App {
                app: "caddy".to_string(),
                message: format!(
                    "xcaddy build failed (code {}): {}",
                    build_out.code,
                    build_out.stderr.trim()
                ),
            });
        }

        let _ = runner.run("systemctl", &["stop", "caddy"], Duration::from_secs(30));
    }

    let caddy_dest = ctx.path("/usr/bin/caddy");
    if let Some(parent) = caddy_dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if built_caddy.is_file() {
        fs::copy(&built_caddy, &caddy_dest)?;
        fs::set_permissions(&caddy_dest, fs::Permissions::from_mode(0o755))?;
    } else if !ctx.is_custom_root() && !ctx.no_sys {
        return Err(CoreError::App {
            app: "caddy".to_string(),
            message: format!(
                "xcaddy did not produce expected binary at {}",
                built_caddy.display()
            ),
        });
    }

    if !ctx.no_sys {
        let dest_str = caddy_dest.to_string_lossy();
        let _ = runner.run(
            "setcap",
            &["cap_net_bind_service=+ep", &dest_str],
            Duration::from_secs(15),
        );
    }

    shared::create_system_user(ctx, "caddy", runner)?;
    if !ctx.no_sys {
        let _ = runner.run("groupadd", &["--system", "caddy"], Duration::from_secs(15));
        let _ = runner.run(
            "usermod",
            &["-aG", "caddy", "caddy"],
            Duration::from_secs(15),
        );
    }

    for dir_abs in ["/etc/caddy", "/etc/ssl/caddy", "/usr/share/caddy"] {
        fs::create_dir_all(ctx.path(dir_abs))?;
    }

    let mut report = AppReport::new(
        "caddy",
        "install",
        true,
        "Caddy (l4/cloudflare/naiveproxy) deployed",
    )
    .with_detail("binary", caddy_dest.to_string_lossy())
    .with_detail("fw_rule", "Caddy_Web");

    // U8: preserve existing Caddyfile and index.html
    let caddyfile = ctx.path("/etc/caddy/Caddyfile");
    if caddyfile.is_file() {
        report = report.with_preserved("/etc/caddy/Caddyfile");
    } else {
        ctx.template_engine().render_to_file(
            "templates/apps/golang/Caddyfile",
            &caddyfile,
            &BTreeMap::new(),
            Some(0o644),
        )?;
    }

    let index_html = ctx.path("/usr/share/caddy/index.html");
    if index_html.is_file() {
        report = report.with_preserved("/usr/share/caddy/index.html");
    } else {
        ctx.template_engine().render_to_file(
            "templates/apps/golang/index.html",
            &index_html,
            &BTreeMap::new(),
            Some(0o644),
        )?;
    }

    if !ctx.no_sys && !ctx.is_custom_root() {
        let _ = runner.run(
            "chown",
            &["-R", "caddy:root", "/etc/caddy", "/etc/ssl/caddy"],
            Duration::from_secs(15),
        );
    }

    shared::deploy_systemd_service(
        ctx,
        "caddy",
        "templates/apps/golang/caddy.service",
        &BTreeMap::new(),
        true,
        runner,
    )?;

    firewall::add_fw_rule(
        &ctx.security_context(),
        "80,443",
        "tcp/udp",
        "Caddy_Web",
        runner,
    )?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "Caddy deployment complete",
        None,
        &[],
    );
    Ok(report)
}

/// Remove Caddy service, binary, configs, firewall rule, and system user.
pub fn uninstall_caddy(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting Caddy uninstallation",
        None,
        &[],
    );

    shared::remove_systemd_service(ctx, "caddy", runner)?;

    let bin = ctx.path("/usr/bin/caddy");
    if bin.exists() || bin.symlink_metadata().is_ok() {
        let _ = fs::remove_file(&bin);
    }
    for dir_abs in ["/etc/caddy", "/usr/share/caddy", "/etc/ssl/caddy"] {
        let p = ctx.path(dir_abs);
        if p.exists() {
            fs::remove_dir_all(&p)?;
        }
    }

    firewall::remove_fw_rule(&ctx.security_context(), "Caddy_Web", runner)?;
    shared::remove_system_user(ctx, "caddy", false, runner)?;

    Ok(AppReport::new(
        "caddy",
        "uninstall",
        true,
        "Caddy removed from system",
    ))
}

/// Apply all 3 stealth anti-probing hunks to Tailscale `cmd/derper/derper.go`.
///
/// Idempotent on a second pass; returns `CoreError::App` (`E4004`) if any upstream anchor is missing.
pub fn patch_derper_source(src: &str) -> Result<String> {
    let has_hunk1 = src.contains(DERPER_CLOSE_CONN_DEF);
    let has_hunk2 = src.contains(DERPER_GEN204_PATCHED);
    let has_orig_root = src.contains(DERPER_ROOT_ORIG);

    // Already fully patched (idempotent 2nd pass)
    if has_hunk1 && has_hunk2 && !has_orig_root && src.matches("closeConn(w)").count() >= 2 {
        return Ok(src.to_string());
    }

    let mut patched = src.to_string();

    // Hunk 1: inject closeConn helper before `func main()`
    if !has_hunk1 {
        let Some(pos) = patched.find(DERPER_MAIN_ANCHOR) else {
            return Err(CoreError::App {
                app: "derper".to_string(),
                message: "derper.go stealth patch failed: missing upstream anchor 'func main()'"
                    .to_string(),
            });
        };
        let insert_text = format!("{DERPER_CLOSE_CONN_DEF}\n");
        patched.insert_str(pos, &insert_text);
    }

    // Hunk 2: restrict `/generate_204` to `Go-http-client/1.1`
    if !has_hunk2 {
        if !patched.contains(DERPER_GEN204_ORIG) {
            return Err(CoreError::App {
                app: "derper".to_string(),
                message: "derper.go stealth patch failed: missing upstream anchor '/generate_204'"
                    .to_string(),
            });
        }
        patched = patched.replace(DERPER_GEN204_ORIG, DERPER_GEN204_PATCHED);
    }

    // Hunk 3: hijack root `DERP\n` response
    if has_orig_root {
        patched = patched.replace(DERPER_ROOT_ORIG, "closeConn(w)");
    } else if !(has_hunk1 && has_hunk2 && patched.matches("closeConn(w)").count() >= 2) {
        return Err(CoreError::App {
            app: "derper".to_string(),
            message:
                r#"derper.go stealth patch failed: missing upstream anchor 'fmt.Fprintf(w, "DERP\n")'"#
                    .to_string(),
        });
    }

    Ok(patched)
}

fn detect_public_ipv4(http: &dyn HttpClient) -> String {
    let req = HttpRequest {
        url: "https://ifconfig.me",
        user_agent: Some(DEFAULT_USER_AGENT),
        connect_timeout: Duration::from_secs(3),
        max_timeout: Duration::from_secs(5),
        ip_preference: IpPreference::Ipv4Only,
    };
    http.get_text(&req)
        .ok()
        .filter(|r| (200..=299).contains(&r.status))
        .map(|r| r.body.trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

/// Self-compile stealth-patched Tailscale DERPer (`v1.94.2`), deploy systemd unit, and sync firewall rules.
pub fn install_derper(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting stealth DERPer build & deployment",
        None,
        &[("tag", TAILSCALE_DERPER_TAG)],
    );

    ensure_go_installed(ctx, http, runner)?;

    let is_cn = ctx.effective_is_cn();
    let target_user = ctx.initial_user();
    let target_home = ctx.user_home(&target_user);
    let path_env = build_go_path_env(&target_home);
    let home_str = target_home.to_string_lossy().into_owned();

    let mut build_env: Vec<(&str, &str)> = vec![("PATH", &path_env), ("HOME", &home_str)];
    if is_cn {
        build_env.push(("GOPROXY", CN_GOPROXY));
    }

    let build_dir = TempDir::new()?;
    let ts_dir = build_dir.path().join("tailscale");

    if !ctx.no_sys {
        network::git_clone_with_fallback(
            &ts_dir,
            TAILSCALE_GIT_URL,
            &["-b", TAILSCALE_DERPER_TAG, "--depth", "1"],
            is_cn,
            runner,
        )?;
    }

    let derper_dir = ts_dir.join("cmd/derper");
    let derper_go = derper_dir.join("derper.go");
    if derper_go.is_file() {
        let orig = fs::read_to_string(&derper_go)?;
        let patched = patch_derper_source(&orig)?;
        sys::write_file_atomic(&derper_go, patched.as_bytes(), Some(0o644))?;
    } else if !ctx.is_custom_root() && !ctx.no_sys {
        return Err(CoreError::App {
            app: "derper".to_string(),
            message: format!("derper.go not found at {}", derper_go.display()),
        });
    }

    let built_derper = build_dir.path().join("derper");
    let built_derper_str = built_derper.to_string_lossy().into_owned();
    let derper_dir_str = derper_dir.to_string_lossy().into_owned();

    if !ctx.no_sys {
        let out = shared::run_as_user(
            ctx,
            &target_user,
            "/usr/local/go/bin/go",
            &[
                "build",
                "-C",
                &derper_dir_str,
                "-v",
                "-o",
                &built_derper_str,
            ],
            &build_env,
            Duration::from_secs(600),
            runner,
        )?;
        if !out.success() {
            return Err(CoreError::App {
                app: "derper".to_string(),
                message: format!(
                    "derper go build failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                ),
            });
        }
    }

    let derper_dest = ctx.path("/usr/bin/derper");
    if let Some(parent) = derper_dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if built_derper.is_file() {
        fs::copy(&built_derper, &derper_dest)?;
        fs::set_permissions(&derper_dest, fs::Permissions::from_mode(0o755))?;
    } else if !ctx.is_custom_root() && !ctx.no_sys {
        return Err(CoreError::App {
            app: "derper".to_string(),
            message: format!(
                "go build did not produce expected binary at {}",
                built_derper.display()
            ),
        });
    }

    let cert_dir = ctx.path("/opt/derper/certs");
    fs::create_dir_all(&cert_dir)?;
    let ip = detect_public_ipv4(http);

    // Clean legacy pre-generated self-signed certs that fail SAN checks
    for ext in ["crt", "key"] {
        let legacy_cert = cert_dir.join(format!("{ip}.{ext}"));
        if legacy_cert.exists() || legacy_cert.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&legacy_cert);
        }
    }

    shared::create_system_user(ctx, "derper", runner)?;
    if !ctx.no_sys && !ctx.is_custom_root() {
        let opt_derper = PathBuf::from("/opt/derper");
        let _ = runner.run(
            "chown",
            &["-R", "derper:derper", &opt_derper.to_string_lossy()],
            Duration::from_secs(15),
        );
    }

    let mut vars = BTreeMap::new();
    vars.insert("DERPER_HOSTNAME".to_string(), ip.clone());
    vars.insert(
        "DERPER_CERT_DIR".to_string(),
        "/opt/derper/certs".to_string(),
    );
    shared::deploy_systemd_service(
        ctx,
        "derper",
        "templates/apps/derper/derper.service",
        &vars,
        true,
        runner,
    )?;

    let sec = ctx.security_context();
    firewall::add_fw_rule(&sec, "34781", "tcp", "DERP_Relay", runner)?;
    firewall::add_fw_rule(&sec, "3478", "udp", "DERP_STUN", runner)?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "stealth DERPer deployment complete",
        None,
        &[("hostname", &ip)],
    );

    Ok(AppReport::new(
        "derper",
        "install",
        true,
        format!("Stealth DERPer deployed (hostname: {ip})"),
    )
    .with_detail("hostname", ip)
    .with_detail("tag", TAILSCALE_DERPER_TAG)
    .with_detail("fw_rules", "DERP_Relay,DERP_STUN"))
}

/// Remove DERPer service, binary, `/opt/derper`, system user, and firewall rules.
pub fn uninstall_derper(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting DERPer uninstallation",
        None,
        &[],
    );

    shared::remove_systemd_service(ctx, "derper", runner)?;

    let bin = ctx.path("/usr/bin/derper");
    if bin.exists() || bin.symlink_metadata().is_ok() {
        let _ = fs::remove_file(&bin);
    }
    let opt_dir = ctx.path("/opt/derper");
    if opt_dir.exists() {
        fs::remove_dir_all(&opt_dir)?;
    }

    shared::remove_system_user(ctx, "derper", false, runner)?;

    let sec = ctx.security_context();
    firewall::remove_fw_rule(&sec, "DERP_Relay", runner)?;
    firewall::remove_fw_rule(&sec, "DERP_STUN", runner)?;

    Ok(AppReport::new(
        "derper",
        "uninstall",
        true,
        "DERPer removed from system",
    ))
}
