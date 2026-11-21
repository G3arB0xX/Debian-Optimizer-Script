use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tempfile::TempDir;

use debopty_core::apps::{
    AppContext, easytier, ferron, golang, realm, rust as rust_app, tailscale,
};
use debopty_core::error::CoreError;
use debopty_core::network::{HttpResponse, MockHttpClient};
use debopty_core::sys::{self, CmdOutput, MockCommandRunner};

fn ok(stdout: &str) -> CmdOutput {
    CmdOutput {
        code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn fail(code: i32, stderr: &str) -> CmdOutput {
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

fn setup_nft_base(ctx: &AppContext) {
    let nft_d = ctx.path("/etc/nftables.d/debopti");
    fs::create_dir_all(&nft_d).unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/nftables.conf"),
        b"table inet filter {\n    chain input {\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n",
        Some(0o644),
    )
    .unwrap();
}

fn setup_fish_sot(ctx: &AppContext, user: &str) {
    make_executable(&ctx.path("/usr/bin/fish"));
    let home = if user == "root" {
        ctx.path("/root")
    } else {
        ctx.path(&format!("/home/{user}"))
    };
    fs::create_dir_all(home.join(".config/fish/conf.d")).unwrap();
    let passwd = if user == "root" {
        "root:x:0:0:root:/root:/bin/bash\n".to_string()
    } else {
        format!("root:x:0:0:root:/root:/bin/bash\n{user}:x:1000:1000::/home/{user}:/usr/bin/fish\n")
    };
    sys::write_file_atomic(&ctx.path("/etc/passwd"), passwd.as_bytes(), Some(0o644)).unwrap();
}

#[test]
fn realm_install_preserves_existing_config_and_uninstall_cleans_up() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());

    let http = MockHttpClient::new();
    http.on_get(
        "https://api.github.com/repos/zhboner/realm/releases/latest",
        HttpResponse {
            status: 200,
            body: r#"{"tag_name":"v2.8.1"}"#.to_string(),
        },
    );
    http.on_download(
        "https://github.com/zhboner/realm/releases/download/v2.8.1/realm-x86_64-unknown-linux-gnu.tar.gz",
        200,
        b"FAKE_TAR_GZ".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("tar", ok(""));
    runner.on_exact("id", &["-u", "realm"], fail(1, "no such user"));
    runner.on_prog("useradd", ok(""));
    runner.on_prog("systemctl", ok(""));
    runner.on_prog("userdel", ok(""));

    // First install renders default config.toml
    let r1 = realm::install_realm(&ctx, &http, &runner).unwrap();
    assert_eq!(r1.app, "realm");
    assert_eq!(
        r1.details.get("version").map(String::as_str),
        Some("v2.8.1")
    );
    assert!(ctx.path("/etc/realm/config.toml").is_file());
    assert!(ctx.path("/etc/systemd/system/realm.service").is_file());
    assert!(r1.preserved_user_custom.is_empty());

    // U8: customize config.toml and reinstall; custom content must be preserved
    let custom_cfg = "# user custom realm rules\n[[endpoints]]\nlisten = \"0.0.0.0:9000\"\n";
    sys::write_file_atomic(
        &ctx.path("/etc/realm/config.toml"),
        custom_cfg.as_bytes(),
        Some(0o644),
    )
    .unwrap();
    runner.on_exact("id", &["-u", "realm"], ok("999\n"));

    let r2 = realm::install_realm(&ctx, &http, &runner).unwrap();
    assert!(
        r2.preserved_user_custom
            .iter()
            .any(|p| p.contains("/etc/realm/config.toml"))
    );
    assert_eq!(
        fs::read_to_string(ctx.path("/etc/realm/config.toml")).unwrap(),
        custom_cfg
    );

    // Uninstall cleans dirs, unit, and user
    let u = realm::uninstall_realm(&ctx, &runner).unwrap();
    assert_eq!(u.action, "uninstall");
    assert!(!ctx.path("/opt/realm").exists());
    assert!(!ctx.path("/etc/realm").exists());
    assert!(!ctx.path("/etc/systemd/system/realm.service").exists());
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "userdel" && a == &["realm"])
    );
}

#[test]
fn realm_rejects_unsupported_arch_riscv64_with_e4004() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("riscv64".to_string());
    let http = MockHttpClient::new();
    let runner = MockCommandRunner::new();

    let err = realm::install_realm(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
    assert!(matches!(err, CoreError::App { ref app, .. } if app == "realm"));
}

#[test]
fn realm_fallback_version_when_github_api_offline_and_propagates_e3001_on_download_failure() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("aarch64".to_string());

    let http = MockHttpClient::new();
    // Both API and release page unmocked -> falls back to v2.7.0, then download fails -> E3001
    let runner = MockCommandRunner::new();
    let err = realm::install_realm(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E3001");
    assert!(
        http.requested_urls()
            .iter()
            .any(|u| u.contains("v2.7.0/realm-aarch64-unknown-linux-gnu.tar.gz"))
    );
}

#[test]
fn ferron_install_legacy_migration_u8_kdl_idempotent_and_webroot_preserve() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    sys::write_file_atomic(
        &ctx.path("/etc/os-release"),
        b"ID=debian\nVERSION_CODENAME=bookworm\n",
        Some(0o644),
    )
    .unwrap();
    // Pre-populate legacy /etc/ferron.kdl and custom /var/www/ferron
    let user_kdl = "server {\n    address \"0.0.0.0:8080\"\n    root \"/srv/custom\"\n}\n";
    sys::write_file_atomic(
        &ctx.path("/etc/ferron.kdl"),
        user_kdl.as_bytes(),
        Some(0o644),
    )
    .unwrap();
    fs::create_dir_all(ctx.path("/var/www/ferron")).unwrap();
    sys::write_file_atomic(
        &ctx.path("/var/www/ferron/index.html"),
        b"<h1>custom site</h1>\n",
        Some(0o644),
    )
    .unwrap();

    let http = MockHttpClient::new();
    http.on_download("https://deb.ferron.sh/signing.pgp", 200, b"PGPKEY".to_vec());

    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg", fail(1, "not installed"));
    runner.on_prog("apt-get", ok(""));
    runner.on_prog("gpg", ok(""));
    runner.on_prog("getent", ok("debopti-certs:x:998:\n"));
    runner.on_prog("id", ok("998\n"));
    runner.on_prog("usermod", ok(""));
    runner.on_prog("systemctl", ok(""));

    let r1 = ferron::install_ferron(&ctx, &http, &runner).unwrap();
    assert_eq!(
        r1.details.get("codename").map(String::as_str),
        Some("bookworm")
    );
    assert!(!ctx.path("/etc/ferron.kdl").exists());
    let kdl_text = fs::read_to_string(ctx.path("/etc/ferron/config.kdl")).unwrap();
    assert!(kdl_text.contains("io_uring #false"));
    assert!(kdl_text.contains("address \"0.0.0.0:8080\""));
    assert_eq!(kdl_text.matches("io_uring #false").count(), 1);
    assert_eq!(
        fs::read_to_string(ctx.path("/var/www/ferron/index.html")).unwrap(),
        "<h1>custom site</h1>\n"
    );
    assert!(
        ctx.path("/etc/systemd/system/ferron.service.d/security.conf")
            .is_file()
    );
    assert!(
        fs::read_to_string(ctx.path("/etc/apt/sources.list.d/ferron.list"))
            .unwrap()
            .contains("https://deb.ferron.sh bookworm main")
    );

    // Reinstall is idempotent: io_uring #false is NOT duplicated
    let _r2 = ferron::install_ferron(&ctx, &http, &runner).unwrap();
    let kdl_after = fs::read_to_string(ctx.path("/etc/ferron/config.kdl")).unwrap();
    assert_eq!(kdl_after.matches("io_uring #false").count(), 1);

    // Uninstall cleans all paths
    ferron::uninstall_ferron(&ctx, &runner).unwrap();
    assert!(!ctx.path("/etc/ferron").exists());
    assert!(!ctx.path("/var/www/ferron").exists());
    assert!(!ctx.path("/etc/apt/sources.list.d/ferron.list").exists());
    assert!(!ctx.path("/etc/systemd/system/ferron.service.d").exists());
}

#[test]
fn ferron_install_fails_with_e4004_when_apt_install_ferron_fails() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    sys::write_file_atomic(
        &ctx.path("/etc/os-release"),
        b"ID=debian\nVERSION_CODENAME=trixie\n",
        Some(0o644),
    )
    .unwrap();

    let http = MockHttpClient::new();
    http.on_download("https://deb.ferron.sh/signing.pgp", 200, b"PGPKEY".to_vec());

    let runner = MockCommandRunner::new();
    // Base deps already installed, ferron package missing and fails to install
    runner.on_prog("dpkg-query", ok("install ok installed"));
    runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "ferron"],
        fail(1, "not installed"),
    );
    runner.on_prog("gpg", ok(""));
    runner.on_prog("apt-get", fail(100, "Unable to locate package ferron"));

    let err = ferron::install_ferron(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
}

#[test]
fn rust_install_cn_mirror_tracks_build_essential_and_uninstall_purges_it() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;
    ctx.arch_override = Some("x86_64".to_string());
    ctx.sudo_user_override = Some("alice".to_string());
    ctx.euid_override = Some(0);
    setup_fish_sot(&ctx, "alice");

    let http = MockHttpClient::new();
    http.on_download(
        "https://rsproxy.cn/rustup/dist/x86_64-unknown-linux-gnu/rustup-init",
        200,
        b"#!/bin/sh\nexit 0\n".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_exact("dpkg", &["-s", "build-essential"], fail(1, "not installed"));
    runner.on_prog("apt-get", ok(""));
    runner.on_prog("sudo", ok("rustc 1.85.0 (4d91de4e4 2025-02-17)\n"));
    runner.on_exact("fish", &["--version"], ok("fish, version 3.7.1\n"));

    let report = rust_app::install_rust(&ctx, &http, &runner).unwrap();
    assert_eq!(
        report.details.get("rustc_version").map(String::as_str),
        Some("1.85.0")
    );
    assert_eq!(
        ctx.config_store()
            .get("RUST_INSTALLED_BUILD_ESSENTIAL")
            .unwrap()
            .as_deref(),
        Some("true")
    );
    let cargo_cfg = fs::read_to_string(ctx.path("/home/alice/.cargo/config.toml")).unwrap();
    assert!(cargo_cfg.contains("rsproxy.cn"));
    let fish_path =
        fs::read_to_string(ctx.path("/home/alice/.config/fish/conf.d/debopti_path.fish")).unwrap();
    assert!(fish_path.contains("fish_add_path $HOME/.cargo/bin"));

    // Uninstall purges build-essential because this module installed it
    let u = rust_app::uninstall_rust(&ctx, &runner).unwrap();
    assert_eq!(
        u.details.get("purged_build_essential").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        ctx.config_store()
            .get("RUST_INSTALLED_BUILD_ESSENTIAL")
            .unwrap()
            .as_deref(),
        Some("false")
    );
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "apt-get" && a == &["purge", "-yq", "build-essential"])
    );
    assert!(!ctx.path("/home/alice/.cargo").exists());
}

#[test]
fn rust_u8_preserves_preexisting_build_essential_and_custom_cargo_config() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;
    ctx.arch_override = Some("x86_64".to_string());
    ctx.sudo_user_override = Some("root".to_string());
    ctx.euid_override = Some(0);

    // Pre-existing custom cargo config (not rsproxy)
    let custom_toml = "[net]\nretry = 5\n";
    sys::write_file_atomic(
        &ctx.path("/root/.cargo/config.toml"),
        custom_toml.as_bytes(),
        Some(0o644),
    )
    .unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://rsproxy.cn/rustup/dist/x86_64-unknown-linux-gnu/rustup-init",
        200,
        b"#!/bin/sh\nexit 0\n".to_vec(),
    );

    let runner = MockCommandRunner::new();
    // build-essential already installed!
    runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "build-essential"],
        ok("install ok installed"),
    );
    ctx.sudo_user_override = Some("alice".to_string());
    setup_fish_sot(&ctx, "alice");
    sys::write_file_atomic(
        &ctx.path("/home/alice/.cargo/config.toml"),
        custom_toml.as_bytes(),
        Some(0o644),
    )
    .unwrap();
    runner.on_prog("sudo", ok("rustc 1.84.0\n"));
    runner.on_exact("fish", &["--version"], ok("fish, version 3.7.1\n"));

    let r = rust_app::install_rust(&ctx, &http, &runner).unwrap();
    assert!(!r.preserved_user_custom.is_empty());
    assert!(!r.warnings.is_empty());
    assert_eq!(
        fs::read_to_string(ctx.path("/home/alice/.cargo/config.toml")).unwrap(),
        custom_toml
    );
    assert!(
        ctx.config_store()
            .get("RUST_INSTALLED_BUILD_ESSENTIAL")
            .unwrap()
            .is_none()
    );

    // Uninstall MUST NOT purge pre-existing build-essential
    let u = rust_app::uninstall_rust(&ctx, &runner).unwrap();
    assert!(
        u.preserved_user_custom
            .contains(&"build-essential".to_string())
    );
    assert!(
        !runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "apt-get" && a.contains(&"build-essential".to_string()))
    );
}

#[test]
fn rust_install_fails_with_e4004_on_rustup_init_failure_and_unsupported_arch() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("riscv64".to_string());
    let http = MockHttpClient::new();
    let runner = MockCommandRunner::new();

    let err = rust_app::install_rust(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");

    ctx.arch_override = Some("x86_64".to_string());
    ctx.sudo_user_override = Some("alice".to_string());
    http.on_download(
        "https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init",
        200,
        b"#!/bin/sh\nexit 1\n".to_vec(),
    );
    runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "build-essential"],
        ok("install ok installed"),
    );
    runner.on_prog("sudo", fail(1, "network timeout"));

    let err2 = rust_app::install_rust(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err2.error_code(), "E4004");
}

#[test]
fn easytier_fresh_install_disables_autostart_and_syncs_fw_and_f2b() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_nft_base(&ctx);
    setup_fish_sot(&ctx, "root");
    make_executable(&ctx.path("/usr/bin/unzip"));

    let http = MockHttpClient::new();
    http.on_download(
        "https://raw.githubusercontent.com/EasyTier/EasyTier/main/script/install.sh",
        200,
        b"#!/bin/bash\nexit 0\n".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("bash", ok(""));
    runner.on_prog("systemctl", ok(""));
    runner.on_prog("nft", ok(""));
    runner.on_prog("fail2ban-client", ok(""));
    runner.on_exact("fish", &["--version"], ok("fish, version 3.7.1\n"));

    let r = easytier::install_easytier(&ctx, &http, &runner).unwrap();
    assert_eq!(r.action, "install");
    assert!(ctx.path("/etc/profile.d/debopti-easytier.sh").is_file());
    assert!(
        ctx.path("/etc/systemd/system/easytier@.service.d/security.conf")
            .is_file()
    );
    assert!(
        ctx.path("/etc/nftables.d/debopti/Easytier_P2P.nft")
            .is_file()
    );
    let nft_rule =
        fs::read_to_string(ctx.path("/etc/nftables.d/debopti/Easytier_P2P.nft")).unwrap();
    assert!(nft_rule.contains("11010 - 11015"));
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["disable", "easytier@default"])
    );
}

#[test]
fn easytier_update_mode_u8_preserves_instances_and_offline_uninstall() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;
    setup_nft_base(&ctx);
    make_executable(&ctx.path("/usr/bin/unzip"));
    // Existing easytier-core triggers update mode
    make_executable(&ctx.path("/opt/easytier/easytier-core"));

    let http = MockHttpClient::new();
    http.on_download(
        "https://gh-proxy.com/https://raw.githubusercontent.com/EasyTier/EasyTier/main/script/install.sh",
        200,
        b"#!/bin/bash\nexit 0\n".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("bash", ok(""));
    runner.on_prog("systemctl", ok(""));
    runner.on_prog("nft", ok(""));
    runner.on_prog("fail2ban-client", ok(""));

    let r = easytier::install_easytier(&ctx, &http, &runner).unwrap();
    assert_eq!(r.action, "update");
    assert!(!r.preserved_user_custom.is_empty());
    // U8: update mode MUST NOT disable user's easytier@default
    assert!(
        !runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["disable", "easytier@default"])
    );

    // Offline uninstall (empty MockHttpClient) still succeeds and cleans local files + fw rule
    let offline_http = MockHttpClient::new();
    let u = easytier::uninstall_easytier(&ctx, &offline_http, &runner).unwrap();
    assert!(!u.warnings.is_empty());
    assert!(!ctx.path("/opt/easytier").exists());
    assert!(!ctx.path("/etc/profile.d/debopti-easytier.sh").exists());
    assert!(
        !ctx.path("/etc/nftables.d/debopti/Easytier_P2P.nft")
            .exists()
    );
}

#[test]
fn easytier_cn_all_proxies_failed_returns_e4004() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;
    make_executable(&ctx.path("/usr/bin/unzip"));

    let http = MockHttpClient::new();
    http.on_download(
        "https://gh-proxy.com/https://raw.githubusercontent.com/EasyTier/EasyTier/main/script/install.sh",
        200,
        b"#!/bin/bash\nexit 1\n".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("bash", fail(1, "proxy error"));

    let err = easytier::install_easytier(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
}

#[test]
fn tailscale_install_update_override_fw_sync_and_uninstall() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    setup_nft_base(&ctx);

    let http = MockHttpClient::new();
    http.on_download(
        "https://tailscale.com/install.sh",
        200,
        b"#!/bin/sh\nexit 0\n".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("sh", ok(""));
    runner.on_prog("systemctl", ok(""));
    runner.on_prog("nft", ok(""));
    runner.on_prog("fail2ban-client", ok(""));
    runner.on_prog("apt-get", ok(""));

    let r1 = tailscale::install_tailscale(&ctx, &http, &runner).unwrap();
    assert_eq!(r1.action, "install");
    assert!(
        ctx.path("/etc/systemd/system/tailscaled.service.d/security.conf")
            .is_file()
    );
    let fw_rule =
        fs::read_to_string(ctx.path("/etc/nftables.d/debopti/Tailscale_P2P.nft")).unwrap();
    assert!(fw_rule.contains("udp dport 41641 accept"));

    // Second run with tailscale binary present detects update mode
    make_executable(&ctx.path("/usr/bin/tailscale"));
    let r2 = tailscale::install_tailscale(&ctx, &http, &runner).unwrap();
    assert_eq!(r2.action, "update");

    // Uninstall removes override, binary, state, and firewall rule
    tailscale::uninstall_tailscale(&ctx, &runner).unwrap();
    assert!(!ctx.path("/usr/bin/tailscale").exists());
    assert!(
        !ctx.path("/etc/systemd/system/tailscaled.service.d")
            .exists()
    );
    assert!(
        !ctx.path("/etc/nftables.d/debopti/Tailscale_P2P.nft")
            .exists()
    );
}

#[test]
fn tailscale_installer_failure_returns_e4004() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    let http = MockHttpClient::new();
    http.on_download(
        "https://tailscale.com/install.sh",
        200,
        b"#!/bin/sh\nexit 1\n".to_vec(),
    );
    let runner = MockCommandRunner::new();
    runner.on_prog("sh", fail(1, "apt timeout"));

    let err = tailscale::install_tailscale(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
}

#[test]
fn golang_install_arm64_and_cn_domain_and_uninstall() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;
    ctx.arch_override = Some("aarch64".to_string());
    ctx.sudo_user_override = Some("alice".to_string());
    setup_fish_sot(&ctx, "alice");

    let http = MockHttpClient::new();
    http.on_get(
        "https://golang.google.cn/VERSION?m=text",
        HttpResponse {
            status: 200,
            body: "go1.24.2\ntime 2025-04-01T00:00:00Z\n".to_string(),
        },
    );
    http.on_download(
        "https://golang.google.cn/dl/go1.24.2.linux-arm64.tar.gz",
        200,
        b"FAKE_GO_TAR".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("tar", ok(""));
    runner.on_exact("fish", &["--version"], ok("fish, version 3.7.1\n"));

    let r = golang::install_go(&ctx, &http, &runner).unwrap();
    assert_eq!(
        r.details.get("version").map(String::as_str),
        Some("go1.24.2")
    );
    assert_eq!(r.details.get("arch").map(String::as_str), Some("arm64"));
    assert_eq!(
        r.details.get("domain").map(String::as_str),
        Some("golang.google.cn")
    );

    let fish_vars =
        fs::read_to_string(ctx.path("/home/alice/.config/fish/conf.d/debopti_vars.fish")).unwrap();
    assert!(fish_vars.contains("set -gx GOPROXY https://goproxy.cn,direct"));

    golang::uninstall_go(&ctx, &runner).unwrap();
    let fish_vars_after =
        fs::read_to_string(ctx.path("/home/alice/.config/fish/conf.d/debopti_vars.fish")).unwrap();
    assert!(!fish_vars_after.contains("GOPROXY"));
}

#[test]
fn golang_rejects_unsupported_arch_riscv64_with_e4004() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("riscv64".to_string());
    let http = MockHttpClient::new();
    let runner = MockCommandRunner::new();

    let err = golang::install_go(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
}

#[test]
fn caddy_install_u8_preserves_caddyfile_and_index_syncs_fw_and_uninstall() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.sudo_user_override = Some("alice".to_string());
    setup_nft_base(&ctx);
    make_executable(&ctx.path("/usr/local/go/bin/go"));

    // Pre-existing custom Caddyfile & index.html (U8)
    let custom_caddyfile = "example.com {\n    respond \"hello\"\n}\n";
    let custom_index = "<html>custom caddy page</html>\n";
    sys::write_file_atomic(
        &ctx.path("/etc/caddy/Caddyfile"),
        custom_caddyfile.as_bytes(),
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/usr/share/caddy/index.html"),
        custom_index.as_bytes(),
        Some(0o644),
    )
    .unwrap();

    let http = MockHttpClient::new();
    let runner = MockCommandRunner::new();
    runner.on_prog("sudo", ok(""));
    runner.on_prog("systemctl", ok(""));
    runner.on_prog("setcap", ok(""));
    runner.on_exact("id", &["-u", "caddy"], fail(1, "no such user"));
    runner.on_prog("useradd", ok(""));
    runner.on_prog("groupadd", ok(""));
    runner.on_prog("usermod", ok(""));
    runner.on_prog("nft", ok(""));
    runner.on_prog("fail2ban-client", ok(""));
    runner.on_prog("userdel", ok(""));

    let r = golang::install_caddy(&ctx, &http, &runner).unwrap();
    assert_eq!(r.app, "caddy");
    assert!(
        r.preserved_user_custom
            .contains(&"/etc/caddy/Caddyfile".to_string())
    );
    assert!(
        r.preserved_user_custom
            .contains(&"/usr/share/caddy/index.html".to_string())
    );
    assert_eq!(
        fs::read_to_string(ctx.path("/etc/caddy/Caddyfile")).unwrap(),
        custom_caddyfile
    );
    assert!(ctx.path("/etc/systemd/system/caddy.service").is_file());
    let fw_rule = fs::read_to_string(ctx.path("/etc/nftables.d/debopti/Caddy_Web.nft")).unwrap();
    assert!(fw_rule.contains("tcp dport { 80,443 } accept"));
    assert!(fw_rule.contains("udp dport { 80,443 } accept"));

    runner.on_exact("id", &["-u", "caddy"], ok("997\n"));
    golang::uninstall_caddy(&ctx, &runner).unwrap();
    assert!(!ctx.path("/etc/caddy").exists());
    assert!(!ctx.path("/etc/nftables.d/debopti/Caddy_Web.nft").exists());
}

#[test]
fn derper_patch_source_applies_3_hunks_is_idempotent_and_fails_loudly_on_missing_anchors() {
    let sample_src = r#"package main

import (
    "fmt"
    "net/http"
)

func main() {
    mux := http.NewServeMux()
    mux.HandleFunc("/generate_204", derphttp.ServeNoContent)
    mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
        fmt.Fprintf(w, "DERP\n")
    })
}
"#;

    let patched = golang::patch_derper_source(sample_src).unwrap();
    assert!(patched.contains("func closeConn(w http.ResponseWriter)"));
    assert!(patched.contains(r#"if r.UserAgent() == "Go-http-client/1.1""#));
    assert!(!patched.contains(r#"fmt.Fprintf(w, "DERP\n")"#));
    assert!(patched.matches("closeConn(w)").count() >= 2);

    // 2nd pass must be 100% idempotent
    let patched_twice = golang::patch_derper_source(&patched).unwrap();
    assert_eq!(patched, patched_twice);

    // Missing `func main()` anchor -> E4004
    let missing_main = sample_src.replace("func main()", "func run()");
    let e1 = golang::patch_derper_source(&missing_main).unwrap_err();
    assert_eq!(e1.error_code(), "E4004");

    // Missing `/generate_204` anchor -> E4004
    let missing_204 = sample_src.replace("derphttp.ServeNoContent", "derphttp.ServeOther");
    let e2 = golang::patch_derper_source(&missing_204).unwrap_err();
    assert_eq!(e2.error_code(), "E4004");

    // Missing `DERP\n` root anchor -> E4004
    let missing_root = sample_src.replace(r#"fmt.Fprintf(w, "DERP\n")"#, r#"fmt.Fprintf(w, "OK")"#);
    let e3 = golang::patch_derper_source(&missing_root).unwrap_err();
    assert_eq!(e3.error_code(), "E4004");
}

#[test]
fn derper_install_cleans_legacy_certs_deploys_service_syncs_fw_and_uninstall() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.sudo_user_override = Some("alice".to_string());
    setup_nft_base(&ctx);
    make_executable(&ctx.path("/usr/local/go/bin/go"));

    // Pre-populate legacy self-signed certs for 203.0.113.10
    let cert_dir = ctx.path("/opt/derper/certs");
    fs::create_dir_all(&cert_dir).unwrap();
    fs::write(cert_dir.join("203.0.113.10.crt"), "OLD_CERT").unwrap();
    fs::write(cert_dir.join("203.0.113.10.key"), "OLD_KEY").unwrap();

    let http = MockHttpClient::new();
    http.on_get(
        "https://ifconfig.me",
        HttpResponse {
            status: 200,
            body: "203.0.113.10\n".to_string(),
        },
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("git", ok(""));
    runner.on_prog("sudo", ok(""));
    runner.on_exact("id", &["-u", "derper"], fail(1, "no such user"));
    runner.on_prog("useradd", ok(""));
    runner.on_prog("systemctl", ok(""));
    runner.on_prog("nft", ok(""));
    runner.on_prog("fail2ban-client", ok(""));
    runner.on_prog("userdel", ok(""));

    let r = golang::install_derper(&ctx, &http, &runner).unwrap();
    assert_eq!(
        r.details.get("hostname").map(String::as_str),
        Some("203.0.113.10")
    );
    assert!(!cert_dir.join("203.0.113.10.crt").exists());
    assert!(!cert_dir.join("203.0.113.10.key").exists());

    let unit = fs::read_to_string(ctx.path("/etc/systemd/system/derper.service")).unwrap();
    assert!(unit.contains("-hostname 203.0.113.10"));
    assert!(unit.contains("-certdir /opt/derper/certs"));

    let relay_nft = fs::read_to_string(ctx.path("/etc/nftables.d/debopti/DERP_Relay.nft")).unwrap();
    assert!(relay_nft.contains("tcp dport 34781 accept"));
    let stun_nft = fs::read_to_string(ctx.path("/etc/nftables.d/debopti/DERP_STUN.nft")).unwrap();
    assert!(stun_nft.contains("udp dport 3478 accept"));

    runner.on_exact("id", &["-u", "derper"], ok("996\n"));
    golang::uninstall_derper(&ctx, &runner).unwrap();
    assert!(!ctx.path("/opt/derper").exists());
    assert!(!ctx.path("/etc/nftables.d/debopti/DERP_Relay.nft").exists());
    assert!(!ctx.path("/etc/nftables.d/debopti/DERP_STUN.nft").exists());
}

#[test]
fn derper_fallback_ip_when_ifconfig_offline_and_build_failure_returns_e4004() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.sudo_user_override = Some("alice".to_string());
    setup_nft_base(&ctx);
    make_executable(&ctx.path("/usr/local/go/bin/go"));

    // Build failure returns E4004
    let http = MockHttpClient::new();
    let runner = MockCommandRunner::new();
    runner.on_prog("git", ok(""));
    runner.on_prog("sudo", fail(2, "compile error"));

    let err = golang::install_derper(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");

    // When build succeeds and ifconfig.me is offline, falls back to 127.0.0.1
    runner.on_prog("sudo", ok(""));
    runner.on_exact("id", &["-u", "derper"], ok("996\n"));
    runner.on_prog("systemctl", ok(""));
    runner.on_prog("nft", ok(""));
    runner.on_prog("fail2ban-client", ok(""));

    let r = golang::install_derper(&ctx, &http, &runner).unwrap();
    assert_eq!(
        r.details.get("hostname").map(String::as_str),
        Some("127.0.0.1")
    );
}
