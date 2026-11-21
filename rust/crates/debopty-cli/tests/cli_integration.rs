use std::fs;
use std::os::unix::fs::symlink;
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn debopty_cmd() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("debopty"))
}

#[test]
fn version_subcommand_text_json_and_file() {
    let out = debopty_cmd()
        .arg("version")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let display = String::from_utf8(out).expect("utf8");
    assert!(
        display.trim().contains('('),
        "unexpected display: {display}"
    );

    let json_out = debopty_cmd()
        .args(["version", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&json_out).expect("valid version json");
    assert!(parsed["build"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(parsed["display"].as_str().is_some_and(|s| !s.is_empty()));

    let tmp = TempDir::new().expect("tempdir");
    let ver_file = tmp.path().join("VERSION");
    fs::write(
        &ver_file,
        "DEBOPTI_SEMVER=\"2.3.4\"\nDEBOPTI_CALVER=\"26.11.22\"\nDEBOPTI_BUILD=\"deadbeef\"\n",
    )
    .expect("write VERSION");

    debopty_cmd()
        .args(["version", "--file", ver_file.to_str().unwrap()])
        .assert()
        .success()
        .stdout("2.3.4 (deadbeef)\n");
}

#[test]
fn status_subcommand_json_and_human() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join("debopti.conf");

    let json_out = debopty_cmd()
        .args(["--config", cfg_path.to_str().unwrap(), "status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&json_out).expect("valid status json");
    assert!(parsed["status"]["version_display"].is_string());
    assert!(parsed["status"]["tools"].is_object());
    let modules = parsed["modules"].as_array().expect("modules array");
    assert_eq!(modules.len(), 21);

    let native_count = modules
        .iter()
        .filter(|m| m["implementation"] == "native-rust")
        .count();
    let bridge_count = modules
        .iter()
        .filter(|m| m["implementation"] == "bash-bridge")
        .count();
    assert_eq!(native_count, 10);
    assert_eq!(bridge_count, 11);

    for promoted in ["network", "system", "security-ssh-nft", "reconcile"] {
        let entry = modules
            .iter()
            .find(|m| m["id"] == promoted)
            .unwrap_or_else(|| panic!("missing module {promoted}"));
        assert_eq!(entry["implementation"], "native-rust");
    }

    let ui_backends = parsed["ui_backends"].as_array().expect("ui_backends array");
    #[cfg(not(feature = "ratatui-ui"))]
    assert_eq!(ui_backends, &[Value::String("console".to_string())]);
    #[cfg(feature = "ratatui-ui")]
    assert_eq!(
        ui_backends,
        &[
            Value::String("console".to_string()),
            Value::String("ratatui".to_string()),
        ]
    );

    debopty_cmd()
        .args(["--config", cfg_path.to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("版本:"))
        .stdout(predicate::str::contains("原生 Rust 10 个 / 兼容桥接 11 个"));
}

#[test]
fn config_set_get_list_roundtrip_and_missing_key() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join("etc/debopti/debopti.conf");

    debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "set",
            "IS_CN_REGION",
            "true",
        ])
        .assert()
        .success();

    debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "set",
            "CUSTOM_KEY",
            "hello world",
        ])
        .assert()
        .success();

    debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "get",
            "IS_CN_REGION",
        ])
        .assert()
        .success()
        .stdout("true\n");

    let list_json = debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "list",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&list_json).expect("valid config json");
    assert_eq!(parsed["IS_CN_REGION"], "true");
    assert_eq!(parsed["CUSTOM_KEY"], "hello world");

    debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "get",
            "NON_EXISTENT_KEY",
        ])
        .assert()
        .failure();
}

#[test]
fn template_list_and_render_stdout_and_file() {
    let list_out = debopty_cmd()
        .args(["template", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list_str = String::from_utf8(list_out).expect("utf8");
    assert!(list_str.contains("templates/common/debopty.sh"));
    assert!(!list_str.contains("INDEX.md"));

    debopty_cmd()
        .args([
            "template",
            "render",
            "templates/common/debopty.sh",
            "--var",
            "BIN_LINK=/usr/local/bin/debopty",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "alias debopty='/usr/local/bin/debopty'",
        ));

    let tmp = TempDir::new().expect("tempdir");
    let out_file = tmp.path().join("nftables.conf");
    debopty_cmd()
        .args([
            "template",
            "render",
            "templates/security/nftables.conf",
            "--out",
            out_file.to_str().unwrap(),
            "--var",
            "SSH_PORT=50022",
            "--var",
            "NFT_BASE_D=/etc/nftables.d",
        ])
        .assert()
        .success();
    let rendered = fs::read_to_string(&out_file).expect("read rendered");
    assert!(rendered.contains("tcp dport 50022 accept"));
    assert!(!rendered.contains("{{SSH_PORT}}"));

    debopty_cmd()
        .args([
            "template",
            "render",
            "templates/common/debopty.sh",
            "--var",
            "BAD_PAIR_WITHOUT_EQUALS",
        ])
        .assert()
        .code(2);
}

#[test]
fn f2b_sync_subcommand_and_multicall_symlink_parity() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("rootfs");
    fs::create_dir_all(root.join("etc/nftables.d/debopti")).expect("mkdir");
    fs::create_dir_all(root.join("etc/fail2ban/jail.d")).expect("mkdir");

    fs::write(
        root.join("etc/nftables.conf"),
        "table inet filter {\n  chain input {\n    policy drop;\n    tcp dport 2222 accept comment \"SSH_Access_Port\"\n  }\n}\n",
    )
    .expect("write nftables.conf");
    fs::write(
        root.join("etc/nftables.d/debopti/web.nft"),
        "tcp dport { 80, 443 } accept\nudp dport 41641 accept\n",
    )
    .expect("write web.nft");

    // 1. Subcommand --print-ports
    debopty_cmd()
        .env("DEBOPTY_F2B_ROOT", &root)
        .env("DEBOPTY_F2B_NO_SYS", "1")
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .args(["f2b-sync", "--print-ports"])
        .assert()
        .success()
        .stdout(
            "SSH_PORT=2222\nTCP_PORTS_NFT={ 80, 443, 2222 }\nUDP_PORTS_NFT=41641\nALL_PUBLIC_PORTS_F2B=80,443,2222,41641\n",
        );

    // 2. Multicall symlink argv[0] == "debopty-f2b-sync"
    let bin_path = assert_cmd::cargo::cargo_bin!("debopty");
    let link_path = tmp.path().join("debopty-f2b-sync");
    symlink(bin_path, &link_path).expect("create multicall symlink");

    let out = Command::new(&link_path)
        .env("DEBOPTY_F2B_ROOT", &root)
        .env("DEBOPTY_F2B_NO_SYS", "1")
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .arg("--print-ports")
        .output()
        .expect("run multicall --print-ports");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "SSH_PORT=2222\nTCP_PORTS_NFT={ 80, 443, 2222 }\nUDP_PORTS_NFT=41641\nALL_PUBLIC_PORTS_F2B=80,443,2222,41641\n"
    );

    // 3. Multicall writes jail & guard files
    let status = Command::new(&link_path)
        .env("DEBOPTY_F2B_ROOT", &root)
        .env("DEBOPTY_F2B_NO_SYS", "1")
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .status()
        .expect("run multicall sync");
    assert!(status.success());
    let jail = fs::read_to_string(root.join("etc/fail2ban/jail.d/99-debopty-ports.local"))
        .expect("read jail");
    let guard = fs::read_to_string(root.join("etc/nftables.d/55-debopty-f2b-guard.nft"))
        .expect("read guard");
    assert!(jail.contains("port = 2222"));
    assert!(jail.contains("port = 80,443,2222,41641"));
    assert!(guard.contains("tcp dport { 80, 443, 2222 } update @abuse_v4"));
    assert!(guard.contains("udp dport 41641 update @abuse_v4"));
}

#[test]
fn bridge_list_and_explicit_execution_with_mock_scripts() {
    let json_out = debopty_cmd()
        .args(["bridge", "--list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&json_out).expect("valid bridge json");
    assert_eq!(parsed["modules"].as_array().unwrap().len(), 21);
    assert!(
        parsed["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "base-optimize")
    );

    let tmp = TempDir::new().expect("tempdir");
    let mock_root = tmp.path().join("mock_repo");
    let marker = tmp.path().join("bridge_marker.txt");
    let cfg_path = tmp.path().join("debopti.conf");
    fs::create_dir_all(mock_root.join("scripts")).expect("mkdir");
    fs::write(
        mock_root.join("scripts/common.sh"),
        "#!/bin/bash\nscript_update() { printf 'BRIDGED|%s' \"${DEBOPTY_FROM_RUST:-}\" > \"${MOCK_MARKER}\"; }\n",
    )
    .expect("write common.sh");
    fs::write(
        mock_root.join("deb_optimizer.sh"),
        "#!/bin/bash\nprintf 'LEGACY_BASH|%s' \"${DEBOPTY_FROM_RUST:-}\" > \"${MOCK_MARKER}\"\n",
    )
    .expect("write deb_optimizer.sh");

    debopty_cmd()
        .env("MOCK_MARKER", &marker)
        .args([
            "--script-dir",
            mock_root.to_str().unwrap(),
            "--config",
            cfg_path.to_str().unwrap(),
            "bridge",
            "script-update",
        ])
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&marker).expect("read"), "BRIDGED|1");

    // --engine bash dispatches LegacyFullEntry
    debopty_cmd()
        .env("MOCK_MARKER", &marker)
        .args([
            "--engine",
            "bash",
            "--script-dir",
            mock_root.to_str().unwrap(),
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&marker).expect("read"), "LEGACY_BASH|1");
}

#[test]
fn ci_non_interactive_exits_zero_immediately() {
    debopty_cmd()
        .env("CI", "true")
        .env("DEBOPTY_ENGINE", "rust")
        .assert()
        .success()
        .stdout(predicate::str::contains("跳过 TUI"));
}

#[test]
fn ui_flag_help_default_and_feature_behavior() {
    debopty_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--ui"))
        .stdout(predicate::str::contains("console"));

    debopty_cmd()
        .env("CI", "true")
        .env("DEBOPTY_ENGINE", "rust")
        .args(["--ui", "console"])
        .assert()
        .success();

    debopty_cmd()
        .env("CI", "true")
        .env("DEBOPTY_ENGINE", "rust")
        .args(["--ui", "auto"])
        .assert()
        .success();

    debopty_cmd()
        .env("CI", "true")
        .env("DEBOPTY_ENGINE", "rust")
        .args(["--ui", "invalid-ui"])
        .assert()
        .code(2);

    #[cfg(not(feature = "ratatui-ui"))]
    {
        debopty_cmd()
            .env("CI", "true")
            .env("DEBOPTY_ENGINE", "rust")
            .args(["--ui", "ratatui"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("ratatui-ui"))
            .stderr(predicate::str::contains("--ui console"));

        debopty_cmd()
            .env("CI", "true")
            .env("DEBOPTY_ENGINE", "rust")
            .env("DEBOPTY_UI", "ratatui")
            .assert()
            .code(2);
    }

    #[cfg(feature = "ratatui-ui")]
    {
        debopty_cmd()
            .env("CI", "true")
            .env("DEBOPTY_ENGINE", "rust")
            .args(["--ui", "ratatui"])
            .assert()
            .success()
            .stdout(predicate::str::contains("跳过 TUI"));

        debopty_cmd()
            .env("CI", "true")
            .env("DEBOPTY_ENGINE", "rust")
            .env("DEBOPTY_UI", "ratatui")
            .assert()
            .success();
    }
}

#[test]
fn system_subcommands_tmux_roundtrip_and_root_isolated_ops() {
    let tmp = TempDir::new().expect("tempdir");
    let custom_conf = tmp.path().join("custom.tmux.conf");
    let backup_conf = tmp.path().join("custom.tmux.conf.debopty.bak");
    fs::write(&custom_conf, "# user custom tmux\nset -g prefix C-a\n").expect("write custom");

    // 1. tmux-setup backs up custom config and renders modern 3.5a directives
    debopty_cmd()
        .args([
            "system",
            "tmux-setup",
            "--conf",
            custom_conf.to_str().unwrap(),
            "--skip-apt",
            "--version-override",
            "tmux 3.5a",
        ])
        .assert()
        .success();

    assert!(backup_conf.is_file());
    assert!(
        fs::read_to_string(&backup_conf)
            .unwrap()
            .contains("set -g prefix C-a")
    );
    let rendered = fs::read_to_string(&custom_conf).unwrap();
    assert!(rendered.contains("# managed-by-debopty"));
    assert!(rendered.contains(r#"set -as terminal-features ",*:RGB""#));
    assert!(rendered.contains("set -g allow-passthrough on"));

    // 2. tmux-uninstall restores backup and cleans .debopty.bak
    debopty_cmd()
        .args([
            "system",
            "tmux-uninstall",
            "--conf",
            custom_conf.to_str().unwrap(),
            "--skip-apt",
        ])
        .assert()
        .success();

    assert!(!backup_conf.exists());
    assert!(
        fs::read_to_string(&custom_conf)
            .unwrap()
            .contains("set -g prefix C-a")
    );

    // 3. Isolated rootfs system ops: ip-forward-toggle, sysctl, limits, logrotate, low-mem, timezone
    let root = tmp.path().join("rootfs");
    fs::create_dir_all(root.join("etc/systemd")).unwrap();
    fs::write(root.join("etc/systemd/logind.conf"), "#NAutoVTs=6\n").unwrap();
    fs::write(root.join("etc/systemd/journald.conf"), "#SystemMaxUse=\n").unwrap();
    fs::write(root.join("etc/logrotate.conf"), "weekly\nrotate 4\n").unwrap();

    let root_s = root.to_str().unwrap();
    debopty_cmd()
        .args(["system", "ip-forward-toggle", "--root", root_s, "--no-sys"])
        .assert()
        .success()
        .stdout(predicate::str::contains("已开启"));
    assert!(
        root.join("etc/sysctl.d/99-debopti-forwarding.conf")
            .is_file()
    );

    debopty_cmd()
        .args(["system", "ip-forward-toggle", "--root", root_s, "--no-sys"])
        .assert()
        .success()
        .stdout(predicate::str::contains("已关闭"));
    assert!(
        !root
            .join("etc/sysctl.d/99-debopti-forwarding.conf")
            .exists()
    );

    debopty_cmd()
        .args(["system", "sysctl", "--root", root_s, "--no-sys"])
        .assert()
        .success();
    assert!(root.join("etc/sysctl.d/99-debopti-optimize.conf").is_file());

    debopty_cmd()
        .args(["system", "limits", "--root", root_s])
        .assert()
        .success();
    assert!(
        root.join("etc/security/limits.d/99-debopti-nofile.conf")
            .is_file()
    );

    debopty_cmd()
        .args(["system", "logrotate", "--root", root_s])
        .assert()
        .success();
    let lr = fs::read_to_string(root.join("etc/logrotate.conf")).unwrap();
    assert!(lr.contains("daily"));
    assert!(lr.contains("rotate 7"));

    debopty_cmd()
        .args(["system", "low-mem", "--root", root_s, "--no-sys"])
        .assert()
        .success();
    assert!(
        fs::read_to_string(root.join("etc/systemd/journald.conf"))
            .unwrap()
            .contains("SystemMaxUse=200M")
    );

    debopty_cmd()
        .args(["system", "timezone", "--no-sys"])
        .assert()
        .success();
}

#[test]
fn security_subcommands_fw_crud_and_ndp_patch_isolated_root() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("secroot");
    let root_s = root.to_str().unwrap();
    fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/fail2ban/jail.d")).unwrap();

    let legacy_nft = "\
table inet filter {
    chain input {
        ip6 nexthdr icmpv6 icmpv6 type echo-request limit rate 5/second accept
        tcp dport 2222 accept comment \"SSH_Access_Port\"
    }
}
";
    fs::write(root.join("etc/nftables.conf"), legacy_nft).unwrap();

    debopty_cmd()
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .args([
            "security",
            "fw-update-ssh",
            "50022",
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .success();

    debopty_cmd()
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .args([
            "security",
            "fw-add",
            "80,443",
            "--proto",
            "tcp/udp",
            "--comment",
            "Caddy_Web",
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .success();

    let rule_path = root.join("etc/nftables.d/debopti/Caddy_Web.nft");
    assert!(rule_path.is_file());
    let rule_txt = fs::read_to_string(&rule_path).unwrap();
    assert!(rule_txt.contains(r#"tcp dport { 80,443 } accept comment "Caddy_Web""#));
    assert!(rule_txt.contains(r#"udp dport { 80,443 } accept comment "Caddy_Web""#));

    let jail = fs::read_to_string(root.join("etc/fail2ban/jail.d/99-debopty-ports.local")).unwrap();
    assert!(jail.contains("port = 80,443,50022"));

    debopty_cmd()
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .args([
            "security",
            "fw-remove",
            "Caddy_Web",
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .success();
    assert!(!rule_path.exists());

    debopty_cmd()
        .args(["security", "ndp-patch", "--root", root_s, "--no-sys"])
        .assert()
        .success();
    let conf_after = fs::read_to_string(root.join("etc/nftables.conf")).unwrap();
    assert!(conf_after.contains("nd-router-solicit"));
    assert!(conf_after.contains("tcp dport 50022 accept comment \"SSH_Access_Port\""));
}

#[test]
fn network_subcommands_dns_unmanaged_noop_and_protected_dir_git_clone() {
    let tmp = TempDir::new().expect("tempdir");
    let resolv = tmp.path().join("resolv.conf");
    fs::write(&resolv, "nameserver 9.9.9.9\n").unwrap();

    // Unmanaged resolv.conf in calibrate mode is an offline no-op
    let out = debopty_cmd()
        .args([
            "network",
            "dns-check",
            "--mode",
            "calibrate",
            "--resolv",
            resolv.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).expect("valid dns-check json");
    assert_eq!(parsed["modified"], false);
    assert_eq!(fs::read_to_string(&resolv).unwrap(), "nameserver 9.9.9.9\n");

    // Protected-dir guard rejects /etc immediately without invoking git or network
    debopty_cmd()
        .args([
            "network",
            "git-clone",
            "https://github.com/example/repo.git",
            "/etc",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("[ERROR][E1003]"))
        .stderr(predicate::str::contains("protected directory"))
        .stderr(predicate::str::contains("hint:"));
}

#[test]
fn diag_strict_stdout_data_vs_stderr_ndjson_and_error_codes() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join("debopti.conf");

    let output = debopty_cmd()
        .args([
            "--log-level",
            "debug",
            "--log-format",
            "json",
            "--config",
            cfg_path.to_str().unwrap(),
            "status",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .clone();

    // stdout MUST be 100% pure JSON data (zero log contamination)
    let stdout_json: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must be pure valid JSON");
    assert_eq!(stdout_json["modules"].as_array().unwrap().len(), 21);

    // stderr MUST contain NDJSON diagnostic lines
    let stderr_str = String::from_utf8(output.stderr).expect("utf8 stderr");
    let lines: Vec<&str> = stderr_str
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(!lines.is_empty(), "expected NDJSON debug records on stderr");
    for line in lines {
        let rec: Value = serde_json::from_str(line).expect("each stderr line must be valid JSON");
        assert!(rec["ts"].is_string());
        assert!(rec["level"].is_string());
        assert!(rec["target"].is_string());
    }

    // JSON error formatting on stderr
    let err_out = debopty_cmd()
        .args(["--log-format", "json", "config", "get", "MISSING_KEY_XYZ"])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(err_out.stdout.is_empty());
    let err_stderr = String::from_utf8(err_out.stderr).expect("utf8");
    let err_rec: Value = serde_json::from_str(err_stderr.trim()).expect("valid JSON error record");
    assert_eq!(err_rec["level"], "error");
    assert_eq!(err_rec["code"], "E1004");
    assert_eq!(err_rec["target"], "debopty::cli");
    assert!(err_rec["hint"].as_str().is_some_and(|h| !h.is_empty()));
}

#[test]
fn reconcile_cli_audit_apply_idempotency_restore_and_mutex_flags() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("dirty_root");
    let root_s = root.to_str().unwrap();

    fs::create_dir_all(root.join("etc/nftables.d")).unwrap();
    fs::write(
        root.join("etc/nftables.conf"),
        "flush ruleset\ntable inet netavark {}\ntable inet filter {\n    chain input {\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/logrotate.conf"),
        "weekly\ndateext\ninclude /etc/logrotate.d\ndaily\n",
    )
    .unwrap();

    // Mutually exclusive flags return exit code 2 (E1002)
    debopty_cmd()
        .args(["reconcile", "--apply", "--restore", root_s])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("[ERROR][E1002]"));

    // 1. Default --audit is read-only
    let audit_out = debopty_cmd()
        .args(["reconcile", "--root", root_s, "--no-sys", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let audit_json: Value = serde_json::from_slice(&audit_out).expect("valid audit json");
    assert_eq!(audit_json["mode"], "audit");
    assert_eq!(audit_json["changes_made"], 0);
    assert!(audit_json["drift_count"].as_u64().unwrap() >= 2);
    assert!(audit_json["backup_dir"].is_null());

    // 2. --apply converges and creates backup_dir
    let apply_out = debopty_cmd()
        .args([
            "reconcile",
            "--apply",
            "--yes",
            "--root",
            root_s,
            "--no-sys",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let apply_json: Value = serde_json::from_slice(&apply_out).expect("valid apply json");
    assert_eq!(apply_json["mode"], "apply");
    assert!(apply_json["changes_made"].as_u64().unwrap() >= 2);
    let backup_dir = apply_json["backup_dir"]
        .as_str()
        .expect("backup_dir returned");
    assert!(
        fs::read_to_string(root.join("etc/nftables.conf"))
            .unwrap()
            .contains("flush table inet filter")
    );
    assert!(
        fs::read_to_string(root.join("etc/logrotate.conf"))
            .unwrap()
            .contains("dateext")
    );

    // 3. Second --apply is idempotent (0 changes, null backup_dir)
    let apply2_out = debopty_cmd()
        .args([
            "reconcile",
            "--apply",
            "--yes",
            "--root",
            root_s,
            "--no-sys",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let apply2_json: Value = serde_json::from_slice(&apply2_out).expect("valid apply2 json");
    assert_eq!(apply2_json["changes_made"], 0);
    assert_eq!(apply2_json["drift_count"], 0);
    assert!(apply2_json["backup_dir"].is_null());

    // 4. --restore reverts files cleanly
    let restore_out = debopty_cmd()
        .args([
            "reconcile",
            "--restore",
            backup_dir,
            "--root",
            root_s,
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let restore_json: Value = serde_json::from_slice(&restore_out).expect("valid restore json");
    assert!(restore_json["restored_count"].as_u64().unwrap() >= 2);
    assert!(
        fs::read_to_string(root.join("etc/nftables.conf"))
            .unwrap()
            .contains("flush ruleset")
    );
}
