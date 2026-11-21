pub mod actions;
pub mod error;
pub mod registry;
pub mod runner;

pub use actions::{AppId, BashAction, BashDispatch, BashFunctionSpec, EngineMode, ModuleImpl};
pub use error::{BridgeError, Result};
pub use registry::{ModuleCategory, ModuleRegistry, ModuleStatusEntry, module_registry};
pub use runner::{BashBridge, DEFAULT_INSTALL_DIR};

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::str::FromStr;

    use tempfile::TempDir;

    use super::*;

    fn create_mock_script_tree() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("scripts/apps")).expect("mkdir scripts/apps");

        fs::write(
            root.join("scripts/common.sh"),
            r#"#!/bin/bash
ensure_runtime_home() { :; }
load_project_config() { :; }
script_update() {
    printf 'UPDATE|%s|%s|%s\n' "${DEBOPTY_FROM_RUST:-}" "${IN_TUI:-}" "${CONFIG_FILE:-}" > "${MOCK_OUT}"
}
"#,
        )
        .expect("write common.sh");

        fs::write(
            root.join("scripts/system.sh"),
            r#"#!/bin/bash
run_base_optimization() {
    printf 'BASE|%s|%s|%s\n' "${DEBOPTY_FROM_RUST:-}" "${IN_TUI:-}" "${SCRIPT_DIR:-}" > "${MOCK_OUT}"
    printf 'BASE_OPTIMIZED="true"\nPODMAN_GC_ENABLED="true"\n' > "${CONFIG_FILE}"
}
toggle_ip_forwarding() {
    printf 'IPFWD|%s\n' "${DEBOPTY_FROM_RUST:-}" > "${MOCK_OUT}"
}
setup_tmux() {
    printf 'TMUX_SETUP|%s\n' "${DEBOPTY_FROM_RUST:-}" > "${MOCK_OUT}"
}
"#,
        )
        .expect("write system.sh");

        fs::write(
            root.join("scripts/apps/podman.sh"),
            r#"#!/bin/bash
install_podman() {
    printf 'PODMAN|%s\n' "${DEBOPTY_FROM_RUST:-}" > "${MOCK_OUT}"
    return 42
}
"#,
        )
        .expect("write podman.sh");

        fs::write(
            root.join("deb_optimizer.sh"),
            r#"#!/bin/bash
printf 'LEGACY|%s|%s\n' "${DEBOPTY_FROM_RUST:-}" "${SCRIPT_DIR:-}" > "${MOCK_OUT}"
"#,
        )
        .expect("write deb_optimizer.sh");

        (tmp, root)
    }

    #[test]
    fn registry_covers_native_and_bridge_modules_and_serializes_json() {
        let reg = ModuleRegistry::new();
        assert_eq!(reg.native_count(), 10);
        assert_eq!(reg.bridge_count(), 11);
        assert_eq!(reg.entries().len(), 21);

        for native_id in [
            "version",
            "config",
            "template",
            "status",
            "f2b-sync",
            "tui",
            "network",
            "system",
            "security-ssh-nft",
            "reconcile",
        ] {
            let entry = reg.get(native_id).expect("native module present");
            assert_eq!(entry.implementation, ModuleImpl::NativeRust);
        }
        for bridge_id in [
            "devops",
            "easytier",
            "ferron",
            "freship",
            "golang",
            "podman",
            "realm",
            "rust",
            "tailscale",
            "warp",
            "xray",
        ] {
            let entry = reg.get(bridge_id).expect("bridge module present");
            assert_eq!(entry.implementation, ModuleImpl::BashBridge);
        }

        let json = serde_json::to_string(&module_registry()).expect("serialize registry");
        let parsed: Vec<ModuleStatusEntry> =
            serde_json::from_str(&json).expect("deserialize registry");
        assert_eq!(parsed.len(), 21);
        assert!(json.contains("\"native-rust\""));
        assert!(json.contains("\"bash-bridge\""));
    }

    #[test]
    fn action_cli_roundtrip_and_dispatch_table_are_complete() {
        for action in BashAction::all_actions() {
            let name = action.cli_name();
            let parsed = BashAction::from_str(&name).expect("roundtrip action");
            assert_eq!(parsed, action);
        }

        // Tmux uses `setup_tmux` in `scripts/system.sh`, matching `scripts/tui.sh`.
        assert_eq!(
            BashAction::InstallApp(AppId::Tmux).dispatch(),
            BashDispatch::Function(BashFunctionSpec::new("scripts/system.sh", "setup_tmux"))
        );
        assert_eq!(
            BashAction::UninstallApp(AppId::Tmux).dispatch(),
            BashDispatch::Function(BashFunctionSpec::new("scripts/system.sh", "uninstall_tmux"))
        );
        assert_eq!(
            EngineMode::from_str("RUST").expect("parse engine"),
            EngineMode::Rust
        );
        assert!(EngineMode::from_str("invalid").is_err());
        assert!(BashAction::from_str("nonexistent-op").is_err());
    }

    #[test]
    fn bridge_executes_mock_bash_module_propagates_guard_and_reloads_config() {
        let (tmp, root) = create_mock_script_tree();
        let out_file = tmp.path().join("mock.out");
        let cfg_file = tmp.path().join("debopti.conf");
        let out_str = out_file.to_string_lossy().to_string();

        let bridge = BashBridge::new(&root, &root, &cfg_file);
        // Even if caller passes DEBOPTY_FROM_RUST=0, bridge MUST force DEBOPTY_FROM_RUST=1.
        let (code, reloaded) = bridge
            .execute_and_reload_config(
                &BashAction::RunBaseOptimization,
                &[("MOCK_OUT", &out_str), ("DEBOPTY_FROM_RUST", "0")],
            )
            .expect("execute_and_reload_config");

        assert_eq!(code, 0);
        assert!(reloaded.base_optimized);
        assert!(reloaded.podman_gc_enabled);

        let captured = fs::read_to_string(&out_file).expect("read mock.out");
        assert_eq!(captured.trim(), format!("BASE|1|true|{}", root.display()));
    }

    #[test]
    fn bridge_preserves_exit_code_and_handles_legacy_entrypoint() {
        let (tmp, root) = create_mock_script_tree();
        let out_file = tmp.path().join("mock.out");
        let cfg_file = tmp.path().join("debopti.conf");
        let out_str = out_file.to_string_lossy().to_string();

        let bridge = BashBridge::new(&root, &root, &cfg_file);
        let code = bridge
            .execute(
                &BashAction::InstallApp(AppId::Podman),
                &[("MOCK_OUT", &out_str)],
            )
            .expect("execute podman");
        assert_eq!(code, 42);
        assert_eq!(
            fs::read_to_string(&out_file).expect("read").trim(),
            "PODMAN|1"
        );

        let legacy_code = bridge
            .execute(&BashAction::LegacyFullEntry, &[("MOCK_OUT", &out_str)])
            .expect("execute legacy entry");
        assert_eq!(legacy_code, 0);
        assert_eq!(
            fs::read_to_string(&out_file).expect("read").trim(),
            format!("LEGACY|1|{}", root.display())
        );
    }

    #[test]
    fn bridge_handles_spaces_and_special_chars_in_script_dir_without_injection() {
        let tmp = TempDir::new().expect("tempdir");
        let weird_dir = tmp.path().join("dir with spaces $(touch pwned)");
        fs::create_dir_all(weird_dir.join("scripts")).expect("mkdir");
        fs::write(
            weird_dir.join("scripts/common.sh"),
            r#"#!/bin/bash
script_update() {
    printf 'SAFE|%s\n' "${DEBOPTY_FROM_RUST}" > "${MOCK_OUT}"
}
"#,
        )
        .expect("write common.sh");

        let out_file = tmp.path().join("safe.out");
        let out_str = out_file.to_string_lossy().to_string();
        let bridge = BashBridge::new(&weird_dir, &weird_dir, tmp.path().join("debopti.conf"));
        let code = bridge
            .execute(&BashAction::ScriptUpdate, &[("MOCK_OUT", &out_str)])
            .expect("execute safely");

        assert_eq!(code, 0);
        assert!(!tmp.path().join("pwned").exists());
        assert_eq!(
            fs::read_to_string(&out_file).expect("read").trim(),
            "SAFE|1"
        );
    }

    #[test]
    fn bridge_discovery_and_missing_script_errors() {
        let tmp = TempDir::new().expect("tempdir");
        let empty = tmp.path().join("empty");
        fs::create_dir_all(&empty).expect("mkdir");

        let err = BashBridge::discover(Some(&empty), None).expect_err("must fail on empty dir");
        assert!(matches!(err, BridgeError::ScriptDirNotFound(_)));

        let err_cand = BashBridge::discover_from_candidates(
            std::slice::from_ref(&empty),
            PathBuf::from(DEFAULT_INSTALL_DIR),
            PathBuf::from("/etc/debopti/debopti.conf"),
        )
        .expect_err("must fail on invalid candidates");
        assert!(matches!(err_cand, BridgeError::ScriptDirNotFound(_)));

        // Valid common.sh but missing scripts/apps/ferron.sh
        fs::create_dir_all(empty.join("scripts")).expect("mkdir");
        fs::write(empty.join("scripts/common.sh"), "#!/bin/bash\n").expect("write");
        let bridge = BashBridge::discover(Some(&empty), None).expect("discover valid dir");
        let missing = bridge
            .build_command(&BashAction::InstallApp(AppId::Ferron), &[])
            .expect_err("missing target module");
        assert!(matches!(missing, BridgeError::MissingScript { .. }));
    }

    #[test]
    fn every_action_target_exists_in_repo_scripts() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let bridge = BashBridge::discover(Some(&repo_root), None).expect("discover repo root");

        for action in BashAction::all_actions() {
            bridge
                .build_command(&action, &[])
                .unwrap_or_else(|e| panic!("build_command failed for {action}: {e}"));

            if let BashDispatch::Function(spec) = action.dispatch() {
                let script_body = fs::read_to_string(repo_root.join(spec.module_rel_path))
                    .expect("read repo module script");
                let fn_decl = format!("{}()", spec.function_name);
                assert!(
                    script_body.contains(&fn_decl),
                    "function `{fn_decl}` missing from {}",
                    spec.module_rel_path
                );
            }
        }
    }

    #[test]
    fn bridge_entrypoint_inserts_double_dash_for_leading_dash_dir_and_pins_core_env() {
        let tmp = TempDir::new().expect("tempdir");
        let dash_dir = tmp.path().join("-dash-root");
        fs::create_dir_all(&dash_dir).expect("mkdir dash dir");
        fs::write(
            dash_dir.join("deb_optimizer.sh"),
            r#"#!/bin/bash
printf '%s|%s|%s|%s|%s|%s|%s\n' \
  "${SCRIPT_DIR:-}" "${INSTALL_DIR:-}" "${CONFIG_FILE:-}" \
  "${IN_TUI:-}" "${DEBOPTY_FROM_RUST:-}" "${DEBOPTY_ENGINE:-}" "${EXTRA_OK:-}" \
  > "${MOCK_OUT}"
"#,
        )
        .expect("write entrypoint");

        let install_dir = tmp.path().join("install");
        let cfg_file = tmp.path().join("debopti.conf");
        let out_file = tmp.path().join("env.out");
        let out_str = out_file.to_string_lossy().to_string();

        let bridge = BashBridge::new(&dash_dir, &install_dir, &cfg_file);
        let malicious_env = [
            ("MOCK_OUT", out_str.as_str()),
            ("EXTRA_OK", "allowed"),
            ("SCRIPT_DIR", "/tmp/hijacked_script"),
            ("INSTALL_DIR", "/tmp/hijacked_install"),
            ("CONFIG_FILE", "/tmp/hijacked.conf"),
            ("IN_TUI", "false"),
            ("DEBOPTY_FROM_RUST", "0"),
            ("DEBOPTY_ENGINE", "rust"),
        ];

        let cmd = bridge
            .build_command(&BashAction::LegacyFullEntry, &malicious_env)
            .expect("build_command");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "--");
        assert_eq!(args[1], dash_dir.join("deb_optimizer.sh").as_os_str());

        let code = bridge
            .execute(&BashAction::LegacyFullEntry, &malicious_env)
            .expect("execute entrypoint");
        assert_eq!(code, 0);
        let captured = fs::read_to_string(&out_file).expect("read env.out");
        assert_eq!(
            captured.trim(),
            format!(
                "{}|{}|{}|true|1|bash|allowed",
                dash_dir.display(),
                install_dir.display(),
                cfg_file.display()
            )
        );
    }
}
