use std::fs;
use std::path::PathBuf;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn debopty_cmd() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("debopty"))
}

const VALID_ED25519_PUBKEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIValidTestKey11111111111111111111111111111111 ops@test";

#[test]
fn cli_reconcile_audit_apply_idempotent_restore_and_rw_fw_01_u8_preservation() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("chaos_rootfs");
    let root_s = root.to_str().unwrap();

    fs::create_dir_all(root.join("etc/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
    fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/ufw")).unwrap();
    fs::create_dir_all(root.join("etc/fail2ban/jail.d")).unwrap();

    fs::write(
        root.join("etc/debopti/debopti.conf"),
        b"\xEF\xBB\xBF# user note\r\nSSH_HARDENED=\"true\"\r\nUSER_FLAG=\"keep\r\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config"),
        "Port 22\nAllowUsers ops\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config.d/90-bad.conf"),
        "BrokenLine\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/nftables.conf"),
        "flush ruleset\ntable inet netavark {}\ntable inet filter {\n    chain input {\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/nftables.d/70-user.nft"),
        "table inet user_custom {}\n",
    )
    .unwrap();
    fs::write(root.join("etc/ufw/ufw.conf"), "ENABLED=yes\n").unwrap();
    fs::write(
        root.join("etc/ufw/user.rules"),
        "-A ufw-user-input -p tcp --dport 8443 -j ACCEPT\n",
    )
    .unwrap();

    // 1. --audit is strictly read-only
    let audit_out = debopty_cmd()
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .args(["reconcile", "--root", root_s, "--no-sys", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let audit_json: Value = serde_json::from_slice(&audit_out).unwrap();
    assert_eq!(audit_json["mode"], "audit");
    assert_eq!(audit_json["changes_made"], 0);
    assert!(audit_json["drift_count"].as_u64().unwrap() >= 4);
    assert!(audit_json["backup_dir"].is_null());

    // 2. --apply converges and preserves RW-FW-01 + U8 artifacts
    let apply1_out = debopty_cmd()
        .env("DEBOPTY_F2B_BACKEND", "systemd")
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
    let apply1_json: Value = serde_json::from_slice(&apply1_out).unwrap();
    assert_eq!(apply1_json["mode"], "apply");
    assert!(apply1_json["changes_made"].as_u64().unwrap() >= 4);
    let backup_dir = PathBuf::from(apply1_json["backup_dir"].as_str().unwrap());
    assert!(backup_dir.join("manifest.json").is_file());

    let nft_after = fs::read_to_string(root.join("etc/nftables.conf")).unwrap();
    assert!(!nft_after.contains("flush ruleset"));
    assert!(nft_after.contains("flush table inet filter"));
    assert!(nft_after.contains("table inet netavark"));
    assert!(root.join("etc/nftables.d/70-user.nft").is_file());
    assert!(
        fs::read_to_string(root.join("etc/debopti/debopti.conf"))
            .unwrap()
            .contains("USER_FLAG=\"keep\"")
    );

    // 3. 2nd --apply is idempotent (changes_made == 0, backup_dir == null)
    let apply2_out = debopty_cmd()
        .env("DEBOPTY_F2B_BACKEND", "systemd")
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
    let apply2_json: Value = serde_json::from_slice(&apply2_out).unwrap();
    assert_eq!(apply2_json["changes_made"], 0);
    assert_eq!(apply2_json["drift_count"], 0);
    assert!(apply2_json["backup_dir"].is_null());

    // 4. --restore restores pre-reconcile state
    debopty_cmd()
        .args([
            "reconcile",
            "--restore",
            backup_dir.to_str().unwrap(),
            "--root",
            root_s,
        ])
        .assert()
        .success();
    assert!(
        fs::read_to_string(root.join("etc/nftables.conf"))
            .unwrap()
            .contains("flush ruleset")
    );
    assert!(root.join("etc/ssh/sshd_config.d/90-bad.conf").is_file());
}

#[test]
fn cli_ssh_harden_non_tty_fail_closed_vs_explicit_non_interactive_pubkey() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("ssh_root");
    let root_s = root.to_str().unwrap();

    fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
    fs::create_dir_all(root.join("etc/debopti")).unwrap();
    fs::create_dir_all(root.join("home/ops")).unwrap();
    fs::write(
        root.join("etc/passwd"),
        "root:x:0:0:root:/root:/bin/bash\nops:x:1000:1000:Ops:/home/ops:/bin/bash\n",
    )
    .unwrap();
    fs::write(root.join("etc/ssh/sshd_config"), "Port 22\n").unwrap();

    // 1. Non-TTY without --non-interactive MUST fail closed with E1003 & 0 mutations
    debopty_cmd()
        .env_remove("NON_INTERACTIVE")
        .args(["security", "ssh-harden", "--root", root_s, "--no-sys"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("[ERROR][E1003]"))
        .stderr(predicate::str::contains("--non-interactive"));
    assert_eq!(
        fs::read_to_string(root.join("etc/ssh/sshd_config")).unwrap(),
        "Port 22\n"
    );

    // 2. Explicit --non-interactive with invalid pubkey fails closed
    debopty_cmd()
        .args([
            "security",
            "ssh-harden",
            "--non-interactive",
            "--pubkey",
            "not-a-valid-key",
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("[ERROR][E1003]"));

    // 3. Explicit --non-interactive + valid Ed25519 pubkey succeeds cleanly
    debopty_cmd()
        .env("DEBOPTY_F2B_BACKEND", "systemd")
        .args([
            "security",
            "ssh-harden",
            "--non-interactive",
            "--pubkey",
            VALID_ED25519_PUBKEY,
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("SSH 加固完成"));

    let auth_keys = fs::read_to_string(root.join("home/ops/.ssh/authorized_keys")).unwrap();
    assert!(auth_keys.contains(VALID_ED25519_PUBKEY));
}

#[test]
fn cli_network_dns_check_inherits_cn_config_and_template_blocks_traversal() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join("debopti.conf");
    let resolv_path = tmp.path().join("resolv.conf");
    fs::write(&cfg_path, "IS_CN_REGION=\"true\"\n").unwrap();
    fs::write(&resolv_path, "nameserver 9.9.9.9\n").unwrap();

    let out = debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "network",
            "dns-check",
            "--mode",
            "calibrate",
            "--resolv",
            resolv_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(parsed["is_cn_region"], true);
    assert_eq!(parsed["modified"], false);

    // Template traversal rejected with E1003 on stderr & empty stdout
    for bad in ["../etc/passwd", "/etc/passwd", "templates/../../VERSION"] {
        debopty_cmd()
            .args(["template", "render", bad])
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("[ERROR][E1003]"))
            .stderr(predicate::str::contains("hint:"));
    }
}

#[test]
fn cli_bridge_leading_dash_script_dir_env_pinning_and_term_dumb_auto_fallback() {
    let tmp = TempDir::new().expect("tempdir");
    // Script directory starting with '-' verifies `--` prevents Bash option injection
    let weird_repo = tmp.path().join("-dash-repo");
    let marker = tmp.path().join("env_pin_marker.txt");
    let cfg_path = tmp.path().join("debopti.conf");
    fs::create_dir_all(weird_repo.join("scripts")).unwrap();
    fs::write(weird_repo.join("scripts/common.sh"), "#!/bin/bash\n").unwrap();
    fs::write(
        weird_repo.join("deb_optimizer.sh"),
        "#!/bin/bash\nprintf 'ENGINE=%s|FROM_RUST=%s|IN_TUI=%s' \"${DEBOPTY_ENGINE:-}\" \"${DEBOPTY_FROM_RUST:-}\" \"${IN_TUI:-}\" > \"${MOCK_MARKER}\"\n",
    )
    .unwrap();

    debopty_cmd()
        .env("MOCK_MARKER", &marker)
        .env("DEBOPTY_ENGINE", "rust")
        .args([
            "--engine",
            "bash",
            "--script-dir",
            weird_repo.to_str().unwrap(),
            "--config",
            cfg_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "ENGINE=bash|FROM_RUST=1|IN_TUI=true"
    );

    // TERM=dumb with --ui auto exits cleanly without panic or raw escape leak
    debopty_cmd()
        .env("TERM", "dumb")
        .env("CI", "true")
        .env("DEBOPTY_ENGINE", "rust")
        .args(["--ui", "auto"])
        .assert()
        .success()
        .stdout(predicate::str::contains("跳过 TUI"));
}

#[test]
fn cli_u10_stdout_stderr_separation_text_json_errors_and_redaction() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join("debopti.conf");

    // 1. Debug + JSON logs stay strictly on stderr while stdout is pure StatusOutput JSON
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

    let status_json: Value = serde_json::from_slice(&output.stdout)
        .expect("stdout must parse as pure JSON with zero log bleed");
    assert_eq!(status_json["modules"].as_array().unwrap().len(), 21);

    let stderr_str = String::from_utf8(output.stderr).unwrap();
    let ndjson_lines: Vec<&str> = stderr_str
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(!ndjson_lines.is_empty());
    for line in ndjson_lines {
        let rec: Value = serde_json::from_str(line).expect("each stderr line must be valid NDJSON");
        assert!(rec["ts"].as_str().unwrap().ends_with('Z'));
        assert!(rec["level"].is_string());
        assert!(rec["target"].is_string());
        assert!(rec["msg"].is_string());
    }

    // 2. Failing command with --log-format text emits [ERROR][E...] + hint: on stderr & empty stdout
    let text_err = debopty_cmd()
        .args([
            "--log-format",
            "text",
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "get",
            "MISSING_KEY_FOR_TEXT_TEST",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(text_err.stdout.is_empty());
    let text_stderr = String::from_utf8(text_err.stderr).unwrap();
    assert!(text_stderr.contains("[ERROR][E1004]"));
    assert!(text_stderr.contains("  hint: "));

    // 3. Failing command with --log-format json emits valid JSON error record with code/msg/hint & empty stdout
    let json_err = debopty_cmd()
        .args([
            "--log-format",
            "json",
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "set",
            "BAD-KEY",
            "password=LeakShouldNotAppear999",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert!(json_err.stdout.is_empty());
    let json_stderr = String::from_utf8(json_err.stderr).unwrap();
    let rec: Value = serde_json::from_str(json_stderr.trim()).unwrap();
    assert_eq!(rec["level"], "error");
    assert_eq!(rec["code"], "E2003");
    assert!(rec["msg"].is_string());
    assert!(rec["hint"].as_str().is_some_and(|h| !h.is_empty()));
    assert!(!json_stderr.contains("LeakShouldNotAppear999"));
}
