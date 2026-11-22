use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;

use debopty_core::agent_workspace::{
    AUDIT_LOG_REL, AgentWorkspaceOptions, AuditEntry, CHECKPOINTS_DIR_REL, CREDENTIALS_DIR_REL,
    DEFAULT_WORKSPACE_DIR, HOST_PROFILE_REL, MEMORY_FILE_REL, WORKSPACE_SCHEMA_VERSION,
    append_audit_entry, append_memory_note, create_checkpoint, init_workspace, list_checkpoints,
    read_audit_log, rollback_checkpoint, verify_workspace, workspace_status,
};
use debopty_core::credentials::{
    CRED_DIR_MODE, CRED_FILE_MODE, CredentialKind, audit_credentials, list_credentials,
    remove_credential, render_systemd_dropin, store_credential, validate_credential_name,
};
use debopty_core::diag;
use debopty_core::sys::{CmdOutput, MockCommandRunner};
use debopty_core::template::TemplateEngine;
use tempfile::TempDir;

fn synth_github_token() -> String {
    format!("{}_{}", ["gh", "p"].concat(), "X".repeat(24))
}

fn synth_aws_key() -> String {
    format!("{}{}", ["AK", "IA"].concat(), "1234567890ABCDEF")
}

fn synth_sk_key() -> String {
    format!("{}-{}", "sk", "proj9876543210abcdef")
}

fn seed_fake_proc(root: &std::path::Path, hostname: &str, kernel: &str) {
    fs::create_dir_all(root.join("proc/sys/kernel")).unwrap();
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(
        root.join("proc/sys/kernel/hostname"),
        format!("{hostname}\n"),
    )
    .unwrap();
    fs::write(root.join("proc/sys/kernel/osrelease"), format!("{kernel}\n")).unwrap();
    fs::write(
        root.join("etc/os-release"),
        "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\n",
    )
    .unwrap();
}

#[test]
fn init_workspace_provisioning_perms_and_second_run_idempotency() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-sim-01", "6.12.1-amd64");

    let opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();

    let first = init_workspace(&opts).expect("first init");
    assert!(first.changed);
    assert_eq!(first.schema_version, WORKSPACE_SCHEMA_VERSION);
    assert_eq!(first.created_files.len(), 14);
    assert!(first.updated_files.is_empty());

    // Exact U29 directory and file permissions
    for rel_dir in [CREDENTIALS_DIR_REL, CHECKPOINTS_DIR_REL, ".agents/logs"] {
        let mode = fs::metadata(ws.join(rel_dir))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o700, "dir {rel_dir} must be 0700");
    }
    for rel_file in [AUDIT_LOG_REL, MEMORY_FILE_REL, ".agents/workspace.json"] {
        let mode = fs::metadata(ws.join(rel_file))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o600, "file {rel_file} must be 0600");
    }
    for rel_doc in ["AGENTS.md", "README.md", HOST_PROFILE_REL] {
        let mode = fs::metadata(ws.join(rel_doc))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o644, "doc {rel_doc} must be 0644");
    }

    let audit_after_first = read_audit_log(&ws, 0).unwrap();
    assert_eq!(audit_after_first.len(), 1);
    assert_eq!(audit_after_first[0].action, "workspace-init");

    // 2nd run MUST be 100% idempotent with 0 audit spam
    let second = init_workspace(&opts).expect("second init");
    assert!(!second.changed);
    assert!(second.created_files.is_empty());
    assert!(second.updated_files.is_empty());
    assert!(second.preserved_files.contains(&MEMORY_FILE_REL.to_string()));
    assert_eq!(read_audit_log(&ws, 0).unwrap().len(), 1);

    let status = workspace_status(&opts, None).expect("status");
    assert!(status.initialized);
    assert!(status.healthy);
    assert_eq!(status.managed_files_present, status.managed_files_expected);
    assert!(status.missing_files.is_empty());
    assert!(status.memory_exists);
    assert_eq!(status.audit_entries, 1);
    assert!(status.credential_audit.ok);
}

#[test]
fn non_destructive_upgrade_preserves_user_edits_and_updates_unmodified_managed_docs() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-upg-01", "6.1.0-amd64");

    let mut opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();
    init_workspace(&opts).expect("initial init");

    // Customize MEMORY.md, audit.jsonl, host-profile.md custom tail, and README.md
    let mem_path = ws.join(MEMORY_FILE_REL);
    let mut mem = fs::read_to_string(&mem_path).unwrap();
    mem.push_str("\n- custom user memory line\n");
    fs::write(&mem_path, &mem).unwrap();

    append_audit_entry(
        &ws,
        &AuditEntry {
            ts: String::new(),
            action: "custom-op".to_string(),
            actor: "admin".to_string(),
            status: "ok".to_string(),
            checkpoint_id: None,
            summary: "manual note".to_string(),
            paths: Vec::new(),
            details: BTreeMap::new(),
        },
    )
    .unwrap();

    let hp_path = ws.join(HOST_PROFILE_REL);
    let mut hp = fs::read_to_string(&hp_path).unwrap();
    hp.push_str("\n- custom rack: HK-DC2\n");
    fs::write(&hp_path, &hp).unwrap();

    let readme_path = ws.join("README.md");
    let custom_readme = "# Custom Operator README\n\n[AGENTS](AGENTS.md)\n";
    fs::write(&readme_path, custom_readme).unwrap();

    // Simulate upgrade: override AGENTS.md.tmpl & README.md.tmpl and bump kernel version
    let ov_dir = tmp.path().join("override-tpl");
    fs::create_dir_all(ov_dir.join("templates/agent")).unwrap();
    let orig_agents = TemplateEngine::new(None)
        .get_raw("templates/agent/AGENTS.md.tmpl")
        .unwrap();
    let updated_agents = orig_agents.replace("unattended or assisted", "upgraded-v2 unattended");
    fs::write(
        ov_dir.join("templates/agent/AGENTS.md.tmpl"),
        updated_agents,
    )
    .unwrap();
    fs::write(
        ov_dir.join("templates/agent/README.md.tmpl"),
        "# Upstream Upgraded README\n",
    )
    .unwrap();
    fs::write(root.join("proc/sys/kernel/osrelease"), "6.12.9-amd64\n").unwrap();

    opts.template_dir = Some(ov_dir);
    let upgrade = init_workspace(&opts).expect("upgrade init");
    assert!(upgrade.changed);
    assert!(upgrade.updated_files.contains(&"AGENTS.md".to_string()));
    assert!(upgrade.updated_files.contains(&HOST_PROFILE_REL.to_string()));
    assert!(upgrade.preserved_files.contains(&"README.md".to_string()));
    assert!(upgrade.preserved_files.contains(&MEMORY_FILE_REL.to_string()));

    // Verify all user assets preserved & unmodified managed docs upgraded
    assert!(fs::read_to_string(ws.join("AGENTS.md"))
        .unwrap()
        .contains("upgraded-v2 unattended"));
    assert_eq!(fs::read_to_string(&readme_path).unwrap(), custom_readme);
    assert!(fs::read_to_string(&mem_path)
        .unwrap()
        .contains("custom user memory line"));
    let hp_after = fs::read_to_string(&hp_path).unwrap();
    assert!(hp_after.contains("6.12.9-amd64"));
    assert!(hp_after.contains("custom rack: HK-DC2"));
    let audit_after = read_audit_log(&ws, 0).unwrap();
    assert!(audit_after.iter().any(|e| e.action == "custom-op"));
    assert!(audit_after.len() >= 3);
}

#[test]
fn checkpoint_mutation_rollback_and_path_traversal_rejection() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-cp-01", "6.12.0-amd64");

    let opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();
    init_workspace(&opts).unwrap();

    // Seed an existing config file (0640) and symlink
    let conf_file = root.join("etc/ssh/sshd_config");
    fs::create_dir_all(conf_file.parent().unwrap()).unwrap();
    fs::write(&conf_file, "Port 2222\nPasswordAuthentication no\n").unwrap();
    fs::set_permissions(&conf_file, fs::Permissions::from_mode(0o640)).unwrap();

    let link_path = root.join("etc/current-profile");
    symlink("/etc/profile-a", &link_path).unwrap();

    let new_file = root.join("etc/newly-added.conf");
    assert!(!new_file.exists());

    // Reject path traversal in create_checkpoint
    assert!(
        create_checkpoint(
            &opts,
            "bad-path",
            "traversal test",
            &["../etc/passwd".to_string()],
            "agent"
        )
        .is_err()
    );

    let cp = create_checkpoint(
        &opts,
        "pre-ssh-edit",
        "before hardening change",
        &[
            "/etc/ssh/sshd_config".to_string(),
            "/etc/current-profile".to_string(),
            "/etc/newly-added.conf".to_string(),
        ],
        "agent-ops",
    )
    .expect("create_checkpoint");
    assert_eq!(cp.entry_count, 3);
    assert!(cp.rolled_back_at.is_none());

    let listed = list_checkpoints(&opts).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].checkpoint_id, cp.checkpoint_id);

    // Mutate file, change mode, retarget symlink, create new file
    fs::write(&conf_file, "Port 22\nPasswordAuthentication yes\n").unwrap();
    fs::set_permissions(&conf_file, fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(&link_path).unwrap();
    symlink("/etc/profile-b", &link_path).unwrap();
    fs::write(&new_file, "temporary drift\n").unwrap();

    // Reject path traversal in rollback_checkpoint
    assert!(rollback_checkpoint(&opts, "../escape", "agent-ops").is_err());

    let rb = rollback_checkpoint(&opts, &cp.checkpoint_id, "agent-ops").expect("rollback");
    assert_eq!(rb.restored_entries, 3);
    assert!(rb.audit_recorded);

    assert_eq!(
        fs::read_to_string(&conf_file).unwrap(),
        "Port 2222\nPasswordAuthentication no\n"
    );
    assert_eq!(
        fs::metadata(&conf_file).unwrap().permissions().mode() & 0o7777,
        0o640
    );
    assert_eq!(
        fs::read_link(&link_path).unwrap(),
        PathBuf::from("/etc/profile-a")
    );
    assert!(!new_file.exists());

    let listed_after = list_checkpoints(&opts).unwrap();
    assert!(listed_after[0].rolled_back_at.is_some());
    let logs = read_audit_log(&ws, 10).unwrap();
    assert!(logs.iter().any(|e| e.action == "checkpoint-rollback"));
}

#[test]
fn u29_credentials_store_encrypt_fail_closed_dropin_and_audit() {
    let tmp = TempDir::new().unwrap();
    let cred_dir = tmp.path().join(".agents/credentials");

    assert!(validate_credential_name("cf-dns_1.key").is_ok());
    assert!(validate_credential_name("../bad").is_err());
    assert!(validate_credential_name("-leading-dash").is_err());

    let raw_secret = synth_github_token();
    let env_secret = format!("API_TOKEN={}\n", synth_sk_key());
    let enc_secret = synth_aws_key();

    let raw_meta = store_credential(
        &cred_dir,
        "gh-token",
        raw_secret.as_bytes(),
        CredentialKind::RawFile,
        None,
    )
    .expect("store raw");
    assert_eq!(raw_meta.mode, CRED_FILE_MODE);
    assert!(raw_meta.perms_ok);
    assert!(!raw_meta.encrypted);

    let env_meta = store_credential(
        &cred_dir,
        "provider",
        env_secret.as_bytes(),
        CredentialKind::EnvFile,
        None,
    )
    .expect("store env");
    assert!(env_meta.path.ends_with("provider.env"));
    assert_eq!(env_meta.mode, CRED_FILE_MODE);

    // Fail-closed when systemd-creds is absent/unmocked (never plaintext downgrade)
    let absent_runner = MockCommandRunner::new();
    let enc_err = store_credential(
        &cred_dir,
        "db-key",
        enc_secret.as_bytes(),
        CredentialKind::SystemdEncrypted,
        Some(&absent_runner),
    )
    .unwrap_err();
    assert!(enc_err.to_string().contains("systemd-creds"));
    assert!(!cred_dir.join("db-key.cred").exists());
    assert!(!cred_dir.join("db-key").exists());

    // SystemdEncrypted succeeds via MockCommandRunner and stores ciphertext with 0600
    let ok_runner = MockCommandRunner::new();
    ok_runner.on_prog(
        "systemd-creds",
        CmdOutput {
            code: 0,
            stdout: "ENCRYPTED_SYSTEMD_CREDS_BLOB_V1".to_string(),
            stderr: String::new(),
            timed_out: false,
        },
    );
    let enc_meta = store_credential(
        &cred_dir,
        "db-key",
        enc_secret.as_bytes(),
        CredentialKind::SystemdEncrypted,
        Some(&ok_runner),
    )
    .expect("store encrypted");
    assert!(enc_meta.encrypted);
    assert_eq!(enc_meta.mode, CRED_FILE_MODE);
    assert!(enc_meta.path.ends_with("db-key.cred"));
    let on_disk = fs::read_to_string(&enc_meta.path).unwrap();
    assert!(!on_disk.contains(&enc_secret));

    // list_credentials exposes metadata only, zero secret material
    let list = list_credentials(&cred_dir).unwrap();
    assert_eq!(list.len(), 3);
    let list_json = serde_json::to_string(&list).unwrap();
    assert!(!list_json.contains(&raw_secret));
    assert!(!list_json.contains(&enc_secret));

    // render_systemd_dropin for all 3 CredentialKinds + sandbox directives
    let tpl = TemplateEngine::new(None);
    for (kind, expected_dir) in [
        (CredentialKind::RawFile, "LoadCredential=gh-token:"),
        (
            CredentialKind::SystemdEncrypted,
            "LoadCredentialEncrypted=db-key:",
        ),
        (CredentialKind::EnvFile, "EnvironmentFile="),
    ] {
        let name = if kind == CredentialKind::SystemdEncrypted {
            "db-key"
        } else {
            "gh-token"
        };
        let snippet = render_systemd_dropin(&tpl, "my-app", name, &cred_dir.join(name), kind)
            .expect("render dropin");
        assert_eq!(snippet.unit, "my-app.service");
        assert!(snippet.content.contains(expected_dir));
        assert!(snippet.content.contains("ProtectSystem=full"));
        assert!(snippet.content.contains("PrivateTmp=true"));
        assert!(snippet.content.contains("NoNewPrivileges=true"));
    }

    // Clean audit passes
    let clean_audit = audit_credentials(&cred_dir, &[], Some(&ok_runner));
    assert!(clean_audit.ok);
    assert!(clean_audit.systemd_creds_available);
    assert_eq!(clean_audit.cred_dir_mode, Some(CRED_DIR_MODE));

    // Inject weak perms + leaked secret + inline systemd SetCredential & Environment
    fs::set_permissions(&raw_meta.path, fs::Permissions::from_mode(0o644)).unwrap();
    let scan_dir = tmp.path().join("scan-extra");
    fs::create_dir_all(&scan_dir).unwrap();
    let leaked_tok = synth_github_token();
    fs::write(
        scan_dir.join("notes.txt"),
        format!("oops leaked {leaked_tok}\n"),
    )
    .unwrap();
    let inline_val = ["my", "Plain", "Secret42"].concat();
    fs::write(
        scan_dir.join("bad.service"),
        format!("[Service]\nSetCredential=api:{inline_val}\nEnvironment=API_TOKEN={inline_val}\n"),
    )
    .unwrap();

    let dirty_audit = audit_credentials(&cred_dir, &[scan_dir], Some(&ok_runner));
    assert!(!dirty_audit.ok);
    let codes: Vec<&str> = dirty_audit
        .findings
        .iter()
        .map(|f| f.code.as_str())
        .collect();
    assert!(codes.contains(&"CRED_FILE_PERMS"));
    assert!(codes.contains(&"SECRET_LEAK"));
    assert!(codes.contains(&"SYSTEMD_INLINE_SET_CREDENTIAL"));
    assert!(codes.contains(&"SYSTEMD_INLINE_ENV_SECRET"));
    let audit_json = serde_json::to_string(&dirty_audit).unwrap();
    assert!(!audit_json.contains(&leaked_tok));
    assert!(!audit_json.contains(&inline_val));

    // Remove credential
    assert!(remove_credential(&cred_dir, "provider").unwrap());
    assert!(!remove_credential(&cred_dir, "provider").unwrap());
    assert_eq!(list_credentials(&cred_dir).unwrap().len(), 2);
}

#[test]
fn u29_redaction_in_memory_audit_and_json_and_u30_verify_workspace_gates() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-verify-01", "6.12.0-amd64");

    let opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();
    init_workspace(&opts).unwrap();

    // Clean initialized workspace MUST pass verify_workspace
    let clean = verify_workspace(&opts, None).expect("verify clean");
    assert!(clean.passed, "expected clean pass, errors: {:?}", clean.errors);
    assert!(clean.checks_run >= 10);

    // U29 redaction in append_memory_note & append_audit_entry
    let secret_tok = synth_github_token();
    let secret_aws = synth_aws_key();
    append_memory_note(
        &ws,
        "重要变更与排障记录",
        &format!("rotated token {secret_tok} and key {secret_aws}"),
    )
    .unwrap();
    let mem_text = fs::read_to_string(ws.join(MEMORY_FILE_REL)).unwrap();
    assert!(!mem_text.contains(&secret_tok));
    assert!(!mem_text.contains(&secret_aws));
    assert!(mem_text.contains("[REDACTED]"));

    append_audit_entry(
        &ws,
        &AuditEntry {
            ts: String::new(),
            action: "test-redact".to_string(),
            actor: "agent".to_string(),
            status: "ok".to_string(),
            checkpoint_id: None,
            summary: format!("saw {secret_tok}"),
            paths: Vec::new(),
            details: BTreeMap::from([
                ("api_token".to_string(), "raw-secret-val".to_string()),
                ("cred_dir".to_string(), "/var/lib/debopty/agent/.agents/credentials".to_string()),
            ]),
        },
    )
    .unwrap();
    let audit_raw = fs::read_to_string(ws.join(AUDIT_LOG_REL)).unwrap();
    assert!(!audit_raw.contains(&secret_tok));
    assert!(!audit_raw.contains("raw-secret-val"));
    assert!(audit_raw.contains("/var/lib/debopty/agent/.agents/credentials"));

    // Still passes verify_workspace after redacted writes!
    let after_redacted = verify_workspace(&opts, None).unwrap();
    assert!(
        after_redacted.passed,
        "unexpected errors after redacted writes: {:?}",
        after_redacted.errors
    );

    // Also confirm diag::redact_json_value directly
    let mut payload = serde_json::json!({
        "token": secret_tok,
        "cred_dir": "/etc/debopti/credentials",
    });
    diag::redact_json_value(&mut payload);
    assert_eq!(payload["token"], "[REDACTED]");
    assert_eq!(payload["cred_dir"], "/etc/debopti/credentials");

    // Now test U30 & U29 failure gates in verify_workspace:
    // 1. Injected YYYY-MM-DD date in .agents/constraints/
    let c10 = ws.join(".agents/constraints/10_system_change_and_style.md");
    let c10_orig = fs::read_to_string(&c10).unwrap();
    fs::write(&c10, format!("{c10_orig}\nUpdated 2026-11-22\n")).unwrap();
    let rep_date = verify_workspace(&opts, None).unwrap();
    assert!(!rep_date.passed);
    assert!(rep_date.errors.iter().any(|e| e.contains("2026-11-22")));
    fs::write(&c10, &c10_orig).unwrap();

    // 2. Injected XML boundary tag & broken relative .md link in README.md
    let readme = ws.join("README.md");
    let readme_orig = fs::read_to_string(&readme).unwrap();
    fs::write(
        &readme,
        format!("{readme_orig}\n<guide>\n[broken](does_not_exist.md)\n"),
    )
    .unwrap();
    let rep_human = verify_workspace(&opts, None).unwrap();
    assert!(!rep_human.passed);
    assert!(rep_human.errors.iter().any(|e| e.contains("XML boundary")));
    assert!(
        rep_human
            .errors
            .iter()
            .any(|e| e.contains("does_not_exist.md"))
    );
    fs::write(&readme, &readme_orig).unwrap();

    // 3. Weak 0644 permission on .agents/logs/audit.jsonl
    let audit_path = ws.join(AUDIT_LOG_REL);
    fs::set_permissions(&audit_path, fs::Permissions::from_mode(0o644)).unwrap();
    let rep_perm = verify_workspace(&opts, None).unwrap();
    assert!(!rep_perm.passed);
    assert!(rep_perm.errors.iter().any(|e| e.contains("audit.jsonl")));
    fs::set_permissions(&audit_path, fs::Permissions::from_mode(0o600)).unwrap();

    // 4. Raw secret leak outside .agents/credentials/
    let mem_path = ws.join(MEMORY_FILE_REL);
    let mem_orig = fs::read_to_string(&mem_path).unwrap();
    fs::write(&mem_path, format!("{mem_orig}\nleaked {secret_aws}\n")).unwrap();
    let rep_leak = verify_workspace(&opts, None).unwrap();
    assert!(!rep_leak.passed);
    assert!(rep_leak.errors.iter().any(|e| e.contains("SECRET_LEAK")));
    fs::write(&mem_path, &mem_orig).unwrap();
}
