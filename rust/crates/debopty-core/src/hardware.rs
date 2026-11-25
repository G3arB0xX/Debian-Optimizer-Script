use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::ProjectConfig;
use crate::sys;
use crate::system::{MAX_SWAP_MB, prefixed};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HardwareTier {
    Micro,  // mem_total_mb <= 1024
    Small,  // 1025..=2048
    Medium, // 2049..=8192
    Large,  // > 8192
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiskTier {
    Small,    // disk_root_total_gb <= 15
    Standard, // 16..=40
    Medium,   // 41..=100
    Large,    // > 100
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostHardware {
    pub cpu_cores: u32,
    pub mem_total_mb: u64,
    pub disk_root_total_gb: u64,
    pub disk_root_avail_gb: u64,
    pub is_container: bool,
    pub has_zram_module: bool,
    pub probed_from_fallback: bool,
}

impl HostHardware {
    /// Deterministic fallback used when `root != "/"` and `<root>/proc/meminfo` does not exist,
    /// so synthetic `TempDir` tests never depend on host CI hardware.
    pub fn deterministic_fallback() -> Self {
        Self {
            cpu_cores: 2,
            mem_total_mb: 4096,
            disk_root_total_gb: 40,
            disk_root_avail_gb: 30,
            is_container: false,
            has_zram_module: true,
            probed_from_fallback: true,
        }
    }

    /// Probes `<root>/proc/meminfo`, `<root>/proc/cpuinfo`, `statvfs(<root>)`, container markers,
    /// and `<root>/sys/module/zram`.
    ///
    /// DETERMINISM CONTRACT: When `root != Path::new("/")` and `<root>/proc/meminfo` does NOT exist,
    /// returns deterministic Medium/Standard fallback (`cpu_cores=2, mem_total_mb=4096,
    /// disk_root_total_gb=40, disk_root_avail_gb=30, is_container=false, has_zram_module=true,
    /// probed_from_fallback=true`).
    pub fn probe(root: &Path) -> Self {
        let is_custom_root = !root.as_os_str().is_empty() && root != Path::new("/");
        let meminfo_path = prefixed(root, "/proc/meminfo");

        if is_custom_root && !meminfo_path.is_file() {
            return Self::deterministic_fallback();
        }

        let mut probed_from_fallback = false;
        let mem_total_mb = match sys::read_mem_total_mb(&meminfo_path) {
            Some(mb) if mb > 0 => mb,
            _ => {
                probed_from_fallback = true;
                4096
            }
        };

        let cpuinfo_path = prefixed(root, "/proc/cpuinfo");
        let cpu_cores = read_cpu_cores(&cpuinfo_path).unwrap_or_else(|| {
            if is_custom_root {
                2
            } else {
                std::thread::available_parallelism()
                    .map(|n| n.get().clamp(1, u32::MAX as usize) as u32)
                    .unwrap_or(2)
            }
        });

        let (disk_root_total_gb, disk_root_avail_gb) = if is_custom_root {
            // Synthetic custom-root (e.g. TempDir on /tmp tmpfs) must not inherit host /tmp disk pressure
            (40, 30)
        } else {
            probe_disk_gb(Path::new("/")).unwrap_or((40, 30))
        };

        let is_container = detect_container(root);
        let has_zram_module = prefixed(root, "/sys/module/zram").is_dir()
            || (!is_custom_root && Path::new("/dev/zram0").exists());

        Self {
            cpu_cores: cpu_cores.max(1),
            mem_total_mb,
            disk_root_total_gb,
            disk_root_avail_gb,
            is_container,
            has_zram_module,
            probed_from_fallback,
        }
    }

    pub fn tier(&self) -> HardwareTier {
        match self.mem_total_mb {
            0..=1024 => HardwareTier::Micro,
            1025..=2048 => HardwareTier::Small,
            2049..=8192 => HardwareTier::Medium,
            _ => HardwareTier::Large,
        }
    }

    pub fn disk_tier(&self) -> DiskTier {
        match self.disk_root_total_gb {
            0..=15 => DiskTier::Small,
            16..=40 => DiskTier::Standard,
            41..=100 => DiskTier::Medium,
            _ => DiskTier::Large,
        }
    }

    pub fn is_low_memory(&self) -> bool {
        self.mem_total_mb <= 1024
    }
}

fn read_cpu_cores(cpuinfo_path: &Path) -> Option<u32> {
    let text = fs::read_to_string(cpuinfo_path).ok()?;
    let count = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("processor")
                && trimmed["processor".len()..].trim_start().starts_with(':')
        })
        .count();
    if count > 0 {
        Some(count.min(u32::MAX as usize) as u32)
    } else {
        None
    }
}

fn detect_container(root: &Path) -> bool {
    if prefixed(root, "/.dockerenv").exists() || prefixed(root, "/run/.containerenv").exists() {
        return true;
    }
    let cgroup = prefixed(root, "/proc/1/cgroup");
    if let Ok(text) = fs::read_to_string(&cgroup) {
        let lower = text.to_ascii_lowercase();
        if lower.contains("docker")
            || lower.contains("lxc")
            || lower.contains("kubepods")
            || lower.contains("containerd")
        {
            return true;
        }
    }
    false
}

fn probe_disk_gb(path: &Path) -> Option<(u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let frag = stat.f_frsize as u128;
    let total_bytes = (stat.f_blocks as u128).saturating_mul(frag);
    let avail_bytes = (stat.f_bavail as u128).saturating_mul(frag);
    const GIB: u128 = 1024 * 1024 * 1024;
    let total_gb = (total_bytes / GIB) as u64;
    let avail_gb = (avail_bytes / GIB) as u64;
    Some((total_gb, avail_gb))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdaptiveTuningProfile {
    pub hardware: HostHardware,
    pub tier: HardwareTier,
    pub disk_tier: DiskTier,
    pub file_max: u64,
    pub limit_nofile: u64,
    pub tcp_fin_timeout: u32,
    pub tcp_max_tw_buckets: u32,
    pub net_rmem_max: u64,
    pub net_wmem_max: u64,
    pub tcp_rmem: String,
    pub tcp_wmem: String,
    pub somaxconn: u32,
    pub netdev_max_backlog: u32,
    pub tcp_max_syn_backlog: u32,
    pub tcp_fastopen: u32,
    pub default_qdisc: String,
    pub tcp_congestion_control: String,
    pub zram_algo: String,
    pub zram_percent: u32,
    pub zram_priority: u32,
    pub swapfile_size_mb: u64,
    pub journal_system_max_use: String,
    pub journal_runtime_max_use: String,
    pub journal_max_retention_sec: String,
    pub cache_retention_age: String,
    pub apt_clean_calendar: String,
    pub logrotate_rotate_count: u32,
    pub podman_log_size_max: u64,
    pub build_jobs: u32,
    pub overrides_applied: BTreeMap<String, String>,
}

fn is_safe_systemd_token(val: &str) -> bool {
    !val.is_empty()
        && val.len() <= 64
        && val
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn override_u64(
    cfg: &ProjectConfig,
    key: &str,
    min: u64,
    max: u64,
    overrides: &mut BTreeMap<String, String>,
) -> Option<u64> {
    let raw = cfg.extra.get(key)?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = raw.parse::<u64>().ok()?;
    if (min..=max).contains(&parsed) {
        overrides.insert(key.to_string(), parsed.to_string());
        Some(parsed)
    } else {
        None
    }
}

fn override_u32(
    cfg: &ProjectConfig,
    key: &str,
    min: u32,
    max: u32,
    overrides: &mut BTreeMap<String, String>,
) -> Option<u32> {
    let raw = cfg.extra.get(key)?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = raw.parse::<u32>().ok()?;
    if (min..=max).contains(&parsed) {
        overrides.insert(key.to_string(), parsed.to_string());
        Some(parsed)
    } else {
        None
    }
}

fn override_token(
    cfg: &ProjectConfig,
    key: &str,
    overrides: &mut BTreeMap<String, String>,
) -> Option<String> {
    let raw = cfg.extra.get(key)?.trim();
    if is_safe_systemd_token(raw) {
        overrides.insert(key.to_string(), raw.to_string());
        Some(raw.to_string())
    } else {
        None
    }
}

fn compute_auto_swapfile_mb(hw: &HostHardware, tier: HardwareTier) -> u64 {
    let base = match tier {
        HardwareTier::Micro => 1024,
        HardwareTier::Small => 2048,
        HardwareTier::Medium if hw.mem_total_mb <= 4096 => 2048,
        HardwareTier::Medium | HardwareTier::Large => 0,
    };
    if base == 0 || hw.disk_root_avail_gb < 3 {
        return 0;
    }
    let avail_mb = hw.disk_root_avail_gb.saturating_mul(1024);
    let max_by_disk = ((avail_mb / 5) / 256) * 256;
    base.min(max_by_disk).min(MAX_SWAP_MB)
}

impl AdaptiveTuningProfile {
    pub fn compute(hw: &HostHardware, cfg: &ProjectConfig) -> Self {
        let tier = hw.tier();
        let disk_tier = hw.disk_tier();
        let mut overrides = BTreeMap::new();

        let (
            base_file_max,
            base_nofile,
            base_tw_buckets,
            base_net_buf,
            base_backlog,
            base_zram_algo,
            base_zram_percent,
            base_journal_runtime,
            base_podman_log_max,
            base_build_jobs,
        ) = match tier {
            HardwareTier::Micro => (
                131_072u64,
                65_535u64,
                16_384u32,
                4_194_304u64,
                4_096u32,
                if hw.cpu_cores <= 1 && hw.mem_total_mb <= 512 {
                    "lz4"
                } else {
                    "zstd"
                },
                60u32,
                "10M",
                10_485_760u64,
                1u32,
            ),
            HardwareTier::Small => (
                262_144u64,
                131_072u64,
                32_768u32,
                8_388_608u64,
                8_192u32,
                "zstd",
                50u32,
                "24M",
                20_971_520u64,
                hw.cpu_cores.clamp(1, 2),
            ),
            HardwareTier::Medium => (
                524_288u64,
                524_288u64,
                50_000u32,
                16_777_216u64,
                16_384u32,
                "zstd",
                40u32,
                "64M",
                52_428_800u64,
                hw.cpu_cores.clamp(1, 4),
            ),
            HardwareTier::Large => (
                1_048_576u64,
                1_048_576u64,
                131_072u32,
                33_554_432u64,
                32_768u32,
                "zstd",
                25u32,
                "128M",
                104_857_600u64,
                hw.cpu_cores.max(1),
            ),
        };

        let (
            base_journal_system,
            base_journal_retention,
            base_cache_age,
            base_apt_calendar,
            base_logrotate_count,
        ) = match disk_tier {
            DiskTier::Small => ("100M", "14day", "14d", "daily", 5u32),
            DiskTier::Standard => ("200M", "30day", "30d", "weekly", 7u32),
            DiskTier::Medium => ("500M", "30day", "30d", "weekly", 7u32),
            DiskTier::Large => ("1G", "60day", "30d", "weekly", 14u32),
        };

        let file_max = override_u64(cfg, "TUNING_FILE_MAX", 16_384, 16_777_216, &mut overrides)
            .unwrap_or(base_file_max);
        let limit_nofile = override_u64(cfg, "TUNING_NOFILE", 4_096, 4_194_304, &mut overrides)
            .unwrap_or(base_nofile);
        let tcp_fin_timeout =
            override_u32(cfg, "TUNING_TCP_FIN_TIMEOUT", 5, 120, &mut overrides).unwrap_or(15);
        let tcp_max_tw_buckets = override_u32(
            cfg,
            "TUNING_TCP_MAX_TW_BUCKETS",
            1_024,
            2_097_152,
            &mut overrides,
        )
        .unwrap_or(base_tw_buckets);
        let net_buf = override_u64(
            cfg,
            "TUNING_NET_BUF_MAX",
            262_144,
            134_217_728,
            &mut overrides,
        )
        .unwrap_or(base_net_buf);
        let backlog = override_u32(cfg, "TUNING_BACKLOG", 512, 262_144, &mut overrides)
            .unwrap_or(base_backlog);
        let tcp_fastopen =
            override_u32(cfg, "TUNING_TCP_FASTOPEN", 0, 3, &mut overrides).unwrap_or(3);
        let default_qdisc = override_token(cfg, "TUNING_DEFAULT_QDISC", &mut overrides)
            .unwrap_or_else(|| "fq".to_string());
        let tcp_congestion_control = override_token(cfg, "TUNING_TCP_CC", &mut overrides)
            .unwrap_or_else(|| "bbr".to_string());

        let zram_algo = override_token(cfg, "ZRAM_ALGO", &mut overrides)
            .unwrap_or_else(|| base_zram_algo.to_string());
        let zram_percent =
            override_u32(cfg, "ZRAM_PERCENT", 1, 100, &mut overrides).unwrap_or(base_zram_percent);
        let zram_priority =
            override_u32(cfg, "ZRAM_PRIORITY", 1, 32_767, &mut overrides).unwrap_or(100);

        let swapfile_size_mb = override_u64(cfg, "SWAP_SIZE_MB", 0, MAX_SWAP_MB, &mut overrides)
            .unwrap_or_else(|| compute_auto_swapfile_mb(hw, tier));

        let journal_system_max_use = override_token(cfg, "JOURNAL_SYSTEM_MAX_USE", &mut overrides)
            .unwrap_or_else(|| base_journal_system.to_string());
        let journal_runtime_max_use =
            override_token(cfg, "JOURNAL_RUNTIME_MAX_USE", &mut overrides)
                .unwrap_or_else(|| base_journal_runtime.to_string());
        let journal_max_retention_sec =
            override_token(cfg, "JOURNAL_MAX_RETENTION_SEC", &mut overrides)
                .unwrap_or_else(|| base_journal_retention.to_string());
        let cache_retention_age = override_token(cfg, "CACHE_RETENTION_AGE", &mut overrides)
            .unwrap_or_else(|| base_cache_age.to_string());
        let apt_clean_calendar = override_token(cfg, "APT_CLEAN_CALENDAR", &mut overrides)
            .unwrap_or_else(|| base_apt_calendar.to_string());
        let logrotate_rotate_count =
            override_u32(cfg, "LOGROTATE_ROTATE_COUNT", 1, 90, &mut overrides)
                .unwrap_or(base_logrotate_count);
        let podman_log_size_max = override_u64(
            cfg,
            "PODMAN_LOG_SIZE_MAX",
            1_048_576,
            1_073_741_824,
            &mut overrides,
        )
        .unwrap_or(base_podman_log_max);
        let build_jobs =
            override_u32(cfg, "BUILD_JOBS", 1, 128, &mut overrides).unwrap_or(base_build_jobs);

        Self {
            hardware: hw.clone(),
            tier,
            disk_tier,
            file_max,
            limit_nofile,
            tcp_fin_timeout,
            tcp_max_tw_buckets,
            net_rmem_max: net_buf,
            net_wmem_max: net_buf,
            tcp_rmem: format!("4096 87380 {net_buf}"),
            tcp_wmem: format!("4096 65536 {net_buf}"),
            somaxconn: backlog,
            netdev_max_backlog: backlog,
            tcp_max_syn_backlog: backlog,
            tcp_fastopen,
            default_qdisc,
            tcp_congestion_control,
            zram_algo,
            zram_percent,
            zram_priority,
            swapfile_size_mb,
            journal_system_max_use,
            journal_runtime_max_use,
            journal_max_retention_sec,
            cache_retention_age,
            apt_clean_calendar,
            logrotate_rotate_count,
            podman_log_size_max,
            build_jobs,
            overrides_applied: overrides,
        }
    }

    pub fn probe_for_root(root: &Path, cfg: &ProjectConfig) -> Self {
        let hw = HostHardware::probe(root);
        Self::compute(&hw, cfg)
    }

    pub fn sysctl_template_vars(&self) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();
        vars.insert("FS_FILE_MAX".to_string(), self.file_max.to_string());
        vars.insert("DEFAULT_QDISC".to_string(), self.default_qdisc.clone());
        vars.insert(
            "TCP_CONGESTION_CONTROL".to_string(),
            self.tcp_congestion_control.clone(),
        );
        vars.insert(
            "TCP_FIN_TIMEOUT".to_string(),
            self.tcp_fin_timeout.to_string(),
        );
        vars.insert(
            "TCP_MAX_TW_BUCKETS".to_string(),
            self.tcp_max_tw_buckets.to_string(),
        );
        vars.insert("NET_RMEM_MAX".to_string(), self.net_rmem_max.to_string());
        vars.insert("NET_WMEM_MAX".to_string(), self.net_wmem_max.to_string());
        vars.insert("TCP_RMEM".to_string(), self.tcp_rmem.clone());
        vars.insert("TCP_WMEM".to_string(), self.tcp_wmem.clone());
        vars.insert("NET_SOMAXCONN".to_string(), self.somaxconn.to_string());
        vars.insert(
            "NET_DEV_MAX_BACKLOG".to_string(),
            self.netdev_max_backlog.to_string(),
        );
        vars.insert(
            "TCP_MAX_SYN_BACKLOG".to_string(),
            self.tcp_max_syn_backlog.to_string(),
        );
        vars.insert("TCP_FASTOPEN".to_string(), self.tcp_fastopen.to_string());
        vars
    }

    pub fn nofile_template_vars(&self) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();
        vars.insert("LIMIT_NOFILE".to_string(), self.limit_nofile.to_string());
        vars
    }

    pub fn zram_template_vars(&self) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();
        vars.insert("ZRAM_ALGO".to_string(), self.zram_algo.clone());
        vars.insert("ZRAM_PERCENT".to_string(), self.zram_percent.to_string());
        vars.insert("ZRAM_PRIORITY".to_string(), self.zram_priority.to_string());
        vars
    }

    pub fn retention_template_vars(&self) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();
        vars.insert(
            "JOURNAL_SYSTEM_MAX_USE".to_string(),
            self.journal_system_max_use.clone(),
        );
        vars.insert(
            "JOURNAL_RUNTIME_MAX_USE".to_string(),
            self.journal_runtime_max_use.clone(),
        );
        vars.insert(
            "JOURNAL_MAX_RETENTION_SEC".to_string(),
            self.journal_max_retention_sec.clone(),
        );
        vars.insert(
            "CACHE_RETENTION_AGE".to_string(),
            self.cache_retention_age.clone(),
        );
        vars.insert(
            "APT_CLEAN_CALENDAR".to_string(),
            self.apt_clean_calendar.clone(),
        );
        vars
    }

    pub fn podman_template_vars(&self) -> BTreeMap<String, String> {
        let mut vars = BTreeMap::new();
        vars.insert(
            "PODMAN_LOG_SIZE_MAX".to_string(),
            self.podman_log_size_max.to_string(),
        );
        vars
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn mk_hw(cores: u32, mem_mb: u64, disk_total: u64, disk_avail: u64) -> HostHardware {
        HostHardware {
            cpu_cores: cores,
            mem_total_mb: mem_mb,
            disk_root_total_gb: disk_total,
            disk_root_avail_gb: disk_avail,
            is_container: false,
            has_zram_module: true,
            probed_from_fallback: false,
        }
    }

    #[test]
    fn computes_all_hardware_and_disk_tiers_accurately() {
        let cfg = ProjectConfig::default();

        // Micro (512M, 1C) + Small disk (10G, 6G avail)
        let micro = AdaptiveTuningProfile::compute(&mk_hw(1, 512, 10, 6), &cfg);
        assert_eq!(micro.tier, HardwareTier::Micro);
        assert_eq!(micro.disk_tier, DiskTier::Small);
        assert!(micro.hardware.is_low_memory());
        assert_eq!(micro.file_max, 131_072);
        assert_eq!(micro.limit_nofile, 65_535);
        assert_eq!(micro.tcp_max_tw_buckets, 16_384);
        assert_eq!(micro.net_rmem_max, 4_194_304);
        assert_eq!(micro.somaxconn, 4_096);
        assert_eq!(micro.zram_algo, "lz4");
        assert_eq!(micro.zram_percent, 60);
        assert_eq!(micro.swapfile_size_mb, 1024);
        assert_eq!(micro.journal_system_max_use, "100M");
        assert_eq!(micro.journal_runtime_max_use, "10M");
        assert_eq!(micro.journal_max_retention_sec, "14day");
        assert_eq!(micro.cache_retention_age, "14d");
        assert_eq!(micro.apt_clean_calendar, "daily");
        assert_eq!(micro.logrotate_rotate_count, 5);
        assert_eq!(micro.podman_log_size_max, 10_485_760);
        assert_eq!(micro.build_jobs, 1);

        // Micro 1024M / 2C uses zstd
        let micro_1g = AdaptiveTuningProfile::compute(&mk_hw(2, 1024, 15, 10), &cfg);
        assert_eq!(micro_1g.zram_algo, "zstd");

        // Small (2048M, 4C) + Standard disk (30G, 20G avail)
        let small = AdaptiveTuningProfile::compute(&mk_hw(4, 2048, 30, 20), &cfg);
        assert_eq!(small.tier, HardwareTier::Small);
        assert_eq!(small.disk_tier, DiskTier::Standard);
        assert_eq!(small.file_max, 262_144);
        assert_eq!(small.limit_nofile, 131_072);
        assert_eq!(small.net_rmem_max, 8_388_608);
        assert_eq!(small.somaxconn, 8_192);
        assert_eq!(small.zram_percent, 50);
        assert_eq!(small.swapfile_size_mb, 2048);
        assert_eq!(small.journal_system_max_use, "200M");
        assert_eq!(small.journal_runtime_max_use, "24M");
        assert_eq!(small.journal_max_retention_sec, "30day");
        assert_eq!(small.apt_clean_calendar, "weekly");
        assert_eq!(small.logrotate_rotate_count, 7);
        assert_eq!(small.build_jobs, 2);

        // Medium (4096M vs 8192M) + Medium disk (80G)
        let med_4g = AdaptiveTuningProfile::compute(&mk_hw(6, 4096, 80, 50), &cfg);
        assert_eq!(med_4g.tier, HardwareTier::Medium);
        assert_eq!(med_4g.disk_tier, DiskTier::Medium);
        assert_eq!(med_4g.swapfile_size_mb, 2048);
        assert_eq!(med_4g.journal_system_max_use, "500M");
        assert_eq!(med_4g.build_jobs, 4);

        let med_8g = AdaptiveTuningProfile::compute(&mk_hw(6, 8192, 80, 50), &cfg);
        assert_eq!(med_8g.tier, HardwareTier::Medium);
        assert_eq!(med_8g.swapfile_size_mb, 0);

        // Large (16384M, 8C) + Large disk (200G)
        let large = AdaptiveTuningProfile::compute(&mk_hw(8, 16384, 200, 150), &cfg);
        assert_eq!(large.tier, HardwareTier::Large);
        assert_eq!(large.disk_tier, DiskTier::Large);
        assert_eq!(large.file_max, 1_048_576);
        assert_eq!(large.limit_nofile, 1_048_576);
        assert_eq!(large.tcp_max_tw_buckets, 131_072);
        assert_eq!(large.net_rmem_max, 33_554_432);
        assert_eq!(large.tcp_rmem, "4096 87380 33554432");
        assert_eq!(large.tcp_wmem, "4096 65536 33554432");
        assert_eq!(large.somaxconn, 32_768);
        assert_eq!(large.zram_percent, 25);
        assert_eq!(large.swapfile_size_mb, 0);
        assert_eq!(large.journal_system_max_use, "1G");
        assert_eq!(large.journal_runtime_max_use, "128M");
        assert_eq!(large.journal_max_retention_sec, "60day");
        assert_eq!(large.logrotate_rotate_count, 14);
        assert_eq!(large.podman_log_size_max, 104_857_600);
        assert_eq!(large.build_jobs, 8);
    }

    #[test]
    fn swapfile_clamps_on_tiny_disk_to_prevent_exhaustion() {
        let cfg = ProjectConfig::default();
        // Avail disk < 3 GiB -> swap = 0
        let tiny = AdaptiveTuningProfile::compute(&mk_hw(1, 512, 8, 2), &cfg);
        assert_eq!(tiny.swapfile_size_mb, 0);

        // Avail disk = 4 GiB -> 20% = 819M -> rounded down to 256M multiple = 768M
        let tight = AdaptiveTuningProfile::compute(&mk_hw(1, 1024, 10, 4), &cfg);
        assert_eq!(tight.swapfile_size_mb, 768);

        // Avail disk = 6 GiB -> 20% = 1228M -> rounded down = 1024M (caps 2048M Small tier to 1024M)
        let cap_small = AdaptiveTuningProfile::compute(&mk_hw(2, 2048, 12, 6), &cfg);
        assert_eq!(cap_small.swapfile_size_mb, 1024);
    }

    #[test]
    fn probes_proc_under_tempdir_and_uses_deterministic_fallback_when_absent() {
        let tmp_empty = TempDir::new().unwrap();
        let fb = HostHardware::probe(tmp_empty.path());
        assert!(fb.probed_from_fallback);
        assert_eq!(fb.cpu_cores, 2);
        assert_eq!(fb.mem_total_mb, 4096);
        assert_eq!(fb.disk_root_total_gb, 40);
        assert_eq!(fb.disk_root_avail_gb, 30);
        assert_eq!(fb.tier(), HardwareTier::Medium);
        assert_eq!(fb.disk_tier(), DiskTier::Standard);

        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("proc")).unwrap();
        fs::create_dir_all(tmp.path().join("sys/module/zram")).unwrap();
        fs::write(
            tmp.path().join("proc/meminfo"),
            "MemTotal:        524288 kB\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("proc/cpuinfo"),
            "processor\t: 0\nmodel name\t: MockCPU\n\nprocessor\t: 1\nmodel name\t: MockCPU\n",
        )
        .unwrap();
        fs::write(tmp.path().join(".dockerenv"), "").unwrap();

        let probed = HostHardware::probe(tmp.path());
        assert!(!probed.probed_from_fallback);
        assert_eq!(probed.mem_total_mb, 512);
        assert_eq!(probed.cpu_cores, 2);
        assert!(probed.is_container);
        assert!(probed.has_zram_module);
        assert_eq!(probed.tier(), HardwareTier::Micro);
    }

    #[test]
    fn debopti_conf_overrides_apply_and_reject_malformed_or_unsafe_values() {
        let mut cfg = ProjectConfig::default();
        cfg.extra
            .insert("TUNING_FILE_MAX".to_string(), "777777".to_string());
        cfg.extra
            .insert("TUNING_NOFILE".to_string(), "262144".to_string());
        cfg.extra.insert("ZRAM_ALGO".to_string(), "lz4".to_string());
        cfg.extra
            .insert("ZRAM_PERCENT".to_string(), "35".to_string());
        cfg.extra
            .insert("JOURNAL_SYSTEM_MAX_USE".to_string(), "300M".to_string());
        cfg.extra
            .insert("SWAP_SIZE_MB".to_string(), "512".to_string());
        cfg.extra.insert("BUILD_JOBS".to_string(), "3".to_string());

        // Malformed / unsafe overrides must be ignored
        cfg.extra
            .insert("TUNING_BACKLOG".to_string(), "not_a_number".to_string());
        cfg.extra
            .insert("ZRAM_PRIORITY".to_string(), "999999".to_string());
        cfg.extra.insert(
            "JOURNAL_MAX_RETENTION_SEC".to_string(),
            "30day\nExecStart=/bin/sh".to_string(),
        );
        cfg.extra.insert(
            "APT_CLEAN_CALENDAR".to_string(),
            "daily; rm -rf /".to_string(),
        );

        let profile = AdaptiveTuningProfile::compute(&mk_hw(2, 4096, 40, 30), &cfg);
        assert_eq!(profile.file_max, 777_777);
        assert_eq!(profile.limit_nofile, 262_144);
        assert_eq!(profile.zram_algo, "lz4");
        assert_eq!(profile.zram_percent, 35);
        assert_eq!(profile.journal_system_max_use, "300M");
        assert_eq!(profile.swapfile_size_mb, 512);
        assert_eq!(profile.build_jobs, 3);

        // Rejected overrides stayed at tier defaults
        assert_eq!(profile.somaxconn, 16_384);
        assert_eq!(profile.zram_priority, 100);
        assert_eq!(profile.journal_max_retention_sec, "30day");
        assert_eq!(profile.apt_clean_calendar, "weekly");
        assert!(!profile.overrides_applied.contains_key("TUNING_BACKLOG"));
        assert!(!profile.overrides_applied.contains_key("APT_CLEAN_CALENDAR"));
        assert_eq!(
            profile
                .overrides_applied
                .get("TUNING_FILE_MAX")
                .map(String::as_str),
            Some("777777")
        );
    }
}
