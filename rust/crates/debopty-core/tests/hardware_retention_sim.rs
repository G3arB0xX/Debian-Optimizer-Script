use std::fs;
use std::path::Path;

use tempfile::TempDir;

use debopty_core::apps::self_manage::{
    SelfInstallOptions, SelfUninstallOptions, self_install, self_uninstall,
};
use debopty_core::apps::shared::AppContext;
use debopty_core::config::ConfigStore;
use debopty_core::hardware::{AdaptiveTuningProfile, DiskTier, HardwareTier, HostHardware};
use debopty_core::reconcile::{
    ReconcileContext, ReconcileDomain, ReconcileMode, audit_system, reconcile_system,
};
use debopty_core::sys::{CmdOutput, MockCommandRunner};
use debopty_core::system::{
    MemoryOptions, RetentionOptions, SystemContext, setup_limits, setup_logrotate, setup_memory,
    setup_retention, setup_sysctl,
};

fn ok_out(stdout: &str) -> CmdOutput {
    CmdOutput {
        code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn seed_mock_host(root: &Path, mem_kb: u64, cpu_cores: u32) {
    fs::create_dir_all(root.join("proc")).unwrap();
    fs::create_dir_all(root.join("etc/systemd")).unwrap();
    fs::create_dir_all(root.join("etc/debopti")).unwrap();
    fs::create_dir_all(root.join("sys/module/zram")).unwrap();
    fs::write(
        root.join("proc/meminfo"),
        format!("MemTotal: {mem_kb} kB\n"),
    )
    .unwrap();
    let mut cpuinfo = String::new();
    for i in 0..cpu_cores {
        cpuinfo.push_str(&format!("processor\t: {i}\nmodel name\t: VirtualCPU\n\n"));
    }
    fs::write(root.join("proc/cpuinfo"), cpuinfo).unwrap();
    fs::write(root.join("proc/swaps"), "Filename\tType\tSize\n").unwrap();
    fs::write(
        root.join("etc/logrotate.conf"),
        "weekly\nrotate 4\n#compress\n",
    )
    .unwrap();
}

#[test]
fn micro_vs_large_profile_apply_then_reconcile_has_zero_drift() {
    // 1. Micro host: 512 MiB RAM, 1 vCPU
    let tmp_micro = TempDir::new().unwrap();
    let root_micro = tmp_micro.path();
    seed_mock_host(root_micro, 524_288, 1);
    let store_micro = ConfigStore::new(root_micro.join("etc/debopti/debopti.conf"));
    store_micro.set("BASE_OPTIMIZED", "true").unwrap();

    let mut sys_micro = SystemContext::with_root(root_micro);
    sys_micro.no_sys = true;
    let runner = MockCommandRunner::new();

    setup_sysctl(&sys_micro, &runner).unwrap();
    setup_limits(&sys_micro).unwrap();
    setup_memory(&sys_micro, &MemoryOptions::default(), &runner).unwrap();
    setup_logrotate(&sys_micro).unwrap();
    setup_retention(
        &sys_micro,
        &RetentionOptions {
            user_homes: vec![root_micro.join("root")],
            enable_user_tmpfiles: true,
            enable_apt_clean: true,
        },
        &runner,
    )
    .unwrap();

    let sysctl_micro =
        fs::read_to_string(root_micro.join("etc/sysctl.d/99-debopti-optimize.conf")).unwrap();
    assert!(sysctl_micro.contains("fs.file-max = 131072"));
    assert!(sysctl_micro.contains("net.core.rmem_max = 4194304"));
    assert!(sysctl_micro.contains("net.core.somaxconn = 4096"));
    let nofile_micro =
        fs::read_to_string(root_micro.join("etc/security/limits.d/99-debopti-nofile.conf"))
            .unwrap();
    assert!(nofile_micro.contains("65535"));
    let zram_micro = fs::read_to_string(root_micro.join("etc/default/zramswap")).unwrap();
    assert!(zram_micro.contains("ALGO=lz4"));
    assert!(zram_micro.contains("PERCENT=60"));

    // Reconcile audit across system domains MUST see 0 drift!
    let mut rec_micro = ReconcileContext::with_root(root_micro);
    rec_micro.no_sys = true;
    rec_micro.domains = vec![
        ReconcileDomain::Sysctl,
        ReconcileDomain::Limits,
        ReconcileDomain::Swap,
        ReconcileDomain::Logrotate,
        ReconcileDomain::Apt,
    ];
    let audit_micro = audit_system(&rec_micro, &runner).unwrap();
    assert_eq!(
        audit_micro.drift_count, 0,
        "micro reconcile must have 0 drift after apply: {:?}",
        audit_micro.findings
    );

    // 2. Large host: 16 GiB RAM, 8 vCPU
    let tmp_large = TempDir::new().unwrap();
    let root_large = tmp_large.path();
    seed_mock_host(root_large, 16 * 1024 * 1024, 8);
    let store_large = ConfigStore::new(root_large.join("etc/debopti/debopti.conf"));
    store_large.set("BASE_OPTIMIZED", "true").unwrap();

    let mut sys_large = SystemContext::with_root(root_large);
    sys_large.no_sys = true;
    setup_sysctl(&sys_large, &runner).unwrap();
    setup_limits(&sys_large).unwrap();
    setup_memory(&sys_large, &MemoryOptions::default(), &runner).unwrap();
    setup_logrotate(&sys_large).unwrap();
    setup_retention(
        &sys_large,
        &RetentionOptions {
            user_homes: vec![root_large.join("root")],
            enable_user_tmpfiles: true,
            enable_apt_clean: true,
        },
        &runner,
    )
    .unwrap();

    let sysctl_large =
        fs::read_to_string(root_large.join("etc/sysctl.d/99-debopti-optimize.conf")).unwrap();
    assert!(sysctl_large.contains("fs.file-max = 1048576"));
    assert!(sysctl_large.contains("net.core.rmem_max = 33554432"));
    assert!(sysctl_large.contains("net.core.somaxconn = 32768"));
    let nofile_large =
        fs::read_to_string(root_large.join("etc/security/limits.d/99-debopti-nofile.conf"))
            .unwrap();
    assert!(nofile_large.contains("1048576"));
    let zram_large = fs::read_to_string(root_large.join("etc/default/zramswap")).unwrap();
    assert!(zram_large.contains("ALGO=zstd"));
    assert!(zram_large.contains("PERCENT=25"));

    let mut rec_large = ReconcileContext::with_root(root_large);
    rec_large.no_sys = true;
    rec_large.domains = vec![
        ReconcileDomain::Sysctl,
        ReconcileDomain::Limits,
        ReconcileDomain::Swap,
        ReconcileDomain::Logrotate,
        ReconcileDomain::Apt,
    ];
    let audit_large = audit_system(&rec_large, &runner).unwrap();
    assert_eq!(
        audit_large.drift_count, 0,
        "large reconcile must have 0 drift after apply: {:?}",
        audit_large.findings
    );

    // Mutate retention drop-in and verify reconcile Apply restores it idempotently
    fs::write(
        root_large.join("etc/systemd/journald.conf.d/99-debopty-retention.conf"),
        "[Journal]\nSystemMaxUse=999G\n",
    )
    .unwrap();
    let apply_fix = reconcile_system(&rec_large, ReconcileMode::Apply, &runner).unwrap();
    assert!(
        apply_fix
            .findings
            .iter()
            .any(|f| f.code == "RETENTION_JOURNALD_SYNC")
    );
    let audit_after = audit_system(&rec_large, &runner).unwrap();
    assert_eq!(audit_after.drift_count, 0);
}

#[test]
fn self_install_deploys_retention_and_self_uninstall_cleans_apt_timer() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("root")).unwrap();
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::write(root.join("etc/passwd"), "root:x:0:0:root:/root:/bin/bash\n").unwrap();

    let fake_bin = tmp.path().join("debopty-src");
    fs::write(&fake_bin, b"#!/bin/sh\necho ok\n").unwrap();

    let mut ctx = AppContext::with_root(root);
    ctx.no_sys = false;
    let runner = MockCommandRunner::new();
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("which", ok_out(""));

    let opts = SelfInstallOptions {
        source_bin: Some(fake_bin),
        skip_agent_workspace: true,
        skip_default_tools: true,
        skip_retention: false,
        ..SelfInstallOptions::default()
    };
    let report = self_install(&ctx, &opts, &runner).unwrap();
    assert_eq!(
        report.details.get("retention").map(String::as_str),
        Some("configured")
    );
    assert!(
        root.join("etc/systemd/journald.conf.d/99-debopty-retention.conf")
            .is_file()
    );
    assert!(
        root.join("etc/apt/apt.conf.d/99-debopty-apt-clean")
            .is_file()
    );
    assert!(
        root.join("etc/systemd/system/debopty-apt-clean.service")
            .is_file()
    );
    assert!(
        root.join("etc/systemd/system/debopty-apt-clean.timer")
            .is_file()
    );
    assert!(
        root.join("root/.config/user-tmpfiles.d/debopty-cache-retention.conf")
            .is_file()
    );
    assert!(!root.join("root/.cache").exists());

    // self-uninstall cleans debopty-apt-clean units & apt conf while keeping journald drop-in when keep_config=true
    let uninst = self_uninstall(
        &ctx,
        &SelfUninstallOptions {
            keep_config: true,
            install_dir: None,
        },
        &runner,
    )
    .unwrap();
    assert!(uninst.changed);
    assert!(
        !root
            .join("etc/apt/apt.conf.d/99-debopty-apt-clean")
            .exists()
    );
    assert!(
        !root
            .join("etc/systemd/system/debopty-apt-clean.service")
            .exists()
    );
    assert!(
        !root
            .join("etc/systemd/system/debopty-apt-clean.timer")
            .exists()
    );
    assert!(
        root.join("etc/systemd/journald.conf.d/99-debopty-retention.conf")
            .is_file()
    );
}

#[test]
fn hardware_tier_disk_tier_matrix_covers_micro_10g_vs_large_200g() {
    let cfg = debopty_core::config::ProjectConfig::default();
    let micro_hw = HostHardware {
        cpu_cores: 1,
        mem_total_mb: 512,
        disk_root_total_gb: 10,
        disk_root_avail_gb: 6,
        is_container: false,
        has_zram_module: true,
        probed_from_fallback: false,
    };
    let large_hw = HostHardware {
        cpu_cores: 8,
        mem_total_mb: 16384,
        disk_root_total_gb: 200,
        disk_root_avail_gb: 150,
        is_container: false,
        has_zram_module: true,
        probed_from_fallback: false,
    };

    let p_micro = AdaptiveTuningProfile::compute(&micro_hw, &cfg);
    let p_large = AdaptiveTuningProfile::compute(&large_hw, &cfg);

    assert_eq!(p_micro.tier, HardwareTier::Micro);
    assert_eq!(p_micro.disk_tier, DiskTier::Small);
    assert_eq!(p_micro.journal_system_max_use, "100M");
    assert_eq!(p_micro.apt_clean_calendar, "daily");
    assert_eq!(p_micro.build_jobs, 1);

    assert_eq!(p_large.tier, HardwareTier::Large);
    assert_eq!(p_large.disk_tier, DiskTier::Large);
    assert_eq!(p_large.journal_system_max_use, "1G");
    assert_eq!(p_large.apt_clean_calendar, "weekly");
    assert_eq!(p_large.swapfile_size_mb, 0);
    assert_eq!(p_large.build_jobs, 8);
}
