use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::Args;
use debopty_core::config::ConfigStore;
use debopty_core::reconcile::{
    FindingSeverity, FindingStatus, ReconcileContext, ReconcileDomain, ReconcileMode,
    ReconcileReport, audit_system, reconcile_system, restore_backup,
};
use debopty_core::sys::RealCommandRunner;
use serde::Serialize;

use crate::{CliError, non_empty_env};

#[derive(Debug, Args)]
pub struct ReconcileArgs {
    /// 只读巡检模式（默认模式，不修改任何系统文件）
    #[arg(long)]
    pub audit: bool,

    /// 应用稳态收敛并自动创建原子备份快照
    #[arg(long)]
    pub apply: bool,

    /// 指定巡检/收敛域（可重复或逗号分隔）: all|ssh|firewall|dns|sysctl|limits|swap|logrotate|apt|legacy
    #[arg(long = "domain", value_name = "DOMAIN")]
    pub domains: Vec<String>,

    /// 根目录前缀（默认 /，用于离线测试与沙箱隔离）
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// 跳过外部系统命令调用（仅处理文件与模板）
    #[arg(long)]
    pub no_sys: bool,

    /// 备份快照存放目录（默认 <root>/var/backups/debopty/reconcile）
    #[arg(long, value_name = "PATH")]
    pub backup_dir: Option<PathBuf>,

    /// 从指定备份快照目录一键回滚全部变更（与 --apply / --audit 互斥）
    #[arg(long, value_name = "RUN_DIR")]
    pub restore: Option<PathBuf>,

    /// 跳过交互确认直接执行 --apply
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// 以纯 JSON 输出报告到 stdout（诊断日志始终输出到 stderr）
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct RestoreOutput {
    backup_dir: PathBuf,
    root_prefix: PathBuf,
    restored_count: usize,
}

fn parse_domains(raw_domains: &[String]) -> Result<Vec<ReconcileDomain>, CliError> {
    if raw_domains.is_empty() {
        return Ok(Vec::new());
    }
    let mut parsed = Vec::new();
    for item in raw_domains {
        for tok in item.split(',') {
            let trimmed = tok.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.eq_ignore_ascii_case("all") {
                return Ok(Vec::new());
            }
            let domain =
                ReconcileDomain::from_str(trimmed).map_err(|e| CliError::Usage(e.to_string()))?;
            if !parsed.contains(&domain) {
                parsed.push(domain);
            }
        }
    }
    Ok(parsed)
}

fn format_severity(sev: FindingSeverity) -> &'static str {
    match sev {
        FindingSeverity::Critical => "CRIT",
        FindingSeverity::Warning => "WARN",
        FindingSeverity::Info => "INFO",
    }
}

fn format_status(st: FindingStatus) -> &'static str {
    match st {
        FindingStatus::Clean => "正常",
        FindingStatus::PreservedUserCustom => "已保留用户配置",
        FindingStatus::WouldFix => "待修复",
        FindingStatus::Fixed => "已修复",
        FindingStatus::Quarantined => "已隔离",
        FindingStatus::SkippedNoSys => "已降级跳过",
    }
}

pub fn print_reconcile_human(report: &ReconcileReport) {
    let mode_str = match report.mode {
        ReconcileMode::Audit => "只读巡检 (Audit)",
        ReconcileMode::Apply => "稳态收敛 (Apply)",
    };
    println!("=== debopty 系统冲突巡检与稳态接管 [{mode_str}] ===");
    println!(
        "冲突/漂移项: {} | 已变更项: {} | 已保留用户自定义项: {}",
        report.drift_count, report.changes_made, report.preserved_count
    );
    if let Some(ref bak) = report.backup_dir {
        println!("原子备份目录: {}", bak.display());
    }
    if report.findings.is_empty() {
        println!("系统状态干净，未检测到冲突或漂移。");
        return;
    }
    println!();
    for f in &report.findings {
        println!(
            "  [{}][{}] ({}) {} — {}",
            format_severity(f.severity),
            format_status(f.status),
            f.domain,
            f.target,
            f.summary
        );
    }
}

pub fn handle_reconcile(
    args: &ReconcileArgs,
    explicit_config_path: Option<&Path>,
    cli_template_dir: Option<&Path>,
) -> Result<i32, CliError> {
    if args.apply && args.restore.is_some() {
        return Err(CliError::Usage(
            "`--apply` 与 `--restore` 互斥，请仅指定其中一个模式".to_string(),
        ));
    }
    if args.audit && args.apply {
        return Err(CliError::Usage(
            "`--audit` 与 `--apply` 互斥，请仅指定其中一个模式".to_string(),
        ));
    }
    if args.audit && args.restore.is_some() {
        return Err(CliError::Usage(
            "`--audit` 与 `--restore` 互斥，请仅指定其中一个模式".to_string(),
        ));
    }

    let root_prefix = args.root.clone().unwrap_or_else(|| PathBuf::from("/"));

    if let Some(ref restore_dir) = args.restore {
        let restored_count = restore_backup(restore_dir, &root_prefix)?;
        if args.json {
            let payload = RestoreOutput {
                backup_dir: restore_dir.clone(),
                root_prefix,
                restored_count,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            println!(
                "已从备份 {} 成功回滚 {} 项配置至 {}。",
                restore_dir.display(),
                restored_count,
                root_prefix.display()
            );
        }
        return Ok(0);
    }

    let mut ctx = ReconcileContext::with_root(&root_prefix);
    if let Some(cfg_path) = explicit_config_path {
        ctx.config_path = cfg_path.to_path_buf();
    }
    if let Some(ref bak) = args.backup_dir {
        ctx.backup_base_dir = bak.clone();
    }
    ctx.template_dir = cli_template_dir.map(Path::to_path_buf);
    ctx.no_sys = args.no_sys;
    ctx.domains = parse_domains(&args.domains)?;
    ctx.f2b_backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
    if let Ok(cfg) = ConfigStore::new(&ctx.config_path).load() {
        ctx.is_cn_region = cfg.is_cn_region_enabled();
    }

    let runner = RealCommandRunner;
    let mode = if args.apply {
        ReconcileMode::Apply
    } else {
        ReconcileMode::Audit
    };

    if mode == ReconcileMode::Apply && !args.yes && !args.json && io::stdin().is_terminal() {
        let preview = audit_system(&ctx, &runner)?;
        print_reconcile_human(&preview);
        if preview.drift_count == 0 {
            return Ok(0);
        }
        print!("\n是否确认应用以上稳态收敛并自动创建备份？[y/N]: ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().read_line(&mut line);
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("已取消应用操作。");
            return Ok(0);
        }
    }

    let report = reconcile_system(&ctx, mode, &runner)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_reconcile_human(&report);
    }
    Ok(0)
}
