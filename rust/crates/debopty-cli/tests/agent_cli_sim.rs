use std::fs;

use assert_cmd::Command as AssertCommand;
use serde_json::Value;
use tempfile::TempDir;

fn debopty_cmd() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("debopty"))
}

#[test]
fn schema_manifest_and_prefix_filter() {
    let out = debopty_cmd()
        .args(["schema", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).expect("valid schema envelope");
    assert_eq!(parsed["schema_version"], "1");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["command"], "schema");
    assert_eq!(parsed["dry_run"], false);
    assert_eq!(parsed["exit_code"], 0);
    assert_eq!(parsed["data"]["tool"], "debopty");
    assert!(parsed["data"]["global_flags"].as_array().unwrap().len() >= 5);
    assert_eq!(parsed["data"]["exit_codes"].as_array().unwrap().len(), 13);
    assert!(parsed["data"]["commands"].as_array().unwrap().len() >= 35);

    let sec_out = debopty_cmd()
        .args(["schema", "--command", "security", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let sec_parsed: Value = serde_json::from_slice(&sec_out).expect("valid filtered schema");
    let cmds = sec_parsed["data"]["commands"].as_array().unwrap();
    assert!(!cmds.is_empty());
    for c in cmds {
        assert!(
            c["name"].as_str().unwrap().starts_with("security"),
            "unexpected command in security filter: {c}"
        );
    }
}

#[test]
fn global_envelope_and_debopty_agent_env_wrap_legacy_json() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join("debopti.conf");

    // 1. Plain --json stays legacy top-level for backward compatibility
    let legacy_out = debopty_cmd()
        .args(["--config", cfg_path.to_str().unwrap(), "status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let legacy_json: Value = serde_json::from_slice(&legacy_out).expect("legacy json");
    assert!(legacy_json["status"].is_object());
    assert!(legacy_json["telemetry"].is_object());
    assert!(legacy_json.get("schema_version").is_none());

    // 2. --envelope wraps in AgentEnvelope v1
    let env_out = debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "status",
            "--envelope",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let env_json: Value = serde_json::from_slice(&env_out).expect("envelope json");
    assert_eq!(env_json["schema_version"], "1");
    assert_eq!(env_json["ok"], true);
    assert_eq!(env_json["command"], "status");
    assert_eq!(env_json["exit_code"], 0);
    assert!(env_json["data"]["status"].is_object());
    assert!(env_json["data"]["telemetry"].is_object());

    // 3. DEBOPTY_AGENT=1 auto-enables envelope mode even without --json/--envelope
    let agent_out = debopty_cmd()
        .env("DEBOPTY_AGENT", "1")
        .args(["version"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let agent_json: Value = serde_json::from_slice(&agent_out).expect("agent env json");
    assert_eq!(agent_json["schema_version"], "1");
    assert_eq!(agent_json["ok"], true);
    assert_eq!(agent_json["command"], "version");
    assert!(agent_json["data"]["display"].is_string());
}

#[test]
fn dry_run_guarantees_zero_mutations_across_commands() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("dry_root");
    let root_s = root.to_str().unwrap();
    fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/debopti")).unwrap();
    fs::write(
        root.join("etc/nftables.conf"),
        "flush ruleset\ntable inet filter {\n  chain input {\n    tcp dport 22 accept comment \"SSH_Access_Port\"\n  }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/debopti/debopti.conf"),
        "IS_CN_REGION=\"false\"\n",
    )
    .unwrap();

    // 1. reconcile --apply --dry-run --json
    let rec_out = debopty_cmd()
        .args([
            "reconcile",
            "--apply",
            "--dry-run",
            "--json",
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rec_json: Value = serde_json::from_slice(&rec_out).expect("reconcile dry-run json");
    assert_eq!(rec_json["ok"], true);
    assert_eq!(rec_json["dry_run"], true);
    assert_eq!(rec_json["data"]["changes_made"], 0);
    assert!(
        fs::read_to_string(root.join("etc/nftables.conf"))
            .unwrap()
            .contains("flush ruleset")
    );

    // 2. system sysctl --dry-run --json
    let sys_out = debopty_cmd()
        .args([
            "system",
            "sysctl",
            "--root",
            root_s,
            "--no-sys",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let sys_json: Value = serde_json::from_slice(&sys_out).expect("system sysctl dry-run");
    assert_eq!(sys_json["ok"], true);
    assert_eq!(sys_json["dry_run"], true);
    assert!(!root.join("etc/sysctl.d/99-debopti-optimize.conf").exists());

    // 3. security fw-add --dry-run --json
    let fw_out = debopty_cmd()
        .args([
            "security",
            "fw-add",
            "8443",
            "--proto",
            "tcp",
            "--comment",
            "DryTest",
            "--root",
            root_s,
            "--no-sys",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let fw_json: Value = serde_json::from_slice(&fw_out).expect("fw-add dry-run");
    assert_eq!(fw_json["ok"], true);
    assert_eq!(fw_json["dry_run"], true);
    assert!(!root.join("etc/nftables.d/debopti/DryTest.nft").exists());

    // 4. app install podman --dry-run --json
    let app_out = debopty_cmd()
        .args([
            "app",
            "install",
            "podman",
            "--root",
            root_s,
            "--no-sys",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let app_json: Value = serde_json::from_slice(&app_out).expect("app install dry-run");
    assert_eq!(app_json["ok"], true);
    assert_eq!(app_json["dry_run"], true);
    assert_eq!(app_json["data"]["target"], "podman");

    // 5. config set --dry-run --json
    let cfg_path = root.join("etc/debopti/debopti.conf");
    let cfg_out = debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "config",
            "set",
            "IS_CN_REGION",
            "true",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let cfg_json: Value = serde_json::from_slice(&cfg_out).expect("config set dry-run");
    assert_eq!(cfg_json["ok"], true);
    assert_eq!(cfg_json["dry_run"], true);
    assert_eq!(
        fs::read_to_string(&cfg_path).unwrap(),
        "IS_CN_REGION=\"false\"\n"
    );

    // 6. self-uninstall --purge-config --dry-run --json (even with --non-interactive and no --yes)
    let uninst_out = debopty_cmd()
        .args([
            "self-uninstall",
            "--purge-config",
            "--root",
            root_s,
            "--no-sys",
            "--non-interactive",
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let uninst_json: Value = serde_json::from_slice(&uninst_out).expect("self-uninstall dry-run");
    assert_eq!(uninst_json["ok"], true);
    assert_eq!(uninst_json["dry_run"], true);
    assert_eq!(uninst_json["data"]["requires_yes"], true);
    assert!(cfg_path.is_file());
}

#[test]
fn deterministic_exit_codes_error_envelopes_and_redaction() {
    let tmp = TempDir::new().expect("tempdir");
    let cfg_path = tmp.path().join("debopti.conf");
    fs::write(&cfg_path, "IS_CN_REGION=\"false\"\n").unwrap();

    // Exit code 2 (usage / clap parse error in --json mode)
    let bad_arg_out = debopty_cmd()
        .args(["--json", "app", "install", "nonexistent-app"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let bad_arg_json: Value = serde_json::from_slice(&bad_arg_out).expect("usage error envelope");
    assert_eq!(bad_arg_json["ok"], false);
    assert_eq!(bad_arg_json["exit_code"], 2);
    assert_eq!(bad_arg_json["error"]["category"], "usage");
    assert!(
        bad_arg_json["error"]["remediation_hint"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );

    // Exit code 4 (not_found) for missing valid config key
    let nf_out = debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--json",
            "config",
            "get",
            "MISSING_AGENT_KEY_404",
        ])
        .assert()
        .code(4)
        .get_output()
        .stdout
        .clone();
    let nf_json: Value = serde_json::from_slice(&nf_out).expect("not_found error envelope");
    assert_eq!(nf_json["ok"], false);
    assert_eq!(nf_json["exit_code"], 4);
    assert_eq!(nf_json["error"]["code"], "E1004");
    assert_eq!(nf_json["error"]["category"], "not_found");

    // Secret redaction in JSON error envelope (invalid key containing secret)
    let secret_key = format!("password={}", "SuperSecretToken999");
    let redact_out = debopty_cmd()
        .args([
            "--config",
            cfg_path.to_str().unwrap(),
            "--json",
            "config",
            "get",
            &secret_key,
        ])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let redact_json: Value = serde_json::from_slice(&redact_out).expect("redacted error envelope");
    assert_eq!(redact_json["ok"], false);
    assert_eq!(redact_json["exit_code"], 2);
    let msg = redact_json["error"]["message"].as_str().unwrap();
    assert!(
        !msg.contains("SuperSecretToken999"),
        "secret leaked in error envelope: {msg}"
    );
    assert!(
        msg.contains("[REDACTED]"),
        "expected redaction marker in: {msg}"
    );

    // Exit code 6 (confirmation_required) on reconcile --apply --non-interactive without --yes
    let root = tmp.path().join("confirm_root");
    let root_s = root.to_str().unwrap();
    let conf_rec_out = debopty_cmd()
        .args([
            "reconcile",
            "--apply",
            "--non-interactive",
            "--json",
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .code(6)
        .get_output()
        .stdout
        .clone();
    let conf_rec_json: Value = serde_json::from_slice(&conf_rec_out).expect("confirm error json");
    assert_eq!(conf_rec_json["ok"], false);
    assert_eq!(conf_rec_json["exit_code"], 6);
    assert_eq!(conf_rec_json["error"]["code"], "E1005");
    assert_eq!(conf_rec_json["error"]["category"], "confirmation_required");

    // Exit code 6 (confirmation_required) on self-uninstall --purge-config --non-interactive without --yes
    let conf_uninst_out = debopty_cmd()
        .args([
            "self-uninstall",
            "--purge-config",
            "--non-interactive",
            "--json",
            "--root",
            root_s,
            "--no-sys",
        ])
        .assert()
        .code(6)
        .get_output()
        .stdout
        .clone();
    let conf_uninst_json: Value =
        serde_json::from_slice(&conf_uninst_out).expect("self-uninstall confirm error json");
    assert_eq!(conf_uninst_json["ok"], false);
    assert_eq!(conf_uninst_json["exit_code"], 6);
    assert_eq!(conf_uninst_json["error"]["code"], "E1005");
}

#[test]
fn bare_debopty_json_non_interactive_exits_zero_with_schema_hint() {
    let out = debopty_cmd()
        .args(["--json", "--non-interactive"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).expect("bare debopty json envelope");
    assert_eq!(parsed["schema_version"], "1");
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["exit_code"], 0);
    assert_eq!(parsed["data"]["skipped_tui"], true);
    assert!(
        parsed["data"]["next_command"]
            .as_str()
            .unwrap()
            .contains("schema")
    );
}

#[test]
fn schema_includes_hardware_retention_ide_audit_and_hides_tui_trampoline() {
    let out = debopty_cmd()
        .args(["schema", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).expect("valid schema json");
    let cmds = parsed["data"]["commands"]
        .as_array()
        .expect("commands array");

    let find_cmd = |name: &str| {
        cmds.iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("missing command '{name}' in schema"))
    };

    let hw = find_cmd("system hardware");
    assert_eq!(hw["mutates"], false);
    assert_eq!(hw["requires_root"], false);

    let ret = find_cmd("system retention");
    assert_eq!(ret["mutates"], true);
    assert_eq!(ret["supports_dry_run"], true);

    let self_install = find_cmd("self-install");
    let si_args = self_install["key_args"].as_array().unwrap();
    assert!(si_args.iter().any(|a| a == "--skip-retention"));

    let audit = find_cmd("agent audit");
    let audit_args = audit["key_args"].as_array().unwrap();
    assert!(audit_args.iter().any(|a| a == "--check-ide-caches"));
    assert!(audit_args.iter().any(|a| a == "--no-ide-cache-check"));
    assert!(audit_args.iter().any(|a| a == "--home-dir <PATH>"));

    // Hidden elevation trampoline MUST NOT appear in public schema
    assert!(
        !cmds.iter().any(|c| c["name"] == "_tui-exec-outcome"),
        "_tui-exec-outcome must stay hidden from schema"
    );
}
