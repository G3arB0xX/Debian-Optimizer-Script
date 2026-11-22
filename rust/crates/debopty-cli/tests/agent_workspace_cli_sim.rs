use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use serde_json::Value;
use tempfile::TempDir;

fn debopty_cmd() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("debopty"))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("resolve repository root")
}

/// Assemble a synthetic GitHub-like token strictly at runtime (zero static literals).
fn runtime_gh_token() -> String {
    let prefix = ["g", "h", "p", "_"].concat();
    let body: String = (0..36)
        .map(|i| {
            let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            alphabet[(i * 7 + 3) % alphabet.len()] as char
        })
        .collect();
    format!("{prefix}{body}")
}

/// Assemble a synthetic sk-* token strictly at runtime (zero static literals).
fn runtime_sk_token() -> String {
    let prefix = ["s", "k", "-", "p", "r", "o", "j", "-"].concat();
    let body: String = (0..32)
        .map(|i| {
            let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            alphabet[(i * 11 + 5) % alphabet.len()] as char
        })
        .collect();
    format!("{prefix}{body}")
}

#[test]
fn schema_registers_all_agent_workspace_and_cred_subcommands() {
    let out = debopty_cmd()
        .args(["schema", "--command", "agent", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: Value = serde_json::from_slice(&out).expect("valid schema envelope");
    assert_eq!(parsed["schema_version"], "1");
    assert_eq!(parsed["ok"], true);
    let cmds = parsed["data"]["commands"]
        .as_array()
        .expect("commands array");
    let names: Vec<&str> = cmds.iter().filter_map(|c| c["name"].as_str()).collect();

    for expected in [
        "agent init",
        "agent status",
        "agent checkpoint",
        "agent rollback",
        "agent audit-log",
        "agent memory",
        "agent verify",
        "agent cred set",
        "agent cred list",
        "agent cred remove",
        "agent cred audit",
        "agent cred systemd-snippet",
    ] {
        assert!(
            names.contains(&expected),
            "missing schema command '{expected}'; got: {names:?}"
        );
    }
}

#[test]
fn agent_workspace_full_lifecycle_redaction_and_cross_tool_e2e() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("rootfs");
    let ws = root.join("var/lib/debopty/agent");
    fs::create_dir_all(root.join("etc")).expect("mkdir etc");
    let target_cfg = root.join("etc/sample.conf");
    fs::write(&target_cfg, "port=22\nmode=initial\n").expect("write sample.conf");

    let ws_s = ws.to_str().unwrap();
    let root_s = root.to_str().unwrap();

    // 1. Init workspace
    let init1_out = debopty_cmd()
        .args(["agent", "init", "--dir", ws_s, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let init1: Value = serde_json::from_slice(&init1_out).expect("init1 json");
    assert_eq!(init1["ok"], true);
    assert_eq!(init1["command"], "agent init");
    assert_eq!(init1["data"]["changed"], true);
    assert!(ws.join("AGENTS.md").is_file());
    assert!(ws.join("README.md").is_file());
    assert!(ws.join("MEMORY.md").is_file());
    assert!(ws.join(".agents/workspace.json").is_file());

    // Verify strict 0700 / 0600 permissions
    assert_eq!(
        fs::metadata(ws.join(".agents/credentials"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(ws.join(".agents/checkpoints"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(ws.join(".agents/logs"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(ws.join("MEMORY.md"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    // 2. Second init is idempotent (changed=false)
    let init2_out = debopty_cmd()
        .args(["agent", "init", "--dir", ws_s, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let init2: Value = serde_json::from_slice(&init2_out).expect("init2 json");
    assert_eq!(init2["data"]["changed"], false);
    assert!(
        init2["data"]["created_files"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        init2["data"]["updated_files"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // 3. Status check
    let st_out = debopty_cmd()
        .args(["agent", "status", "--dir", ws_s, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let st: Value = serde_json::from_slice(&st_out).expect("status json");
    assert_eq!(st["ok"], true);
    assert_eq!(st["data"]["initialized"], true);
    assert_eq!(st["data"]["healthy"], true);
    assert_eq!(st["data"]["credential_audit"]["ok"], true);

    // 4. Checkpoint + mutate + non-interactive exit-6 guard + rollback
    let cp_out = debopty_cmd()
        .args([
            "agent",
            "checkpoint",
            "--dir",
            ws_s,
            "--root",
            root_s,
            "--label",
            "sample",
            "--reason",
            "before edit",
            "--path",
            "/etc/sample.conf",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let cp: Value = serde_json::from_slice(&cp_out).expect("checkpoint json");
    let cp_id = cp["data"]["checkpoint_id"]
        .as_str()
        .expect("checkpoint_id")
        .to_string();
    assert_eq!(cp["data"]["entry_count"], 1);

    // List checkpoints
    let cp_list_out = debopty_cmd()
        .args(["agent", "checkpoint", "--dir", ws_s, "--list", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let cp_list: Value = serde_json::from_slice(&cp_list_out).expect("checkpoint list json");
    assert_eq!(cp_list["data"].as_array().unwrap().len(), 1);

    // Mutate file
    fs::write(&target_cfg, "port=9999\nmode=mutated\n").expect("mutate sample.conf");

    // Rollback without --yes in --non-interactive -> exit 6
    let rb_denied = debopty_cmd()
        .args([
            "--non-interactive",
            "--json",
            "agent",
            "rollback",
            "--dir",
            ws_s,
            "--root",
            root_s,
            &cp_id,
        ])
        .assert()
        .code(6)
        .get_output()
        .stdout
        .clone();
    let rb_denied_json: Value = serde_json::from_slice(&rb_denied).expect("rollback exit 6 json");
    assert_eq!(rb_denied_json["ok"], false);
    assert_eq!(rb_denied_json["exit_code"], 6);
    assert_eq!(rb_denied_json["error"]["category"], "confirmation_required");
    assert_eq!(
        fs::read_to_string(&target_cfg).unwrap(),
        "port=9999\nmode=mutated\n"
    );

    // Rollback with --yes -> exit 0 & restored byte-for-byte
    let rb_ok = debopty_cmd()
        .args([
            "--non-interactive",
            "--yes",
            "--json",
            "agent",
            "rollback",
            "--dir",
            ws_s,
            "--root",
            root_s,
            &cp_id,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rb_ok_json: Value = serde_json::from_slice(&rb_ok).expect("rollback ok json");
    assert_eq!(rb_ok_json["ok"], true);
    assert_eq!(rb_ok_json["data"]["restored_entries"], 1);
    assert_eq!(rb_ok_json["data"]["audit_recorded"], true);
    assert_eq!(
        fs::read_to_string(&target_cfg).unwrap(),
        "port=22\nmode=initial\n"
    );

    // 5. Audit log append + global JSON & disk redaction
    let gh_tok = runtime_gh_token();
    let audit_summary = format!("rotated deploy token {gh_tok} cleanly");
    let audit_add_out = debopty_cmd()
        .args([
            "agent",
            "audit-log",
            "--dir",
            ws_s,
            "--append",
            "--action",
            "token-rotate",
            "--summary",
            &audit_summary,
            "--status",
            "ok",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let audit_add_str = String::from_utf8_lossy(&audit_add_out);
    assert!(
        !audit_add_str.contains(&gh_tok),
        "raw secret leaked in audit-log JSON stdout"
    );
    assert!(audit_add_str.contains("[REDACTED]"));
    let audit_disk = fs::read_to_string(ws.join(".agents/logs/audit.jsonl")).unwrap();
    assert!(
        !audit_disk.contains(&gh_tok),
        "raw secret leaked in audit.jsonl"
    );

    // 6. Memory append + global JSON & disk redaction
    let sk_tok = runtime_sk_token();
    let mem_note = format!("verified api key {sk_tok} via LoadCredential");
    let mem_add_out = debopty_cmd()
        .args([
            "agent", "memory", "--dir", ws_s, "--append", &mem_note, "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let mem_add_json: Value = serde_json::from_slice(&mem_add_out).expect("mem add json");
    assert_eq!(mem_add_json["data"]["appended"], true);
    let mem_disk = fs::read_to_string(ws.join("MEMORY.md")).unwrap();
    assert!(
        !mem_disk.contains(&sk_tok),
        "raw secret leaked in MEMORY.md"
    );
    assert!(mem_disk.contains("[REDACTED]"));

    let mem_read_out = debopty_cmd()
        .args(["agent", "memory", "--dir", ws_s, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let mem_read_str = String::from_utf8_lossy(&mem_read_out);
    assert!(!mem_read_str.contains(&sk_tok));
    assert!(mem_read_str.contains("[REDACTED]"));

    // 7. Verify workspace passes (exit 0)
    let verify_ok = debopty_cmd()
        .args(["agent", "verify", "--dir", ws_s, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let verify_ok_json: Value = serde_json::from_slice(&verify_ok).expect("verify ok json");
    assert_eq!(verify_ok_json["ok"], true);
    assert_eq!(verify_ok_json["exit_code"], 0);
    assert_eq!(verify_ok_json["data"]["passed"], true);

    // Negative verify: loosen credentials dir perm to 0755 -> exit 1 & ok=false
    fs::set_permissions(
        ws.join(".agents/credentials"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("chmod 0755");
    let verify_fail = debopty_cmd()
        .args(["agent", "verify", "--dir", ws_s, "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let verify_fail_json: Value = serde_json::from_slice(&verify_fail).expect("verify fail json");
    assert_eq!(verify_fail_json["ok"], false);
    assert_eq!(verify_fail_json["exit_code"], 1);
    assert_eq!(verify_fail_json["data"]["passed"], false);
    // Restore 0700
    fs::set_permissions(
        ws.join(".agents/credentials"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("chmod 0700");

    // 8. Cross-tool U29 + U30 E2E proof on the live provisioned workspace!
    let repo = repo_root();
    let fmt_out = Command::new("python3")
        .args([
            "-B",
            repo.join("tools/verify_agent_fmt.py").to_str().unwrap(),
            "--root",
            ws_s,
        ])
        .output()
        .expect("run verify_agent_fmt.py");
    assert!(
        fmt_out.status.success(),
        "verify_agent_fmt.py --root <ws> failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&fmt_out.stdout),
        String::from_utf8_lossy(&fmt_out.stderr)
    );

    let hyg_out = Command::new("python3")
        .args([
            "-B",
            repo.join("tools/verify_repo_hygiene.py").to_str().unwrap(),
            "--root",
            ws_s,
            "--check-workspace-perms",
        ])
        .output()
        .expect("run verify_repo_hygiene.py");
    assert!(
        hyg_out.status.success(),
        "verify_repo_hygiene.py --root <ws> --check-workspace-perms failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&hyg_out.stdout),
        String::from_utf8_lossy(&hyg_out.stderr)
    );
}

#[test]
fn agent_cred_lifecycle_and_dry_run_zero_writes() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().join("rootfs");
    let ws = tmp.path().join("cred_ws");
    let dry_ws = tmp.path().join("dry_ws");
    let ws_s = ws.to_str().unwrap();
    let root_s = root.to_str().unwrap();
    let dry_ws_s = dry_ws.to_str().unwrap();

    // Dry-run init writes NOTHING to disk
    let dry_init = debopty_cmd()
        .args(["--dry-run", "--json", "agent", "init", "--dir", dry_ws_s])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let dry_init_json: Value = serde_json::from_slice(&dry_init).expect("dry init json");
    assert_eq!(dry_init_json["ok"], true);
    assert_eq!(dry_init_json["dry_run"], true);
    assert!(
        !dry_ws.exists(),
        "dry-run init must not create workspace dir"
    );

    // Initialize real workspace
    debopty_cmd()
        .args(["agent", "init", "--dir", ws_s, "--json"])
        .assert()
        .success();

    // cred set without --value-stdin or --from-file fails with usage exit code 2
    debopty_cmd()
        .args([
            "agent", "cred", "set", "--dir", ws_s, "--name", "cf_dns", "--json",
        ])
        .assert()
        .code(2);

    // cred set with forbidden --value flag fails with usage exit code 2
    debopty_cmd()
        .args([
            "agent", "cred", "set", "--dir", ws_s, "--name", "cf_dns", "--value", "nope", "--json",
        ])
        .assert()
        .code(2);

    // cred set --dry-run writes NOTHING
    debopty_cmd()
        .args([
            "--dry-run",
            "--json",
            "agent",
            "cred",
            "set",
            "--dir",
            ws_s,
            "--name",
            "dry_cred",
            "--kind",
            "raw",
            "--value-stdin",
        ])
        .assert()
        .success();
    assert!(!ws.join(".agents/credentials/dry_cred").exists());

    // cred set via --value-stdin with runtime-assembled secret
    let secret = runtime_gh_token();
    let set_out = debopty_cmd()
        .args([
            "agent",
            "cred",
            "set",
            "--dir",
            ws_s,
            "--name",
            "cf_dns",
            "--kind",
            "raw",
            "--value-stdin",
            "--json",
        ])
        .write_stdin(secret.as_bytes())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let set_str = String::from_utf8_lossy(&set_out);
    assert!(
        !set_str.contains(&secret),
        "cred set must never echo secret"
    );
    let set_json: Value = serde_json::from_slice(&set_out).expect("cred set json");
    assert_eq!(set_json["data"]["name"], "cf_dns");
    assert_eq!(set_json["data"]["perms_ok"], true);
    assert_eq!(
        fs::metadata(ws.join(".agents/credentials/cf_dns"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    // cred list returns metadata only (zero secret)
    let list_out = debopty_cmd()
        .args(["agent", "cred", "list", "--dir", ws_s, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let list_str = String::from_utf8_lossy(&list_out);
    assert!(!list_str.contains(&secret));
    let list_json: Value = serde_json::from_slice(&list_out).expect("cred list json");
    assert_eq!(list_json["data"].as_array().unwrap().len(), 1);

    // cred audit passes
    let audit_out = debopty_cmd()
        .args(["agent", "cred", "audit", "--dir", ws_s, "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let audit_json: Value = serde_json::from_slice(&audit_out).expect("cred audit json");
    assert_eq!(audit_json["data"]["ok"], true);

    // cred systemd-snippet --write-dropin --dry-run writes NOTHING
    debopty_cmd()
        .args([
            "--dry-run",
            "--json",
            "agent",
            "cred",
            "systemd-snippet",
            "--dir",
            ws_s,
            "--root",
            root_s,
            "--unit",
            "demo",
            "--name",
            "cf_dns",
            "--kind",
            "raw",
            "--write-dropin",
        ])
        .assert()
        .success();
    let expected_dropin =
        root.join("etc/systemd/system/demo.service.d/90-debopty-credentials.conf");
    assert!(!expected_dropin.exists(), "dry-run must not write dropin");

    // cred systemd-snippet --write-dropin writes hardened dropin
    let snip_out = debopty_cmd()
        .args([
            "agent",
            "cred",
            "systemd-snippet",
            "--dir",
            ws_s,
            "--root",
            root_s,
            "--unit",
            "demo",
            "--name",
            "cf_dns",
            "--kind",
            "raw",
            "--write-dropin",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let snip_json: Value = serde_json::from_slice(&snip_out).expect("snippet json");
    assert_eq!(snip_json["data"]["written"], true);
    let dropin_text = fs::read_to_string(&expected_dropin).expect("read dropin");
    assert!(dropin_text.contains("LoadCredential=cf_dns:"));
    assert!(dropin_text.contains("ProtectSystem=full"));
    assert!(dropin_text.contains("PrivateTmp=true"));
    assert!(dropin_text.contains("NoNewPrivileges=true"));

    // cred remove requires --yes under --non-interactive (exit 6)
    debopty_cmd()
        .args([
            "--non-interactive",
            "--json",
            "agent",
            "cred",
            "remove",
            "--dir",
            ws_s,
            "--name",
            "cf_dns",
        ])
        .assert()
        .code(6);
    assert!(ws.join(".agents/credentials/cf_dns").is_file());

    // cred remove with --yes succeeds
    let rm_out = debopty_cmd()
        .args([
            "--non-interactive",
            "--yes",
            "--json",
            "agent",
            "cred",
            "remove",
            "--dir",
            ws_s,
            "--name",
            "cf_dns",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rm_json: Value = serde_json::from_slice(&rm_out).expect("cred remove json");
    assert_eq!(rm_json["data"]["removed"], true);
    assert!(!ws.join(".agents/credentials/cf_dns").exists());
}

#[test]
fn self_install_forwards_agent_workspace_flags() {
    let tmp = TempDir::new().expect("tempdir");
    let root1 = tmp.path().join("r1");
    let root2 = tmp.path().join("r2");
    let fake_bin = tmp.path().join("fake-debopty");
    fs::write(&fake_bin, b"#!/bin/sh\nexit 0\n").expect("write fake bin");
    fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755)).expect("chmod fake bin");

    // 1. Custom --agent-workspace-dir provisions workspace
    debopty_cmd()
        .args([
            "self-install",
            "--root",
            root1.to_str().unwrap(),
            "--source-bin",
            fake_bin.to_str().unwrap(),
            "--no-sys",
            "--agent-workspace-dir",
            "/srv/custom-agent",
            "--json",
        ])
        .assert()
        .success();
    assert!(root1.join("srv/custom-agent/AGENTS.md").is_file());

    // 2. --skip-agent-workspace skips provisioning
    debopty_cmd()
        .args([
            "self-install",
            "--root",
            root2.to_str().unwrap(),
            "--source-bin",
            fake_bin.to_str().unwrap(),
            "--no-sys",
            "--skip-agent-workspace",
            "--json",
        ])
        .assert()
        .success();
    assert!(!root2.join("var/lib/debopty/agent").exists());
}
