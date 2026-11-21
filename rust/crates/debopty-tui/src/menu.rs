use debopty_bridge::{AppId, BashAction};
use debopty_core::status::StatusSnapshot;

/// Menu screens mirroring `scripts/tui.sh` 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuId {
    Main,
    Devops,
    Podman,
    RustStack,
    GoStack,
    WarpStack,
    Maintenance,
    AppCrud(AppId),
}

/// Visual status indicator shown on the right column of a menu row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBadge {
    Active(&'static str),
    Inactive(&'static str),
}

impl StatusBadge {
    pub fn plain_text(self) -> String {
        match self {
            Self::Active(label) => format!("● {label}"),
            Self::Inactive(label) => format!("○ {label}"),
        }
    }
}

/// Internal menu row action resolved by `AppState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    Navigate(MenuId),
    Execute(BashAction),
    ExecuteWithConfirm { action: BashAction, prompt: String },
    NativeF2bSync,
    NativeReconcile,
    Back,
    Exit,
}

/// A selectable menu item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: u8,
    pub icon: &'static str,
    pub label: &'static str,
    pub status: Option<StatusBadge>,
    pub action: MenuAction,
}

/// Either a selectable item or a visual divider line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

/// Complete rendered model of the active menu screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuScreen {
    pub id: MenuId,
    pub title: String,
    pub subtitle: String,
    pub entries: Vec<MenuEntry>,
}

impl MenuScreen {
    pub fn items(&self) -> Vec<&MenuItem> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    pub fn max_item_id(&self) -> u8 {
        self.items().iter().map(|item| item.id).max().unwrap_or(0)
    }
}

fn tool_installed(status: &StatusSnapshot, key: &str) -> bool {
    status.tools.get(key).copied().unwrap_or(false)
}

fn tool_badge(status: &StatusSnapshot, key: &str) -> StatusBadge {
    if tool_installed(status, key) {
        StatusBadge::Active("已就绪")
    } else {
        StatusBadge::Inactive("未部署")
    }
}

fn combined_badge(status: &StatusSnapshot, keys: &[&str]) -> StatusBadge {
    if keys.iter().any(|key| tool_installed(status, key)) {
        StatusBadge::Active("已就绪")
    } else {
        StatusBadge::Inactive("未部署")
    }
}

fn toggle_badge(enabled: bool) -> StatusBadge {
    if enabled {
        StatusBadge::Active("已开启")
    } else {
        StatusBadge::Inactive("已关闭")
    }
}

fn item(
    id: u8,
    icon: &'static str,
    label: &'static str,
    status: Option<StatusBadge>,
    action: MenuAction,
) -> MenuEntry {
    MenuEntry::Item(MenuItem {
        id,
        icon,
        label,
        status,
        action,
    })
}

/// Constructs a `MenuScreen` with exact 1:1 parity against `scripts/tui.sh`.
pub fn build_menu(menu: MenuId, status: &StatusSnapshot) -> MenuScreen {
    match menu {
        MenuId::Main => build_main_menu(status),
        MenuId::Devops => build_devops_menu(status),
        MenuId::Podman => build_podman_menu(status),
        MenuId::RustStack => build_rust_menu(status),
        MenuId::GoStack => build_go_menu(status),
        MenuId::WarpStack => build_warp_menu(status),
        MenuId::Maintenance => build_maintenance_menu(),
        MenuId::AppCrud(app) => build_app_crud_menu(app),
    }
}

fn build_main_menu(status: &StatusSnapshot) -> MenuScreen {
    let region = if status.is_cn_region {
        "中国大陆 (镜像加速)"
    } else {
        "海外地区 (直连模式)"
    };
    let subtitle = format!("{} | {}", status.version_display, region);

    let freship_action = if tool_installed(status, "freship") {
        MenuAction::Execute(BashAction::ManageFreship)
    } else {
        MenuAction::Execute(BashAction::InstallApp(AppId::Freship))
    };

    MenuScreen {
        id: MenuId::Main,
        title: "debopty".to_string(),
        subtitle,
        entries: vec![
            item(
                1,
                "⚡",
                "一键系统级基础优化",
                None,
                MenuAction::Execute(BashAction::RunBaseOptimization),
            ),
            item(
                2,
                "🌐",
                "路由转发模式控制",
                Some(toggle_badge(status.ip_forward)),
                MenuAction::Execute(BashAction::ToggleIpForwarding),
            ),
            MenuEntry::Separator,
            item(
                3,
                "🛠️",
                "运维与终端工具集",
                Some(combined_badge(
                    status,
                    &["fish", "micro", "yazi", "lego", "tmux"],
                )),
                MenuAction::Navigate(MenuId::Devops),
            ),
            item(
                4,
                "🐳",
                "Podman 容器环境 (Rootless)",
                Some(tool_badge(status, "podman")),
                MenuAction::Navigate(MenuId::Podman),
            ),
            item(
                5,
                "🦀",
                "Rust 环境与应用",
                Some(tool_badge(status, "rustc")),
                MenuAction::Navigate(MenuId::RustStack),
            ),
            item(
                6,
                "🐹",
                "GoLang 环境与应用",
                Some(tool_badge(status, "go")),
                MenuAction::Navigate(MenuId::GoStack),
            ),
            item(
                7,
                "🛰️",
                "Xray Core 节点管理",
                Some(tool_badge(status, "xray")),
                MenuAction::Execute(BashAction::ManageXray),
            ),
            item(
                8,
                "🚀",
                "WARP & Usque 代理栈",
                Some(combined_badge(status, &["warp-cli", "usque"])),
                MenuAction::Navigate(MenuId::WarpStack),
            ),
            item(
                9,
                "🔗",
                "Easytier 虚拟组网",
                Some(tool_badge(status, "easytier-core")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Easytier)),
            ),
            item(
                10,
                "🌊",
                "Tailscale 虚拟组网",
                Some(tool_badge(status, "tailscale")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Tailscale)),
            ),
            item(
                11,
                "🌱",
                "FreshIP IP 养护",
                Some(tool_badge(status, "freship")),
                freship_action,
            ),
            MenuEntry::Separator,
            item(
                12,
                "⚙️",
                "脚本维护 (更新/卸载)",
                None,
                MenuAction::Navigate(MenuId::Maintenance),
            ),
            item(0, "🔙", "退出脚本", None, MenuAction::Exit),
        ],
    }
}

fn build_devops_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::Devops,
        title: "运维与终端工具".to_string(),
        subtitle: "Main > DevOps Tools".to_string(),
        entries: vec![
            item(
                1,
                "🐟",
                "Fish Shell 现代化 Shell",
                Some(tool_badge(status, "fish")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Fish)),
            ),
            item(
                2,
                "📝",
                "Micro Editor 文本编辑器",
                Some(tool_badge(status, "micro")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Micro)),
            ),
            item(
                3,
                "🗂️",
                "Yazi 极速终端文件管理器",
                Some(tool_badge(status, "yazi")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Yazi)),
            ),
            item(
                4,
                "📜",
                "Lego 证书",
                Some(tool_badge(status, "lego")),
                MenuAction::Execute(BashAction::ManageLego),
            ),
            item(
                5,
                "🪟",
                "Tmux 终端复用器",
                Some(tool_badge(status, "tmux")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Tmux)),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回主菜单", None, MenuAction::Back),
        ],
    }
}

fn build_podman_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::Podman,
        title: "Podman 容器环境".to_string(),
        subtitle: "Main > Podman".to_string(),
        entries: vec![
            item(
                1,
                "✨",
                "安装 / 更新",
                None,
                MenuAction::Execute(BashAction::InstallApp(AppId::Podman)),
            ),
            item(
                2,
                "🗑️",
                "卸载并清理",
                None,
                MenuAction::ExecuteWithConfirm {
                    action: BashAction::UninstallApp(AppId::Podman),
                    prompt: "确定要移除 Podman 环境吗？[y/N]".to_string(),
                },
            ),
            item(
                3,
                "🧹",
                "镜像清理定时器",
                Some(toggle_badge(status.podman_gc_enabled)),
                MenuAction::Execute(BashAction::TogglePodmanGc),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回主菜单", None, MenuAction::Back),
        ],
    }
}

fn build_rust_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::RustStack,
        title: "Rust 环境与应用".to_string(),
        subtitle: "Main > Rust Stack".to_string(),
        entries: vec![
            item(
                1,
                "🦀",
                "Rust 语言环境 rustup",
                Some(tool_badge(status, "rustc")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::RustSdk)),
            ),
            item(
                2,
                "🔗",
                "Realm 转发服务器",
                Some(tool_badge(status, "realm")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Realm)),
            ),
            item(
                3,
                "🌐",
                "Ferron Web 服务器",
                Some(tool_badge(status, "ferron")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Ferron)),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回主菜单", None, MenuAction::Back),
        ],
    }
}

fn build_go_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::GoStack,
        title: "GoLang 环境与应用".to_string(),
        subtitle: "Main > Go Stack".to_string(),
        entries: vec![
            item(
                1,
                "🐹",
                "Go 语言环境 SDK",
                Some(tool_badge(status, "go")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::GoSdk)),
            ),
            item(
                2,
                "🌐",
                "Caddy 自编译插件版",
                Some(tool_badge(status, "caddy")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Caddy)),
            ),
            item(
                3,
                "📡",
                "DERP 隐身转发节点",
                Some(tool_badge(status, "derper")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Derper)),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回主菜单", None, MenuAction::Back),
        ],
    }
}

fn build_warp_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::WarpStack,
        title: "代理与出站管理".to_string(),
        subtitle: "Main > Proxy Stack".to_string(),
        entries: vec![
            item(
                1,
                "🚀",
                "CF WARP 官方客户端",
                Some(tool_badge(status, "warp-cli")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Warp)),
            ),
            item(
                2,
                "🛰️",
                "Usque MASQUE 协议客户端",
                Some(tool_badge(status, "usque")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Usque)),
            ),
            item(
                3,
                "📝",
                "生成 Xray WG 出站 JSON",
                None,
                MenuAction::Execute(BashAction::GenerateWarpXray),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回主菜单", None, MenuAction::Back),
        ],
    }
}

fn build_maintenance_menu() -> MenuScreen {
    MenuScreen {
        id: MenuId::Maintenance,
        title: "脚本维护".to_string(),
        subtitle: "Main > Maintenance".to_string(),
        entries: vec![
            item(
                1,
                "🆙",
                "检查并同步最新版本",
                None,
                MenuAction::Execute(BashAction::ScriptUpdate),
            ),
            item(
                2,
                "🗑️",
                "彻底卸载脚本及资产",
                None,
                MenuAction::Execute(BashAction::ScriptUninstall),
            ),
            item(
                3,
                "🩺",
                "系统冲突巡检与稳态接管 (Reconcile)",
                None,
                MenuAction::NativeReconcile,
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回主菜单", None, MenuAction::Back),
        ],
    }
}

fn build_app_crud_menu(app: AppId) -> MenuScreen {
    let name = app.display_name();
    MenuScreen {
        id: MenuId::AppCrud(app),
        title: format!("{name} 管理"),
        subtitle: format!("Main > {name}"),
        entries: vec![
            item(
                1,
                "✨",
                "安装 / 更新",
                None,
                MenuAction::Execute(BashAction::InstallApp(app)),
            ),
            item(
                2,
                "🗑️",
                "卸载并清理",
                None,
                MenuAction::ExecuteWithConfirm {
                    action: BashAction::UninstallApp(app),
                    prompt: format!("确定要移除 {name} 吗？[y/N]"),
                },
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回上级菜单", None, MenuAction::Back),
        ],
    }
}
