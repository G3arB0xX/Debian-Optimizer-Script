use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::diag::redact_sensitive;
use crate::error::Result;
use crate::security::f2b_sync::detect_ssh_port_from_configs;
use crate::sys::{CmdOutput, CommandRunner};

pub const HISTORY_CAP: usize = 48;

const BRAILLE_BASE: u32 = 0x2800;
const BRAILLE_LEFT_DOTS: [u8; 4] = [0x40, 0x04, 0x02, 0x01];
const BRAILLE_RIGHT_DOTS: [u8; 4] = [0x80, 0x20, 0x10, 0x08];

const SENSITIVE_LINE_KEYWORDS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "token",
    "secret",
    "private_key",
    "privatekey",
    "uuid",
    "credential",
    "authorization",
    "api_key",
    "apikey",
    "auth_key",
    "authkey",
];

struct NoopRunner;

impl CommandRunner for NoopRunner {
    fn run(&self, _prog: &str, _args: &[&str], _timeout: Duration) -> Result<CmdOutput> {
        Ok(CmdOutput {
            code: 1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModuleDetail {
    pub service_state: String,
    pub config_path: String,
    pub config_exists: bool,
    pub ports: Vec<String>,
    pub kv_facts: Vec<(String, String)>,
    pub config_excerpt: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSpecs {
    pub hostname: String,
    pub os_name: String,
    pub arch: String,
    pub kernel: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub mem_total_kib: u64,
    pub swap_total_kib: u64,
    pub disk_root_used_gib_x10: Option<u32>,
    pub disk_root_total_gib_x10: Option<u32>,
    pub tcp_cc: String,
    pub default_qdisc: String,
    pub ipv4_forward: bool,
    pub ipv6_forward: bool,
    pub file_max: u64,
    pub swappiness: Option<u32>,
    pub ssh_port: u16,
    pub ssh_password_auth: Option<bool>,
    pub nftables_configured: bool,
    pub fw_open_ports: Vec<u16>,
    pub fail2ban_configured: bool,
    pub fail2ban_backend: String,
    pub module_details: BTreeMap<String, ModuleDetail>,
}

impl Default for SystemSpecs {
    fn default() -> Self {
        Self {
            hostname: "debian".to_string(),
            os_name: "Debian".to_string(),
            arch: std::env::consts::ARCH.to_string(),
            kernel: String::new(),
            cpu_model: String::new(),
            cpu_cores: 1,
            mem_total_kib: 0,
            swap_total_kib: 0,
            disk_root_used_gib_x10: None,
            disk_root_total_gib_x10: None,
            tcp_cc: String::new(),
            default_qdisc: String::new(),
            ipv4_forward: false,
            ipv6_forward: false,
            file_max: 0,
            swappiness: None,
            ssh_port: 22,
            ssh_password_auth: None,
            nftables_configured: false,
            fw_open_ports: Vec::new(),
            fail2ban_configured: false,
            fail2ban_backend: String::new(),
            module_details: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LiveSample {
    pub cpu_pct: f64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub procs_running: u32,
    pub procs_total: u32,
    pub mem_used_kib: u64,
    pub mem_total_kib: u64,
    pub mem_pct: f64,
    pub swap_used_kib: u64,
    pub swap_total_kib: u64,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TelemetryHistory {
    pub cpu_pct: VecDeque<f64>,
    pub load1: VecDeque<f64>,
    pub mem_pct: VecDeque<f64>,
    pub net_kbps: VecDeque<f64>,
    pub latest: LiveSample,
}

fn push_bounded(queue: &mut VecDeque<f64>, val: f64) {
    let clean = if val.is_finite() { val.max(0.0) } else { 0.0 };
    if queue.len() >= HISTORY_CAP {
        queue.pop_front();
    }
    queue.push_back(clean);
}

fn fnv1a_step(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn hash_f64_quantized(hash: u64, val: f64, scale: f64) -> u64 {
    let q = if val.is_finite() {
        (val * scale).round() as i64
    } else {
        0
    };
    fnv1a_step(hash, &q.to_le_bytes())
}

fn hash_queue(mut hash: u64, q: &VecDeque<f64>, scale: f64) -> u64 {
    hash = fnv1a_step(hash, &(q.len() as u64).to_le_bytes());
    for &v in q {
        hash = hash_f64_quantized(hash, v, scale);
    }
    hash
}

impl TelemetryHistory {
    pub fn push_sample(&mut self, sample: LiveSample) {
        push_bounded(&mut self.cpu_pct, sample.cpu_pct);
        push_bounded(&mut self.load1, sample.load1);
        push_bounded(&mut self.mem_pct, sample.mem_pct);
        let net_kbps = (sample.net_rx_bps.max(0.0) + sample.net_tx_bps.max(0.0)) / 1024.0;
        push_bounded(&mut self.net_kbps, net_kbps);
        self.latest = sample;
    }

    pub fn visual_fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        h = hash_queue(h, &self.cpu_pct, 10.0);
        h = hash_queue(h, &self.load1, 100.0);
        h = hash_queue(h, &self.mem_pct, 10.0);
        h = hash_queue(h, &self.net_kbps, 10.0);
        h = hash_f64_quantized(h, self.latest.cpu_pct, 10.0);
        h = hash_f64_quantized(h, self.latest.load1, 100.0);
        h = hash_f64_quantized(h, self.latest.load5, 100.0);
        h = hash_f64_quantized(h, self.latest.load15, 100.0);
        h = fnv1a_step(h, &self.latest.procs_running.to_le_bytes());
        h = fnv1a_step(h, &self.latest.procs_total.to_le_bytes());
        h = fnv1a_step(h, &self.latest.mem_used_kib.to_le_bytes());
        h = fnv1a_step(h, &self.latest.mem_total_kib.to_le_bytes());
        h = hash_f64_quantized(h, self.latest.mem_pct, 10.0);
        h = fnv1a_step(h, &self.latest.swap_used_kib.to_le_bytes());
        h = fnv1a_step(h, &self.latest.swap_total_kib.to_le_bytes());
        h = hash_f64_quantized(h, self.latest.net_rx_bps, 1.0);
        h = hash_f64_quantized(h, self.latest.net_tx_bps, 1.0);
        h = fnv1a_step(h, &self.latest.uptime_secs.to_le_bytes());
        h
    }

    pub fn demo_fixture(phase: usize) -> (SystemSpecs, TelemetryHistory) {
        let specs = demo_specs();
        let mut history = TelemetryHistory::default();
        let mem_total = specs.mem_total_kib;
        let swap_total = specs.swap_total_kib;
        let base_uptime: u64 = 14 * 86_400 + 6 * 3600 + 22 * 60;

        for i in 0..40usize {
            let t = (i + phase) as f64 * 0.28;
            let cpu_pct = (26.0 + 14.0 * t.sin() + 6.5 * (t * 2.1).cos()).clamp(4.0, 96.0);
            let load1 = (0.48 + 0.34 * (t * 0.85).sin() + 0.12 * (t * 1.9).cos()).clamp(0.05, 4.0);
            let load5 = (0.44 + 0.18 * (t * 0.45).sin()).clamp(0.05, 4.0);
            let load15 = (0.39 + 0.09 * (t * 0.25).sin()).clamp(0.05, 4.0);
            let mem_pct = (44.0 + 7.5 * (t * 0.5).sin() + 2.0 * (t * 1.3).cos()).clamp(15.0, 92.0);
            let mem_used_kib = ((mem_total as f64) * (mem_pct / 100.0)).round() as u64;
            let swap_used_kib = ((swap_total as f64) * 0.06).round() as u64;
            let net_rx_bps =
                (420_000.0 + 260_000.0 * (t * 1.15).sin() + 90_000.0 * (t * 2.6).cos())
                    .max(12_000.0);
            let net_tx_bps =
                (290_000.0 + 190_000.0 * (t * 0.95).cos() + 65_000.0 * (t * 2.2).sin())
                    .max(8_000.0);
            let procs_running = if cpu_pct > 36.0 { 3 } else { 2 };

            history.push_sample(LiveSample {
                cpu_pct,
                load1,
                load5,
                load15,
                procs_running,
                procs_total: 168,
                mem_used_kib,
                mem_total_kib: mem_total,
                mem_pct,
                swap_used_kib,
                swap_total_kib: swap_total,
                net_rx_bps,
                net_tx_bps,
                uptime_secs: base_uptime + ((i + phase) as u64) * 2,
            });
        }

        (specs, history)
    }
}

fn demo_specs() -> SystemSpecs {
    let mut details = BTreeMap::new();
    details.insert(
        "base".to_string(),
        ModuleDetail {
            service_state: "configured".to_string(),
            config_path: "/etc/sysctl.d/99-debopti-optimize.conf".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                ("拥塞算法".to_string(), "bbr".to_string()),
                ("队列调度".to_string(), "fq".to_string()),
                ("文件句柄".to_string(), "1.0M".to_string()),
                ("交换倾向".to_string(), "10".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "net.core.default_qdisc = fq\nnet.ipv4.tcp_congestion_control = bbr\nvm.swappiness = 10\nfs.file-max = 1048576\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "ip_forward".to_string(),
        ModuleDetail {
            service_state: "active".to_string(),
            config_path: "/etc/sysctl.d/99-debopti-forwarding.conf".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                ("IPv4 转发".to_string(), "已开启".to_string()),
                ("IPv6 转发".to_string(), "已开启".to_string()),
                ("配置状态".to_string(), "已持久化".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "net.ipv4.ip_forward = 1\nnet.ipv6.conf.all.forwarding = 1\nnet.ipv6.conf.default.forwarding = 1\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "podman".to_string(),
        ModuleDetail {
            service_state: "timer".to_string(),
            config_path: "/etc/sysctl.d/99-debopti-podman.conf".to_string(),
            config_exists: true,
            ports: vec!["rootless".to_string()],
            kv_facts: vec![
                ("运行模式".to_string(), "无根容器 (rootless)".to_string()),
                ("运行用户".to_string(), "apps".to_string()),
                ("清理周期".to_string(), "每周 (timer)".to_string()),
                ("权限范围".to_string(), "0..2147483647".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "net.ipv4.ping_group_range = 0 2147483647\nnet.ipv4.ip_unprivileged_port_start = 80\nfs.inotify.max_user_watches = 524288\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "xray".to_string(),
        ModuleDetail {
            service_state: "active".to_string(),
            config_path: "/usr/local/etc/xray/config.json".to_string(),
            config_exists: true,
            ports: vec!["443/tcp".to_string()],
            kv_facts: vec![
                ("核心引擎".to_string(), "xray (/usr/local/bin/xray)".to_string()),
                ("传输协议".to_string(), "reality/vless".to_string()),
                ("监听端口".to_string(), "443/tcp".to_string()),
                ("服务单元".to_string(), "xray.service (systemd)".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "\"log\": { \"loglevel\": \"warning\" },\n\"inbounds\": [{ \"port\": 443, \"protocol\": \"vless\" }],\n\"streamSettings\": { \"network\": \"tcp\", \"security\": \"reality\" }\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "ferron".to_string(),
        ModuleDetail {
            service_state: "active".to_string(),
            config_path: "/etc/ferron/config.kdl".to_string(),
            config_exists: true,
            ports: vec!["80/tcp".to_string(), "443/tcp".to_string()],
            kv_facts: vec![
                ("服务引擎".to_string(), "ferron (/usr/bin/ferron)".to_string()),
                ("TLS 证书".to_string(), "自动 ACME".to_string()),
                ("证书目录".to_string(), "/etc/ferron/certs".to_string()),
                ("监听端口".to_string(), "80/tcp, 443/tcp".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "edge.example.com {\n  proxy \"http://127.0.0.1:8080\"\n  tls \"/etc/ferron/certs/edge.crt\" \"/etc/ferron/certs/edge.key\"\n}\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "caddy".to_string(),
        ModuleDetail {
            service_state: "configured".to_string(),
            config_path: "/etc/caddy/Caddyfile".to_string(),
            config_exists: true,
            ports: vec!["80/tcp".to_string(), "443/tcp".to_string()],
            kv_facts: vec![
                ("服务类型".to_string(), "HTTPS 反向代理 (Caddy)".to_string()),
                (
                    "程序状态".to_string(),
                    "已安装 (/usr/bin/caddy)".to_string(),
                ),
                ("服务单元".to_string(), "caddy.service".to_string()),
                ("监听端口".to_string(), "80/tcp, 443/tcp".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "edge.example.com {\n  reverse_proxy 127.0.0.1:8080\n  encode zstd gzip\n}\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "derper".to_string(),
        ModuleDetail {
            service_state: "configured".to_string(),
            config_path: "/etc/systemd/system/derper.service".to_string(),
            config_exists: true,
            ports: vec!["8443/tcp".to_string(), "3478/udp".to_string()],
            kv_facts: vec![
                ("服务类型".to_string(), "Tailscale DERP 中继".to_string()),
                ("程序状态".to_string(), "已安装 (/opt/derper/derper)".to_string()),
                ("服务单元".to_string(), "derper.service".to_string()),
                ("中继端口".to_string(), "8443/tcp, 3478/udp".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "[Service]\nExecStart=/opt/derper/derper -a :8443 -stun-port 3478\nRestart=on-failure\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "realm".to_string(),
        ModuleDetail {
            service_state: "configured".to_string(),
            config_path: "/etc/realm/config.toml".to_string(),
            config_exists: true,
            ports: vec!["10000/tcp".to_string()],
            kv_facts: vec![
                ("服务类型".to_string(), "高性能端口转发 (Realm)".to_string()),
                ("程序状态".to_string(), "已安装 (/usr/local/bin/realm)".to_string()),
                ("服务单元".to_string(), "realm.service".to_string()),
                ("配置摘要".to_string(), "TOML 转发规则已就绪".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "[network]\nno_tcp = false\nuse_udp = true\n[[endpoints]]\nlisten = \"0.0.0.0:10000\"\nremote = \"127.0.0.1:10001\"\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "warp".to_string(),
        ModuleDetail {
            service_state: "active".to_string(),
            config_path: "/etc/systemd/system/warp-svc.service.d/override.conf".to_string(),
            config_exists: true,
            ports: vec!["40000/tcp".to_string()],
            kv_facts: vec![
                ("服务类型".to_string(), "Cloudflare WARP 出口".to_string()),
                (
                    "程序状态".to_string(),
                    "已安装 (/usr/bin/warp-cli)".to_string(),
                ),
                ("服务单元".to_string(), "warp-svc.service".to_string()),
                (
                    "代理端口".to_string(),
                    "40000/tcp (本地 SOCKS5)".to_string(),
                ),
            ],
            config_excerpt: sanitize_config_excerpt(
                "[Service]\nProtectSystem=full\nPrivateTmp=true\nNoNewPrivileges=true\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "usque".to_string(),
        ModuleDetail {
            service_state: "configured".to_string(),
            config_path: "/opt/usque/config.json".to_string(),
            config_exists: true,
            ports: vec!["1080/tcp".to_string()],
            kv_facts: vec![
                (
                    "服务类型".to_string(),
                    "MASQUE 轻量代理 (Usque)".to_string(),
                ),
                (
                    "程序状态".to_string(),
                    "已安装 (/opt/usque/usque)".to_string(),
                ),
                ("服务单元".to_string(), "usque.service".to_string()),
                ("本地监听".to_string(), "1080/tcp (SOCKS5)".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "{\n  \"socks_listen\": \"127.0.0.1:1080\",\n  \"mtu\": 1280\n}\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "easytier".to_string(),
        ModuleDetail {
            service_state: "active".to_string(),
            config_path: "/etc/easytier/config.toml".to_string(),
            config_exists: true,
            ports: vec!["11010/udp".to_string()],
            kv_facts: vec![
                ("服务类型".to_string(), "去中心化 Mesh 组网".to_string()),
                ("程序状态".to_string(), "已安装 (/usr/local/bin/easytier-core)".to_string()),
                ("服务单元".to_string(), "easytier@.service".to_string()),
                ("组网端口".to_string(), "11010/udp (P2P)".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "instance_name = \"mesh-edge\"\nipv4 = \"10.144.0.1/24\"\nlisteners = [\"udp://0.0.0.0:11010\"]\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "tailscale".to_string(),
        ModuleDetail {
            service_state: "active".to_string(),
            config_path: "/etc/default/tailscaled".to_string(),
            config_exists: true,
            ports: vec!["41641/udp".to_string()],
            kv_facts: vec![
                ("组网服务".to_string(), "tailscaled".to_string()),
                (
                    "程序状态".to_string(),
                    "已安装 (/usr/bin/tailscale)".to_string(),
                ),
                ("服务单元".to_string(), "tailscaled.service".to_string()),
                ("UDP 端口".to_string(), "41641/udp".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "PORT=\"41641\"\nFLAGS=\"--tun=userspace-networking\"\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "freship".to_string(),
        ModuleDetail {
            service_state: "timer".to_string(),
            config_path: "/etc/freship/freship.conf".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                (
                    "服务类型".to_string(),
                    "Cloudflare 动态域名 (DDNS)".to_string(),
                ),
                (
                    "程序状态".to_string(),
                    "已部署 (/opt/freship/core/freship_core.sh)".to_string(),
                ),
                ("定时任务".to_string(), "freship-ipv4.timer".to_string()),
                ("配置摘要".to_string(), "已配置域名与同步策略".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "DOMAIN=\"edge.example.com\"\nTARGET_CC=\"HK\"\nIP_VERSION=\"ipv4\"\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "fish".to_string(),
        ModuleDetail {
            service_state: "cli".to_string(),
            config_path: "/etc/fish/conf.d/debopty.fish".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                (
                    "工具类型".to_string(),
                    "交互终端 (Fish + Starship)".to_string(),
                ),
                ("程序状态".to_string(), "已安装 (/usr/bin/fish)".to_string()),
                ("SOT 同步".to_string(), "已启用多用户同步".to_string()),
                (
                    "配置摘要".to_string(),
                    "含 debopty.fish / starship".to_string(),
                ),
            ],
            config_excerpt: sanitize_config_excerpt(
                "set -g fish_greeting \"\"\nstarship init fish | source\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "micro".to_string(),
        ModuleDetail {
            service_state: "cli".to_string(),
            config_path: "/etc/profile.d/micro_env.sh".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                ("工具类型".to_string(), "现代终端编辑器 (Micro)".to_string()),
                (
                    "程序状态".to_string(),
                    "已安装 (/usr/local/bin/micro)".to_string(),
                ),
                ("剪贴板".to_string(), "已配置 OSC52 / 环境变量".to_string()),
                (
                    "插件集成".to_string(),
                    "omni / session 会话保持".to_string(),
                ),
            ],
            config_excerpt: sanitize_config_excerpt(
                "export EDITOR=micro\nexport VISUAL=micro\nexport MICRO_TRUECOLOR=1\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "yazi".to_string(),
        ModuleDetail {
            service_state: "cli".to_string(),
            config_path: "/etc/profile.d/yazi_wrapper.sh".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                ("工具类型".to_string(), "终端文件管理器 (Yazi)".to_string()),
                ("程序状态".to_string(), "已安装 (/usr/local/bin/yazi)".to_string()),
                ("Shell 联动".to_string(), "已注入 y 快捷退出定位".to_string()),
                ("依赖工具".to_string(), "ffmpeg / 7z / jq / fd / rg".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "y() {\n  local tmp=\"$(mktemp -t yazi-cwd.XXXXXX)\"\n  yazi \"$@\" --cwd-file=\"$tmp\"\n}\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "tmux".to_string(),
        ModuleDetail {
            service_state: "cli".to_string(),
            config_path: "/etc/tmux.conf".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                ("工具类型".to_string(), "终端会话复用 (Tmux)".to_string()),
                ("程序状态".to_string(), "已安装 (/usr/bin/tmux)".to_string()),
                ("配色终端".to_string(), "tmux-256color + 鼠标支持".to_string()),
                ("配置摘要".to_string(), "全局快捷键与状态栏".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "set -g mouse on\nset -g default-terminal \"tmux-256color\"\nset -g history-limit 20000\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "lego".to_string(),
        ModuleDetail {
            service_state: "timer".to_string(),
            config_path: "/etc/systemd/system/debopti-lego-renew.timer".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                (
                    "工具类型".to_string(),
                    "ACME 泛域名证书签发 (Lego)".to_string(),
                ),
                (
                    "程序状态".to_string(),
                    "已安装 (/usr/local/bin/lego)".to_string(),
                ),
                (
                    "续期定时器".to_string(),
                    "已启用 (debopti-lego-renew.timer)".to_string(),
                ),
                ("证书目录".to_string(), "/etc/ferron/certs".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "[Timer]\nOnCalendar=daily\nRandomizedDelaySec=1h\nPersistent=true\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "rust".to_string(),
        ModuleDetail {
            service_state: "cli".to_string(),
            config_path: "/root/.cargo/config.toml".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                (
                    "工具类型".to_string(),
                    "Rust 工具链 (rustup + cargo)".to_string(),
                ),
                (
                    "程序状态".to_string(),
                    "已安装 (/root/.cargo/bin/rustc)".to_string(),
                ),
                ("镜像加速".to_string(), "已配置 Cargo 镜像/环境".to_string()),
                (
                    "工具组件".to_string(),
                    "rustc / cargo / clippy / rustfmt".to_string(),
                ),
            ],
            config_excerpt: sanitize_config_excerpt(
                "[source.crates-io]\nreplace-with = \"rsproxy-sparse\"\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "go".to_string(),
        ModuleDetail {
            service_state: "cli".to_string(),
            config_path: "/etc/profile.d/golang.sh".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                ("工具类型".to_string(), "Go 语言开发工具链".to_string()),
                (
                    "程序状态".to_string(),
                    "已安装 (/usr/local/go/bin/go)".to_string(),
                ),
                (
                    "环境变量".to_string(),
                    "已写入 /etc/profile.d/golang.sh".to_string(),
                ),
                ("安装目录".to_string(), "/usr/local/go".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "export GOROOT=/usr/local/go\nexport PATH=\"$GOROOT/bin:$HOME/go/bin:$PATH\"\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "devops".to_string(),
        ModuleDetail {
            service_state: "cli".to_string(),
            config_path: "/etc/tmux.conf".to_string(),
            config_exists: true,
            ports: Vec::new(),
            kv_facts: vec![
                ("交互终端".to_string(), "fish + starship".to_string()),
                ("编辑器".to_string(), "micro + yazi".to_string()),
                ("终端复用".to_string(), "tmux".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "set -g mouse on\nset -g default-terminal \"tmux-256color\"\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "reconcile".to_string(),
        ModuleDetail {
            service_state: "configured".to_string(),
            config_path: "/etc/nftables.conf".to_string(),
            config_exists: true,
            ports: vec![
                "80/tcp".to_string(),
                "443/tcp".to_string(),
                "2222/tcp".to_string(),
                "41641/udp".to_string(),
            ],
            kv_facts: vec![
                ("SSH 端口".to_string(), "2222".to_string()),
                ("密码登录".to_string(), "已禁用".to_string()),
                ("防火墙".to_string(), "nftables".to_string()),
                ("放行端口".to_string(), "4 个".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "table inet filter {\n  chain input { type filter hook input priority 0; policy drop; }\n  tcp dport { 80, 443, 2222 } accept\n}\n",
                6,
                54,
            ),
        },
    );
    details.insert(
        "f2b_sync".to_string(),
        ModuleDetail {
            service_state: "timer".to_string(),
            config_path: "/etc/fail2ban/jail.d/99-debopty.local".to_string(),
            config_exists: true,
            ports: vec![
                "80/tcp".to_string(),
                "443/tcp".to_string(),
                "2222/tcp".to_string(),
            ],
            kv_facts: vec![
                ("日志后端".to_string(), "systemd".to_string()),
                ("SSH 端口".to_string(), "2222".to_string()),
                ("同步定时器".to_string(), "已启用".to_string()),
                ("同步方式".to_string(), "路径监听 + 定时".to_string()),
            ],
            config_excerpt: sanitize_config_excerpt(
                "[DEFAULT]\nbackend = systemd\nbanaction = nftables-multiport\n[sshd]\nenabled = true\nport = 2222\n",
                6,
                54,
            ),
        },
    );

    SystemSpecs {
        hostname: "deb-edge-01".to_string(),
        os_name: "Debian 12".to_string(),
        arch: "x86_64".to_string(),
        kernel: "6.12.9-amd64".to_string(),
        cpu_model: "EPYC 7763".to_string(),
        cpu_cores: 4,
        mem_total_kib: 4 * 1024 * 1024,
        swap_total_kib: 2 * 1024 * 1024,
        disk_root_used_gib_x10: Some(184),
        disk_root_total_gib_x10: Some(500),
        tcp_cc: "bbr".to_string(),
        default_qdisc: "fq".to_string(),
        ipv4_forward: true,
        ipv6_forward: true,
        file_max: 1_048_576,
        swappiness: Some(10),
        ssh_port: 2222,
        ssh_password_auth: Some(false),
        nftables_configured: true,
        fw_open_ports: vec![80, 443, 2222, 41641],
        fail2ban_configured: true,
        fail2ban_backend: "systemd".to_string(),
        module_details: details,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuJiffies {
    pub busy: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetBytes {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct TelemetryCollector {
    root_prefix: PathBuf,
    prev_cpu: Option<CpuJiffies>,
    prev_net: Option<NetBytes>,
    prev_instant: Option<Instant>,
}

impl TelemetryCollector {
    pub fn new(root_prefix: impl Into<PathBuf>) -> Self {
        Self {
            root_prefix: root_prefix.into(),
            prev_cpu: None,
            prev_net: None,
            prev_instant: None,
        }
    }

    pub fn sample_now(&mut self) -> LiveSample {
        let now = Instant::now();
        let dt_secs = self
            .prev_instant
            .map(|prev| now.saturating_duration_since(prev).as_secs_f64())
            .unwrap_or(0.0);
        self.prev_instant = Some(now);
        self.sample_internal(dt_secs)
    }

    pub fn sample_with_elapsed(&mut self, dt_secs: f64) -> LiveSample {
        self.prev_instant = Some(Instant::now());
        self.sample_internal(dt_secs)
    }

    fn sample_internal(&mut self, dt_secs: f64) -> LiveSample {
        let mut sample = LiveSample::default();

        if let Ok(stat_txt) = fs::read_to_string(under_root(&self.root_prefix, "proc/stat"))
            && let Some(cur_cpu) = parse_proc_stat_cpu(&stat_txt)
        {
            sample.cpu_pct = self
                .prev_cpu
                .map(|prev| compute_cpu_pct(prev, cur_cpu))
                .unwrap_or(0.0);
            self.prev_cpu = Some(cur_cpu);
        }

        if let Ok(load_txt) = fs::read_to_string(under_root(&self.root_prefix, "proc/loadavg")) {
            let (l1, l5, l15, running, total) = parse_proc_loadavg(&load_txt);
            sample.load1 = l1;
            sample.load5 = l5;
            sample.load15 = l15;
            sample.procs_running = running;
            sample.procs_total = total;
        }

        if let Ok(mem_txt) = fs::read_to_string(under_root(&self.root_prefix, "proc/meminfo")) {
            let (mem_used, mem_total, swap_used, swap_total) = parse_proc_meminfo(&mem_txt);
            sample.mem_used_kib = mem_used;
            sample.mem_total_kib = mem_total;
            sample.swap_used_kib = swap_used;
            sample.swap_total_kib = swap_total;
            sample.mem_pct = if mem_total > 0 {
                ((mem_used as f64 / mem_total as f64) * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
        }

        if let Ok(net_txt) = fs::read_to_string(under_root(&self.root_prefix, "proc/net/dev")) {
            let cur_net = parse_proc_net_dev(&net_txt);
            if dt_secs > 0.0
                && dt_secs.is_finite()
                && let Some(prev) = self.prev_net
            {
                if cur_net.rx_bytes >= prev.rx_bytes {
                    sample.net_rx_bps = (cur_net.rx_bytes - prev.rx_bytes) as f64 / dt_secs;
                }
                if cur_net.tx_bytes >= prev.tx_bytes {
                    sample.net_tx_bps = (cur_net.tx_bytes - prev.tx_bytes) as f64 / dt_secs;
                }
            }
            self.prev_net = Some(cur_net);
        }

        if let Ok(up_txt) = fs::read_to_string(under_root(&self.root_prefix, "proc/uptime")) {
            sample.uptime_secs = parse_proc_uptime(&up_txt);
        }

        sample
    }
}

fn under_root(root: &Path, rel: &str) -> PathBuf {
    let clean = rel.trim_start_matches('/');
    if root.as_os_str().is_empty() || root == Path::new("/") {
        PathBuf::from("/").join(clean)
    } else {
        root.join(clean)
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn probe_system_specs(root_prefix: &Path) -> SystemSpecs {
    probe_system_specs_with_overrides(root_prefix, None, None)
}

pub fn probe_system_specs_with_overrides(
    root_prefix: &Path,
    arch_override: Option<&str>,
    disk_override: Option<(u32, u32)>,
) -> SystemSpecs {
    let hostname = read_trimmed(&under_root(root_prefix, "proc/sys/kernel/hostname"))
        .or_else(|| read_trimmed(&under_root(root_prefix, "etc/hostname")))
        .unwrap_or_else(|| "debian".to_string());

    let os_name = probe_os_name(root_prefix);
    let arch = arch_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());

    let kernel =
        read_trimmed(&under_root(root_prefix, "proc/sys/kernel/osrelease")).unwrap_or_default();

    let (cpu_model, cpu_cores) = probe_cpu_info(root_prefix);

    let (_, mem_total_kib, _, swap_total_kib) =
        fs::read_to_string(under_root(root_prefix, "proc/meminfo"))
            .map(|txt| parse_proc_meminfo(&txt))
            .unwrap_or((0, 0, 0, 0));

    let (disk_root_used_gib_x10, disk_root_total_gib_x10) = match disk_override {
        Some((used_x10, total_x10)) => (Some(used_x10), Some(total_x10)),
        None => probe_disk_usage_gib_x10(root_prefix),
    };

    let tcp_cc = read_trimmed(&under_root(
        root_prefix,
        "proc/sys/net/ipv4/tcp_congestion_control",
    ))
    .unwrap_or_default();
    let default_qdisc = read_trimmed(&under_root(root_prefix, "proc/sys/net/core/default_qdisc"))
        .unwrap_or_default();
    let ipv4_forward = read_trimmed(&under_root(root_prefix, "proc/sys/net/ipv4/ip_forward"))
        .is_some_and(|v| v == "1");
    let ipv6_forward = read_trimmed(&under_root(
        root_prefix,
        "proc/sys/net/ipv6/conf/all/forwarding",
    ))
    .is_some_and(|v| v == "1");
    let file_max = read_trimmed(&under_root(root_prefix, "proc/sys/fs/file-max"))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let swappiness = read_trimmed(&under_root(root_prefix, "proc/sys/vm/swappiness"))
        .and_then(|v| v.parse::<u32>().ok());

    let ssh_port = detect_ssh_port_from_configs(root_prefix, true, &NoopRunner);
    let ssh_password_auth = probe_ssh_password_auth(root_prefix);
    let (nftables_configured, fw_open_ports) = probe_nftables(root_prefix);
    let (fail2ban_configured, fail2ban_backend) = probe_fail2ban(root_prefix);

    let mut specs = SystemSpecs {
        hostname,
        os_name,
        arch,
        kernel,
        cpu_model,
        cpu_cores,
        mem_total_kib,
        swap_total_kib,
        disk_root_used_gib_x10,
        disk_root_total_gib_x10,
        tcp_cc,
        default_qdisc,
        ipv4_forward,
        ipv6_forward,
        file_max,
        swappiness,
        ssh_port,
        ssh_password_auth,
        nftables_configured,
        fw_open_ports,
        fail2ban_configured,
        fail2ban_backend,
        module_details: BTreeMap::new(),
    };

    specs.module_details = probe_module_details(root_prefix, &specs);
    specs
}

fn ubuntu_patch_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b([0-9]+\.[0-9]+)\.[0-9]+\b").expect("valid ubuntu version regex")
    })
}

fn paren_codename_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s*\([^)]*\)").expect("valid codename regex"))
}

fn compact_os_name(raw: &str) -> String {
    let no_gnu = raw.replace("GNU/Linux ", "");
    let no_paren = paren_codename_re().replace_all(&no_gnu, "");
    let no_lts = no_paren
        .trim()
        .strip_suffix(" LTS")
        .unwrap_or(no_paren.trim())
        .trim();
    let compacted = ubuntu_patch_re().replace(no_lts, "$1");
    let collapsed: Vec<&str> = compacted.split_whitespace().collect();
    if collapsed.is_empty() {
        "Debian".to_string()
    } else {
        collapsed.join(" ")
    }
}

fn probe_os_name(root_prefix: &Path) -> String {
    let os_release = under_root(root_prefix, "etc/os-release");
    let Ok(content) = fs::read_to_string(&os_release) else {
        return "Debian".to_string();
    };
    let mut pretty: Option<String> = None;
    let mut name: Option<String> = None;
    let mut version_id: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("PRETTY_NAME=") {
            pretty = Some(val.trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = trimmed.strip_prefix("NAME=") {
            name = Some(val.trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = trimmed.strip_prefix("VERSION_ID=") {
            version_id = Some(val.trim_matches('"').trim_matches('\'').to_string());
        }
    }

    if let Some(p) = pretty.filter(|s| !s.is_empty()) {
        return compact_os_name(&p);
    }
    match (name, version_id) {
        (Some(n), Some(v)) if !n.is_empty() => compact_os_name(&format!("{n} {v}")),
        (Some(n), None) if !n.is_empty() => compact_os_name(&n),
        _ => "Debian".to_string(),
    }
}

fn probe_cpu_info(root_prefix: &Path) -> (String, u32) {
    let Ok(content) = fs::read_to_string(under_root(root_prefix, "proc/cpuinfo")) else {
        return (String::new(), 1);
    };
    let mut cores: u32 = 0;
    let mut raw_model: Option<String> = None;

    for line in content.lines() {
        if let Some((key, val)) = line.split_once(':') {
            let k = key.trim();
            let v = val.trim();
            if k == "processor" {
                cores = cores.saturating_add(1);
            } else if raw_model.is_none()
                && matches!(k, "model name" | "Model" | "Hardware")
                && !v.is_empty()
            {
                raw_model = Some(v.to_string());
            }
        }
    }

    let model = raw_model.map(|m| shorten_cpu_model(&m)).unwrap_or_default();
    (model, cores.max(1))
}

fn probe_disk_usage_gib_x10(root_prefix: &Path) -> (Option<u32>, Option<u32>) {
    let target = if root_prefix.as_os_str().is_empty() {
        Path::new("/")
    } else {
        root_prefix
    };
    let Ok(c_path) = CString::new(target.as_os_str().as_bytes()) else {
        return (None, None);
    };

    // SAFETY: `stat` is zero-initialized and populated by POSIX `statvfs` for a valid NUL-terminated path.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
    if rc != 0 {
        return (None, None);
    }

    #[allow(clippy::unnecessary_cast)]
    let frsize = if st.f_frsize > 0 {
        st.f_frsize as u64
    } else {
        st.f_bsize as u64
    };
    #[allow(clippy::unnecessary_cast)]
    let blocks = st.f_blocks as u64;
    #[allow(clippy::unnecessary_cast)]
    let bfree = st.f_bfree as u64;

    let total_bytes = blocks.saturating_mul(frsize);
    if total_bytes == 0 {
        return (None, None);
    }
    let used_bytes = total_bytes.saturating_sub(bfree.saturating_mul(frsize));
    let gib = 1024.0 * 1024.0 * 1024.0;
    let used_x10 = ((used_bytes as f64 / gib) * 10.0).round() as u32;
    let total_x10 = ((total_bytes as f64 / gib) * 10.0).round() as u32;
    (Some(used_x10), Some(total_x10))
}

fn sshd_match_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^[ \t]*match[ \t]+").expect("valid sshd match regex"))
}

fn sshd_pass_auth_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^[ \t]*passwordauthentication(?:[ \t]*=[ \t]*|[ \t]+)(yes|no)\b")
            .expect("valid sshd password auth regex")
    })
}

fn parse_sshd_password_auth_file(content: &str) -> Option<bool> {
    for line in content.lines() {
        let code = line.split('#').next().unwrap_or(line);
        if sshd_match_re().is_match(code) {
            break;
        }
        if let Some(caps) = sshd_pass_auth_re().captures(code) {
            return Some(caps[1].eq_ignore_ascii_case("yes"));
        }
    }
    None
}

fn probe_ssh_password_auth(root_prefix: &Path) -> Option<bool> {
    let conf_d = under_root(root_prefix, "etc/ssh/sshd_config.d");
    let mut dropins: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&conf_d) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|e| e == "conf") {
                dropins.push(p);
            }
        }
    }
    dropins.sort();

    for path in dropins {
        if let Ok(content) = fs::read_to_string(&path)
            && let Some(val) = parse_sshd_password_auth_file(&content)
        {
            return Some(val);
        }
    }

    if let Ok(content) = fs::read_to_string(under_root(root_prefix, "etc/ssh/sshd_config"))
        && let Some(val) = parse_sshd_password_auth_file(&content)
    {
        return Some(val);
    }

    None
}

fn nft_dport_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:tcp|udp)[ \t]+dport[ \t]+(\{[^}]*\}|[0-9]+)")
            .expect("valid nft dport regex")
    })
}

fn collect_nft_ports(content: &str, out: &mut BTreeSet<u16>) {
    for line in content.lines() {
        let code = line.split('#').next().unwrap_or(line);
        for caps in nft_dport_re().captures_iter(code) {
            let expr = caps[1].replace(['{', '}'], "");
            for item in expr.split(',') {
                if let Ok(port) = item.trim().parse::<u16>()
                    && port >= 1
                {
                    out.insert(port);
                }
            }
        }
    }
}

fn probe_nftables(root_prefix: &Path) -> (bool, Vec<u16>) {
    let mut configured = false;
    let mut ports = BTreeSet::new();

    let nft_conf = under_root(root_prefix, "etc/nftables.conf");
    if nft_conf.is_file() {
        configured = true;
        if let Ok(txt) = fs::read_to_string(&nft_conf) {
            collect_nft_ports(&txt, &mut ports);
        }
    }

    let nft_d = under_root(root_prefix, "etc/nftables.d");
    if nft_d.is_dir() {
        let mut files: Vec<PathBuf> = WalkDir::new(&nft_d)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "nft"))
            .map(|e| e.into_path())
            .collect();
        files.sort();
        if !files.is_empty() {
            configured = true;
        }
        for path in files {
            if let Ok(txt) = fs::read_to_string(&path) {
                collect_nft_ports(&txt, &mut ports);
            }
        }
    }

    (configured, ports.into_iter().collect())
}

fn parse_fail2ban_backend(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let code = trimmed
            .split('#')
            .next()
            .unwrap_or(trimmed)
            .split(';')
            .next()
            .unwrap_or(trimmed);
        if let Some((k, v)) = code.split_once('=')
            && k.trim().eq_ignore_ascii_case("backend")
        {
            let val = v.trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn probe_fail2ban(root_prefix: &Path) -> (bool, String) {
    let debopty_jail = under_root(root_prefix, "etc/fail2ban/jail.d/99-debopty.local");
    let ports_jail = under_root(root_prefix, "etc/fail2ban/jail.d/99-debopty-ports.local");
    let jail_local = under_root(root_prefix, "etc/fail2ban/jail.local");

    let configured = debopty_jail.is_file() || ports_jail.is_file() || jail_local.is_file();
    let backend = [&debopty_jail, &jail_local]
        .iter()
        .find_map(|p| {
            fs::read_to_string(p)
                .ok()
                .and_then(|c| parse_fail2ban_backend(&c))
        })
        .unwrap_or_default();

    (configured, backend)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

fn line_has_sensitive_keyword(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    SENSITIVE_LINE_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

pub fn sanitize_config_excerpt(raw: &str, max_lines: usize, max_chars: usize) -> Vec<String> {
    if max_lines == 0 || max_chars == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("//")
            || trimmed.starts_with(';')
        {
            continue;
        }
        if line_has_sensitive_keyword(trimmed) {
            continue;
        }
        let redacted = redact_sensitive(trimmed);
        if redacted.contains("[REDACTED_PRIVATE_KEY]") {
            continue;
        }
        out.push(truncate_chars(&redacted, max_chars));
        if out.len() >= max_lines {
            break;
        }
    }
    out
}

fn first_existing_config(root_prefix: &Path, candidates: &[&str]) -> (String, bool, Vec<String>) {
    for &abs in candidates {
        let path = under_root(root_prefix, abs);
        if path.is_file() {
            let excerpt = fs::read_to_string(&path)
                .map(|raw| sanitize_config_excerpt(&raw, 6, 54))
                .unwrap_or_default();
            return (abs.to_string(), true, excerpt);
        }
    }
    let fallback = candidates.first().copied().unwrap_or("").to_string();
    (fallback, false, Vec::new())
}

fn any_file_exists(root_prefix: &Path, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|abs| under_root(root_prefix, abs).is_file())
}

fn first_existing_file<'a>(root_prefix: &Path, candidates: &'a [&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .copied()
        .find(|abs| under_root(root_prefix, abs).is_file())
}

fn probe_module_details(root_prefix: &Path, specs: &SystemSpecs) -> BTreeMap<String, ModuleDetail> {
    let mut details = BTreeMap::new();

    // base
    {
        let (config_path, config_exists, config_excerpt) = first_existing_config(
            root_prefix,
            &[
                "/etc/sysctl.d/99-debopti-optimize.conf",
                "/etc/security/limits.d/99-debopti-nofile.conf",
            ],
        );
        let state = if config_exists {
            "configured"
        } else {
            "uninstalled"
        };
        let swappiness_str = specs
            .swappiness
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".to_string());
        details.insert(
            "base".to_string(),
            ModuleDetail {
                service_state: state.to_string(),
                config_path,
                config_exists,
                ports: Vec::new(),
                kv_facts: vec![
                    (
                        "拥塞算法".to_string(),
                        if specs.tcp_cc.is_empty() {
                            "—".to_string()
                        } else {
                            specs.tcp_cc.clone()
                        },
                    ),
                    (
                        "队列调度".to_string(),
                        if specs.default_qdisc.is_empty() {
                            "—".to_string()
                        } else {
                            specs.default_qdisc.clone()
                        },
                    ),
                    ("文件句柄".to_string(), format_count_compact(specs.file_max)),
                    ("交换倾向".to_string(), swappiness_str),
                ],
                config_excerpt,
            },
        );
    }

    // ip_forward
    {
        let (config_path, config_exists, config_excerpt) =
            first_existing_config(root_prefix, &["/etc/sysctl.d/99-debopti-forwarding.conf"]);
        let state = if specs.ipv4_forward || specs.ipv6_forward {
            "active"
        } else if config_exists {
            "configured"
        } else {
            "uninstalled"
        };
        details.insert(
            "ip_forward".to_string(),
            ModuleDetail {
                service_state: state.to_string(),
                config_path,
                config_exists,
                ports: Vec::new(),
                kv_facts: vec![
                    (
                        "IPv4 转发".to_string(),
                        if specs.ipv4_forward {
                            "已开启"
                        } else {
                            "已关闭"
                        }
                        .to_string(),
                    ),
                    (
                        "IPv6 转发".to_string(),
                        if specs.ipv6_forward {
                            "已开启"
                        } else {
                            "已关闭"
                        }
                        .to_string(),
                    ),
                    (
                        "配置状态".to_string(),
                        if config_exists {
                            "已持久化"
                        } else {
                            "未写入"
                        }
                        .to_string(),
                    ),
                ],
                config_excerpt,
            },
        );
    }

    // reconcile
    {
        let (config_path, config_exists, config_excerpt) = first_existing_config(
            root_prefix,
            &[
                "/etc/nftables.conf",
                "/etc/ssh/sshd_config.d/00-debopty-hardening.conf",
                "/etc/ssh/sshd_config",
            ],
        );
        let state = if specs.nftables_configured || specs.ssh_password_auth.is_some() {
            "configured"
        } else {
            "uninstalled"
        };
        let ports: Vec<String> = specs
            .fw_open_ports
            .iter()
            .take(4)
            .map(|p| format!("{p}/tcp"))
            .collect();
        let pass_auth = match specs.ssh_password_auth {
            Some(true) => "已开启",
            Some(false) => "已禁用",
            None => "系统默认",
        };
        details.insert(
            "reconcile".to_string(),
            ModuleDetail {
                service_state: state.to_string(),
                config_path,
                config_exists,
                ports,
                kv_facts: vec![
                    ("SSH 端口".to_string(), specs.ssh_port.to_string()),
                    ("密码登录".to_string(), pass_auth.to_string()),
                    (
                        "防火墙".to_string(),
                        if specs.nftables_configured {
                            "nftables"
                        } else {
                            "未配置"
                        }
                        .to_string(),
                    ),
                    (
                        "放行端口".to_string(),
                        if specs.fw_open_ports.is_empty() {
                            "无".to_string()
                        } else {
                            format!("{} 个", specs.fw_open_ports.len())
                        },
                    ),
                ],
                config_excerpt,
            },
        );
    }

    // f2b_sync
    {
        let (config_path, config_exists, config_excerpt) = first_existing_config(
            root_prefix,
            &[
                "/etc/fail2ban/jail.d/99-debopty-ports.local",
                "/etc/fail2ban/jail.d/99-debopty.local",
                "/etc/fail2ban/jail.local",
            ],
        );
        let timer_exists = any_file_exists(
            root_prefix,
            &["/etc/systemd/system/debopty-f2b-port-sync.timer"],
        );
        let path_exists = any_file_exists(
            root_prefix,
            &["/etc/systemd/system/debopty-f2b-port-sync.path"],
        );
        let state = if timer_exists || path_exists {
            "timer"
        } else if specs.fail2ban_configured || config_exists {
            "configured"
        } else {
            "uninstalled"
        };
        let ports: Vec<String> = specs
            .fw_open_ports
            .iter()
            .take(4)
            .map(|p| format!("{p}/tcp"))
            .collect();
        let sync_mode = match (path_exists, timer_exists) {
            (true, true) => "路径监听 + 定时",
            (true, false) => "路径监听",
            (false, true) => "定时巡检",
            (false, false) => "手动触发",
        };
        details.insert(
            "f2b_sync".to_string(),
            ModuleDetail {
                service_state: state.to_string(),
                config_path,
                config_exists,
                ports,
                kv_facts: vec![
                    (
                        "日志后端".to_string(),
                        if specs.fail2ban_backend.is_empty() {
                            "—".to_string()
                        } else {
                            specs.fail2ban_backend.clone()
                        },
                    ),
                    ("SSH 端口".to_string(), specs.ssh_port.to_string()),
                    (
                        "同步定时器".to_string(),
                        if timer_exists || path_exists {
                            "已启用"
                        } else {
                            "未启用"
                        }
                        .to_string(),
                    ),
                    ("同步方式".to_string(), sync_mode.to_string()),
                ],
                config_excerpt,
            },
        );
    }

    // podman
    {
        let (config_path, config_exists, config_excerpt) = first_existing_config(
            root_prefix,
            &[
                "/etc/sysctl.d/99-debopti-podman.conf",
                "/etc/sudoers.d/debopti-podman",
                "/etc/containers/containers.conf",
            ],
        );
        let bin_path =
            first_existing_file(root_prefix, &["/usr/bin/podman", "/usr/local/bin/podman"]);
        let bin_present = bin_path.is_some();
        let timer_present = any_file_exists(
            root_prefix,
            &[
                "/etc/systemd/system/debopty-podman-gc.timer",
                "/etc/systemd/system/podman-auto-update.timer",
            ],
        );
        let state = if timer_present {
            "timer"
        } else if bin_present {
            "active"
        } else if config_exists {
            "configured"
        } else {
            "uninstalled"
        };
        details.insert(
            "podman".to_string(),
            ModuleDetail {
                service_state: state.to_string(),
                config_path,
                config_exists,
                ports: if bin_present {
                    vec!["rootless".to_string()]
                } else {
                    Vec::new()
                },
                kv_facts: vec![
                    ("运行模式".to_string(), "无根容器 (rootless)".to_string()),
                    (
                        "程序状态".to_string(),
                        match bin_path {
                            Some(p) => format!("已安装 ({p})"),
                            None => "未安装".to_string(),
                        },
                    ),
                    (
                        "清理周期".to_string(),
                        if timer_present {
                            "已启用 (timer)"
                        } else {
                            "未启用"
                        }
                        .to_string(),
                    ),
                    (
                        "权限范围".to_string(),
                        if config_exists {
                            "已配置 sysctl/sudo"
                        } else {
                            "未配置"
                        }
                        .to_string(),
                    ),
                ],
                config_excerpt,
            },
        );
    }

    // Network/Proxy services
    struct ServiceProbeSpec<'a> {
        slug: &'a str,
        configs: &'a [&'a str],
        bins: &'a [&'a str],
        units: &'a [&'a str],
        default_ports: &'a [&'a str],
        runtime_label: &'a str,
        unit_key: &'a str,
        extra_key: &'a str,
        extra_present: &'a str,
        extra_absent: &'a str,
    }

    let service_specs = [
        ServiceProbeSpec {
            slug: "xray",
            configs: &["/usr/local/etc/xray/config.json", "/etc/xray/config.json"],
            bins: &["/usr/local/bin/xray", "/opt/xray/xray", "/usr/bin/xray"],
            units: &[
                "/etc/systemd/system/xray.service",
                "/lib/systemd/system/xray.service",
                "/etc/systemd/system/debopti-xray-geodata.timer",
            ],
            default_ports: &["443/tcp"],
            runtime_label: "代理内核 (VLESS/Reality)",
            unit_key: "服务单元",
            extra_key: "传输协议",
            extra_present: "VLESS / Reality",
            extra_absent: "443/tcp (未配置)",
        },
        ServiceProbeSpec {
            slug: "ferron",
            configs: &[
                "/etc/ferron/config.kdl",
                "/etc/ferron/ferron.yaml",
                "/etc/ferron.kdl",
            ],
            bins: &["/usr/bin/ferron", "/usr/local/bin/ferron"],
            units: &[
                "/etc/systemd/system/ferron.service",
                "/lib/systemd/system/ferron.service",
            ],
            default_ports: &["80/tcp", "443/tcp"],
            runtime_label: "反向代理与静态托管",
            unit_key: "服务单元",
            extra_key: "TLS 证书",
            extra_present: "自动 ACME / 本地证书",
            extra_absent: "80/443 (未配置)",
        },
        ServiceProbeSpec {
            slug: "caddy",
            configs: &["/etc/caddy/Caddyfile"],
            bins: &["/usr/bin/caddy", "/usr/local/bin/caddy"],
            units: &[
                "/etc/systemd/system/caddy.service",
                "/lib/systemd/system/caddy.service",
            ],
            default_ports: &["80/tcp", "443/tcp"],
            runtime_label: "HTTPS 反向代理 (Caddy)",
            unit_key: "服务单元",
            extra_key: "监听端口",
            extra_present: "80/tcp, 443/tcp",
            extra_absent: "80/443 (待部署)",
        },
        ServiceProbeSpec {
            slug: "derper",
            configs: &[
                "/etc/systemd/system/derper.service",
                "/opt/derper/derper.conf",
            ],
            bins: &["/opt/derper/derper", "/usr/local/bin/derper"],
            units: &["/etc/systemd/system/derper.service"],
            default_ports: &["8443/tcp", "3478/udp"],
            runtime_label: "Tailscale DERP 中继",
            unit_key: "服务单元",
            extra_key: "中继端口",
            extra_present: "8443/tcp, 3478/udp",
            extra_absent: "8443/3478 (待部署)",
        },
        ServiceProbeSpec {
            slug: "realm",
            configs: &["/etc/realm/config.toml"],
            bins: &["/opt/realm/realm", "/usr/local/bin/realm"],
            units: &["/etc/systemd/system/realm.service"],
            default_ports: &[],
            runtime_label: "高性能端口转发 (Realm)",
            unit_key: "服务单元",
            extra_key: "配置摘要",
            extra_present: "TOML 转发规则已就绪",
            extra_absent: "未配置转发规则",
        },
        ServiceProbeSpec {
            slug: "warp",
            configs: &[
                "/etc/systemd/system/warp-svc.service.d/override.conf",
                "/var/lib/cloudflare-warp/conf.json",
            ],
            bins: &["/usr/bin/warp-cli", "/usr/local/bin/warp-cli"],
            units: &[
                "/etc/systemd/system/warp-svc.service",
                "/lib/systemd/system/warp-svc.service",
                "/etc/systemd/system/warp-svc.service.d/override.conf",
            ],
            default_ports: &["40000/tcp"],
            runtime_label: "Cloudflare WARP 出口",
            unit_key: "服务单元",
            extra_key: "代理端口",
            extra_present: "40000/tcp (本地 SOCKS5)",
            extra_absent: "40000/tcp (待注册)",
        },
        ServiceProbeSpec {
            slug: "usque",
            configs: &["/opt/usque/config.json", "/etc/usque/config.json"],
            bins: &["/opt/usque/usque", "/usr/local/bin/usque"],
            units: &["/etc/systemd/system/usque.service"],
            default_ports: &["1080/tcp"],
            runtime_label: "MASQUE 轻量代理 (Usque)",
            unit_key: "服务单元",
            extra_key: "本地监听",
            extra_present: "1080/tcp (SOCKS5)",
            extra_absent: "1080/tcp (待部署)",
        },
        ServiceProbeSpec {
            slug: "easytier",
            configs: &["/etc/easytier/config.toml", "/opt/easytier/config.toml"],
            bins: &[
                "/opt/easytier/easytier-core",
                "/usr/local/bin/easytier-core",
            ],
            units: &[
                "/etc/systemd/system/easytier@.service",
                "/etc/systemd/system/easytier.service",
            ],
            default_ports: &["11010/udp"],
            runtime_label: "去中心化 Mesh 组网",
            unit_key: "服务单元",
            extra_key: "组网端口",
            extra_present: "11010/udp (P2P)",
            extra_absent: "11010/udp (待组网)",
        },
        ServiceProbeSpec {
            slug: "tailscale",
            configs: &[
                "/etc/systemd/system/tailscaled.service.d/override.conf",
                "/etc/default/tailscaled",
            ],
            bins: &[
                "/usr/bin/tailscale",
                "/usr/sbin/tailscaled",
                "/usr/local/bin/tailscale",
            ],
            units: &[
                "/etc/systemd/system/tailscaled.service",
                "/lib/systemd/system/tailscaled.service",
                "/etc/systemd/system/tailscaled.service.d/override.conf",
            ],
            default_ports: &["41641/udp"],
            runtime_label: "WireGuard Mesh 组网",
            unit_key: "服务单元",
            extra_key: "UDP 端口",
            extra_present: "41641/udp",
            extra_absent: "41641/udp (待配置)",
        },
        ServiceProbeSpec {
            slug: "freship",
            configs: &["/etc/freship/freship.conf", "/etc/freship/config.env"],
            bins: &["/opt/freship/core/freship_core.sh"],
            units: &[
                "/etc/systemd/system/freship-ipv4.timer",
                "/etc/systemd/system/freship-ipv6.timer",
                "/etc/systemd/system/freship.timer",
            ],
            default_ports: &[],
            runtime_label: "Cloudflare 动态域名 (DDNS)",
            unit_key: "定时任务",
            extra_key: "配置摘要",
            extra_present: "已配置域名与同步策略",
            extra_absent: "未配置域名参数",
        },
    ];

    for spec in &service_specs {
        let (config_path, config_exists, config_excerpt) =
            first_existing_config(root_prefix, spec.configs);
        let bin_path = first_existing_file(root_prefix, spec.bins);
        let bin_present = bin_path.is_some();
        let unit_path = first_existing_file(root_prefix, spec.units);
        let state = if bin_present {
            "active"
        } else if config_exists {
            "configured"
        } else {
            "uninstalled"
        };
        let ports = if bin_present || config_exists {
            spec.default_ports
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            Vec::new()
        };
        let unit_val = match unit_path.and_then(|u| Path::new(u).file_name()) {
            Some(name) => format!("已部署 ({})", name.to_string_lossy()),
            None => "未部署".to_string(),
        };
        details.insert(
            spec.slug.to_string(),
            ModuleDetail {
                service_state: state.to_string(),
                config_path,
                config_exists,
                ports,
                kv_facts: vec![
                    ("服务类型".to_string(), spec.runtime_label.to_string()),
                    (
                        "程序状态".to_string(),
                        match bin_path {
                            Some(p) => format!("已安装 ({p})"),
                            None => "未安装".to_string(),
                        },
                    ),
                    (spec.unit_key.to_string(), unit_val),
                    (
                        spec.extra_key.to_string(),
                        if config_exists || bin_present {
                            spec.extra_present
                        } else {
                            spec.extra_absent
                        }
                        .to_string(),
                    ),
                ],
                config_excerpt,
            },
        );
    }

    // CLI / Timer / DevOps tools
    struct CliProbeSpec<'a> {
        slug: &'a str,
        configs: &'a [&'a str],
        bins: &'a [&'a str],
        timer_unit: Option<&'a str>,
        kind_label: &'a str,
        extra_key_1: &'a str,
        extra_val_1_present: &'a str,
        extra_val_1_absent: &'a str,
        extra_key_2: &'a str,
        extra_val_2_present: &'a str,
        extra_val_2_absent: &'a str,
    }

    let cli_specs = [
        CliProbeSpec {
            slug: "fish",
            configs: &[
                "/etc/fish/conf.d/debopty.fish",
                "/etc/fish/conf.d/yazi.fish",
                "/etc/starship.toml",
            ],
            bins: &["/usr/bin/fish", "/usr/local/bin/fish"],
            timer_unit: None,
            kind_label: "交互终端 (Fish + Starship)",
            extra_key_1: "SOT 同步",
            extra_val_1_present: "已启用多用户同步",
            extra_val_1_absent: "按需同步",
            extra_key_2: "配置摘要",
            extra_val_2_present: "含 debopty.fish / starship",
            extra_val_2_absent: "默认配置",
        },
        CliProbeSpec {
            slug: "micro",
            configs: &["/etc/profile.d/micro_env.sh"],
            bins: &["/usr/local/bin/micro", "/usr/bin/micro"],
            timer_unit: None,
            kind_label: "现代终端编辑器 (Micro)",
            extra_key_1: "剪贴板",
            extra_val_1_present: "已配置 OSC52 / 环境变量",
            extra_val_1_absent: "未配置环境变量",
            extra_key_2: "插件集成",
            extra_val_2_present: "omni / session 会话保持",
            extra_val_2_absent: "omni / session (待部署)",
        },
        CliProbeSpec {
            slug: "yazi",
            configs: &[
                "/etc/profile.d/yazi_wrapper.sh",
                "/etc/fish/conf.d/yazi.fish",
            ],
            bins: &["/usr/local/bin/yazi", "/usr/bin/yazi"],
            timer_unit: None,
            kind_label: "终端文件管理器 (Yazi)",
            extra_key_1: "Shell 联动",
            extra_val_1_present: "已注入 y 快捷退出定位",
            extra_val_1_absent: "未注入包装脚本",
            extra_key_2: "依赖工具",
            extra_val_2_present: "ffmpeg / 7z / jq / fd / rg",
            extra_val_2_absent: "ffmpeg / 7z / jq / fd / rg",
        },
        CliProbeSpec {
            slug: "tmux",
            configs: &["/etc/tmux.conf"],
            bins: &["/usr/bin/tmux", "/usr/local/bin/tmux"],
            timer_unit: None,
            kind_label: "终端会话复用 (Tmux)",
            extra_key_1: "配色终端",
            extra_val_1_present: "tmux-256color + 鼠标支持",
            extra_val_1_absent: "未配置 /etc/tmux.conf",
            extra_key_2: "配置摘要",
            extra_val_2_present: "全局快捷键与状态栏",
            extra_val_2_absent: "使用默认配置",
        },
        CliProbeSpec {
            slug: "lego",
            configs: &[
                "/etc/systemd/system/debopti-lego-renew.timer",
                "/etc/systemd/system/debopti-lego-renew.service",
            ],
            bins: &["/usr/local/bin/lego", "/usr/bin/lego"],
            timer_unit: Some("/etc/systemd/system/debopti-lego-renew.timer"),
            kind_label: "ACME 泛域名证书签发 (Lego)",
            extra_key_1: "续期定时器",
            extra_val_1_present: "已启用 (debopti-lego-renew.timer)",
            extra_val_1_absent: "未启用",
            extra_key_2: "证书目录",
            extra_val_2_present: "/etc/ferron/certs",
            extra_val_2_absent: "/etc/ferron/certs",
        },
        CliProbeSpec {
            slug: "rust",
            configs: &["/root/.cargo/config.toml", "/etc/profile.d/rust.sh"],
            bins: &[
                "/root/.cargo/bin/rustc",
                "/usr/local/bin/rustc",
                "/usr/bin/rustc",
            ],
            timer_unit: None,
            kind_label: "Rust 工具链 (rustup + cargo)",
            extra_key_1: "镜像加速",
            extra_val_1_present: "已配置 Cargo 镜像/环境",
            extra_val_1_absent: "默认 crates.io",
            extra_key_2: "工具组件",
            extra_val_2_present: "rustc / cargo / clippy / rustfmt",
            extra_val_2_absent: "rustc / cargo / clippy / rustfmt",
        },
        CliProbeSpec {
            slug: "go",
            configs: &["/etc/profile.d/golang.sh"],
            bins: &["/usr/local/go/bin/go", "/usr/bin/go"],
            timer_unit: None,
            kind_label: "Go 语言开发工具链",
            extra_key_1: "环境变量",
            extra_val_1_present: "已写入 /etc/profile.d/golang.sh",
            extra_val_1_absent: "未配置 GOPATH/PATH",
            extra_key_2: "安装目录",
            extra_val_2_present: "/usr/local/go",
            extra_val_2_absent: "/usr/local/go",
        },
    ];

    let sot_users_exists = any_file_exists(root_prefix, &["/etc/debopti/sot_known_users"]);

    for spec in &cli_specs {
        let (config_path, config_exists, config_excerpt) =
            first_existing_config(root_prefix, spec.configs);
        let bin_path = first_existing_file(root_prefix, spec.bins);
        let bin_present = bin_path.is_some();
        let has_timer = spec
            .timer_unit
            .is_some_and(|u| under_root(root_prefix, u).is_file());
        let state = if has_timer {
            "timer"
        } else if bin_present {
            "cli"
        } else if config_exists {
            "configured"
        } else {
            "uninstalled"
        };
        let val1 = if spec.slug == "fish" {
            if sot_users_exists {
                spec.extra_val_1_present
            } else {
                spec.extra_val_1_absent
            }
        } else if spec.timer_unit.is_some() {
            if has_timer {
                spec.extra_val_1_present
            } else {
                spec.extra_val_1_absent
            }
        } else if config_exists {
            spec.extra_val_1_present
        } else {
            spec.extra_val_1_absent
        };
        let val2 = if config_exists || bin_present {
            spec.extra_val_2_present
        } else {
            spec.extra_val_2_absent
        };
        details.insert(
            spec.slug.to_string(),
            ModuleDetail {
                service_state: state.to_string(),
                config_path,
                config_exists,
                ports: Vec::new(),
                kv_facts: vec![
                    ("工具类型".to_string(), spec.kind_label.to_string()),
                    (
                        "程序状态".to_string(),
                        match bin_path {
                            Some(p) => format!("已安装 ({p})"),
                            None => "未安装".to_string(),
                        },
                    ),
                    (spec.extra_key_1.to_string(), val1.to_string()),
                    (spec.extra_key_2.to_string(), val2.to_string()),
                ],
                config_excerpt,
            },
        );
    }

    details
}

pub fn parse_proc_stat_cpu(content: &str) -> Option<CpuJiffies> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed
            .strip_prefix("cpu ")
            .or_else(|| trimmed.strip_prefix("cpu\t"))
        {
            let cols: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|tok| tok.parse::<u64>().ok())
                .collect();
            if cols.len() < 4 {
                return None;
            }
            let total: u64 = cols.iter().take(8).copied().sum();
            let idle = cols[3];
            let iowait = cols.get(4).copied().unwrap_or(0);
            let idle_all = idle.saturating_add(iowait);
            let busy = total.saturating_sub(idle_all);
            return Some(CpuJiffies { busy, total });
        }
    }
    None
}

pub fn compute_cpu_pct(prev: CpuJiffies, cur: CpuJiffies) -> f64 {
    if cur.total <= prev.total || cur.busy < prev.busy {
        return 0.0;
    }
    let d_total = cur.total - prev.total;
    let d_busy = cur.busy - prev.busy;
    ((d_busy as f64 / d_total as f64) * 100.0).clamp(0.0, 100.0)
}

pub fn parse_proc_loadavg(content: &str) -> (f64, f64, f64, u32, u32) {
    let mut parts = content.split_whitespace();
    let l1 = parts
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0);
    let l5 = parts
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0);
    let l15 = parts
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(0.0);
    let (running, total) = parts
        .next()
        .and_then(|s| s.split_once('/'))
        .map(|(r, t)| (r.parse::<u32>().unwrap_or(0), t.parse::<u32>().unwrap_or(0)))
        .unwrap_or((0, 0));
    (l1, l5, l15, running, total)
}

pub fn parse_proc_meminfo(content: &str) -> (u64, u64, u64, u64) {
    let mut mem_total: u64 = 0;
    let mut mem_free: u64 = 0;
    let mut mem_available: Option<u64> = None;
    let mut buffers: u64 = 0;
    let mut cached: u64 = 0;
    let mut swap_total: u64 = 0;
    let mut swap_free: u64 = 0;

    for line in content.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(val) = rest
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        match key.trim() {
            "MemTotal" => mem_total = val,
            "MemFree" => mem_free = val,
            "MemAvailable" => mem_available = Some(val),
            "Buffers" => buffers = val,
            "Cached" => cached = val,
            "SwapTotal" => swap_total = val,
            "SwapFree" => swap_free = val,
            _ => {}
        }
    }

    let avail =
        mem_available.unwrap_or_else(|| mem_free.saturating_add(buffers).saturating_add(cached));
    let mem_used = mem_total.saturating_sub(avail);
    let swap_used = swap_total.saturating_sub(swap_free);
    (mem_used, mem_total, swap_used, swap_total)
}

pub fn parse_proc_net_dev(content: &str) -> NetBytes {
    let mut rx_total: u64 = 0;
    let mut tx_total: u64 = 0;

    for line in content.lines() {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" || iface.trim().is_empty() {
            continue;
        }
        let cols: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();
        if cols.len() >= 9 {
            rx_total = rx_total.saturating_add(cols[0]);
            tx_total = tx_total.saturating_add(cols[8]);
        }
    }

    NetBytes {
        rx_bytes: rx_total,
        tx_bytes: tx_total,
    }
}

pub fn parse_proc_uptime(content: &str) -> u64 {
    content
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v.floor() as u64)
        .unwrap_or(0)
}

pub fn format_braille_curve(
    samples: &VecDeque<f64>,
    width_cells: usize,
    min_v: f64,
    max_v: f64,
) -> String {
    if width_cells == 0 {
        return String::new();
    }

    let slots_cap = width_cells * 2;
    let mut slots: Vec<Option<f64>> = vec![None; slots_cap];
    let present = samples.len().min(slots_cap);
    let skip = samples.len().saturating_sub(slots_cap);
    let pad = slots_cap - present;

    for (idx, &val) in samples.iter().skip(skip).enumerate() {
        slots[pad + idx] = Some(val);
    }

    let span = max_v - min_v;
    let level_for = |v: f64| -> usize {
        if !v.is_finite() || !span.is_finite() || span <= 0.0 {
            return 0;
        }
        let norm = ((v - min_v) / span).clamp(0.0, 1.0);
        (norm * 3.0).round() as usize
    };

    let mut out = String::with_capacity(width_cells * 3);
    for cell in 0..width_cells {
        let mut mask: u8 = 0;
        if let Some(left_val) = slots[cell * 2] {
            let lvl = level_for(left_val);
            for dot in &BRAILLE_LEFT_DOTS[..=lvl] {
                mask |= *dot;
            }
        }
        if let Some(right_val) = slots[cell * 2 + 1] {
            let lvl = level_for(right_val);
            for dot in &BRAILLE_RIGHT_DOTS[..=lvl] {
                mask |= *dot;
            }
        }
        let ch = char::from_u32(BRAILLE_BASE + u32::from(mask)).unwrap_or('\u{2800}');
        out.push(ch);
    }
    out
}

pub fn format_bytes_rate(bps: f64) -> String {
    if !bps.is_finite() || bps <= 0.0 {
        return "0 B/s".to_string();
    }
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    if bps < KIB {
        format!("{:.0} B/s", bps)
    } else if bps < MIB {
        format!("{:.1} KB/s", bps / KIB)
    } else if bps < GIB {
        format!("{:.1} MB/s", bps / MIB)
    } else {
        format!("{:.2} GB/s", bps / GIB)
    }
}

pub fn format_kib_compact(kib: u64) -> String {
    if kib == 0 {
        return "0K".to_string();
    }
    if kib < 1024 {
        return format!("{kib}K");
    }
    let mib = kib as f64 / 1024.0;
    if mib < 1024.0 {
        return format!("{:.0}M", mib);
    }
    let gib = mib / 1024.0;
    if gib < 1024.0 {
        return format!("{:.1}G", gib);
    }
    format!("{:.1}T", gib / 1024.0)
}

pub fn format_uptime_compact(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    if secs < 86_400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            return format!("{h}h{m}m");
        }
        return format!("{h}h");
    }
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    if h > 0 {
        format!("{d}d{h}h")
    } else {
        format!("{d}d")
    }
}

pub fn format_count_compact(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        return format!("{:.1}K", n as f64 / 1_000.0);
    }
    if n < 1_000_000_000 {
        return format!("{:.1}M", n as f64 / 1_000_000.0);
    }
    format!("{:.1}B", n as f64 / 1_000_000_000.0)
}

fn cpu_cores_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b[0-9]+-core\b").expect("valid cpu cores regex"))
}

fn cpu_freq_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\s*@\s*[0-9]+(?:\.[0-9]+)?\s*[GM]Hz").expect("valid cpu freq regex")
    })
}

pub fn shorten_cpu_model(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let cleaned = trimmed
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace("(C)", "")
        .replace("(r)", "")
        .replace("(tm)", "");
    let no_freq = cpu_freq_re().replace_all(&cleaned, "");
    let no_cores = cpu_cores_re().replace_all(&no_freq, "");

    let tokens: Vec<&str> = no_cores
        .split_whitespace()
        .filter(|tok| !matches!(tok.to_ascii_lowercase().as_str(), "cpu" | "processor"))
        .collect();

    let mut joined = tokens.join(" ");
    for prefix in ["AMD ", "Intel "] {
        if let Some(rest) = joined.strip_prefix(prefix)
            && !rest.is_empty()
        {
            joined = rest.to_string();
            break;
        }
    }

    if joined.is_empty() {
        trimmed.to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn kv_facts_use_chinese_labels_and_preserve_service_states() {
        let (demo, _) = TelemetryHistory::demo_fixture(0);
        let allowed_states = ["active", "configured", "timer", "cli", "uninstalled"];
        let all_22_slugs = [
            "base",
            "ip_forward",
            "reconcile",
            "f2b_sync",
            "podman",
            "xray",
            "ferron",
            "caddy",
            "derper",
            "realm",
            "warp",
            "usque",
            "easytier",
            "tailscale",
            "freship",
            "fish",
            "micro",
            "yazi",
            "tmux",
            "lego",
            "rust",
            "go",
        ];
        let forbidden_labels = [
            "TCP CC",
            "Qdisc",
            "file-max",
            "Swappiness",
            "IPv4 Fwd",
            "IPv6 Fwd",
            "GC Timer",
            "Ping GID",
            "PassAuth",
            "UDP Port",
            "Backend",
            "Binary",
            "Role",
            "Type",
        ];

        for slug in all_22_slugs {
            let d = demo
                .module_details
                .get(slug)
                .unwrap_or_else(|| panic!("demo_specs missing slug {slug}"));
            assert!(
                (3..=4).contains(&d.kv_facts.len()),
                "demo_specs {slug} expected 3-4 kv_facts, got {}",
                d.kv_facts.len()
            );
            assert!(
                !d.config_excerpt.is_empty() && d.config_excerpt.len() <= 6,
                "demo_specs {slug} expected 1-6 excerpt lines, got {}",
                d.config_excerpt.len()
            );
        }

        for detail in demo.module_details.values() {
            assert!(
                allowed_states.contains(&detail.service_state.as_str()),
                "unexpected service_state: {}",
                detail.service_state
            );
            for (k, _) in &detail.kv_facts {
                assert!(
                    !forbidden_labels.contains(&k.as_str()),
                    "cryptic label still present in demo_specs: {k}"
                );
            }
        }
        assert_eq!(demo.module_details["base"].kv_facts[0].0, "拥塞算法");
        assert_eq!(
            demo.module_details["reconcile"].kv_facts[1],
            ("密码登录".to_string(), "已禁用".to_string())
        );

        let tmp = TempDir::new().unwrap();
        let probed = probe_system_specs(tmp.path());
        for slug in all_22_slugs {
            let d = probed
                .module_details
                .get(slug)
                .unwrap_or_else(|| panic!("probe_system_specs missing slug {slug}"));
            assert!(
                (3..=4).contains(&d.kv_facts.len()),
                "probe_system_specs {slug} expected 3-4 kv_facts, got {}",
                d.kv_facts.len()
            );
        }
        for detail in probed.module_details.values() {
            assert!(
                allowed_states.contains(&detail.service_state.as_str()),
                "unexpected probed service_state: {}",
                detail.service_state
            );
            for (k, _) in &detail.kv_facts {
                assert!(
                    !forbidden_labels.contains(&k.as_str()),
                    "cryptic label still present in probe_system_specs: {k}"
                );
            }
        }
        assert_eq!(probed.module_details["xray"].kv_facts.len(), 4);
        assert_eq!(
            probed.module_details["xray"].kv_facts[0],
            (
                "服务类型".to_string(),
                "代理内核 (VLESS/Reality)".to_string()
            )
        );
        assert_eq!(probed.module_details["fish"].kv_facts.len(), 4);
        assert_eq!(
            probed.module_details["fish"].kv_facts[0],
            (
                "工具类型".to_string(),
                "交互终端 (Fish + Starship)".to_string()
            )
        );
    }
}
