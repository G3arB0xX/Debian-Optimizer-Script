use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use debopty_core::config::ConfigStore;
use debopty_core::network::{
    DnsFixMode, DnsFixOptions, UreqHttpClient, check_and_fix_dns, download_with_fallback,
    get_latest_github_release, git_clone_with_fallback, global_netcheck,
};
use debopty_core::sys::RealCommandRunner;
use serde::Serialize;

use crate::CliError;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DnsModeArg {
    Initial,
    Calibrate,
}

impl From<DnsModeArg> for DnsFixMode {
    fn from(value: DnsModeArg) -> Self {
        match value {
            DnsModeArg::Initial => Self::Initial,
            DnsModeArg::Calibrate => Self::Calibrate,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum NetworkAction {
    /// 两阶段 DNS 健康检查与自动校准
    DnsCheck {
        /// 执行阶段: initial | calibrate
        #[arg(long, value_enum)]
        mode: DnsModeArg,
        /// resolv.conf 文件路径覆盖 (默认 /etc/resolv.conf)
        #[arg(long, value_name = "PATH")]
        resolv: Option<PathBuf>,
        /// 备份文件路径覆盖 (默认 <resolv>.debopti.bak)
        #[arg(long, value_name = "PATH")]
        backup: Option<PathBuf>,
        /// 按中国大陆 DNS 列表校准
        #[arg(long)]
        cn: bool,
        /// 以 JSON 格式输出报告
        #[arg(long)]
        json: bool,
    },
    /// 检测服务器网络区域 (中国大陆/海外) 并持久化到配置
    Netcheck {
        /// 忽略缓存强制重新探测
        #[arg(long)]
        force: bool,
        /// 以 JSON 格式输出报告
        #[arg(long)]
        json: bool,
    },
    /// 获取 GitHub 仓库最新 Release 标签
    LatestRelease {
        /// GitHub 仓库 (owner/repo)
        repo: String,
        /// 启用中国大陆 API 加速池
        #[arg(long)]
        cn: bool,
    },
    /// 使用镜像池容错下载文件
    Download {
        /// 原始下载链接
        url: String,
        /// 本地输出路径
        #[arg(long, value_name = "PATH")]
        out: PathBuf,
        /// 启用中国大陆 GitHub 镜像加速池
        #[arg(long)]
        cn: bool,
    },
    /// 使用镜像池容错执行 git clone (受保护目录守卫 + origin 还原)
    GitClone {
        /// Git 仓库 URL
        url: String,
        /// 目标克隆目录
        dest: PathBuf,
        /// 启用中国大陆 Git 镜像加速池
        #[arg(long)]
        cn: bool,
        /// 透传给 git clone 的额外参数 (置于 `--` 之后)
        #[arg(last = true)]
        extra_args: Vec<String>,
    },
}

#[derive(Debug, Serialize)]
struct DnsCheckOutput {
    modified: bool,
    has_v4: bool,
    has_v6: bool,
    nameservers: Vec<String>,
}

#[derive(Debug, Serialize)]
struct NetcheckOutput {
    is_cn_region: bool,
    cached: bool,
    env_exports: BTreeMap<String, String>,
}

fn effective_is_cn(flag_cn: bool, config_path: &Path) -> bool {
    if flag_cn {
        return true;
    }
    ConfigStore::new(config_path)
        .load()
        .map(|cfg| cfg.is_cn_region.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn handle_network(action: &NetworkAction, config_path: &Path) -> Result<i32, CliError> {
    let http = UreqHttpClient::new();
    let runner = RealCommandRunner;

    match action {
        NetworkAction::DnsCheck {
            mode,
            resolv,
            backup,
            cn,
            json,
        } => {
            let defaults = DnsFixOptions::default();
            let resolv_path = resolv.clone().unwrap_or(defaults.resolv_path);
            let backup_path = backup.clone().unwrap_or_else(|| {
                if resolv.is_some() {
                    PathBuf::from(format!("{}.debopti.bak", resolv_path.display()))
                } else {
                    defaults.backup_path
                }
            });
            let opts = DnsFixOptions {
                resolv_path,
                backup_path,
                is_cn_region: *cn,
            };
            let report = check_and_fix_dns((*mode).into(), &opts, &http, &runner)?;
            if *json {
                let payload = DnsCheckOutput {
                    modified: report.modified,
                    has_v4: report.has_v4,
                    has_v6: report.has_v6,
                    nameservers: report.nameservers,
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else if report.modified {
                println!(
                    "DNS 配置已更新 (IPv4={}, IPv6={}, nameservers={})",
                    report.has_v4,
                    report.has_v6,
                    report.nameservers.join(", ")
                );
            } else {
                println!("DNS 配置无需变更。");
            }
            Ok(0)
        }
        NetworkAction::Netcheck { force, json } => {
            let store = ConfigStore::new(config_path);
            let report = global_netcheck(&store, *force, &http)?;
            if *json {
                let payload = NetcheckOutput {
                    is_cn_region: report.is_cn_region,
                    cached: report.cached,
                    env_exports: report.env_exports,
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                let region = if report.is_cn_region {
                    "中国大陆 (镜像加速)"
                } else {
                    "海外直连"
                };
                let src = if report.cached {
                    "缓存"
                } else {
                    "实时探测"
                };
                println!("网络区域: {region} ({src})");
                for (k, v) in &report.env_exports {
                    println!("{k}={v}");
                }
            }
            Ok(0)
        }
        NetworkAction::LatestRelease { repo, cn } => {
            let is_cn = effective_is_cn(*cn, config_path);
            match get_latest_github_release(repo, is_cn, &http)? {
                Some(tag) => {
                    println!("{tag}");
                    Ok(0)
                }
                None => Err(CliError::NotFound(format!(
                    "未能获取仓库 {repo} 的最新 Release 版本"
                ))),
            }
        }
        NetworkAction::Download { url, out, cn } => {
            let is_cn = effective_is_cn(*cn, config_path);
            let winner = download_with_fallback(out, url, is_cn, &http)?;
            println!("{winner}");
            Ok(0)
        }
        NetworkAction::GitClone {
            url,
            dest,
            cn,
            extra_args,
        } => {
            let is_cn = effective_is_cn(*cn, config_path);
            let extra_refs: Vec<&str> = extra_args.iter().map(String::as_str).collect();
            let winner = git_clone_with_fallback(dest, url, &extra_refs, is_cn, &runner)?;
            println!("{winner}");
            Ok(0)
        }
    }
}
