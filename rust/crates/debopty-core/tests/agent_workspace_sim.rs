use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;

use debopty_core::agent_workspace::{
    AUDIT_LOG_REL, AgentWorkspaceOptions, AuditEntry, CACHE_DIR_REL, CHECKPOINTS_DIR_REL,
    CREDENTIALS_DIR_REL, DEFAULT_WORKSPACE_DIR, EXIT_MEMORY_BLOAT, EXIT_MEMORY_SECRET_LEAK,
    EXIT_MIGRATION_CONFLICT, EXIT_SECRET_DETECTED, EXIT_WRITE_GATE_REJECT, HOST_PROFILE_REL,
    LEGACY_ROOT_MEMORY_REL, LEGACY_SSOT_REL, MCP_SERVERS_REL, MEMORY_ARCHIVE_DIR_REL,
    MEMORY_DIR_REL, MEMORY_FILE_REL, MEMORY_INDEX_CACHE_REL, MEMORY_INDEX_MD_REL,
    MemoryTopicInput, MemoryTracking, OverrideAction, SwitchMode, TMP_DIR_REL, TopicStatus,
    TopicType, WORKSPACE_SCHEMA_VERSION, WriteGateVerdict, append_audit_entry, append_memory_note,
    audit_workspace, builtin_adapter_registry, consolidate_memory, create_checkpoint,
    init_workspace, lint_memory, list_checkpoints, manage_override, migrate_workspace,
    promote_memory, read_audit_log, rebuild_memory_index, rollback_checkpoint, search_memory,
    switch_harnesses, verify_workspace, workspace_status, write_memory_topic,
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
    fs::write(
        root.join("proc/sys/kernel/osrelease"),
        format!("{kernel}\n"),
    )
    .unwrap();
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
    assert_eq!(WORKSPACE_SCHEMA_VERSION, "2");
    assert_eq!(first.created_files.len(), 28);
    assert!(first.updated_files.is_empty());
    assert_eq!(first.memory_tracking, MemoryTracking::Local);
    assert!(!first.memory_tracking_notice.is_empty());
    assert!(first.active_harnesses.is_empty());
    assert!(!ws.join("CLAUDE.md").exists());
    assert!(!ws.join("GEMINI.md").exists());

    // Exact U29/v2 directory and file permissions
    for rel_dir in [
        CREDENTIALS_DIR_REL,
        CHECKPOINTS_DIR_REL,
        ".agents/logs",
        MEMORY_DIR_REL,
        MEMORY_ARCHIVE_DIR_REL,
        CACHE_DIR_REL,
        TMP_DIR_REL,
    ] {
        let mode = fs::metadata(ws.join(rel_dir)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o700, "dir {rel_dir} must be 0700");
    }
    for rel_file in [
        AUDIT_LOG_REL,
        MEMORY_FILE_REL,
        HOST_PROFILE_REL,
        ".agents/workspace.json",
    ] {
        let mode = fs::metadata(ws.join(rel_file))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o600, "file {rel_file} must be 0600");
    }
    for rel_doc in [
        "AGENTS.md",
        "README.md",
        ".gitignore",
        ".agents/schemas/repo_blueprint.json",
    ] {
        let mode = fs::metadata(ws.join(rel_doc)).unwrap().permissions().mode() & 0o7777;
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
    assert!(
        second
            .preserved_files
            .contains(&MEMORY_FILE_REL.to_string())
    );
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
    assert!(
        upgrade
            .updated_files
            .contains(&HOST_PROFILE_REL.to_string())
    );
    assert!(upgrade.preserved_files.contains(&"README.md".to_string()));
    assert!(
        upgrade
            .preserved_files
            .contains(&MEMORY_FILE_REL.to_string())
    );

    // Verify all user assets preserved & unmodified managed docs upgraded
    assert!(
        fs::read_to_string(ws.join("AGENTS.md"))
            .unwrap()
            .contains("upgraded-v2 unattended")
    );
    assert_eq!(fs::read_to_string(&readme_path).unwrap(), custom_readme);
    assert!(
        fs::read_to_string(&mem_path)
            .unwrap()
            .contains("custom user memory line")
    );
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
    assert!(
        clean.passed,
        "expected clean pass, errors: {:?}",
        clean.errors
    );
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
                (
                    "cred_dir".to_string(),
                    "/var/lib/debopty/agent/.agents/credentials".to_string(),
                ),
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

fn sample_topic(id: &str, title: &str, scope: &str, body: &str) -> MemoryTopicInput {
    MemoryTopicInput {
        id: id.to_string(),
        title: title.to_string(),
        topic_type: TopicType::Procedural,
        status: TopicStatus::Active,
        scope: scope.to_string(),
        tags: vec!["vps".to_string(), "ops".to_string()],
        sources: vec!["cli:agent-memory".to_string()],
        confidence: 0.9,
        body: body.to_string(),
        ttl_days: None,
        expires_at: None,
        supersedes: None,
        allow_update: false,
        actor: Some("test-agent".to_string()),
    }
}

#[test]
fn coala_write_gate_reject_codes_verdicts_and_sub_sharding_bm25_search() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-mem-01", "6.12.0-amd64");

    let opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();
    init_workspace(&opts).unwrap();

    // 1. Secret leak rejected with exit code 23
    let leak_body = format!("use token {} for deploy", synth_github_token());
    let err_secret = write_memory_topic(
        &opts,
        &sample_topic("bad-secret", "Leaked Token", "security", &leak_body),
    )
    .unwrap_err();
    assert_eq!(err_secret.agent_exit_code(), Some(EXIT_MEMORY_SECRET_LEAK));

    // 2. Bad schema / empty sources rejected with exit code 21
    let mut bad_schema = sample_topic("BAD SLUG!", "Bad Slug", "vps", "hello");
    let err_slug = write_memory_topic(&opts, &bad_schema).unwrap_err();
    assert_eq!(err_slug.agent_exit_code(), Some(EXIT_WRITE_GATE_REJECT));
    bad_schema.id = "valid-id".to_string();
    bad_schema.sources.clear();
    let err_src = write_memory_topic(&opts, &bad_schema).unwrap_err();
    assert_eq!(err_src.agent_exit_code(), Some(EXIT_WRITE_GATE_REJECT));

    // 3. Bloat (>200 lines & >80-line raw fence) rejected with exit code 22
    let big_body = (0..210)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let err_bloat = write_memory_topic(
        &opts,
        &sample_topic("too-long", "Too Long", "vps", &big_body),
    )
    .unwrap_err();
    assert_eq!(err_bloat.agent_exit_code(), Some(EXIT_MEMORY_BLOAT));

    let fence_body = format!(
        "```bash\n{}\n```",
        (0..85)
            .map(|i| format!("echo {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let err_fence = write_memory_topic(
        &opts,
        &sample_topic("big-fence", "Big Fence", "vps", &fence_body),
    )
    .unwrap_err();
    assert_eq!(err_fence.agent_exit_code(), Some(EXIT_MEMORY_BLOAT));

    // 4. ADD -> NOOP -> DUPLICATE_CONFLICT (21) -> UPDATE -> SUPERSEDE
    let t1 = sample_topic(
        "nftables-baseline",
        "NFTables 防火墙端口基线",
        "security",
        "仅放行 2222/tcp 与 443/tcp，默认 drop 入站流量。",
    );
    let r_add = write_memory_topic(&opts, &t1).unwrap();
    assert_eq!(r_add.verdict, WriteGateVerdict::Add);
    assert!(r_add.changed);

    let r_noop = write_memory_topic(&opts, &t1).unwrap();
    assert_eq!(r_noop.verdict, WriteGateVerdict::Noop);
    assert!(!r_noop.changed);

    let mut t1_mod = t1.clone();
    t1_mod.body = "更新：放行 2222/tcp、443/tcp 与 8443/udp。".to_string();
    let err_dup = write_memory_topic(&opts, &t1_mod).unwrap_err();
    assert_eq!(err_dup.agent_exit_code(), Some(EXIT_WRITE_GATE_REJECT));

    t1_mod.allow_update = true;
    let r_upd = write_memory_topic(&opts, &t1_mod).unwrap();
    assert_eq!(r_upd.verdict, WriteGateVerdict::Update);
    assert!(r_upd.checkpoint_id.is_some());

    let mut t2 = sample_topic(
        "nftables-baseline-v2",
        "NFTables 防火墙基线第二版",
        "security",
        "全面迁移至 nftables 集合白名单与 Fail2ban 自动端口同步。",
    );
    t2.supersedes = Some("nftables-baseline".to_string());
    let r_sup = write_memory_topic(&opts, &t2).unwrap();
    assert_eq!(r_sup.verdict, WriteGateVerdict::Supersede);
    assert!(r_sup.checkpoint_id.is_some());

    // 5. >15 active topics -> domain sub-sharding + overflow INDEX.md + EN/ZH BM25 search
    for i in 1..=16 {
        let mut t = sample_topic(
            &format!("topic-{i:02}"),
            &format!("运维主题 {i} 网络路由与端口优化"),
            "network",
            &format!("Debian sysctl BBR congestion control tuning note #{i} 拥塞控制优化。"),
        );
        t.tags = vec!["network".to_string(), format!("batch-{i}")];
        let rep = write_memory_topic(&opts, &t).unwrap();
        assert_eq!(rep.verdict, WriteGateVerdict::Add);
    }

    let idx_rep = rebuild_memory_index(&opts).unwrap();
    assert!(idx_rep.overflow_catalog_active);
    assert!(idx_rep.active_topics > 15);
    assert!(idx_rep.toc_lines <= 150);
    assert!(ws.join(MEMORY_INDEX_CACHE_REL).is_file());
    let mem_idx_text = fs::read_to_string(ws.join(MEMORY_INDEX_MD_REL)).unwrap();
    assert!(mem_idx_text.contains("OVERFLOW_TOPICS:"));

    // Verify EN + ZH BM25 search
    let sr_zh = search_memory(&opts, "防火墙 端口同步", 5).unwrap();
    assert!(!sr_zh.hits.is_empty());
    assert_eq!(sr_zh.hits[0].topic_id, "nftables-baseline-v2");
    assert!(sr_zh.hits[0].pointer.contains(":L"));
    assert!(sr_zh.hits[0].snippet.lines().count() <= 20);

    let sr_en = search_memory(&opts, "congestion control BBR", 5).unwrap();
    assert!(!sr_en.hits.is_empty());

    // Workspace verify & memory lint must pass cleanly even with >15 topics & sub-sharding!
    let lint = lint_memory(&opts).unwrap();
    assert!(lint.passed, "lint errors: {:?}", lint.errors);
    let ver = verify_workspace(&opts, None).unwrap();
    assert!(ver.passed, "verify errors: {:?}", ver.errors);
}

#[test]
fn coala_consolidate_promote_and_memory_tracking_gitignore() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-cons-01", "6.12.0-amd64");

    let mut opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();
    init_workspace(&opts).unwrap();

    // Verify default local .gitignore + user custom line preservation
    let gi_path = ws.join(".gitignore");
    let mut gi = fs::read_to_string(&gi_path).unwrap();
    assert!(gi.contains(".agents/MEMORY.md"));
    assert!(gi.contains(".agents/memory/"));
    gi.push_str("\n# user custom rule\n*.local-bak\n");
    fs::write(&gi_path, &gi).unwrap();

    opts.memory_tracking = Some(MemoryTracking::Tracked);
    let tracked_rep = init_workspace(&opts).unwrap();
    assert_eq!(tracked_rep.memory_tracking, MemoryTracking::Tracked);
    let gi_tracked = fs::read_to_string(&gi_path).unwrap();
    assert!(!gi_tracked.contains("\n.agents/MEMORY.md\n"));
    assert!(gi_tracked.contains(".agents/memory/host-profile.md"));
    assert!(gi_tracked.contains("*.local-bak"));

    // Write an expired episodic topic and a high-confidence procedural topic
    let mut expired = sample_topic(
        "old-incident",
        "历史临时故障",
        "vps",
        "旧的临时排查记录，已过期。",
    );
    expired.topic_type = TopicType::Episodic;
    expired.expires_at = Some("2020-01-01T00:00:00Z".to_string());
    write_memory_topic(&opts, &expired).unwrap();

    let high_conf = sample_topic(
        "ssh-hardening-rule",
        "SSH 密钥登录强制规则",
        "security",
        "禁止密码登录并始终在变更前使用 sshd -t 预检。",
    );
    write_memory_topic(&opts, &high_conf).unwrap();

    // Consolidate plan -> apply
    let plan = consolidate_memory(&opts, false).unwrap();
    assert!(!plan.applied);
    assert!(plan.items.iter().any(|i| i.action == "archive_expired"));
    assert!(plan.promotion_candidates >= 1);

    let applied = consolidate_memory(&opts, true).unwrap();
    assert!(applied.applied);
    assert_eq!(applied.archived_count, 1);
    assert!(applied.checkpoint_id.is_some());
    assert!(ws.join(".agents/memory/archive/old-incident.md").is_file());
    assert!(!ws.join(".agents/memory/old-incident.md").exists());

    // Promote high-confidence topic to Tier-1 constraint doc
    let prom = promote_memory(
        &opts,
        "ssh-hardening-rule",
        "20_security_and_credentials.md",
    )
    .unwrap();
    assert!(prom.promoted);
    assert!(prom.checkpoint_id.is_some());
    assert!(ws.join(&prom.archive_path).is_file());
    let c20 = fs::read_to_string(ws.join(".agents/constraints/20_security_and_credentials.md")).unwrap();
    assert!(c20.contains("SSH 密钥登录强制规则"));
    let ver = verify_workspace(&opts, None).unwrap();
    assert!(ver.passed, "verify errors after promote: {:?}", ver.errors);
}

#[test]
fn harness_adapters_switch_dirty_guard_harvest_and_override_bay() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-har-01", "6.12.0-amd64");

    let opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();
    init_workspace(&opts).unwrap();

    let reg = builtin_adapter_registry();
    assert_eq!(reg.adapters.len(), 14);

    // Add compat preset (claude-code + gemini-cli)
    let sw_add = switch_harnesses(&opts, SwitchMode::Add, "compat", false, false).unwrap();
    assert!(sw_add.changed);
    assert!(ws.join("CLAUDE.md").is_file());
    assert!(ws.join("GEMINI.md").is_file());

    // Modify CLAUDE.md manually -> removing without --harvest fails closed with rc=25
    let claude_path = ws.join("CLAUDE.md");
    let custom_shim_text = "# Custom Operator Rule\nAlways run nft -c -f before reload.\n";
    fs::write(&claude_path, custom_shim_text).unwrap();

    let err_dirty = switch_harnesses(&opts, SwitchMode::Remove, "claude-code", false, false).unwrap_err();
    assert_eq!(err_dirty.agent_exit_code(), Some(EXIT_MIGRATION_CONFLICT));
    assert!(claude_path.is_file());

    // With --harvest -> preserves verbatim in 90_project_legacy_ssot.md and removes shim
    let sw_harv = switch_harnesses(&opts, SwitchMode::Remove, "claude-code", true, false).unwrap();
    assert!(sw_harv.changed);
    assert!(sw_harv.harvested_projections.contains(&"CLAUDE.md".to_string()));
    assert!(!claude_path.exists());
    let ssot = fs::read_to_string(ws.join(LEGACY_SSOT_REL)).unwrap();
    assert!(ssot.contains("Always run nft -c -f before reload."));

    // Override bay init / list / remove
    let ov_init = manage_override(
        &opts,
        OverrideAction::Init {
            harness: "cursor".to_string(),
            json: false,
        },
    )
    .unwrap();
    assert!(ov_init.changed);
    assert!(ws.join(".agents/overrides/cursor.md").is_file());

    let ov_list = manage_override(&opts, OverrideAction::List).unwrap();
    assert_eq!(ov_list.overrides, vec![".agents/overrides/cursor.md"]);

    let ov_rm = manage_override(
        &opts,
        OverrideAction::Remove {
            harness: "cursor".to_string(),
        },
    )
    .unwrap();
    assert!(ov_rm.changed);
    assert!(ov_rm.checkpoint_id.is_some());
    assert!(!ws.join(".agents/overrides/cursor.md").exists());
}

#[test]
fn lossless_legacy_migration_idempotency_secret_gate_and_rollback_latest() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("rootfs");
    seed_fake_proc(&root, "vps-mig-01", "6.12.0-amd64");

    let opts = AgentWorkspaceOptions::new(&root, DEFAULT_WORKSPACE_DIR);
    let ws = opts.effective_workspace_dir();
    fs::create_dir_all(&ws).unwrap();

    // Seed a rich v1 / legacy workspace fixture
    let orig_root_mem = "# Old Root Memory\n- Port 2222 SSH baseline\n- Custom relay on 8443\n";
    fs::write(ws.join(LEGACY_ROOT_MEMORY_REL), orig_root_mem).unwrap();
    fs::create_dir_all(ws.join(".agent/memory")).unwrap();
    fs::write(
        ws.join(".agent/memory/old-notes.md"),
        "# Legacy Note\nKeep zram enabled at 50%.\n",
    )
    .unwrap();
    fs::write(
        ws.join(".cursorrules"),
        "Always check journalctl after systemd restart.\n",
    )
    .unwrap();
    fs::write(
        ws.join("CLAUDE.md"),
        "# Legacy Custom Claude Rules\nPrefer podman rootless containers.\n",
    )
    .unwrap();
    fs::create_dir_all(ws.join(".agent/skills/demo")).unwrap();
    fs::write(ws.join(".agent/skills/demo/SKILL.md"), "# Demo Skill\n").unwrap();
    fs::write(
        ws.join(".mcp.json"),
        r#"{"mcpServers":{"local-tools":{"command":"npx","args":["-y","demo"],"env":{"TOKEN":"${TOKEN}"}}}}"#,
    )
    .unwrap();

    // 1. Secret fail-closed gate (rc=26) when a plaintext secret is present in legacy file
    let bad_secret_path = ws.join(".agent/memory/secret-leak.md");
    fs::write(&bad_secret_path, format!("leaked {}", synth_aws_key())).unwrap();
    let audit_bad = audit_workspace(&opts, None).unwrap();
    assert!(!audit_bad.standard);
    assert!(audit_bad.secret_detected);
    let mig_err = migrate_workspace(&opts, None).unwrap_err();
    assert_eq!(mig_err.agent_exit_code(), Some(EXIT_SECRET_DETECTED));
    fs::remove_file(&bad_secret_path).unwrap();

    // 2. Clean audit detects legacy items without secrets
    let audit_pre = audit_workspace(&opts, None).unwrap();
    assert!(!audit_pre.standard);
    assert!(!audit_pre.secret_detected);
    assert_eq!(audit_pre.legacy_memory_files.len(), 2);
    assert_eq!(audit_pre.legacy_constraint_files.len(), 2);
    assert_eq!(audit_pre.legacy_skill_dirs.len(), 1);
    assert_eq!(audit_pre.legacy_mcp_files.len(), 1);

    // 3. First lossless migration
    let mig1 = migrate_workspace(&opts, None).unwrap();
    assert!(mig1.changed);
    assert!(mig1.checkpoint_id.is_some());
    assert_eq!(mig1.migrated_memories.len(), 2);
    assert_eq!(mig1.migrated_constraints.len(), 2);
    assert_eq!(mig1.migrated_skills.len(), 1);
    assert_eq!(mig1.migrated_mcp_servers, 1);
    assert!(mig1.cleaned_legacy_files.is_empty()); // never deleted unless cleanup_legacy=true

    // Verify migrated assets
    let ssot = fs::read_to_string(ws.join(LEGACY_SSOT_REL)).unwrap();
    assert!(ssot.contains("Always check journalctl after systemd restart."));
    assert!(ssot.contains("Prefer podman rootless containers."));
    assert!(ws.join(".agents/skills/demo/SKILL.md").is_file());
    assert!(ws.join(MCP_SERVERS_REL).is_file());
    assert!(ws.join(".agents/memory/legacy-root-memory.md").is_file());

    // 4. 2nd run MUST be 100% idempotent (0 duplicate entries in 90_project_legacy_ssot.md)
    let ssot_before = fs::read_to_string(ws.join(LEGACY_SSOT_REL)).unwrap();
    let mig2 = migrate_workspace(&opts, None).unwrap();
    assert!(mig2.migrated_memories.is_empty());
    assert!(mig2.migrated_constraints.is_empty());
    assert!(mig2.already_migrated_skipped >= 4);
    assert_eq!(fs::read_to_string(ws.join(LEGACY_SSOT_REL)).unwrap(), ssot_before);

    // 5. Cleanup legacy + rollback "latest" restores original legacy files byte-exact!
    let mut cleanup_opts = opts.clone();
    cleanup_opts.cleanup_legacy = true;
    let mig3 = migrate_workspace(&cleanup_opts, None).unwrap();
    assert!(!mig3.cleaned_legacy_files.is_empty());
    assert!(!ws.join(LEGACY_ROOT_MEMORY_REL).exists());
    assert!(!ws.join(".cursorrules").exists());

    let rb = rollback_checkpoint(&opts, "latest", "test-operator").unwrap();
    assert!(rb.restored_entries > 0);
    assert_eq!(
        fs::read_to_string(ws.join(LEGACY_ROOT_MEMORY_REL)).unwrap(),
        orig_root_mem
    );
    assert!(ws.join(".cursorrules").is_file());
}
