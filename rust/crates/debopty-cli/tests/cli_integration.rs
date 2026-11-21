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
    assert_eq!(modules.len(), 20);

    debopty_cmd()
        .args(["--config", cfg_path.to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("版本:"));
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
    assert_eq!(parsed["modules"].as_array().unwrap().len(), 20);
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
