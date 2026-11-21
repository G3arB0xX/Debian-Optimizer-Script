use serde::{Deserialize, Serialize};

use crate::actions::ModuleImpl;

/// Logical category of a registered debopty module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleCategory {
    Core,
    Security,
    Tui,
    App,
}

/// Migration status entry for a single debopty module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleStatusEntry {
    pub id: String,
    pub category: ModuleCategory,
    pub implementation: ModuleImpl,
    pub source: String,
    pub summary: String,
}

impl ModuleStatusEntry {
    fn new(
        id: &str,
        category: ModuleCategory,
        implementation: ModuleImpl,
        source: &str,
        summary: &str,
    ) -> Self {
        Self {
            id: id.to_string(),
            category,
            implementation,
            source: source.to_string(),
            summary: summary.to_string(),
        }
    }
}

/// Registry describing which debopty modules run natively in Rust vs via `BashBridge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRegistry {
    entries: Vec<ModuleStatusEntry>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self {
            entries: module_registry(),
        }
    }

    pub fn entries(&self) -> &[ModuleStatusEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&ModuleStatusEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn native_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.implementation == ModuleImpl::NativeRust)
            .count()
    }

    pub fn bridge_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.implementation == ModuleImpl::BashBridge)
            .count()
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the authoritative Wave-3 module implementation registry for `debopty status --json`.
pub fn module_registry() -> Vec<ModuleStatusEntry> {
    vec![
        // Native Rust modules (Wave 1 + Wave 2 + Wave 3: 10 modules)
        ModuleStatusEntry::new(
            "version",
            ModuleCategory::Core,
            ModuleImpl::NativeRust,
            "debopty-core::version",
            "SemVer 2.0.0 & CalVer/Build 身份校验与解析",
        ),
        ModuleStatusEntry::new(
            "config",
            ModuleCategory::Core,
            ModuleImpl::NativeRust,
            "debopty-core::config",
            "Bash 兼容配置读写与原子持久化",
        ),
        ModuleStatusEntry::new(
            "template",
            ModuleCategory::Core,
            ModuleImpl::NativeRust,
            "debopty-core::template",
            "内嵌模板单一事实源渲染引擎",
        ),
        ModuleStatusEntry::new(
            "status",
            ModuleCategory::Core,
            ModuleImpl::NativeRust,
            "debopty-core::status",
            "系统状态与工具探测快照",
        ),
        ModuleStatusEntry::new(
            "f2b-sync",
            ModuleCategory::Security,
            ModuleImpl::NativeRust,
            "debopty-core::security::f2b_sync",
            "Fail2ban 与 nftables 公网端口自动同步",
        ),
        ModuleStatusEntry::new(
            "tui",
            ModuleCategory::Tui,
            ModuleImpl::NativeRust,
            "debopty-tui",
            "交互式终端菜单与 CJK 精确栅格渲染",
        ),
        ModuleStatusEntry::new(
            "network",
            ModuleCategory::Core,
            ModuleImpl::NativeRust,
            "debopty-core::network",
            "DNS 校准、地区识别与纯 Rust HTTP 镜像下载",
        ),
        ModuleStatusEntry::new(
            "system",
            ModuleCategory::Core,
            ModuleImpl::NativeRust,
            "debopty-core::system",
            "系统基础调优、路由转发与 Tmux 部署",
        ),
        ModuleStatusEntry::new(
            "security-ssh-nft",
            ModuleCategory::Security,
            ModuleImpl::NativeRust,
            "debopty-core::security::{ssh,firewall,fail2ban}",
            "SSH 安全加固、nftables 与 Fail2ban 初始化",
        ),
        ModuleStatusEntry::new(
            "reconcile",
            ModuleCategory::Core,
            ModuleImpl::NativeRust,
            "debopty-core::reconcile",
            "脏系统接管、9 域冲突巡检与无损稳态收敛",
        ),
        // Unported app modules (BashBridge, 11 modules)
        ModuleStatusEntry::new(
            "devops",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/devops.sh",
            "Fish / Micro / Yazi / Lego 运维工具集",
        ),
        ModuleStatusEntry::new(
            "easytier",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/easytier.sh",
            "Easytier 虚拟组网部署与卸载",
        ),
        ModuleStatusEntry::new(
            "ferron",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/ferron.sh",
            "Ferron Web 服务器部署与卸载",
        ),
        ModuleStatusEntry::new(
            "freship",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/freship.sh",
            "FreshIP IP 养护安装与管理",
        ),
        ModuleStatusEntry::new(
            "golang",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/golang.sh",
            "Go SDK / Caddy / DERP 服务栈",
        ),
        ModuleStatusEntry::new(
            "podman",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/podman.sh",
            "Podman Rootless 容器与镜像清理定时器",
        ),
        ModuleStatusEntry::new(
            "realm",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/realm.sh",
            "Realm 转发服务器部署与卸载",
        ),
        ModuleStatusEntry::new(
            "rust",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/rust.sh",
            "Rust rustup 工具链部署与卸载",
        ),
        ModuleStatusEntry::new(
            "tailscale",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/tailscale.sh",
            "Tailscale 虚拟组网部署与卸载",
        ),
        ModuleStatusEntry::new(
            "warp",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/warp.sh",
            "CF WARP / Usque 代理栈与 Xray WG 生成",
        ),
        ModuleStatusEntry::new(
            "xray",
            ModuleCategory::App,
            ModuleImpl::BashBridge,
            "scripts/apps/xray.sh",
            "Xray Core 节点安装与交互式管理",
        ),
    ]
}
