use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use walkdir::WalkDir;

use crate::config::ConfigStore;
use crate::diag::{self, LogLevel};
use crate::error::Result;
use crate::reconcile::backup::BackupSession;
use crate::reconcile::{
    FindingSeverity, FindingStatus, LEGACY_MANAGED_MARKER, MANAGED_FILE_MARKER, ReconcileContext,
    ReconcileDomain, ReconcileFinding, ReconcileMode,
};
use crate::sys::{self, CommandRunner};

const MANAGED_SYSCTL_KEYS: &[&str] = &[
    "net.core.default_qdisc",
    "net.ipv4.tcp_congestion_control",
    "fs.file-max",
];

fn is_container_env(ctx: &ReconcileContext) -> bool {
    ctx.path("/.dockerenv").exists() || ctx.path("/run/.containerenv").exists()
}

fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

// =========================================================
// Domain 3: DNS
// =========================================================

fn count_valid_nameservers(content: &str) -> usize {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("nameserver") {
                rest.chars().next().is_some_and(char::is_whitespace)
                    && !rest.trim().is_empty()
                    && !rest.trim().starts_with('#')
            } else {
                false
            }
        })
        .count()
}

fn render_managed_resolv(existing: &str, is_cn_region: bool) -> String {
    let mut lines = vec![MANAGED_FILE_MARKER.to_string()];
    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("domain ")
            || trimmed.starts_with("search ")
            || trimmed.starts_with("options ")
        {
            lines.push(trimmed.to_string());
        }
    }
    if is_cn_region {
        lines.push("nameserver 223.5.5.5".to_string());
        lines.push("nameserver 119.29.29.29".to_string());
    } else {
        lines.push("nameserver 1.1.1.1".to_string());
        lines.push("nameserver 8.8.8.8".to_string());
    }
    lines.join("\n") + "\n"
}

pub(crate) fn reconcile_dns(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::dns",
        None,
        "reconciling DNS domain",
        None,
        &[],
    );

    let resolv_abs = "/etc/resolv.conf";
    let resolv_path = ctx.path(resolv_abs);
    let Ok(sym_meta) = fs::symlink_metadata(&resolv_path) else {
        return Ok(());
    };

    if sym_meta.file_type().is_symlink() {
        if !resolv_path.exists() {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(resolv_abs)?;
                fs::remove_file(&resolv_path)?;
                let rendered = render_managed_resolv("", ctx.is_cn_region);
                sys::write_file_atomic(&resolv_path, rendered.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Dns,
                code: "DNS_BROKEN_RESOLV".to_string(),
                severity: FindingSeverity::Critical,
                status,
                target: resolv_abs.to_string(),
                summary: "修复悬空 /etc/resolv.conf 符号链接并写入可用 DNS 解析器".to_string(),
                backup_rel_path: bak,
            });
            return Ok(());
        }

        let content = fs::read_to_string(&resolv_path).unwrap_or_default();
        if count_valid_nameservers(&content) == 0 {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(resolv_abs)?;
                fs::remove_file(&resolv_path)?;
                let rendered = render_managed_resolv(&content, ctx.is_cn_region);
                sys::write_file_atomic(&resolv_path, rendered.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Dns,
                code: "DNS_BROKEN_RESOLV".to_string(),
                severity: FindingSeverity::Critical,
                status,
                target: resolv_abs.to_string(),
                summary: "符号链接指向的 resolv.conf 缺少有效 nameserver，已重建可用配置"
                    .to_string(),
                backup_rel_path: bak,
            });
        } else {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Dns,
                code: "DNS_SYMLINK_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: resolv_abs.to_string(),
                summary: "保留健康的 resolv.conf 符号链接（如 systemd-resolved）".to_string(),
                backup_rel_path: None,
            });
        }
        return Ok(());
    }

    // Regular file
    let content = fs::read_to_string(&resolv_path).unwrap_or_default();
    if count_valid_nameservers(&content) == 0 {
        let (status, bak) = if mode == ReconcileMode::Apply {
            let b = backup.snapshot_path(resolv_abs)?;
            let rendered = render_managed_resolv(&content, ctx.is_cn_region);
            sys::write_file_atomic(&resolv_path, rendered.as_bytes(), Some(0o644))?;
            (FindingStatus::Fixed, b)
        } else {
            (FindingStatus::WouldFix, None)
        };
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Dns,
            code: "DNS_BROKEN_RESOLV".to_string(),
            severity: FindingSeverity::Critical,
            status,
            target: resolv_abs.to_string(),
            summary:
                "/etc/resolv.conf 缺少有效 nameserver，已写入托管 DNS 并保留 domain/search/options"
                    .to_string(),
            backup_rel_path: bak,
        });
        return Ok(());
    }

    let file_mode = sym_meta.permissions().mode() & 0o777;
    if (file_mode & 0o004) == 0 {
        let (status, bak) = if mode == ReconcileMode::Apply {
            let b = backup.snapshot_path(resolv_abs)?;
            fs::set_permissions(&resolv_path, fs::Permissions::from_mode(0o644))?;
            (FindingStatus::Fixed, b)
        } else {
            (FindingStatus::WouldFix, None)
        };
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Dns,
            code: "DNS_RESOLV_PERMS".to_string(),
            severity: FindingSeverity::Warning,
            status,
            target: resolv_abs.to_string(),
            summary: format!("/etc/resolv.conf 权限过严 ({file_mode:04o})，修正为 0644"),
            backup_rel_path: bak,
        });
    }

    if !content.contains(MANAGED_FILE_MARKER) && !content.contains(LEGACY_MANAGED_MARKER) {
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Dns,
            code: "DNS_CUSTOM_RESOLV_PRESERVED".to_string(),
            severity: FindingSeverity::Info,
            status: FindingStatus::PreservedUserCustom,
            target: resolv_abs.to_string(),
            summary: "保留正常工作的非托管用户 /etc/resolv.conf".to_string(),
            backup_rel_path: None,
        });
    }

    Ok(())
}

// =========================================================
// Domain 4: Sysctl (incl. RW-MEM-01 container guard)
// =========================================================

pub(crate) fn reconcile_sysctl(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    runner: &dyn CommandRunner,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::sysctl",
        None,
        "reconciling Sysctl domain",
        None,
        &[],
    );

    let cfg = ConfigStore::new(&ctx.config_path)
        .load()
        .unwrap_or_default();
    let engine = ctx.template_engine();
    let mut mutated = false;

    let opt_abs = "/etc/sysctl.d/99-debopti-optimize.conf";
    let opt_path = ctx.path(opt_abs);
    let manages_sysctl = opt_path.is_file() || cfg.base_optimized;

    if manages_sysctl {
        let expected = ensure_trailing_newline(engine.render_to_string(
            "templates/system/99-debopti-optimize.conf",
            &BTreeMap::new(),
        )?);
        let current = fs::read_to_string(&opt_path).unwrap_or_default();
        if !opt_path.is_file() || current != expected {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(opt_abs)?;
                sys::write_file_atomic(&opt_path, expected.as_bytes(), Some(0o644))?;
                mutated = true;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Sysctl,
                code: "SYSCTL_OPTIMIZE_SYNC".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: opt_abs.to_string(),
                summary: "同步 /etc/sysctl.d/99-debopti-optimize.conf 内核优化模板".to_string(),
                backup_rel_path: bak,
            });
        }
    }

    let sysctl_d = ctx.path("/etc/sysctl.d");
    if sysctl_d.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(&sysctl_d)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "conf"))
            .collect();
        files.sort();

        for file in files {
            let Some(fname) = file.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let logical = format!("/etc/sysctl.d/{fname}");

            if let Ok(meta) = fs::metadata(&file) {
                let file_mode = meta.permissions().mode() & 0o777;
                if (file_mode & 0o004) == 0 {
                    let (status, bak) = if mode == ReconcileMode::Apply {
                        let b = backup.snapshot_path(&logical)?;
                        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))?;
                        mutated = true;
                        (FindingStatus::Fixed, b)
                    } else {
                        (FindingStatus::WouldFix, None)
                    };
                    findings.push(ReconcileFinding {
                        domain: ReconcileDomain::Sysctl,
                        code: "SYSCTL_FILE_PERMS".to_string(),
                        severity: FindingSeverity::Warning,
                        status,
                        target: logical.clone(),
                        summary: format!("sysctl 配置文件权限异常 ({file_mode:04o})，修正为 0644"),
                        backup_rel_path: bak,
                    });
                }
            }

            if fname == "99-debopti-optimize.conf" || fname == "99-debopti-forwarding.conf" {
                continue;
            }

            let content = fs::read_to_string(&file).unwrap_or_default();
            let mut has_custom_keys = false;
            let mut shadowed_count = 0usize;
            let mut out_lines = Vec::new();

            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                    out_lines.push(line.to_string());
                    continue;
                }
                if let Some((k_raw, _v_raw)) = trimmed.split_once('=') {
                    let key = k_raw.trim();
                    if manages_sysctl && MANAGED_SYSCTL_KEYS.contains(&key) {
                        shadowed_count += 1;
                        out_lines.push(format!("# [debopty-reconcile] shadowed: {line}"));
                    } else {
                        has_custom_keys = true;
                        out_lines.push(line.to_string());
                    }
                } else {
                    out_lines.push(line.to_string());
                }
            }

            if shadowed_count > 0 {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(&logical)?;
                    let next = ensure_trailing_newline(out_lines.join("\n"));
                    sys::write_file_atomic(&file, next.as_bytes(), Some(0o644))?;
                    mutated = true;
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Sysctl,
                    code: "SYSCTL_CONFLICT_SHADOWED".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: logical.clone(),
                    summary: format!(
                        "注释 {shadowed_count} 条冲突 sysctl 键，保留其余用户自定义项"
                    ),
                    backup_rel_path: bak,
                });
            }

            if has_custom_keys {
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Sysctl,
                    code: "SYSCTL_CUSTOM_KEYS_PRESERVED".to_string(),
                    severity: FindingSeverity::Info,
                    status: FindingStatus::PreservedUserCustom,
                    target: logical,
                    summary: "保留用户自定义 sysctl 内核参数".to_string(),
                    backup_rel_path: None,
                });
            }
        }
    }

    // Apply sysctl --system in live mode, degrading gracefully in LXC/container (RW-MEM-01)
    if mode == ReconcileMode::Apply && !ctx.no_sys && mutated {
        let out = runner.run("sysctl", &["--system"], Duration::from_secs(15));
        let restricted = match out {
            Ok(o) if o.success() => false,
            Ok(o) => {
                let err_low = o.stderr.to_ascii_lowercase();
                is_container_env(ctx)
                    || err_low.contains("read-only file system")
                    || err_low.contains("permission denied")
                    || err_low.contains("operation not permitted")
                    || err_low.contains("cannot stat")
            }
            Err(_) => is_container_env(ctx),
        };
        if restricted {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Sysctl,
                code: "SYSCTL_CONTAINER_RESTRICTED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::SkippedNoSys,
                target: "/proc/sys".to_string(),
                summary: "检测到容器/受限内核环境，已跳过只读 sysctl 内核应用（配置文件已就绪）"
                    .to_string(),
                backup_rel_path: None,
            });
        }
    }

    Ok(())
}

// =========================================================
// Domain 5: Limits
// =========================================================

fn systemd_conf_needs_manager_header(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            return false;
        }
        if trimmed.contains('=') {
            return true;
        }
    }
    false
}

fn systemd_conf_has_custom_directives(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with(';')
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            continue;
        }
        if let Some((k, _)) = trimmed.split_once('=')
            && !k.trim().eq_ignore_ascii_case("DefaultLimitNOFILE")
        {
            return true;
        }
    }
    false
}

pub(crate) fn reconcile_limits(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::limits",
        None,
        "reconciling Limits domain",
        None,
        &[],
    );

    let cfg = ConfigStore::new(&ctx.config_path)
        .load()
        .unwrap_or_default();
    let engine = ctx.template_engine();

    for abs in ["/etc/systemd/system.conf", "/etc/systemd/user.conf"] {
        let path = ctx.path(abs);
        if !path.is_file() {
            continue;
        }

        if let Ok(meta) = fs::metadata(&path) {
            let file_mode = meta.permissions().mode() & 0o777;
            if (file_mode & 0o004) == 0 {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(abs)?;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Limits,
                    code: "LIMITS_SYSTEMD_PERMS".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: abs.to_string(),
                    summary: format!("{abs} 权限异常 ({file_mode:04o})，修正为 0644"),
                    backup_rel_path: bak,
                });
            }
        }

        let content = fs::read_to_string(&path).unwrap_or_default();
        if systemd_conf_needs_manager_header(&content) {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(abs)?;
                let next = format!("[Manager]\n{}", ensure_trailing_newline(content.clone()));
                sys::write_file_atomic(&path, next.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Limits,
                code: "LIMITS_SYSTEMD_HEADER".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: abs.to_string(),
                summary: format!("{abs} 缺失 [Manager] 节头，已自动补齐并保留全部指令"),
                backup_rel_path: bak,
            });
        }

        if systemd_conf_has_custom_directives(&content) {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Limits,
                code: "LIMITS_CUSTOM_SYSTEMD_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: abs.to_string(),
                summary: format!("保留 {abs} 中的用户自定义 systemd 指令"),
                backup_rel_path: None,
            });
        }
    }

    let nofile_abs = "/etc/security/limits.d/99-debopti-nofile.conf";
    let nofile_path = ctx.path(nofile_abs);
    if nofile_path.is_file() || cfg.base_optimized {
        let expected = ensure_trailing_newline(
            engine.render_to_string("templates/system/99-debopti-nofile.conf", &BTreeMap::new())?,
        );
        let current = fs::read_to_string(&nofile_path).unwrap_or_default();
        if !nofile_path.is_file() || current != expected {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(nofile_abs)?;
                sys::write_file_atomic(&nofile_path, expected.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Limits,
                code: "LIMITS_NOFILE_SYNC".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: nofile_abs.to_string(),
                summary: "同步 /etc/security/limits.d/99-debopti-nofile.conf 模板".to_string(),
                backup_rel_path: bak,
            });
        }
    }

    let limits_d = ctx.path("/etc/security/limits.d");
    if limits_d.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(&limits_d)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "conf"))
            .collect();
        files.sort();
        for file in files {
            let Some(fname) = file.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let logical = format!("/etc/security/limits.d/{fname}");
            if let Ok(meta) = fs::metadata(&file) {
                let file_mode = meta.permissions().mode() & 0o777;
                if (file_mode & 0o004) == 0 {
                    let (status, bak) = if mode == ReconcileMode::Apply {
                        let b = backup.snapshot_path(&logical)?;
                        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))?;
                        (FindingStatus::Fixed, b)
                    } else {
                        (FindingStatus::WouldFix, None)
                    };
                    findings.push(ReconcileFinding {
                        domain: ReconcileDomain::Limits,
                        code: "LIMITS_FILE_PERMS".to_string(),
                        severity: FindingSeverity::Warning,
                        status,
                        target: logical.clone(),
                        summary: format!("limits.d 文件权限异常 ({file_mode:04o})，修正为 0644"),
                        backup_rel_path: bak,
                    });
                }
            }
            if fname != "99-debopti-nofile.conf" {
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Limits,
                    code: "LIMITS_CUSTOM_DROPIN_PRESERVED".to_string(),
                    severity: FindingSeverity::Info,
                    status: FindingStatus::PreservedUserCustom,
                    target: logical,
                    summary: format!("保留用户自定义 limits.d 配置 ({fname})"),
                    backup_rel_path: None,
                });
            }
        }
    }

    Ok(())
}

// =========================================================
// Domain 6: Swap (incl. RW-MEM-01 LXC zram guard)
// =========================================================

fn is_zramswap_broken(content: &str) -> bool {
    if content.contains('\r') || content.trim().is_empty() {
        return true;
    }
    let mut saw_algo = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            return true;
        };
        let key = k.trim();
        let val = v.trim().trim_matches('"').trim_matches('\'');
        if key == "ALGO" {
            if val.is_empty() {
                return true;
            }
            saw_algo = true;
        } else if key == "PERCENT" {
            let Ok(pct) = val.parse::<u32>() else {
                return true;
            };
            if pct == 0 || pct > 100 {
                return true;
            }
        }
    }
    !saw_algo
}

pub(crate) fn reconcile_swap(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    runner: &dyn CommandRunner,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::swap",
        None,
        "reconciling Swap domain",
        None,
        &[],
    );

    let engine = ctx.template_engine();
    let fstab_abs = "/etc/fstab";
    let fstab_path = ctx.path(fstab_abs);

    if fstab_path.is_file() {
        let raw = fs::read_to_string(&fstab_path).unwrap_or_default();
        let file_mode = fs::metadata(&fstab_path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o644);
        let perms_bad = (file_mode & 0o004) == 0;
        let has_crlf = raw.contains('\r');
        let swapfile_exists = ctx.path("/swapfile").exists();

        let mut seen_swapfile = false;
        let mut dup_swapfile = false;
        let mut missing_swapfile_entry = false;
        let mut has_custom_mounts = false;
        let mut out_lines = Vec::new();

        for raw_line in raw.lines() {
            let line = raw_line.trim_end_matches('\r');
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                out_lines.push(line.to_string());
                continue;
            }
            let first_field = trimmed.split_whitespace().next().unwrap_or("");
            if first_field == "/swapfile" {
                if !swapfile_exists {
                    missing_swapfile_entry = true;
                    out_lines.push(format!("# [debopty-reconcile] missing-swapfile: {line}"));
                } else if seen_swapfile {
                    dup_swapfile = true;
                } else {
                    seen_swapfile = true;
                    out_lines.push(line.to_string());
                }
            } else {
                has_custom_mounts = true;
                out_lines.push(line.to_string());
            }
        }

        if perms_bad || has_crlf || dup_swapfile || missing_swapfile_entry {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(fstab_abs)?;
                let next = ensure_trailing_newline(out_lines.join("\n"));
                sys::write_file_atomic(&fstab_path, next.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Swap,
                code: "SWAP_FSTAB_REPAIRED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: fstab_abs.to_string(),
                summary: format!(
                    "修复 /etc/fstab（权限={}, CRLF={}, 重复swapfile={}, 缺失swapfile={}）",
                    perms_bad, has_crlf, dup_swapfile, missing_swapfile_entry
                ),
                backup_rel_path: bak,
            });
        }

        if has_custom_mounts {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Swap,
                code: "SWAP_CUSTOM_FSTAB_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: fstab_abs.to_string(),
                summary: "完整保留 /etc/fstab 中的用户挂载项、数据盘与独立交换分区".to_string(),
                backup_rel_path: None,
            });
        }
    }

    let zram_abs = "/etc/default/zramswap";
    let zram_path = ctx.path(zram_abs);
    if zram_path.is_file() {
        let raw = fs::read_to_string(&zram_path).unwrap_or_default();
        let file_mode = fs::metadata(&zram_path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o644);
        let perms_bad = (file_mode & 0o004) == 0;
        let broken = is_zramswap_broken(&raw);

        if broken || perms_bad {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(zram_abs)?;
                if broken {
                    let expected = ensure_trailing_newline(
                        engine.render_to_string("templates/system/zramswap", &BTreeMap::new())?,
                    );
                    sys::write_file_atomic(&zram_path, expected.as_bytes(), Some(0o644))?;
                } else {
                    fs::set_permissions(&zram_path, fs::Permissions::from_mode(0o644))?;
                }
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Swap,
                code: "SWAP_ZRAM_CONFIG_REPAIRED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: zram_abs.to_string(),
                summary: "规范化 /etc/default/zramswap 配置模板与权限".to_string(),
                backup_rel_path: bak,
            });
        }

        // RW-MEM-01: LXC/container or unavailable zram module -> graceful SkippedNoSys
        let zram_sysfs_present = ctx.path("/sys/module/zram").is_dir();
        let zram_unavailable = is_container_env(ctx)
            || (!ctx.no_sys
                && !zram_sysfs_present
                && !runner
                    .run("modprobe", &["-n", "zram"], Duration::from_secs(5))
                    .map(|o| o.success())
                    .unwrap_or(false));
        if zram_unavailable {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Swap,
                code: "SWAP_ZRAM_UNAVAILABLE_IN_CONTAINER".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::SkippedNoSys,
                target: zram_abs.to_string(),
                summary: "容器或当前内核不支持 zram 模块，已降级跳过 zram 加载".to_string(),
                backup_rel_path: None,
            });
        }
    }

    Ok(())
}

// =========================================================
// Domain 7: Logrotate
// =========================================================

fn is_logrotate_freq(tok: &str) -> bool {
    matches!(tok, "daily" | "weekly" | "monthly" | "yearly")
}

pub(crate) fn reconcile_logrotate(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::logrotate",
        None,
        "reconciling Logrotate domain",
        None,
        &[],
    );

    let conf_abs = "/etc/logrotate.conf";
    let conf_path = ctx.path(conf_abs);
    if conf_path.is_file() {
        let raw = fs::read_to_string(&conf_path).unwrap_or_default();
        let file_mode = fs::metadata(&conf_path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o644);
        let perms_bad = (file_mode & 0o004) == 0;

        let lines: Vec<&str> = raw.lines().map(|l| l.trim_end_matches('\r')).collect();
        let include_idx = lines
            .iter()
            .position(|l| l.trim().starts_with("include /etc/logrotate.d"));

        let mut freq_positions: Vec<(usize, &str)> = Vec::new();
        let mut has_custom_global = false;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if is_logrotate_freq(trimmed) {
                freq_positions.push((i, trimmed));
            } else if trimmed == "dateext"
                || trimmed.starts_with("su ")
                || trimmed.starts_with("create")
            {
                has_custom_global = true;
            }
        }

        let has_conflict = freq_positions.len() > 1
            || freq_positions.iter().any(|(_, f)| *f != "daily")
            || include_idx.is_some_and(|inc| freq_positions.iter().any(|(pos, _)| *pos > inc));

        if perms_bad || has_conflict {
            let mut out: Vec<String> = Vec::new();
            let mut inserted_daily = false;
            for line in &lines {
                let trimmed = line.trim();
                if is_logrotate_freq(trimmed) {
                    continue;
                }
                if trimmed.starts_with("include /etc/logrotate.d") && !inserted_daily {
                    out.push("daily".to_string());
                    inserted_daily = true;
                }
                out.push((*line).to_string());
            }
            if !inserted_daily {
                out.insert(0, "daily".to_string());
            }
            let next = ensure_trailing_newline(out.join("\n"));

            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(conf_abs)?;
                sys::write_file_atomic(&conf_path, next.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Logrotate,
                code: "LOGROTATE_NORMALIZED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: conf_abs.to_string(),
                summary: "收敛 /etc/logrotate.conf 周期指令为唯一 daily（位于 include 之前）并保留自定义项"
                    .to_string(),
                backup_rel_path: bak,
            });
        }

        if has_custom_global {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Logrotate,
                code: "LOGROTATE_CUSTOM_DIRECTIVES_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: conf_abs.to_string(),
                summary: "保留 /etc/logrotate.conf 中的自定义全局指令（dateext/su/create 等）"
                    .to_string(),
                backup_rel_path: None,
            });
        }
    }

    let dropin_dir = ctx.path("/etc/logrotate.d");
    if dropin_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&dropin_dir)
    {
        let mut count = 0usize;
        for entry in entries.flatten() {
            if entry.path().is_file() {
                count += 1;
            }
        }
        if count > 0 {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Logrotate,
                code: "LOGROTATE_DROPINS_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: "/etc/logrotate.d".to_string(),
                summary: format!("完整保留 /etc/logrotate.d 下 {count} 个日志轮转配置"),
                backup_rel_path: None,
            });
        }
    }

    Ok(())
}

// =========================================================
// Domain 8: APT (incl. RW-MEM-01 dpkg lock & deb822)
// =========================================================

fn parse_deb822_suites(content: &str) -> BTreeSet<String> {
    let mut suites = BTreeSet::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Suites:") {
            for tok in rest.split_whitespace() {
                suites.insert(tok.to_string());
            }
        }
    }
    suites
}

fn is_official_debian_deb_line(line: &str, deb822_suites: &BTreeSet<String>) -> bool {
    let trimmed = line.trim();
    if !(trimmed.starts_with("deb ") || trimmed.starts_with("deb-src ")) {
        return false;
    }
    let is_official_host = trimmed.contains("deb.debian.org")
        || trimmed.contains("security.debian.org")
        || trimmed.contains("mirrors.tuna.tsinghua.edu.cn/debian");
    if !is_official_host {
        return false;
    }
    deb822_suites.is_empty()
        || deb822_suites
            .iter()
            .any(|s| trimmed.split_whitespace().any(|tok| tok == s))
}

pub(crate) fn reconcile_apt(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    runner: &dyn CommandRunner,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::apt",
        None,
        "reconciling APT domain",
        None,
        &[],
    );

    let engine = ctx.template_engine();
    let debian_ver = sys::detect_debian_major(&ctx.path("/etc/debian_version")).unwrap_or(0);
    let sources_list_abs = "/etc/apt/sources.list";
    let sources_list_path = ctx.path(sources_list_abs);
    let sources_d_path = ctx.path("/etc/apt/sources.list.d");
    let debian_sources_path = ctx.path("/etc/apt/sources.list.d/debian.sources");

    // 1. Check permissions on sources.list & sources.list.d/*.{list,sources}
    if sources_list_path.is_file()
        && let Ok(meta) = fs::metadata(&sources_list_path)
    {
        let file_mode = meta.permissions().mode() & 0o777;
        if (file_mode & 0o004) == 0 {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(sources_list_abs)?;
                fs::set_permissions(&sources_list_path, fs::Permissions::from_mode(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Apt,
                code: "APT_SOURCES_PERMS".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: sources_list_abs.to_string(),
                summary: format!("sources.list 权限异常 ({file_mode:04o})，修正为 0644"),
                backup_rel_path: bak,
            });
        }
    }

    if sources_d_path.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(&sources_d_path)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e == "list" || e == "sources")
            })
            .collect();
        files.sort();

        for file in files {
            let Some(fname) = file.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let logical = format!("/etc/apt/sources.list.d/{fname}");
            if let Ok(meta) = fs::metadata(&file) {
                let file_mode = meta.permissions().mode() & 0o777;
                if (file_mode & 0o004) == 0 {
                    let (status, bak) = if mode == ReconcileMode::Apply {
                        let b = backup.snapshot_path(&logical)?;
                        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))?;
                        (FindingStatus::Fixed, b)
                    } else {
                        (FindingStatus::WouldFix, None)
                    };
                    findings.push(ReconcileFinding {
                        domain: ReconcileDomain::Apt,
                        code: "APT_SOURCES_PERMS".to_string(),
                        severity: FindingSeverity::Warning,
                        status,
                        target: logical.clone(),
                        summary: format!("{fname} 权限异常 ({file_mode:04o})，修正为 0644"),
                        backup_rel_path: bak,
                    });
                }
            }

            if fname != "debian.sources" {
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Apt,
                    code: "APT_THIRD_PARTY_REPOS_PRESERVED".to_string(),
                    severity: FindingSeverity::Info,
                    status: FindingStatus::PreservedUserCustom,
                    target: logical,
                    summary: format!("完整保留第三方软件源配置 ({fname})"),
                    backup_rel_path: None,
                });
            }
        }
    }

    // 2. Debian 10 Buster EOL dead mirror repair
    if sources_list_path.is_file() {
        let raw = fs::read_to_string(&sources_list_path).unwrap_or_default();
        if debian_ver == 10
            && raw.lines().any(|l| {
                let t = l.trim();
                !t.starts_with('#')
                    && (t.contains("deb.debian.org") || t.contains("security.debian.org"))
            })
        {
            let tmpl = if ctx.is_cn_region {
                "templates/system/sources.list.buster.cn"
            } else {
                "templates/system/sources.list.buster.global"
            };
            let rendered =
                ensure_trailing_newline(engine.render_to_string(tmpl, &BTreeMap::new())?);
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(sources_list_abs)?;
                sys::write_file_atomic(&sources_list_path, rendered.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Apt,
                code: "APT_BUSTER_EOL_REPAIRED".to_string(),
                severity: FindingSeverity::Critical,
                status,
                target: sources_list_abs.to_string(),
                summary: "修复 Debian 10 Buster 已下线的官方镜像源为归档源".to_string(),
                backup_rel_path: bak,
            });
        } else {
            // 3. Deduplicate identical active deb lines + resolve DEB822 debian.sources overlap
            let deb822_suites = if debian_sources_path.is_file() {
                parse_deb822_suites(&fs::read_to_string(&debian_sources_path).unwrap_or_default())
            } else {
                BTreeSet::new()
            };
            let has_deb822 = debian_sources_path.is_file();

            let mut seen_deb: BTreeSet<String> = BTreeSet::new();
            let mut out_lines: Vec<String> = Vec::new();
            let mut dup_count = 0usize;
            let mut overlap_count = 0usize;

            for raw_line in raw.lines() {
                let line = raw_line.trim_end_matches('\r');
                let trimmed = line.trim();
                if trimmed.starts_with("deb ") || trimmed.starts_with("deb-src ") {
                    let canonical = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
                    if has_deb822 && is_official_debian_deb_line(trimmed, &deb822_suites) {
                        overlap_count += 1;
                        out_lines.push(format!(
                            "# [debopty-reconcile] shadowed-by-debian.sources: {line}"
                        ));
                        continue;
                    }
                    if !seen_deb.insert(canonical) {
                        dup_count += 1;
                        continue;
                    }
                }
                out_lines.push(line.to_string());
            }

            if dup_count > 0 || overlap_count > 0 {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(sources_list_abs)?;
                    let next = ensure_trailing_newline(out_lines.join("\n"));
                    sys::write_file_atomic(&sources_list_path, next.as_bytes(), Some(0o644))?;
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                let code = if overlap_count > 0 {
                    "APT_DEB822_OVERLAP_RESOLVED"
                } else {
                    "APT_SOURCES_DEDUPED"
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Apt,
                    code: code.to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: sources_list_abs.to_string(),
                    summary: format!(
                        "整理 /etc/apt/sources.list（去重 {dup_count} 条，消除与 debian.sources 重叠 {overlap_count} 条）"
                    ),
                    backup_rel_path: bak,
                });
            }
        }
    }

    // 4. RW-MEM-01: Detect active/stale dpkg/apt locks — NEVER blindly rm an active lock!
    let lock_paths = [
        "/var/lib/dpkg/lock-frontend",
        "/var/lib/dpkg/lock",
        "/var/lib/apt/lists/lock",
    ];
    let existing_locks: Vec<&str> = lock_paths
        .iter()
        .copied()
        .filter(|p| ctx.path(p).exists())
        .collect();

    if !existing_locks.is_empty() {
        let mut active_lock: Option<&str> = None;
        if !ctx.no_sys {
            for &lock_abs in &existing_locks {
                let lock_real = ctx.path(lock_abs).to_string_lossy().into_owned();
                if runner
                    .run("fuser", &[&lock_real], Duration::from_secs(5))
                    .map(|o| o.success())
                    .unwrap_or(false)
                {
                    active_lock = Some(lock_abs);
                    break;
                }
            }
        } else {
            // In no_sys mode we cannot probe live PIDs with fuser; report safely without deleting lock!
            active_lock = Some(existing_locks[0]);
        }

        if let Some(lock_abs) = active_lock {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Apt,
                code: "APT_DPKG_LOCK_ACTIVE".to_string(),
                severity: FindingSeverity::Warning,
                status: FindingStatus::SkippedNoSys,
                target: lock_abs.to_string(),
                summary: "检测到 dpkg/apt 锁文件，为防止破坏包数据库禁止盲删，请确认无 apt 进程后运行 dpkg --configure -a"
                    .to_string(),
                backup_rel_path: None,
            });
        } else if !ctx.no_sys {
            // Stale lock (no process holds it): inspect `dpkg --audit`
            let audit_out = runner.run("dpkg", &["--audit"], Duration::from_secs(15));
            let needs_repair = match audit_out {
                Ok(ref o) => !o.success() || !o.stdout.trim().is_empty(),
                Err(_) => false,
            };
            if needs_repair {
                let status = if mode == ReconcileMode::Apply {
                    let env = &[("DEBIAN_FRONTEND", "noninteractive")];
                    let _ = runner.run_with_env(
                        "dpkg",
                        &["--configure", "-a"],
                        env,
                        Duration::from_secs(120),
                    );
                    let _ = runner.run_with_env(
                        "apt-get",
                        &["-f", "install", "-yq"],
                        env,
                        Duration::from_secs(180),
                    );
                    FindingStatus::Fixed
                } else {
                    FindingStatus::WouldFix
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Apt,
                    code: "APT_DPKG_BROKEN_REPAIRED".to_string(),
                    severity: FindingSeverity::Critical,
                    status,
                    target: "/var/lib/dpkg/status".to_string(),
                    summary: "检测到中断的 dpkg 事务，已通过 dpkg --configure -a 与 apt-get -f install 修复"
                        .to_string(),
                    backup_rel_path: None,
                });
            }
        }
    }

    Ok(())
}

// =========================================================
// Domain 9: Legacy (/opt/debopti, shortcuts, corrupt debopti.conf)
// =========================================================

fn is_valid_conf_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn escape_bash_double_quoted(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for ch in val.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn parse_lenient_value(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(end) = rest.rfind('"') {
            return (rest[..end].to_string(), false);
        }
        return (rest.to_string(), true); // unclosed double quote
    }
    if let Some(rest) = trimmed.strip_prefix('\'') {
        if let Some(end) = rest.rfind('\'') {
            return (rest[..end].to_string(), false);
        }
        return (rest.to_string(), true); // unclosed single quote
    }
    (trimmed.to_string(), false)
}

fn inspect_and_repair_debopti_conf(
    raw_bytes: &[u8],
    template_raw: &str,
    file_mode: u32,
) -> (bool, String, bool) {
    let has_bom = raw_bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let has_nul = raw_bytes.contains(&0);
    let cleaned_str = String::from_utf8_lossy(if has_bom { &raw_bytes[3..] } else { raw_bytes })
        .replace('\0', "");
    let has_crlf = cleaned_str.contains('\r');
    let perms_bad = (file_mode & 0o004) == 0;

    let template_comments: BTreeSet<String> = template_raw
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('#'))
        .map(str::to_string)
        .collect();
    let template_keys: BTreeSet<String> = template_raw
        .lines()
        .filter_map(|l| l.trim().split_once('=').map(|(k, _)| k.trim().to_string()))
        .filter(|k| is_valid_conf_key(k))
        .collect();

    let mut seen_keys = BTreeSet::new();
    let mut has_dup = false;
    let mut has_malformed_quote = false;
    let mut kv_values: BTreeMap<String, String> = BTreeMap::new();
    let mut custom_keys: Vec<String> = Vec::new();
    let mut custom_comments: Vec<String> = Vec::new();

    for raw_line in cleaned_str.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            if !template_comments.contains(trimmed) {
                custom_comments.push(line.to_string());
            }
            continue;
        }
        if let Some((k_raw, v_raw)) = trimmed.split_once('=') {
            let key = k_raw.trim();
            if is_valid_conf_key(key) {
                if !seen_keys.insert(key.to_string()) {
                    has_dup = true;
                }
                let (val, bad_q) = parse_lenient_value(v_raw);
                if bad_q {
                    has_malformed_quote = true;
                }
                if !template_keys.contains(key) && !custom_keys.iter().any(|c| c == key) {
                    custom_keys.push(key.to_string());
                }
                kv_values.insert(key.to_string(), val);
            }
        }
    }

    let needs_repair =
        has_bom || has_nul || has_crlf || has_dup || has_malformed_quote || perms_bad;
    let has_user_custom = !custom_keys.is_empty() || !custom_comments.is_empty();

    // Reconstruct cleanly from templates/common/debopti.conf preserving ALL known & custom keys + comments
    let mut out_lines: Vec<String> = Vec::new();
    for line in template_raw.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some((k_raw, _)) = trimmed.split_once('=')
        {
            let key = k_raw.trim();
            if let Some(val) = kv_values.get(key) {
                out_lines.push(format!("{key}=\"{}\"", escape_bash_double_quoted(val)));
                continue;
            }
        }
        out_lines.push(line.to_string());
    }

    if has_user_custom {
        out_lines.push(String::new());
        for c in &custom_comments {
            out_lines.push(c.clone());
        }
        for k in &custom_keys {
            if let Some(val) = kv_values.get(k) {
                out_lines.push(format!("{k}=\"{}\"", escape_bash_double_quoted(val)));
            }
        }
    }

    let reconstructed = ensure_trailing_newline(out_lines.join("\n"));
    (needs_repair, reconstructed, has_user_custom)
}

pub(crate) fn reconcile_legacy(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::legacy",
        None,
        "reconciling Legacy domain",
        None,
        &[],
    );

    let engine = ctx.template_engine();

    // 1. Clean legacy /opt/debopti (backing up every file inside first!)
    let old_opt = ctx.path("/opt/debopti");
    if old_opt.is_dir() {
        let (status, bak) = if mode == ReconcileMode::Apply {
            let mut first_bak = None;
            for entry in WalkDir::new(&old_opt)
                .follow_links(false)
                .into_iter()
                .filter_map(std::result::Result::ok)
            {
                if (entry.file_type().is_file() || entry.file_type().is_symlink())
                    && let Ok(rel) = entry.path().strip_prefix(&ctx.root_prefix)
                {
                    let abs_logical = format!("/{}", rel.to_string_lossy());
                    let b = backup.snapshot_path(&abs_logical)?;
                    if first_bak.is_none() {
                        first_bak = b;
                    }
                }
            }
            fs::remove_dir_all(&old_opt)?;
            (FindingStatus::Fixed, first_bak)
        } else {
            (FindingStatus::WouldFix, None)
        };
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Legacy,
            code: "LEGACY_OPT_DIR".to_string(),
            severity: FindingSeverity::Warning,
            status,
            target: "/opt/debopti".to_string(),
            summary: "备份并清理遗留安装目录 /opt/debopti".to_string(),
            backup_rel_path: bak,
        });
    }

    // 2. Clean legacy shortcuts & shell profiles, ensuring debopty.{sh,fish} via TemplateEngine
    let legacy_bin_abs = "/usr/local/bin/debopti";
    let legacy_sh_abs = "/etc/profile.d/debopti.sh";
    let legacy_fish_abs = "/etc/fish/conf.d/debopti.fish";

    let has_legacy_bin = fs::symlink_metadata(ctx.path(legacy_bin_abs)).is_ok();
    let has_legacy_sh = fs::symlink_metadata(ctx.path(legacy_sh_abs)).is_ok();
    let has_legacy_fish = fs::symlink_metadata(ctx.path(legacy_fish_abs)).is_ok();

    if has_legacy_bin || has_legacy_sh || has_legacy_fish {
        let (status, bak) = if mode == ReconcileMode::Apply {
            let mut first_bak = None;
            let mut vars = BTreeMap::new();
            vars.insert("BIN_LINK".to_string(), "/usr/local/bin/debopty".to_string());

            if has_legacy_bin {
                first_bak = backup.snapshot_path(legacy_bin_abs)?;
                fs::remove_file(ctx.path(legacy_bin_abs))?;
            }
            if has_legacy_sh {
                let b = backup.snapshot_path(legacy_sh_abs)?;
                if first_bak.is_none() {
                    first_bak = b;
                }
                fs::remove_file(ctx.path(legacy_sh_abs))?;
                let new_sh_abs = "/etc/profile.d/debopty.sh";
                let _ = backup.snapshot_path(new_sh_abs)?;
                let rendered = ensure_trailing_newline(
                    engine.render_to_string("templates/common/debopty.sh", &vars)?,
                );
                sys::write_file_atomic(&ctx.path(new_sh_abs), rendered.as_bytes(), Some(0o644))?;
            }
            if has_legacy_fish {
                let b = backup.snapshot_path(legacy_fish_abs)?;
                if first_bak.is_none() {
                    first_bak = b;
                }
                fs::remove_file(ctx.path(legacy_fish_abs))?;
                let new_fish_abs = "/etc/fish/conf.d/debopty.fish";
                let _ = backup.snapshot_path(new_fish_abs)?;
                let rendered = ensure_trailing_newline(
                    engine.render_to_string("templates/common/debopty.fish", &vars)?,
                );
                sys::write_file_atomic(&ctx.path(new_fish_abs), rendered.as_bytes(), Some(0o644))?;
            }
            (FindingStatus::Fixed, first_bak)
        } else {
            (FindingStatus::WouldFix, None)
        };
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Legacy,
            code: "LEGACY_SHORTCUTS_CLEANED".to_string(),
            severity: FindingSeverity::Warning,
            status,
            target: legacy_bin_abs.to_string(),
            summary: "清理遗留 debopti 快捷命令/Shell 配置并同步新版 debopty 入口".to_string(),
            backup_rel_path: bak,
        });
    }

    // Preserve unrelated shell profiles (U8)
    for (dir_abs, skip_names) in [
        ("/etc/profile.d", &["debopti.sh", "debopty.sh"][..]),
        ("/etc/fish/conf.d", &["debopti.fish", "debopty.fish"][..]),
    ] {
        let dir = ctx.path(dir_abs);
        if dir.is_dir()
            && let Ok(entries) = fs::read_dir(&dir)
        {
            let mut others = Vec::new();
            for entry in entries.flatten() {
                if entry.path().is_file()
                    && let Some(name) = entry.file_name().to_str()
                    && !skip_names.contains(&name)
                {
                    others.push(format!("{dir_abs}/{name}"));
                }
            }
            others.sort();
            for other in others {
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Legacy,
                    code: "LEGACY_SHELL_PROFILES_PRESERVED".to_string(),
                    severity: FindingSeverity::Info,
                    status: FindingStatus::PreservedUserCustom,
                    target: other,
                    summary: "保留无关的用户/系统 Shell 启动脚本".to_string(),
                    backup_rel_path: None,
                });
            }
        }
    }

    // 3. Repair corrupt /etc/debopti/debopti.conf while preserving ALL custom keys & comments (U8)
    let conf_path = &ctx.config_path;
    if conf_path.is_file() {
        let conf_logical = conf_path
            .strip_prefix(&ctx.root_prefix)
            .map(|r| format!("/{}", r.to_string_lossy()))
            .unwrap_or_else(|_| "/etc/debopti/debopti.conf".to_string());
        let raw_bytes = fs::read(conf_path)?;
        let file_mode = fs::metadata(conf_path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o644);
        let template_raw = engine.get_raw("templates/common/debopti.conf")?;
        let (needs_repair, reconstructed, has_user_custom) =
            inspect_and_repair_debopti_conf(&raw_bytes, &template_raw, file_mode);

        if needs_repair {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(&conf_logical)?;
                sys::write_file_atomic(conf_path, reconstructed.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Legacy,
                code: "LEGACY_CONF_REPAIRED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: conf_logical.clone(),
                summary: "修复损坏的 debopti.conf（BOM/NUL/CRLF/重复键/引号/权限）并保留全部自定义键值与注释"
                    .to_string(),
                backup_rel_path: bak,
            });
        }

        if has_user_custom {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Legacy,
                code: "LEGACY_CUSTOM_CONF_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: conf_logical,
                summary: "保留 debopti.conf 中的用户自定义键与注释".to_string(),
                backup_rel_path: None,
            });
        }
    }

    Ok(())
}
