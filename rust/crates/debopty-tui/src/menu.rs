use debopty_bridge::{AppId, BashAction};
use debopty_core::status::StatusSnapshot;

/// Top-level modern Workspace Pill Tabs for `ratatui-ui` (U17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkspaceTab {
    #[default]
    Overview,
    SysSec,
    NetProxy,
    DevOps,
    Reconcile,
}

impl WorkspaceTab {
    pub const ALL: [Self; 5] = [
        Self::Overview,
        Self::SysSec,
        Self::NetProxy,
        Self::DevOps,
        Self::Reconcile,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "概览",
            Self::SysSec => "系统安全",
            Self::NetProxy => "网络代理",
            Self::DevOps => "运维容器",
            Self::Reconcile => "稳态维护",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Overview => Self::SysSec,
            Self::SysSec => Self::NetProxy,
            Self::NetProxy => Self::DevOps,
            Self::DevOps => Self::Reconcile,
            Self::Reconcile => Self::Overview,
        }
    }

    pub const fn prev(self) -> Self {
        match self {
            Self::Overview => Self::Reconcile,
            Self::SysSec => Self::Overview,
            Self::NetProxy => Self::SysSec,
            Self::DevOps => Self::NetProxy,
            Self::Reconcile => Self::DevOps,
        }
    }
}

/// Structured, zero-prose technical metadata rendered in the Live Action Inspector card (U17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionInspectorMeta {
    pub kind: &'static str,
    pub unit: &'static str,
    pub path: &'static str,
    pub ports: &'static str,
    pub cli: &'static str,
}

/// Single searchable command entry for the global fuzzy `Ctrl+K` Command Palette (U17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub domain: &'static str,
    pub label: &'static str,
    pub keywords: &'static str,
    pub status: Option<StatusBadge>,
    pub action: MenuAction,
}

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

impl MenuAction {
    /// Concise action category label for the Telemetry selected-item inspector.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Navigate(_) => "子菜单",
            Self::Execute(_) => "执行",
            Self::ExecuteWithConfirm { .. } => "需确认",
            Self::NativeF2bSync | Self::NativeReconcile => "维护",
            Self::Back => "返回上级",
            Self::Exit => "退出",
        }
    }

    /// Returns the `SystemSpecs::module_details` lookup slug for this action, if any.
    pub fn detail_key(&self) -> &'static str {
        match self {
            Self::Navigate(MenuId::Devops) => "fish",
            Self::Navigate(MenuId::Podman) => "podman",
            Self::Navigate(MenuId::RustStack) => "rust",
            Self::Navigate(MenuId::GoStack) => "go",
            Self::Navigate(MenuId::WarpStack) => "warp",
            Self::Navigate(MenuId::Maintenance) => "reconcile",
            Self::Navigate(MenuId::AppCrud(app)) => app_detail_key(*app),
            Self::Navigate(MenuId::Main) | Self::Back | Self::Exit => "",
            Self::Execute(action) | Self::ExecuteWithConfirm { action, .. } => {
                bash_action_detail_key(*action)
            }
            Self::NativeF2bSync => "f2b_sync",
            Self::NativeReconcile => "reconcile",
        }
    }

    /// Equivalent non-interactive CLI command hint for power users.
    pub fn cli_hint(&self) -> &'static str {
        self.inspector_meta().cli
    }

    /// Structured technical metadata for the U17 Live Action Inspector Bento card.
    pub fn inspector_meta(&self) -> ActionInspectorMeta {
        let kind = self.kind_label();
        match self {
            Self::Navigate(MenuId::Devops) => ActionInspectorMeta {
                kind,
                unit: "fish · yazi · tmux",
                path: "/etc/profile.d/ · ~/.config/",
                ports: "—",
                cli: "debopty app install <fish|micro|yazi|tmux|lego>",
            },
            Self::Navigate(MenuId::Podman) => ActionInspectorMeta {
                kind,
                unit: "podman · gc.timer",
                path: "/home/apps/srv · /etc/containers/",
                ports: "rootless",
                cli: "debopty app install podman",
            },
            Self::Navigate(MenuId::RustStack) => ActionInspectorMeta {
                kind,
                unit: "realm · ferron",
                path: "/etc/realm/ · /etc/ferron/",
                ports: "80,443/tcp",
                cli: "debopty app install <rust|realm|ferron>",
            },
            Self::Navigate(MenuId::GoStack) => ActionInspectorMeta {
                kind,
                unit: "caddy · derper",
                path: "/etc/caddy/ · /opt/derper/",
                ports: "80,443/tcp · 3478/udp",
                cli: "debopty app install <go|caddy|derper>",
            },
            Self::Navigate(MenuId::WarpStack) => ActionInspectorMeta {
                kind,
                unit: "warp-svc · usque",
                path: "/etc/usque/ · /var/lib/cloudflare-warp/",
                ports: "2408/udp · 443/udp",
                cli: "debopty app install <warp|usque>",
            },
            Self::Navigate(MenuId::Maintenance) => ActionInspectorMeta {
                kind,
                unit: "f2b-sync · reconcile",
                path: "/opt/debopty/bin/debopty",
                ports: "—",
                cli: "debopty reconcile --audit",
            },
            Self::Navigate(MenuId::AppCrud(app)) => app_inspector_meta(*app, false, kind),
            Self::Navigate(MenuId::Main) => ActionInspectorMeta {
                kind,
                unit: "debopty",
                path: "/etc/debopti/debopti.conf",
                ports: "—",
                cli: "debopty",
            },
            Self::Execute(action) | Self::ExecuteWithConfirm { action, .. } => {
                bash_action_inspector_meta(*action, kind)
            }
            Self::NativeF2bSync => ActionInspectorMeta {
                kind,
                unit: "nftables · fail2ban",
                path: "/etc/fail2ban/jail.d/99-debopty-ports.local",
                ports: "auto-sync",
                cli: "debopty f2b-sync",
            },
            Self::NativeReconcile => ActionInspectorMeta {
                kind,
                unit: "9-domain reconciler",
                path: "/etc/debopti/ · /etc/ssh/ · /etc/nftables*",
                ports: "non-destructive",
                cli: "debopty reconcile --audit",
            },
            Self::Back => ActionInspectorMeta {
                kind,
                unit: "navigation",
                path: "—",
                ports: "—",
                cli: "[Esc / 0]",
            },
            Self::Exit => ActionInspectorMeta {
                kind,
                unit: "session",
                path: "—",
                ports: "—",
                cli: "[q / Esc]",
            },
        }
    }
}

fn app_inspector_meta(app: AppId, uninstall: bool, kind: &'static str) -> ActionInspectorMeta {
    let cli = app_cli_hint(app, uninstall);
    match app {
        AppId::Fish => ActionInspectorMeta {
            kind,
            unit: "fish (login shell)",
            path: "/etc/fish/conf.d/debopty.fish",
            ports: "—",
            cli,
        },
        AppId::Micro => ActionInspectorMeta {
            kind,
            unit: "micro (editor)",
            path: "/usr/local/bin/micro",
            ports: "—",
            cli,
        },
        AppId::Yazi => ActionInspectorMeta {
            kind,
            unit: "yazi (file manager)",
            path: "/usr/local/bin/yazi",
            ports: "—",
            cli,
        },
        AppId::Tmux => ActionInspectorMeta {
            kind,
            unit: "tmux (multiplexer)",
            path: "/etc/tmux.conf",
            ports: "—",
            cli,
        },
        AppId::Podman => ActionInspectorMeta {
            kind,
            unit: "podman.socket (rootless)",
            path: "/home/apps/srv · /etc/containers/",
            ports: "rootless",
            cli,
        },
        AppId::RustSdk => ActionInspectorMeta {
            kind,
            unit: "rustup · cargo · rustc",
            path: "~/.cargo/bin/",
            ports: "—",
            cli,
        },
        AppId::Realm => ActionInspectorMeta {
            kind,
            unit: "realm.service",
            path: "/etc/realm/config.toml",
            ports: "tcp/udp relay",
            cli,
        },
        AppId::Ferron => ActionInspectorMeta {
            kind,
            unit: "ferron.service",
            path: "/etc/ferron/ferron.yaml",
            ports: "80,443/tcp",
            cli,
        },
        AppId::GoSdk => ActionInspectorMeta {
            kind,
            unit: "go toolchain",
            path: "/usr/local/go/bin/go",
            ports: "—",
            cli,
        },
        AppId::Caddy => ActionInspectorMeta {
            kind,
            unit: "caddy.service",
            path: "/etc/caddy/Caddyfile",
            ports: "80,443/tcp+udp",
            cli,
        },
        AppId::Derper => ActionInspectorMeta {
            kind,
            unit: "derper.service",
            path: "/opt/derper/",
            ports: "443/tcp · 3478/udp",
            cli,
        },
        AppId::Xray => ActionInspectorMeta {
            kind,
            unit: "xray.service",
            path: "/usr/local/etc/xray/config.json",
            ports: "443/tcp+udp",
            cli,
        },
        AppId::Warp => ActionInspectorMeta {
            kind,
            unit: "warp-svc.service",
            path: "/var/lib/cloudflare-warp/",
            ports: "2408/udp",
            cli,
        },
        AppId::Usque => ActionInspectorMeta {
            kind,
            unit: "usque.service (MASQUE)",
            path: "/etc/usque/config.json",
            ports: "443/udp",
            cli,
        },
        AppId::Easytier => ActionInspectorMeta {
            kind,
            unit: "easytier.service",
            path: "/opt/easytier/config.toml",
            ports: "11010-11015",
            cli,
        },
        AppId::Tailscale => ActionInspectorMeta {
            kind,
            unit: "tailscaled.service",
            path: "/var/lib/tailscale/",
            ports: "41641/udp",
            cli,
        },
        AppId::Freship => ActionInspectorMeta {
            kind,
            unit: "freship-core@.timer",
            path: "/opt/freship/config.env",
            ports: "v4/v6 outbound",
            cli,
        },
    }
}

fn bash_action_inspector_meta(action: BashAction, kind: &'static str) -> ActionInspectorMeta {
    let cli = bash_action_cli_hint(action);
    match action {
        BashAction::RunBaseOptimization => ActionInspectorMeta {
            kind,
            unit: "sysctl · sshd · nft",
            path: "/etc/sysctl.d/99-debopti-optimize.conf",
            ports: "SSH + nft",
            cli,
        },
        BashAction::ToggleIpForwarding => ActionInspectorMeta {
            kind,
            unit: "net.ipv4/v6 forward",
            path: "/etc/sysctl.d/99-debopti-forwarding.conf",
            ports: "L3 routing",
            cli,
        },
        BashAction::InstallApp(app) => app_inspector_meta(app, false, kind),
        BashAction::UninstallApp(app) => app_inspector_meta(app, true, kind),
        BashAction::TogglePodmanGc => ActionInspectorMeta {
            kind,
            unit: "podman-gc.timer (weekly)",
            path: "/etc/systemd/system/podman-gc.timer",
            ports: "—",
            cli,
        },
        BashAction::ManageXray => app_inspector_meta(AppId::Xray, false, kind),
        BashAction::GenerateWarpXray => ActionInspectorMeta {
            kind,
            unit: "warp-xray outbound",
            path: "stdout JSON (WireGuard)",
            ports: "2408/udp",
            cli,
        },
        BashAction::ManageFreship => app_inspector_meta(AppId::Freship, false, kind),
        BashAction::ManageLego => ActionInspectorMeta {
            kind,
            unit: "debopti-lego.timer",
            path: "/etc/debopti/lego/",
            ports: "ACME DNS-01",
            cli,
        },
        BashAction::ScriptUpdate => ActionInspectorMeta {
            kind,
            unit: "atomic SHA-256 self-update",
            path: "/opt/debopty/bin/debopty",
            ports: "HTTPS",
            cli,
        },
        BashAction::ScriptUninstall => ActionInspectorMeta {
            kind,
            unit: "purge /opt/debopty + symlinks",
            path: "/opt/debopty (keeps /etc/debopti)",
            ports: "—",
            cli,
        },
        BashAction::LegacyFullEntry => ActionInspectorMeta {
            kind,
            unit: "debopty",
            path: "/opt/debopty/bin/debopty",
            ports: "—",
            cli,
        },
    }
}

fn app_detail_key(app: AppId) -> &'static str {
    match app {
        AppId::Fish => "fish",
        AppId::Micro => "micro",
        AppId::Yazi => "yazi",
        AppId::Tmux => "tmux",
        AppId::Podman => "podman",
        AppId::RustSdk => "rust",
        AppId::Realm => "realm",
        AppId::Ferron => "ferron",
        AppId::GoSdk => "go",
        AppId::Caddy => "caddy",
        AppId::Derper => "derper",
        AppId::Xray => "xray",
        AppId::Warp => "warp",
        AppId::Usque => "usque",
        AppId::Easytier => "easytier",
        AppId::Tailscale => "tailscale",
        AppId::Freship => "freship",
    }
}

fn bash_action_detail_key(action: BashAction) -> &'static str {
    match action {
        BashAction::RunBaseOptimization => "base",
        BashAction::ToggleIpForwarding => "ip_forward",
        BashAction::InstallApp(app) | BashAction::UninstallApp(app) => app_detail_key(app),
        BashAction::TogglePodmanGc => "podman",
        BashAction::ManageXray => "xray",
        BashAction::GenerateWarpXray => "warp",
        BashAction::ManageFreship => "freship",
        BashAction::ManageLego => "lego",
        BashAction::ScriptUpdate | BashAction::ScriptUninstall | BashAction::LegacyFullEntry => {
            "reconcile"
        }
    }
}

fn app_cli_hint(app: AppId, uninstall: bool) -> &'static str {
    if uninstall {
        match app {
            AppId::Fish => "debopty app uninstall fish",
            AppId::Micro => "debopty app uninstall micro",
            AppId::Yazi => "debopty app uninstall yazi",
            AppId::Tmux => "debopty app uninstall tmux",
            AppId::Podman => "debopty app uninstall podman",
            AppId::RustSdk => "debopty app uninstall rust",
            AppId::Realm => "debopty app uninstall realm",
            AppId::Ferron => "debopty app uninstall ferron",
            AppId::GoSdk => "debopty app uninstall go",
            AppId::Caddy => "debopty app uninstall caddy",
            AppId::Derper => "debopty app uninstall derper",
            AppId::Xray => "debopty app uninstall xray",
            AppId::Warp => "debopty app uninstall warp",
            AppId::Usque => "debopty app uninstall usque",
            AppId::Easytier => "debopty app uninstall easytier",
            AppId::Tailscale => "debopty app uninstall tailscale",
            AppId::Freship => "debopty app uninstall freship",
        }
    } else {
        match app {
            AppId::Fish => "debopty app install fish",
            AppId::Micro => "debopty app install micro",
            AppId::Yazi => "debopty app install yazi",
            AppId::Tmux => "debopty app install tmux",
            AppId::Podman => "debopty app install podman",
            AppId::RustSdk => "debopty app install rust",
            AppId::Realm => "debopty app install realm",
            AppId::Ferron => "debopty app install ferron",
            AppId::GoSdk => "debopty app install go",
            AppId::Caddy => "debopty app install caddy",
            AppId::Derper => "debopty app install derper",
            AppId::Xray => "debopty app install xray",
            AppId::Warp => "debopty app install warp",
            AppId::Usque => "debopty app install usque",
            AppId::Easytier => "debopty app install easytier",
            AppId::Tailscale => "debopty app install tailscale",
            AppId::Freship => "debopty app install freship",
        }
    }
}

fn bash_action_cli_hint(action: BashAction) -> &'static str {
    match action {
        BashAction::RunBaseOptimization => "debopty system base",
        BashAction::ToggleIpForwarding => "debopty system ip-forward-toggle",
        BashAction::InstallApp(app) => app_cli_hint(app, false),
        BashAction::UninstallApp(app) => app_cli_hint(app, true),
        BashAction::TogglePodmanGc => "debopty app podman-gc-toggle",
        BashAction::ManageXray => "debopty app xray-status",
        BashAction::GenerateWarpXray => "debopty app warp-xray-gen",
        BashAction::ManageFreship => "debopty app freship-status",
        BashAction::ManageLego => "debopty app lego-list",
        BashAction::ScriptUpdate => "debopty self-update",
        BashAction::ScriptUninstall => "debopty self-uninstall",
        BashAction::LegacyFullEntry => "debopty",
    }
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

/// Either a selectable item, a labeled section divider, or a plain visual divider line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Section(&'static str),
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
                MenuEntry::Section(_) | MenuEntry::Separator => None,
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
        StatusBadge::Active("就绪")
    } else {
        StatusBadge::Inactive("未装")
    }
}

fn combined_badge(status: &StatusSnapshot, keys: &[&str]) -> StatusBadge {
    if keys.iter().any(|key| tool_installed(status, key)) {
        StatusBadge::Active("就绪")
    } else {
        StatusBadge::Inactive("未装")
    }
}

fn toggle_badge(enabled: bool) -> StatusBadge {
    if enabled {
        StatusBadge::Active("开启")
    } else {
        StatusBadge::Inactive("关闭")
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
        "CN · 镜像"
    } else {
        "海外 · 直连"
    };
    let subtitle = format!("{} │ {}", status.version_display, region);

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
            MenuEntry::Section("系统"),
            item(
                1,
                "⚡",
                "基础优化",
                Some(if status.base_optimized {
                    StatusBadge::Active("生效")
                } else {
                    StatusBadge::Inactive("未调")
                }),
                MenuAction::Execute(BashAction::RunBaseOptimization),
            ),
            item(
                2,
                "🌐",
                "路由转发",
                Some(toggle_badge(status.ip_forward)),
                MenuAction::Execute(BashAction::ToggleIpForwarding),
            ),
            MenuEntry::Section("应用"),
            item(
                3,
                "🔧",
                "运维工具",
                Some(combined_badge(
                    status,
                    &["fish", "micro", "yazi", "lego", "tmux"],
                )),
                MenuAction::Navigate(MenuId::Devops),
            ),
            item(
                4,
                "🐳",
                "Podman",
                Some(tool_badge(status, "podman")),
                MenuAction::Navigate(MenuId::Podman),
            ),
            item(
                5,
                "🦀",
                "Rust",
                Some(tool_badge(status, "rustc")),
                MenuAction::Navigate(MenuId::RustStack),
            ),
            item(
                6,
                "🐹",
                "Go",
                Some(tool_badge(status, "go")),
                MenuAction::Navigate(MenuId::GoStack),
            ),
            item(
                7,
                "📡",
                "Xray Core",
                Some(tool_badge(status, "xray")),
                MenuAction::Execute(BashAction::ManageXray),
            ),
            item(
                8,
                "🚀",
                "WARP / Usque",
                Some(combined_badge(status, &["warp-cli", "usque"])),
                MenuAction::Navigate(MenuId::WarpStack),
            ),
            item(
                9,
                "🔗",
                "Easytier",
                Some(tool_badge(status, "easytier-core")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Easytier)),
            ),
            item(
                10,
                "🌊",
                "Tailscale",
                Some(tool_badge(status, "tailscale")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Tailscale)),
            ),
            item(
                11,
                "🌱",
                "FreshIP",
                Some(tool_badge(status, "freship")),
                freship_action,
            ),
            MenuEntry::Section("维护"),
            item(
                12,
                "🧰",
                "维护管理",
                None,
                MenuAction::Navigate(MenuId::Maintenance),
            ),
            item(0, "🔙", "退出", None, MenuAction::Exit),
        ],
    }
}

fn build_devops_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::Devops,
        title: "运维工具".to_string(),
        subtitle: "Main › DevOps".to_string(),
        entries: vec![
            MenuEntry::Section("工具"),
            item(
                1,
                "🐟",
                "Fish",
                Some(tool_badge(status, "fish")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Fish)),
            ),
            item(
                2,
                "📝",
                "Micro",
                Some(tool_badge(status, "micro")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Micro)),
            ),
            item(
                3,
                "📁",
                "Yazi",
                Some(tool_badge(status, "yazi")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Yazi)),
            ),
            item(
                4,
                "📜",
                "Lego ACME",
                Some(tool_badge(status, "lego")),
                MenuAction::Execute(BashAction::ManageLego),
            ),
            item(
                5,
                "🪟",
                "Tmux",
                Some(tool_badge(status, "tmux")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Tmux)),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回", None, MenuAction::Back),
        ],
    }
}

fn build_podman_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::Podman,
        title: "Podman".to_string(),
        subtitle: "Main › Podman".to_string(),
        entries: vec![
            MenuEntry::Section("操作"),
            item(
                1,
                "✨",
                "安装 / 更新",
                None,
                MenuAction::Execute(BashAction::InstallApp(AppId::Podman)),
            ),
            item(
                2,
                "❌",
                "卸载",
                None,
                MenuAction::ExecuteWithConfirm {
                    action: BashAction::UninstallApp(AppId::Podman),
                    prompt: "移除 Podman 环境？ [y/N]".to_string(),
                },
            ),
            item(
                3,
                "🧹",
                "镜像 GC",
                Some(toggle_badge(status.podman_gc_enabled)),
                MenuAction::Execute(BashAction::TogglePodmanGc),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回", None, MenuAction::Back),
        ],
    }
}

fn build_rust_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::RustStack,
        title: "Rust".to_string(),
        subtitle: "Main › Rust".to_string(),
        entries: vec![
            MenuEntry::Section("组件"),
            item(
                1,
                "🦀",
                "Rustup",
                Some(tool_badge(status, "rustc")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::RustSdk)),
            ),
            item(
                2,
                "🔗",
                "Realm",
                Some(tool_badge(status, "realm")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Realm)),
            ),
            item(
                3,
                "🌐",
                "Ferron",
                Some(tool_badge(status, "ferron")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Ferron)),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回", None, MenuAction::Back),
        ],
    }
}

fn build_go_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::GoStack,
        title: "Go".to_string(),
        subtitle: "Main › Go".to_string(),
        entries: vec![
            MenuEntry::Section("组件"),
            item(
                1,
                "🐹",
                "Go SDK",
                Some(tool_badge(status, "go")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::GoSdk)),
            ),
            item(
                2,
                "🌐",
                "Caddy",
                Some(tool_badge(status, "caddy")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Caddy)),
            ),
            item(
                3,
                "📡",
                "DERPer",
                Some(tool_badge(status, "derper")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Derper)),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回", None, MenuAction::Back),
        ],
    }
}

fn build_warp_menu(status: &StatusSnapshot) -> MenuScreen {
    MenuScreen {
        id: MenuId::WarpStack,
        title: "WARP / Usque".to_string(),
        subtitle: "Main › Proxy".to_string(),
        entries: vec![
            MenuEntry::Section("代理"),
            item(
                1,
                "🚀",
                "WARP",
                Some(tool_badge(status, "warp-cli")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Warp)),
            ),
            item(
                2,
                "🛸",
                "Usque",
                Some(tool_badge(status, "usque")),
                MenuAction::Navigate(MenuId::AppCrud(AppId::Usque)),
            ),
            item(
                3,
                "📝",
                "导出 WG 出站",
                None,
                MenuAction::Execute(BashAction::GenerateWarpXray),
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回", None, MenuAction::Back),
        ],
    }
}

fn build_maintenance_menu() -> MenuScreen {
    MenuScreen {
        id: MenuId::Maintenance,
        title: "维护管理".to_string(),
        subtitle: "Main › Maintenance".to_string(),
        entries: vec![
            MenuEntry::Section("操作"),
            item(
                1,
                "🆙",
                "检查更新",
                None,
                MenuAction::Execute(BashAction::ScriptUpdate),
            ),
            item(
                2,
                "❌",
                "卸载 debopty",
                None,
                MenuAction::ExecuteWithConfirm {
                    action: BashAction::ScriptUninstall,
                    prompt: "卸载 debopty 及安装目录？ [y/N]".to_string(),
                },
            ),
            item(3, "🩺", "稳态巡检", None, MenuAction::NativeReconcile),
            item(4, "🔒", "Fail2ban 同步", None, MenuAction::NativeF2bSync),
            MenuEntry::Separator,
            item(0, "🔙", "返回", None, MenuAction::Back),
        ],
    }
}

fn build_app_crud_menu(app: AppId) -> MenuScreen {
    let name = app.display_name();
    MenuScreen {
        id: MenuId::AppCrud(app),
        title: name.to_string(),
        subtitle: format!("Main › {name}"),
        entries: vec![
            MenuEntry::Section("操作"),
            item(
                1,
                "✨",
                "安装 / 更新",
                None,
                MenuAction::Execute(BashAction::InstallApp(app)),
            ),
            item(
                2,
                "❌",
                "卸载",
                None,
                MenuAction::ExecuteWithConfirm {
                    action: BashAction::UninstallApp(app),
                    prompt: format!("移除 {name}？ [y/N]"),
                },
            ),
            MenuEntry::Separator,
            item(0, "🔙", "返回", None, MenuAction::Back),
        ],
    }
}

/// Constructs a direct-access workspace screen for top Workspace Pill Tabs (U17).
pub fn build_workspace_menu(tab: WorkspaceTab, status: &StatusSnapshot) -> MenuScreen {
    match tab {
        WorkspaceTab::Overview => build_main_menu(status),
        WorkspaceTab::SysSec => MenuScreen {
            id: MenuId::Main,
            title: "debopty".to_string(),
            subtitle: "Workspace › 系统安全".to_string(),
            entries: vec![
                MenuEntry::Section("系统调优"),
                item(
                    1,
                    "⚡",
                    "基础优化",
                    Some(if status.base_optimized {
                        StatusBadge::Active("生效")
                    } else {
                        StatusBadge::Inactive("未调")
                    }),
                    MenuAction::Execute(BashAction::RunBaseOptimization),
                ),
                item(
                    2,
                    "🌐",
                    "路由转发",
                    Some(toggle_badge(status.ip_forward)),
                    MenuAction::Execute(BashAction::ToggleIpForwarding),
                ),
                MenuEntry::Section("安全防御"),
                item(3, "🔒", "Fail2ban 同步", None, MenuAction::NativeF2bSync),
                item(4, "🩺", "稳态巡检", None, MenuAction::NativeReconcile),
                MenuEntry::Section("会话"),
                item(0, "🔙", "退出", None, MenuAction::Exit),
            ],
        },
        WorkspaceTab::NetProxy => {
            let freship_action = if tool_installed(status, "freship") {
                MenuAction::Execute(BashAction::ManageFreship)
            } else {
                MenuAction::Execute(BashAction::InstallApp(AppId::Freship))
            };
            MenuScreen {
                id: MenuId::Main,
                title: "debopty".to_string(),
                subtitle: "Workspace › 网络代理".to_string(),
                entries: vec![
                    MenuEntry::Section("代理栈"),
                    item(
                        1,
                        "📡",
                        "Xray Core",
                        Some(tool_badge(status, "xray")),
                        MenuAction::Execute(BashAction::ManageXray),
                    ),
                    item(
                        2,
                        "🚀",
                        "WARP",
                        Some(tool_badge(status, "warp-cli")),
                        MenuAction::Navigate(MenuId::AppCrud(AppId::Warp)),
                    ),
                    item(
                        3,
                        "🛸",
                        "Usque",
                        Some(tool_badge(status, "usque")),
                        MenuAction::Navigate(MenuId::AppCrud(AppId::Usque)),
                    ),
                    item(
                        4,
                        "📝",
                        "导出 WG 出站",
                        None,
                        MenuAction::Execute(BashAction::GenerateWarpXray),
                    ),
                    MenuEntry::Section("组网与服务"),
                    item(
                        5,
                        "🔗",
                        "Easytier",
                        Some(tool_badge(status, "easytier-core")),
                        MenuAction::Navigate(MenuId::AppCrud(AppId::Easytier)),
                    ),
                    item(
                        6,
                        "🌊",
                        "Tailscale",
                        Some(tool_badge(status, "tailscale")),
                        MenuAction::Navigate(MenuId::AppCrud(AppId::Tailscale)),
                    ),
                    item(
                        7,
                        "📡",
                        "DERPer",
                        Some(tool_badge(status, "derper")),
                        MenuAction::Navigate(MenuId::AppCrud(AppId::Derper)),
                    ),
                    item(
                        8,
                        "🌱",
                        "FreshIP",
                        Some(tool_badge(status, "freship")),
                        freship_action,
                    ),
                    MenuEntry::Section("会话"),
                    item(0, "🔙", "退出", None, MenuAction::Exit),
                ],
            }
        }
        WorkspaceTab::DevOps => MenuScreen {
            id: MenuId::Main,
            title: "debopty".to_string(),
            subtitle: "Workspace › 运维容器".to_string(),
            entries: vec![
                MenuEntry::Section("容器与终端"),
                item(
                    1,
                    "🐳",
                    "Podman",
                    Some(tool_badge(status, "podman")),
                    MenuAction::Navigate(MenuId::Podman),
                ),
                item(
                    2,
                    "🧹",
                    "镜像 GC",
                    Some(toggle_badge(status.podman_gc_enabled)),
                    MenuAction::Execute(BashAction::TogglePodmanGc),
                ),
                item(
                    3,
                    "🐟",
                    "Fish",
                    Some(tool_badge(status, "fish")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::Fish)),
                ),
                item(
                    4,
                    "📝",
                    "Micro",
                    Some(tool_badge(status, "micro")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::Micro)),
                ),
                item(
                    5,
                    "📁",
                    "Yazi",
                    Some(tool_badge(status, "yazi")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::Yazi)),
                ),
                item(
                    6,
                    "🪟",
                    "Tmux",
                    Some(tool_badge(status, "tmux")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::Tmux)),
                ),
                item(
                    7,
                    "📜",
                    "Lego ACME",
                    Some(tool_badge(status, "lego")),
                    MenuAction::Execute(BashAction::ManageLego),
                ),
                MenuEntry::Section("语言与 Web"),
                item(
                    8,
                    "🦀",
                    "Rustup",
                    Some(tool_badge(status, "rustc")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::RustSdk)),
                ),
                item(
                    9,
                    "🐹",
                    "Go SDK",
                    Some(tool_badge(status, "go")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::GoSdk)),
                ),
                item(
                    10,
                    "🌐",
                    "Ferron",
                    Some(tool_badge(status, "ferron")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::Ferron)),
                ),
                item(
                    11,
                    "🌐",
                    "Caddy",
                    Some(tool_badge(status, "caddy")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::Caddy)),
                ),
                item(
                    12,
                    "🔗",
                    "Realm",
                    Some(tool_badge(status, "realm")),
                    MenuAction::Navigate(MenuId::AppCrud(AppId::Realm)),
                ),
                MenuEntry::Section("会话"),
                item(0, "🔙", "退出", None, MenuAction::Exit),
            ],
        },
        WorkspaceTab::Reconcile => MenuScreen {
            id: MenuId::Main,
            title: "debopty".to_string(),
            subtitle: "Workspace › 稳态维护".to_string(),
            entries: vec![
                MenuEntry::Section("稳态收敛"),
                item(1, "🩺", "稳态巡检", None, MenuAction::NativeReconcile),
                item(2, "🔒", "Fail2ban 同步", None, MenuAction::NativeF2bSync),
                MenuEntry::Section("生命周期"),
                item(
                    3,
                    "🆙",
                    "检查更新",
                    None,
                    MenuAction::Execute(BashAction::ScriptUpdate),
                ),
                item(
                    4,
                    "❌",
                    "卸载 debopty",
                    None,
                    MenuAction::ExecuteWithConfirm {
                        action: BashAction::ScriptUninstall,
                        prompt: "卸载 debopty 及安装目录？ [y/N]".to_string(),
                    },
                ),
                MenuEntry::Section("会话"),
                item(0, "🔙", "退出", None, MenuAction::Exit),
            ],
        },
    }
}

/// Builds the flat global command index for the `Ctrl+K` fuzzy Command Palette (U17).
pub fn build_palette_entries(status: &StatusSnapshot) -> Vec<PaletteEntry> {
    let freship_action = if tool_installed(status, "freship") {
        MenuAction::Execute(BashAction::ManageFreship)
    } else {
        MenuAction::Execute(BashAction::InstallApp(AppId::Freship))
    };

    vec![
        PaletteEntry {
            domain: "系统",
            label: "基础优化",
            keywords: "base optimize sysctl bbr ssh limits swap",
            status: Some(if status.base_optimized {
                StatusBadge::Active("生效")
            } else {
                StatusBadge::Inactive("未调")
            }),
            action: MenuAction::Execute(BashAction::RunBaseOptimization),
        },
        PaletteEntry {
            domain: "系统",
            label: "路由转发",
            keywords: "ip forward routing sysctl net",
            status: Some(toggle_badge(status.ip_forward)),
            action: MenuAction::Execute(BashAction::ToggleIpForwarding),
        },
        PaletteEntry {
            domain: "维护",
            label: "稳态巡检",
            keywords: "reconcile audit drift steady state backup",
            status: None,
            action: MenuAction::NativeReconcile,
        },
        PaletteEntry {
            domain: "安全",
            label: "Fail2ban 同步",
            keywords: "f2b fail2ban nftables firewall ports sync",
            status: None,
            action: MenuAction::NativeF2bSync,
        },
        PaletteEntry {
            domain: "容器",
            label: "Podman",
            keywords: "podman container rootless docker",
            status: Some(tool_badge(status, "podman")),
            action: MenuAction::Navigate(MenuId::Podman),
        },
        PaletteEntry {
            domain: "容器",
            label: "镜像 GC",
            keywords: "podman gc prune timer cleanup",
            status: Some(toggle_badge(status.podman_gc_enabled)),
            action: MenuAction::Execute(BashAction::TogglePodmanGc),
        },
        PaletteEntry {
            domain: "代理",
            label: "Xray Core",
            keywords: "xray proxy geoip geosite vless",
            status: Some(tool_badge(status, "xray")),
            action: MenuAction::Execute(BashAction::ManageXray),
        },
        PaletteEntry {
            domain: "代理",
            label: "WARP",
            keywords: "warp cloudflare proxy vpn",
            status: Some(tool_badge(status, "warp-cli")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Warp)),
        },
        PaletteEntry {
            domain: "代理",
            label: "Usque",
            keywords: "usque masque warp proxy",
            status: Some(tool_badge(status, "usque")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Usque)),
        },
        PaletteEntry {
            domain: "代理",
            label: "导出 WG 出站",
            keywords: "warp xray wireguard outbound json",
            status: None,
            action: MenuAction::Execute(BashAction::GenerateWarpXray),
        },
        PaletteEntry {
            domain: "组网",
            label: "Easytier",
            keywords: "easytier mesh p2p vpn net",
            status: Some(tool_badge(status, "easytier-core")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Easytier)),
        },
        PaletteEntry {
            domain: "组网",
            label: "Tailscale",
            keywords: "tailscale mesh wireguard vpn net",
            status: Some(tool_badge(status, "tailscale")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Tailscale)),
        },
        PaletteEntry {
            domain: "网络",
            label: "FreshIP",
            keywords: "freship ip timer dual stack net",
            status: Some(tool_badge(status, "freship")),
            action: freship_action,
        },
        PaletteEntry {
            domain: "运维",
            label: "Fish",
            keywords: "fish shell devops",
            status: Some(tool_badge(status, "fish")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Fish)),
        },
        PaletteEntry {
            domain: "运维",
            label: "Micro",
            keywords: "micro editor devops",
            status: Some(tool_badge(status, "micro")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Micro)),
        },
        PaletteEntry {
            domain: "运维",
            label: "Yazi",
            keywords: "yazi file manager fm devops",
            status: Some(tool_badge(status, "yazi")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Yazi)),
        },
        PaletteEntry {
            domain: "运维",
            label: "Tmux",
            keywords: "tmux multiplexer terminal devops",
            status: Some(tool_badge(status, "tmux")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Tmux)),
        },
        PaletteEntry {
            domain: "安全",
            label: "Lego ACME",
            keywords: "lego ssl tls cert acme cloudflare",
            status: Some(tool_badge(status, "lego")),
            action: MenuAction::Execute(BashAction::ManageLego),
        },
        PaletteEntry {
            domain: "语言",
            label: "Rustup",
            keywords: "rust rustc cargo rustup sdk",
            status: Some(tool_badge(status, "rustc")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::RustSdk)),
        },
        PaletteEntry {
            domain: "服务",
            label: "Realm",
            keywords: "realm relay tcp udp forward rust",
            status: Some(tool_badge(status, "realm")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Realm)),
        },
        PaletteEntry {
            domain: "服务",
            label: "Ferron",
            keywords: "ferron web http reverse proxy rust",
            status: Some(tool_badge(status, "ferron")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Ferron)),
        },
        PaletteEntry {
            domain: "语言",
            label: "Go SDK",
            keywords: "go golang sdk toolchain",
            status: Some(tool_badge(status, "go")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::GoSdk)),
        },
        PaletteEntry {
            domain: "服务",
            label: "Caddy",
            keywords: "caddy web http tls go",
            status: Some(tool_badge(status, "caddy")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Caddy)),
        },
        PaletteEntry {
            domain: "服务",
            label: "DERPer",
            keywords: "derp derper tailscale relay go",
            status: Some(tool_badge(status, "derper")),
            action: MenuAction::Navigate(MenuId::AppCrud(AppId::Derper)),
        },
        PaletteEntry {
            domain: "维护",
            label: "检查更新",
            keywords: "self update upgrade release sha256",
            status: None,
            action: MenuAction::Execute(BashAction::ScriptUpdate),
        },
        PaletteEntry {
            domain: "维护",
            label: "卸载 debopty",
            keywords: "self uninstall purge remove",
            status: None,
            action: MenuAction::ExecuteWithConfirm {
                action: BashAction::ScriptUninstall,
                prompt: "卸载 debopty 及安装目录？ [y/N]".to_string(),
            },
        },
    ]
}
