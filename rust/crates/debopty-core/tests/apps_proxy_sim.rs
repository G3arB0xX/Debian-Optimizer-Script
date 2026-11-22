use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use debopty_core::apps::freship::{self, FRESHIP_REPO_RAW, FreshipSelection, FreshipWorkMode};
use debopty_core::apps::shared::AppContext;
use debopty_core::apps::warp::{self, WARP_ENDPOINT, WARP_PUBLIC_KEY};
use debopty_core::apps::xray::{self, XrayRuleset};
use debopty_core::error::Result;
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

fn sample_freship_map_json() -> &'static str {
    r#"{
      "version": "4.2.0",
      "continents": [
        {
          "id": "AS",
          "name": "Asia",
          "countries": [
            {
              "id": "JP",
              "name": "Japan",
              "keyword_file": "jp.txt",
              "states": [
                {
                  "id": "Default",
                  "name": "Default",
                  "cities": [
                    { "id": "Tokyo", "name": "Tokyo" },
                    { "id": "Osaka", "name": "Osaka" }
                  ]
                }
              ]
            },
            {
              "id": "UK",
              "name": "United Kingdom",
              "keyword_file": "uk.txt",
              "states": [
                {
                  "id": "ENG",
                  "name": "England",
                  "cities": [
                    { "id": "London", "name": "London" }
                  ]
                }
              ]
            }
          ]
        }
      ]
    }"#
}

// ============================================================================
// WARP & Usque & wgcf-xray tests
// ============================================================================

#[test]
fn warp_fresh_install_registers_configures_wireguard_and_injects_override() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    sys::write_file_atomic(
        &ctx.path("/etc/os-release"),
        b"ID=debian\nVERSION_CODENAME=bookworm\n",
        Some(0o644),
    )
    .unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://pkg.cloudflareclient.com/pubkey.gpg",
        200,
        b"FAKE-CF-PGP".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", err_out(1, "not installed"));
    runner.on_prog("apt-get", ok_out(""));
    runner.on_prog("gpg", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("warp-cli", ok_out("Status update: Disconnected"));

    let report = warp::install_warp(&ctx, &http, &runner).unwrap();
    assert_eq!(report.app, "warp");
    assert_eq!(report.action, "install");
    assert!(report.changed);
    assert!(report.preserved_user_custom.is_empty());

    let repo_list = ctx.path("/etc/apt/sources.list.d/cloudflare-client.list");
    assert!(repo_list.is_file());
    assert!(
        fs::read_to_string(&repo_list)
            .unwrap()
            .contains("https://pkg.cloudflareclient.com/ bookworm main")
    );

    let override_conf = ctx.path("/etc/systemd/system/warp-svc.service.d/security.conf");
    assert!(override_conf.is_file());
    assert!(
        fs::read_to_string(&override_conf)
            .unwrap()
            .contains("MemoryMax=120M")
    );

    let invs = runner.invocations();
    assert!(
        invs.iter()
            .any(|(p, a)| { p == "warp-cli" && a == &["--accept-tos", "registration", "new"] })
    );
    assert!(invs.iter().any(|(p, a)| {
        p == "warp-cli" && a == &["--accept-tos", "tunnel", "protocol", "set", "WireGuard"]
    }));
    assert!(
        invs.iter()
            .any(|(p, a)| { p == "warp-cli" && a == &["--accept-tos", "proxy", "port", "40000"] })
    );
}

#[test]
fn warp_update_preserves_existing_registration_and_mode_u8() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    make_executable(&ctx.path("/usr/bin/warp-cli"));
    sys::write_file_atomic(
        &ctx.path("/etc/os-release"),
        b"ID=debian\nVERSION_CODENAME=trixie\n",
        Some(0o644),
    )
    .unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://pkg.cloudflareclient.com/pubkey.gpg",
        200,
        b"FAKE-CF-PGP".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", ok_out("install ok installed"));
    runner.on_prog("apt-get", ok_out(""));
    runner.on_prog("gpg", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("warp-cli", ok_out("Status update: Connected"));

    let report = warp::install_warp(&ctx, &http, &runner).unwrap();
    assert_eq!(report.action, "update");
    assert!(!report.preserved_user_custom.is_empty());

    let invs = runner.invocations();
    assert!(
        !invs
            .iter()
            .any(|(p, a)| p == "warp-cli" && a.contains(&"registration".to_string())),
        "update mode must NOT re-register WARP"
    );
    assert!(
        !invs
            .iter()
            .any(|(p, a)| p == "warp-cli" && a.contains(&"mode".to_string())),
        "update mode must NOT overwrite existing WARP mode"
    );
}

#[test]
fn warp_daemon_timeout_returns_e4004_app_error() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    sys::write_file_atomic(
        &ctx.path("/etc/os-release"),
        b"ID=debian\nVERSION_CODENAME=bookworm\n",
        Some(0o644),
    )
    .unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://pkg.cloudflareclient.com/pubkey.gpg",
        200,
        b"FAKE-CF-PGP".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", ok_out("install ok installed"));
    runner.on_prog("apt-get", ok_out(""));
    runner.on_prog("gpg", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("warp-cli", err_out(1, "Unable to connect to daemon"));

    let err = warp::install_warp(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
}

#[test]
fn warp_uninstall_cleans_repo_override_and_opt_dir() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    sys::write_file_atomic(
        &ctx.path("/etc/apt/sources.list.d/cloudflare-client.list"),
        b"deb ...\n",
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/systemd/system/warp-svc.service.d/security.conf"),
        b"[Service]\n",
        Some(0o644),
    )
    .unwrap();
    fs::create_dir_all(ctx.path("/opt/cloudflare-warp")).unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("warp-cli", ok_out(""));
    runner.on_prog("apt-get", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));

    let report = warp::uninstall_warp(&ctx, &runner).unwrap();
    assert_eq!(report.action, "uninstall");
    assert!(
        !ctx.path("/etc/apt/sources.list.d/cloudflare-client.list")
            .exists()
    );
    assert!(!ctx.path("/etc/systemd/system/warp-svc.service.d").exists());
    assert!(!ctx.path("/opt/cloudflare-warp").exists());
}

#[test]
fn usque_install_cn_parses_api_json_and_never_uses_ghfast_top() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;
    ctx.arch_override = Some("aarch64".to_string());

    let api_json = r#"{
        "tag_name": "v1.4.2",
        "assets": [
            {
                "name": "usque_1.4.2_linux_amd64.zip",
                "browser_download_url": "https://github.com/Diniboy1123/usque/releases/download/v1.4.2/usque_1.4.2_linux_amd64.zip"
            },
            {
                "name": "usque_1.4.2_linux_arm64.zip",
                "browser_download_url": "https://github.com/Diniboy1123/usque/releases/download/v1.4.2/usque_1.4.2_linux_arm64.zip"
            }
        ]
    }"#;

    let http = MockHttpClient::new();
    http.on_get_err(
        "https://gh-proxy.com/https://api.github.com/repos/Diniboy1123/usque/releases/latest",
        "primary proxy timeout",
    );
    http.on_get(
        "https://edgeone.gh-proxy.org/https://api.github.com/repos/Diniboy1123/usque/releases/latest",
        HttpResponse {
            status: 200,
            body: api_json.to_string(),
        },
    );
    http.on_download(
        "https://gh-proxy.com/https://github.com/Diniboy1123/usque/releases/download/v1.4.2/usque_1.4.2_linux_arm64.zip",
        200,
        b"FAKE-ZIP-BYTES".to_vec(),
    );

    struct UsqueExtractRunner(MockCommandRunner);
    impl CommandRunner for UsqueExtractRunner {
        fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput> {
            if prog == "unzip" && args.len() >= 4 && args[2] == "-d" {
                let bin = Path::new(args[3]).join("usque");
                fs::create_dir_all(args[3]).unwrap();
                fs::write(&bin, b"#!/bin/sh\necho usque\n").unwrap();
            }
            self.0.run(prog, args, timeout)
        }
    }

    let inner = MockCommandRunner::new();
    inner.on_prog("dpkg-query", ok_out("install ok installed"));
    inner.on_prog("unzip", ok_out(""));
    inner.on_prog("id", err_out(1, "no such user"));
    inner.on_prog("useradd", ok_out(""));
    inner.on_prog("systemctl", ok_out(""));
    let runner = UsqueExtractRunner(inner);

    let report = warp::install_usque(&ctx, &http, &runner).unwrap();
    assert_eq!(report.action, "install-usque");
    assert_eq!(
        report.details.get("arch").map(String::as_str),
        Some("arm64")
    );
    assert!(ctx.path("/opt/usque/usque").is_file());
    assert!(ctx.path("/etc/systemd/system/usque.service").is_file());

    let api_urls: Vec<String> = http
        .requests()
        .into_iter()
        .filter(|r| !r.is_download)
        .map(|r| r.url)
        .collect();
    assert!(
        !api_urls.iter().any(|u| u.contains("ghfast.top")),
        "GitHub API requests must never use ghfast.top: {api_urls:?}"
    );
}

#[test]
fn usque_install_rejects_unsupported_arch_with_e4004() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.no_sys = true;
    ctx.arch_override = Some("riscv64".to_string());

    let http = MockHttpClient::new();
    let runner = MockCommandRunner::new();
    let err = warp::install_usque(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
}

#[test]
fn usque_uninstall_removes_service_dir_and_user() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    fs::create_dir_all(ctx.path("/opt/usque")).unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/systemd/system/usque.service"),
        b"[Unit]\n",
        Some(0o644),
    )
    .unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("systemctl", ok_out(""));
    runner.on_exact("id", &["-u", "usque"], ok_out("999"));
    runner.on_prog("userdel", ok_out(""));

    let report = warp::uninstall_usque(&ctx, &runner).unwrap();
    assert_eq!(report.action, "uninstall-usque");
    assert!(!ctx.path("/opt/usque").exists());
    assert!(!ctx.path("/etc/systemd/system/usque.service").exists());
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "userdel" && a == &["usque"])
    );
}

type RecordedCalls = Arc<Mutex<Vec<(String, Vec<String>)>>>;

struct WgcfFixtureRunner {
    account_toml: String,
    profile_conf: String,
    calls: RecordedCalls,
}

impl CommandRunner for WgcfFixtureRunner {
    fn run(&self, prog: &str, args: &[&str], _timeout: Duration) -> Result<CmdOutput> {
        let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.calls
            .lock()
            .unwrap()
            .push((prog.to_string(), owned.clone()));
        if owned.first().map(String::as_str) == Some("register") {
            if let Some(pos) = owned.iter().position(|a| a == "--config")
                && let Some(path) = owned.get(pos + 1)
            {
                fs::write(path, &self.account_toml).unwrap();
            }
        } else if owned.first().map(String::as_str) == Some("generate")
            && let Some(pos) = owned.iter().position(|a| a == "--profile")
            && let Some(path) = owned.get(pos + 1)
        {
            fs::write(path, &self.profile_conf).unwrap();
        }
        Ok(ok_out("ok"))
    }
}

#[test]
fn warp_xray_generate_end_to_end_and_computes_reserved() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());

    let http = MockHttpClient::new();
    http.on_download(
        "https://github.com/ViRb3/wgcf/releases/download/v2.2.22/wgcf_2.2.22_linux_amd64",
        200,
        b"#!/bin/sh\nexit 0\n".to_vec(),
    );

    // device_id "AQIDBA==" decodes to [1, 2, 3, 4] -> reserved [1, 2, 3]
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = WgcfFixtureRunner {
        account_toml: "device_id = 'AQIDBA=='\nprivate_key = 'SUPER_SECRET_WG_KEY_12345='\n"
            .to_string(),
        profile_conf: "[Interface]\nAddress = 172.16.0.2/32, 2606:4700:110::1/128\n".to_string(),
        calls: Arc::clone(&calls),
    };

    let outbound = warp::generate_warp_xray(&ctx, &http, &runner).unwrap();
    assert_eq!(outbound.secret_key, "SUPER_SECRET_WG_KEY_12345=");
    assert_eq!(outbound.ipv4, "172.16.0.2");
    assert_eq!(outbound.reserved, [1, 2, 3]);
    assert_eq!(outbound.public_key, WARP_PUBLIC_KEY);
    assert_eq!(outbound.endpoint, WARP_ENDPOINT);

    let parsed: serde_json::Value = serde_json::from_str(&outbound.json).unwrap();
    assert_eq!(parsed["tag"], "warp");
    assert_eq!(parsed["protocol"], "wireguard");
    assert_eq!(
        parsed["settings"]["secretKey"],
        "SUPER_SECRET_WG_KEY_12345="
    );
    assert_eq!(parsed["settings"]["reserved"], serde_json::json!([1, 2, 3]));
    assert_eq!(calls.lock().unwrap().len(), 2);
}

// ============================================================================
// Xray Core tests
// ============================================================================

#[test]
fn xray_rewrite_install_script_mirrors_idempotent_and_preserves_url_zip() {
    let raw = concat!(
        "LATEST='https://api.github.com/repos/XTLS/Xray-core/releases/latest'\n",
        "GEO='https://github.com/XTLS/Xray-rules-dat/releases/latest/download/geosite.dat'\n",
        "DL='https://github.com/XTLS/Xray-core/releases/download/v25.1/Xray.zip'\n",
        "url_zip=\"https://github.com/XTLS/Xray-core/releases/download/${TAG}/Xray-linux-64.zip\"\n",
    );
    let r1 = xray::rewrite_install_script_mirrors(raw, "https://gh-proxy.com/").unwrap();
    let r2 = xray::rewrite_install_script_mirrors(&r1, "https://gh-proxy.com").unwrap();
    assert_eq!(r1, r2);
    assert!(r1.contains("https://gh-proxy.com/https://api.github.com/repos/XTLS/"));
    assert!(r1.contains("https://gh-proxy.com/https://github.com/XTLS/Xray-rules-dat/releases/"));
    assert!(r1.contains("DL='https://gh-proxy.com/https://github.com/XTLS/Xray-core/releases/"));
    assert!(
        r1.contains("url_zip=\"https://github.com/XTLS/Xray-core/releases/download/${TAG}/"),
        "url_zip= line must remain un-prefixed for --beta JSON matching"
    );
}

#[test]
fn xray_install_cn_mirror_fallback_disables_autostart_and_preserves_geo_pref() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.is_cn_region = true;

    // Pre-persist XRAY_GEO_AUTO_UPDATE=true to verify U8 preservation across reinstall.
    ctx.config_store()
        .set("XRAY_GEO_AUTO_UPDATE", "true")
        .unwrap();

    // Seed regular geosite.dat/geoip.dat as if installed by install-release.sh.
    let asset_dir = ctx.path("/usr/local/share/xray");
    fs::create_dir_all(&asset_dir).unwrap();
    fs::write(asset_dir.join("geosite.dat"), vec![b'A'; 120_000]).unwrap();
    fs::write(asset_dir.join("geoip.dat"), vec![b'B'; 120_000]).unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://gh-proxy.com/https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh",
        200,
        b"#!/bin/bash\nAPI='https://api.github.com/repos/XTLS/Xray-core/releases'\n".to_vec(),
    );
    // Primary mirror fails probe; second mirror succeeds.
    http.on_get_err(
        "https://gh-proxy.com/https://api.github.com/repos/XTLS/Xray-core/releases/latest",
        "timeout",
    );
    http.on_get(
        "https://edgeone.gh-proxy.org/https://api.github.com/repos/XTLS/Xray-core/releases/latest",
        HttpResponse {
            status: 200,
            body: r#"{"tag_name":"v25.1.1"}"#.to_string(),
        },
    );
    http.on_get(
        "https://edgeone.gh-proxy.org/https://github.com/XTLS/Xray-core/releases/latest",
        HttpResponse {
            status: 200,
            body: "ok".to_string(),
        },
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("bash", ok_out("installed"));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_exact(
        "crontab",
        &["-l"],
        ok_out("0 2 * * * /usr/local/bin/user-backup.sh\n"),
    );
    runner.on_exact("crontab", &["-"], ok_out(""));

    let report = xray::install_xray(&ctx, &http, &runner).unwrap();
    assert_eq!(report.action, "install");
    assert_eq!(
        report.details.get("cn_mirror").map(String::as_str),
        Some("https://edgeone.gh-proxy.org")
    );
    assert_eq!(
        report.details.get("geo_auto_update").map(String::as_str),
        Some("true")
    );

    // Regular files moved to .official and symlinked
    assert!(asset_dir.join("geosite.dat.official").is_file());
    assert_eq!(
        fs::read_link(asset_dir.join("geosite.dat")).unwrap(),
        Path::new("geosite.dat.official")
    );
    // Autostart disabled per 10 §6
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["disable", "xray"])
    );
    // Security override deployed
    assert!(
        ctx.path("/etc/systemd/system/xray.service.d/security.conf")
            .is_file()
    );
    // Cron job script deployed and user cron preserved
    assert!(ctx.path("/usr/local/bin/xray-rule-update.sh").is_file());
    let stdin_calls = runner.stdin_invocations();
    let cron_written = &stdin_calls
        .iter()
        .find(|(p, a, _)| p == "crontab" && a == &["-"])
        .unwrap()
        .2;
    assert!(cron_written.contains("/usr/local/bin/user-backup.sh"));
    assert!(cron_written.contains("/usr/local/bin/xray-rule-update.sh"));
}

#[test]
fn xray_toggle_ruleset_rejects_truncated_dat_under_102400_bytes() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());

    let http = MockHttpClient::new();
    // Truncated 512-byte payload (<= 102400) must be rejected!
    http.on_download(
        "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat",
        200,
        vec![b'X'; 512],
    );

    let runner = MockCommandRunner::new();
    let err = xray::toggle_xray_ruleset(&ctx, &http, &runner).unwrap_err();
    assert_eq!(err.error_code(), "E4004");
    assert!(
        !ctx.path("/usr/local/share/xray/geosite.dat.loyalsoldier")
            .exists()
    );
}

#[test]
fn xray_toggle_ruleset_swaps_symlinks_atomically_and_decouples_cron() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    let asset_dir = ctx.path("/usr/local/share/xray");
    fs::create_dir_all(&asset_dir).unwrap();
    fs::write(asset_dir.join("geosite.dat.official"), vec![b'O'; 110_000]).unwrap();
    fs::write(asset_dir.join("geoip.dat.official"), vec![b'O'; 110_000]).unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat",
        200,
        vec![b'L'; 110_000],
    );
    http.on_download(
        "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat",
        200,
        vec![b'L'; 110_000],
    );

    let runner = MockCommandRunner::new();
    runner.on_exact("systemctl", &["is-active", "--quiet", "xray"], ok_out(""));
    runner.on_exact("systemctl", &["restart", "xray"], ok_out(""));

    // Toggle official -> loyalsoldier
    let rep1 = xray::toggle_xray_ruleset(&ctx, &http, &runner).unwrap();
    assert_eq!(
        rep1.details.get("ruleset").map(String::as_str),
        Some("loyalsoldier")
    );
    assert_eq!(
        fs::read_link(asset_dir.join("geosite.dat")).unwrap(),
        Path::new("geosite.dat.loyalsoldier")
    );
    assert_eq!(
        ctx.config_store().get("XRAY_RULESET").unwrap().as_deref(),
        Some("loyalsoldier")
    );
    // Must NEVER touch crontab during ruleset toggle (10 §6 decoupling)
    assert!(
        !runner.invocations().iter().any(|(p, _)| p == "crontab"),
        "toggle_xray_ruleset must not touch crontab"
    );
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["restart", "xray"])
    );

    // Toggle back loyalsoldier -> official
    let rep2 = xray::toggle_xray_ruleset(&ctx, &http, &runner).unwrap();
    assert_eq!(
        rep2.details.get("ruleset").map(String::as_str),
        Some("official")
    );
    assert_eq!(
        fs::read_link(asset_dir.join("geosite.dat")).unwrap(),
        Path::new("geosite.dat.official")
    );
}

#[test]
fn xray_cron_setup_and_cleanup_preserves_existing_user_crontab_u8() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());

    let existing_user_cron =
        "15 4 * * * /home/alice/backup.sh\n@reboot /usr/local/bin/custom-agent\n";
    let runner_add = MockCommandRunner::new();
    runner_add.on_exact("crontab", &["-l"], ok_out(existing_user_cron));
    runner_add.on_exact("crontab", &["-"], ok_out(""));

    let script = xray::setup_xray_cron_job(&ctx, &runner_add).unwrap();
    assert!(script.is_file());
    assert_eq!(
        fs::metadata(&script).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let added_cron = &runner_add.stdin_invocations()[0].2;
    assert!(added_cron.contains("/home/alice/backup.sh"));
    assert!(added_cron.contains("/usr/local/bin/custom-agent"));
    assert!(added_cron.contains("/usr/local/bin/xray-rule-update.sh"));

    // Now remove xray cron and confirm both user entries survive 100% intact.
    let runner_rm = MockCommandRunner::new();
    runner_rm.on_exact("crontab", &["-l"], ok_out(added_cron));
    runner_rm.on_exact("crontab", &["-"], ok_out(""));

    xray::cleanup_xray_geodata_cron(&ctx, &runner_rm).unwrap();
    assert!(!script.exists());
    let remaining_cron = &runner_rm.stdin_invocations()[0].2;
    assert!(remaining_cron.contains("/home/alice/backup.sh"));
    assert!(remaining_cron.contains("/usr/local/bin/custom-agent"));
    assert!(!remaining_cron.contains("/usr/local/bin/xray-rule-update.sh"));
}

#[test]
fn xray_uninstall_cleans_units_dirs_cron_and_config_keys() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    ctx.config_store()
        .set("XRAY_RULESET", "loyalsoldier")
        .unwrap();
    ctx.config_store()
        .set("XRAY_GEO_AUTO_UPDATE", "true")
        .unwrap();

    fs::create_dir_all(ctx.path("/usr/local/share/xray")).unwrap();
    fs::create_dir_all(ctx.path("/etc/xray")).unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/systemd/system/xray.service"),
        b"[Unit]\n",
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/usr/local/bin/xray-rule-update.sh"),
        b"#!/bin/bash\n",
        Some(0o755),
    )
    .unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh",
        200,
        b"#!/bin/bash\nexit 0\n".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("bash", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_exact(
        "crontab",
        &["-l"],
        ok_out("30 3 * * 1 /usr/local/bin/xray-rule-update.sh >/dev/null 2>&1\n"),
    );
    runner.on_exact("crontab", &["-"], ok_out(""));

    let report = xray::uninstall_xray(&ctx, &http, &runner).unwrap();
    assert_eq!(report.action, "uninstall");
    assert!(!ctx.path("/usr/local/share/xray").exists());
    assert!(!ctx.path("/etc/xray").exists());
    assert!(!ctx.path("/etc/systemd/system/xray.service").exists());
    assert_eq!(
        ctx.config_store().get("XRAY_RULESET").unwrap().as_deref(),
        Some("")
    );
    assert_eq!(
        ctx.config_store()
            .get("XRAY_GEO_AUTO_UPDATE")
            .unwrap()
            .as_deref(),
        Some("")
    );

    let status = xray::probe_xray_status(&ctx, &runner);
    assert!(!status.installed);
    assert_eq!(status.ruleset, XrayRuleset::Official);
}

// ============================================================================
// FreshIP v4 tests
// ============================================================================

#[test]
fn freship_map_parse_and_offline_cache_fallback_u8() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());

    // Pre-populate local offline map cache
    let cache = ctx.path("/opt/freship/data/map.json");
    sys::write_file_atomic(&cache, sample_freship_map_json().as_bytes(), Some(0o644)).unwrap();

    let http = MockHttpClient::new();
    http.on_download_err(
        format!("{FRESHIP_REPO_RAW}/data/map.json"),
        "network unreachable",
    );

    let (map, used_offline) = freship::fetch_freship_map(&ctx, &http).unwrap();
    assert!(used_offline);
    assert_eq!(map.version, "4.2.0");
    let sel = FreshipSelection::from_map_default(&map).unwrap();
    assert_eq!(sel.country_id, "JP");
    assert_eq!(sel.city_name, "Tokyo");
    assert_eq!(sel.remote_path(), "JP/Default/Tokyo");
}

#[test]
fn freship_utc_offset_and_target_cc_parity() {
    assert_eq!(freship::utc_offset_for_country("JP"), "+9");
    assert_eq!(freship::utc_offset_for_country("HK"), "+8");
    assert_eq!(freship::utc_offset_for_country("DE"), "+1");
    assert_eq!(freship::utc_offset_for_country("US"), "-5");
    assert_eq!(freship::utc_offset_for_country("AU"), "+10");
    assert_eq!(freship::utc_offset_for_country("UNKNOWN"), "+0");

    assert_eq!(freship::target_cc_for_country("UK"), "GB");
    assert_eq!(freship::target_cc_for_country("UK-ENG"), "GB");
    assert_eq!(freship::target_cc_for_country("US-NY"), "US");
}

fn mock_freship_downloads(http: &MockHttpClient, remote_path: &str, kw_file: &str) {
    http.on_download(
        "https://github.com/lwthiker/curl-impersonate/releases/download/v0.6.1/curl-impersonate-v0.6.1.x86_64-linux-gnu.tar.gz",
        200,
        b"FAKE-CURL-IMPERSONATE".to_vec(),
    );
    http.on_download(
        format!("{FRESHIP_REPO_RAW}/data/regions/{remote_path}.json"),
        200,
        b"{\"region\":\"ok\"}\n".to_vec(),
    );
    http.on_download(
        format!("{FRESHIP_REPO_RAW}/data/keywords/{kw_file}"),
        200,
        b"keyword1\n".to_vec(),
    );
    http.on_download(
        format!("{FRESHIP_REPO_RAW}/data/user_agents.txt"),
        200,
        b"Mozilla/5.0\n".to_vec(),
    );
    http.on_download(
        format!("{FRESHIP_REPO_RAW}/data/map.json"),
        200,
        sample_freship_map_json().as_bytes().to_vec(),
    );
}

#[test]
fn freship_install_ipv4_only_ipv6_only_and_dual_stack_timer_selection() {
    for (mode, expect_v4, expect_v6) in [
        (FreshipWorkMode::Ipv4Only, true, false),
        (FreshipWorkMode::Ipv6Only, false, true),
        (FreshipWorkMode::DualStack, true, true),
    ] {
        let tmp = TempDir::new().unwrap();
        let mut ctx = AppContext::with_root(tmp.path());
        ctx.arch_override = Some("x86_64".to_string());
        let http = MockHttpClient::new();
        mock_freship_downloads(&http, "UK/ENG/London", "uk.txt");
        let runner = MockCommandRunner::new();
        runner.on_prog("dpkg-query", ok_out("install ok installed"));
        runner.on_prog("id", ok_out("998"));
        runner.on_prog("tar", ok_out(""));
        runner.on_prog("systemctl", ok_out(""));

        let sel = FreshipSelection {
            country_id: "UK".to_string(),
            state_id: "ENG".to_string(),
            city_id: "London".to_string(),
            city_name: "London".to_string(),
            kw_filename: "uk.txt".to_string(),
            work_mode: Some(mode),
            bind_ipv4: Some("203.0.113.10".to_string()),
            bind_ipv6: Some("2001:db8::10".to_string()),
        };

        let report = freship::install_freship(&ctx, Some(sel), &http, &runner).unwrap();
        assert_eq!(
            report.details.get("work_mode").map(String::as_str),
            Some(mode.as_str())
        );
        assert_eq!(
            report.details.get("target_cc").map(String::as_str),
            Some("GB")
        );

        let invs = runner.invocations();
        let enabled_v4 = invs
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["enable", "--now", "freship-core@v4.timer"]);
        let enabled_v6 = invs
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["enable", "--now", "freship-core@v6.timer"]);
        let enabled_updater = invs
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["enable", "--now", "freship-updater.timer"]);
        assert_eq!(enabled_v4, expect_v4, "v4 timer mismatch for {mode}");
        assert_eq!(enabled_v6, expect_v6, "v6 timer mismatch for {mode}");
        assert!(enabled_updater, "updater timer must always be enabled");
    }
}

#[test]
fn freship_conf_0600_mode_and_all_7_core_scripts_executable() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.no_sys = true;
    ctx.arch_override = Some("x86_64".to_string());

    let http = MockHttpClient::new();
    mock_freship_downloads(&http, "JP/Default/Tokyo", "jp.txt");
    let runner = MockCommandRunner::new();

    let sel = FreshipSelection {
        country_id: "JP".to_string(),
        state_id: "Default".to_string(),
        city_id: "Tokyo".to_string(),
        city_name: "Tokyo".to_string(),
        kw_filename: "jp.txt".to_string(),
        work_mode: Some(FreshipWorkMode::Ipv4Only),
        bind_ipv4: Some("198.51.100.5".to_string()),
        bind_ipv6: None,
    };

    freship::install_freship(&ctx, Some(sel), &http, &runner).unwrap();

    let conf = ctx.path("/etc/freship/freship.conf");
    assert!(conf.is_file());
    assert_eq!(
        fs::metadata(&conf).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let conf_txt = fs::read_to_string(&conf).unwrap();
    assert!(conf_txt.contains("REGION_CODE=\"JP\""));
    assert!(conf_txt.contains("UTC_OFFSET=\"+9\""));
    assert!(conf_txt.contains("WORK_MODE=\"ipv4_only\""));

    for script in [
        "freship_lib.sh",
        "freship_probe.sh",
        "freship_mod_google.sh",
        "freship_mod_trust.sh",
        "freship_runner.sh",
        "freship_updater.sh",
        "freship_core.sh",
    ] {
        let p = ctx.path(&format!("/opt/freship/core/{script}"));
        assert!(p.is_file(), "missing core script {script}");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    for unit in [
        "freship-core@.service",
        "freship-core@.timer",
        "freship-updater.service",
        "freship-updater.timer",
    ] {
        assert!(ctx.path(&format!("/etc/systemd/system/{unit}")).is_file());
    }
}

#[test]
fn freship_probe_status_reads_state_files_and_reconfigure_resets_them() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());
    let http = MockHttpClient::new();
    mock_freship_downloads(&http, "JP/Default/Tokyo", "jp.txt");
    mock_freship_downloads(&http, "US/CA/LosAngeles", "us.txt");
    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", ok_out("install ok installed"));
    runner.on_prog("id", ok_out("998"));
    runner.on_prog("tar", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));

    let sel1 = FreshipSelection {
        country_id: "JP".to_string(),
        state_id: "Default".to_string(),
        city_id: "Tokyo".to_string(),
        city_name: "Tokyo".to_string(),
        kw_filename: "jp.txt".to_string(),
        work_mode: Some(FreshipWorkMode::DualStack),
        bind_ipv4: Some("203.0.113.1".to_string()),
        bind_ipv6: Some("2001:db8::1".to_string()),
    };
    freship::install_freship(&ctx, Some(sel1), &http, &runner).unwrap();

    // Seed probe state files
    sys::write_file_atomic(
        &ctx.path("/etc/freship/state/v4.state"),
        b"RUN_MODE=maintain\nLAST_SCORE=ok\n",
        Some(0o644),
    )
    .unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/freship/state/v6.state"),
        b"RUN_MODE=simulate\nLAST_SCORE=drift\n",
        Some(0o644),
    )
    .unwrap();

    let status = freship::probe_freship_status(&ctx, &runner);
    assert!(status.installed);
    assert_eq!(status.region_code, "JP");
    assert_eq!(status.work_mode, Some(FreshipWorkMode::DualStack));
    assert!(status.v4_timer_active);
    assert!(status.v6_timer_active);
    assert_eq!(status.v4_state.as_ref().unwrap().run_mode, "maintain");
    assert_eq!(status.v4_state.as_ref().unwrap().last_score, "ok");
    assert_eq!(status.v6_state.as_ref().unwrap().last_score, "drift");

    // Reconfigure must clear v4.state & v6.state
    let sel2 = FreshipSelection {
        country_id: "US".to_string(),
        state_id: "CA".to_string(),
        city_id: "LosAngeles".to_string(),
        city_name: "Los Angeles".to_string(),
        kw_filename: "us.txt".to_string(),
        work_mode: Some(FreshipWorkMode::Ipv4Only),
        bind_ipv4: Some("203.0.113.2".to_string()),
        bind_ipv6: None,
    };
    freship::reconfigure_freship(&ctx, Some(sel2), &http, &runner).unwrap();
    assert!(!ctx.path("/etc/freship/state/v4.state").exists());
    assert!(!ctx.path("/etc/freship/state/v6.state").exists());

    let status_after = freship::probe_freship_status(&ctx, &runner);
    assert_eq!(status_after.region_code, "US");
    assert_eq!(status_after.work_mode, Some(FreshipWorkMode::Ipv4Only));
    assert!(status_after.v4_state.is_none());
}

#[test]
fn freship_journal_log_normalization_and_uninstall_cleanup() {
    let raw = concat!(
        "2026-11-21T12:00:01+0800 freship[42]: [FreshIP] v4 | 2026-11-21 | score=ok\n",
        "2026-11-21T12:05:00Z freship: [FreshIP] hot data updated\n",
        "\n",
        "garbage line\n",
    );
    let lines = freship::format_freship_journal_logs(raw);
    assert_eq!(
        lines,
        vec![
            "2026-11-21 12:00:01 [FreshIP] v4 | score=ok",
            "2026-11-21 12:05:00 [FreshIP] hot data updated",
        ]
    );

    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    fs::create_dir_all(ctx.path("/opt/freship/core")).unwrap();
    fs::create_dir_all(ctx.path("/etc/freship")).unwrap();
    sys::write_file_atomic(
        &ctx.path("/etc/systemd/system/freship-core@.timer"),
        b"[Unit]\n",
        Some(0o644),
    )
    .unwrap();

    let runner = MockCommandRunner::new();
    runner.on_prog("systemctl", ok_out(""));
    runner.on_exact("id", &["-u", "freship"], ok_out("997"));
    runner.on_prog("userdel", ok_out(""));

    let report = freship::uninstall_freship(&ctx, &runner).unwrap();
    assert_eq!(report.action, "uninstall");
    assert!(!ctx.path("/opt/freship").exists());
    assert!(!ctx.path("/etc/freship").exists());
    assert!(!ctx.path("/etc/systemd/system/freship-core@.timer").exists());
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "userdel" && a == &["freship"])
    );
}

#[test]
fn freship_sync_data_invokes_updater_as_freship_user() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    make_executable(&ctx.path("/opt/freship/core/freship_updater.sh"));

    let runner = MockCommandRunner::new();
    runner.on_prog("sudo", ok_out("synced"));

    let report = freship::sync_freship_data(&ctx, &runner).unwrap();
    assert_eq!(report.action, "sync-data");
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "sudo" && a.contains(&"freship".to_string()))
    );
}

#[test]
fn xray_upgrade_preserves_service_and_avoids_dangling_geodata_symlinks_be_p1_13() {
    let tmp = TempDir::new().unwrap();
    let ctx = AppContext::with_root(tmp.path());
    // Pre-existing xray binary signals upgrade mode
    make_executable(&ctx.path("/usr/local/bin/xray"));
    ctx.config_store()
        .set("XRAY_RULESET", "loyalsoldier")
        .unwrap();

    let http = MockHttpClient::new();
    http.on_download(
        "https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh",
        200,
        b"#!/bin/bash\nexit 0\n".to_vec(),
    );
    // Intentionally leave geodata URLs unmocked so download fails!

    let runner = MockCommandRunner::new();
    runner.on_prog("bash", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("crontab", ok_out(""));

    let report = xray::install_xray(&ctx, &http, &runner).unwrap();
    assert_eq!(report.action, "update");
    assert!(
        !runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "systemctl" && a == &["disable", "xray"]),
        "BE-P1-13 (U8): upgrading existing xray MUST NOT disable running xray.service"
    );
    assert!(
        fs::symlink_metadata(ctx.path("/usr/local/share/xray/geosite.dat")).is_err(),
        "BE-P1-13: must NOT create dangling geosite.dat symlink when target file is missing"
    );
    assert!(
        fs::symlink_metadata(ctx.path("/usr/local/share/xray/geoip.dat")).is_err(),
        "BE-P1-13: must NOT create dangling geoip.dat symlink when target file is missing"
    );
    assert!(!report.warnings.is_empty());
}

#[test]
fn usque_missing_binary_errors_e4004_and_warp_uninstall_cleans_keyring_be_p2_11() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());

    let api_json = r#"{"tag_name":"v1.4.2","assets":[{"name":"usque_1.4.2_linux_amd64.zip","browser_download_url":"https://github.com/Diniboy1123/usque/releases/download/v1.4.2/usque_1.4.2_linux_amd64.zip"}]}"#;
    let http = MockHttpClient::new();
    http.on_get(
        "https://api.github.com/repos/Diniboy1123/usque/releases/latest",
        HttpResponse {
            status: 200,
            body: api_json.to_string(),
        },
    );
    http.on_download(
        "https://github.com/Diniboy1123/usque/releases/download/v1.4.2/usque_1.4.2_linux_amd64.zip",
        200,
        b"EMPTY_ZIP".to_vec(),
    );

    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", ok_out("install ok installed"));
    runner.on_prog("unzip", ok_out("")); // Does not materialize `usque` binary!

    let err = warp::install_usque(&ctx, &http, &runner).unwrap_err();
    assert_eq!(
        err.error_code(),
        "E4004",
        "BE-P2-11: install_usque must fail with E4004 when archive lacks usque binary"
    );

    // WARP uninstall must remove cloudflare-warp-archive-keyring.gpg
    let keyring = ctx.path("/usr/share/keyrings/cloudflare-warp-archive-keyring.gpg");
    sys::write_file_atomic(&keyring, b"KEYRING", Some(0o644)).unwrap();
    runner.on_prog("warp-cli", ok_out(""));
    runner.on_prog("apt-get", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    warp::uninstall_warp(&ctx, &runner).unwrap();
    assert!(
        !keyring.exists(),
        "BE-P2-11: uninstall_warp must remove cloudflare-warp-archive-keyring.gpg"
    );
}

#[test]
fn freship_download_failure_preserves_existing_opt_and_validates_inputs_be_p1_25_p2_13() {
    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());

    // Pre-existing working FreshIP installation
    let sentinel = ctx.path("/opt/freship/core/existing_sentinel.sh");
    sys::write_file_atomic(&sentinel, b"#!/bin/bash\n# keep\n", Some(0o755)).unwrap();

    let http = MockHttpClient::new(); // No mocked downloads -> network fails!
    let runner = MockCommandRunner::new();
    runner.on_prog("dpkg-query", ok_out("install ok installed"));
    runner.on_prog("id", ok_out("998"));

    let valid_sel = FreshipSelection {
        country_id: "JP".to_string(),
        state_id: "Default".to_string(),
        city_id: "Tokyo".to_string(),
        city_name: "Tokyo".to_string(),
        kw_filename: "jp.txt".to_string(),
        work_mode: Some(FreshipWorkMode::Ipv4Only),
        bind_ipv4: Some("198.51.100.5".to_string()),
        bind_ipv6: None,
    };

    let err = freship::install_freship(&ctx, Some(valid_sel.clone()), &http, &runner).unwrap_err();
    assert_eq!(
        err.error_code(),
        "E4004",
        "BE-P1-25: FreshIP must fail loudly on curl-impersonate/asset download failure"
    );
    assert!(
        sentinel.is_file(),
        "BE-P1-25: failed FreshIP install must preserve existing /opt/freship untouched"
    );

    // BE-P2-13: reject path traversal / shell metacharacters / invalid bind IPs
    for mut bad in [
        {
            let mut s = valid_sel.clone();
            s.country_id = "../etc".to_string();
            s
        },
        {
            let mut s = valid_sel.clone();
            s.kw_filename = "jp.txt;rm -rf /".to_string();
            s
        },
        {
            let mut s = valid_sel.clone();
            s.bind_ipv4 = Some("not-an-ip;id".to_string());
            s
        },
        {
            let mut s = valid_sel.clone();
            s.city_name = "Tokyo\"$(id)".to_string();
            s
        },
    ] {
        let _ = &mut bad;
        assert!(
            bad.validate().is_err(),
            "BE-P2-13: invalid FreshIP selection must be rejected: {bad:?}"
        );
    }
}

#[test]
fn self_update_persists_new_build_and_self_uninstall_cleans_f2b_units_be_p1_16() {
    use debopty_core::apps::self_manage::{
        SelfUninstallOptions, SelfUpdateOptions, self_uninstall, self_update, sha256_hex,
    };

    let tmp = TempDir::new().unwrap();
    let mut ctx = AppContext::with_root(tmp.path());
    ctx.arch_override = Some("x86_64".to_string());

    let asset = "debopty-v9.9.9-linux-amd64.tar.gz";
    let payload = b"FAKE-RELEASE-TARBALL";
    let digest = sha256_hex(payload);

    let http = MockHttpClient::new();
    http.on_download(
        "https://releases.example.test/SHA256SUMS",
        200,
        format!("{digest}  {asset}\n").into_bytes(),
    );
    http.on_download(
        format!("https://releases.example.test/{asset}"),
        200,
        payload.to_vec(),
    );

    struct ReleaseExtractRunner(MockCommandRunner);
    impl CommandRunner for ReleaseExtractRunner {
        fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput> {
            if prog == "tar" && args.len() >= 4 && args[2] == "-C" {
                let dir = Path::new(args[3]);
                fs::create_dir_all(dir).unwrap();
                fs::write(dir.join("debopty"), b"#!/bin/sh\necho new\n").unwrap();
                fs::write(
                    dir.join("VERSION"),
                    "DEBOPTI_SEMVER=\"9.9.9\"\nDEBOPTI_CALVER=\"26.11.22\"\nDEBOPTI_BUILD=\"9999abcd\"\n",
                )
                .unwrap();
            }
            self.0.run(prog, args, timeout)
        }
    }

    let inner = MockCommandRunner::new();
    inner.on_prog("tar", ok_out(""));
    inner.on_prog("systemctl", ok_out(""));
    let runner = ReleaseExtractRunner(inner);

    let upd = self_update(
        &ctx,
        &SelfUpdateOptions {
            release_base_url: Some("https://releases.example.test".to_string()),
            tag: Some("v9.9.9".to_string()),
            force: true,
            ..SelfUpdateOptions::default()
        },
        &http,
        &runner,
    )
    .unwrap();
    assert_eq!(
        ctx.config_store().get("SCRIPT_BUILD").unwrap().as_deref(),
        Some("9999abcd"),
        "BE-P1-16: self_update must persist NEW release's SCRIPT_BUILD, not old running binary's build"
    );
    assert_eq!(
        upd.details.get("script_build").map(String::as_str),
        Some("9999abcd")
    );

    // Seed debopty-f2b-port-sync systemd units and verify self_uninstall disables & removes them
    for unit in [
        "debopty-f2b-port-sync.service",
        "debopty-f2b-port-sync.path",
        "debopty-f2b-port-sync.timer",
    ] {
        sys::write_file_atomic(
            &ctx.path(&format!("/etc/systemd/system/{unit}")),
            b"[Unit]\n",
            Some(0o644),
        )
        .unwrap();
    }

    self_uninstall(&ctx, &SelfUninstallOptions::default(), &runner).unwrap();
    for unit in [
        "debopty-f2b-port-sync.service",
        "debopty-f2b-port-sync.path",
        "debopty-f2b-port-sync.timer",
    ] {
        assert!(
            !ctx.path(&format!("/etc/systemd/system/{unit}")).exists(),
            "BE-P1-16: self_uninstall must remove {unit}"
        );
        assert!(
            runner
                .0
                .invocations()
                .iter()
                .any(|(p, a)| p == "systemctl" && a == &["disable", "--now", unit]),
            "BE-P1-16: self_uninstall must disable --now {unit}"
        );
    }
}
