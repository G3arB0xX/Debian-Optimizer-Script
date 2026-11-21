use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::BridgeError;

/// Execution engine selection for Rust/Bash coexistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineMode {
    #[default]
    Auto,
    Rust,
    Bash,
}

impl EngineMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Rust => "rust",
            Self::Bash => "bash",
        }
    }
}

impl fmt::Display for EngineMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EngineMode {
    type Err = BridgeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "rust" => Ok(Self::Rust),
            "bash" => Ok(Self::Bash),
            other => Err(BridgeError::UnknownEngineMode(other.to_string())),
        }
    }
}

/// Implementation backend for a debopty functional module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleImpl {
    NativeRust,
    BashBridge,
}

impl ModuleImpl {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NativeRust => "native-rust",
            Self::BashBridge => "bash-bridge",
        }
    }
}

impl fmt::Display for ModuleImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Managed application identifiers matching `scripts/tui.sh` and `scripts/apps/*.sh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppId {
    Fish,
    Micro,
    Yazi,
    Tmux,
    Podman,
    RustSdk,
    Realm,
    Ferron,
    GoSdk,
    Caddy,
    Derper,
    Xray,
    Warp,
    Usque,
    Easytier,
    Tailscale,
    Freship,
}

impl AppId {
    pub const ALL: &'static [AppId] = &[
        Self::Fish,
        Self::Micro,
        Self::Yazi,
        Self::Tmux,
        Self::Podman,
        Self::RustSdk,
        Self::Realm,
        Self::Ferron,
        Self::GoSdk,
        Self::Caddy,
        Self::Derper,
        Self::Xray,
        Self::Warp,
        Self::Usque,
        Self::Easytier,
        Self::Tailscale,
        Self::Freship,
    ];

    /// Display name matching `scripts/tui.sh::handle_submenu`.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Fish => "Fish",
            Self::Micro => "Micro",
            Self::Yazi => "Yazi",
            Self::Tmux => "Tmux",
            Self::Podman => "Podman",
            Self::RustSdk => "Rust SDK",
            Self::Realm => "Realm",
            Self::Ferron => "Ferron",
            Self::GoSdk => "Go SDK",
            Self::Caddy => "Caddy",
            Self::Derper => "DERP",
            Self::Xray => "Xray",
            Self::Warp => "WARP",
            Self::Usque => "Usque",
            Self::Easytier => "Easytier",
            Self::Tailscale => "Tailscale",
            Self::Freship => "FreshIP",
        }
    }

    /// CLI slug used in `debopty bridge install-<slug>` / `uninstall-<slug>`.
    pub fn cli_slug(self) -> &'static str {
        match self {
            Self::Fish => "fish",
            Self::Micro => "micro",
            Self::Yazi => "yazi",
            Self::Tmux => "tmux",
            Self::Podman => "podman",
            Self::RustSdk => "rust-sdk",
            Self::Realm => "realm",
            Self::Ferron => "ferron",
            Self::GoSdk => "go-sdk",
            Self::Caddy => "caddy",
            Self::Derper => "derper",
            Self::Xray => "xray",
            Self::Warp => "warp",
            Self::Usque => "usque",
            Self::Easytier => "easytier",
            Self::Tailscale => "tailscale",
            Self::Freship => "freship",
        }
    }

    /// Corresponding tool key in `debopty_core::status::StatusSnapshot::tools`.
    pub fn status_tool_key(self) -> &'static str {
        match self {
            Self::Fish => "fish",
            Self::Micro => "micro",
            Self::Yazi => "yazi",
            Self::Tmux => "tmux",
            Self::Podman => "podman",
            Self::RustSdk => "rustc",
            Self::Realm => "realm",
            Self::Ferron => "ferron",
            Self::GoSdk => "go",
            Self::Caddy => "caddy",
            Self::Derper => "derper",
            Self::Xray => "xray",
            Self::Warp => "warp-cli",
            Self::Usque => "usque",
            Self::Easytier => "easytier-core",
            Self::Tailscale => "tailscale",
            Self::Freship => "freship",
        }
    }

    /// Static Bash module relative path and install function name.
    pub fn install_target(self) -> BashFunctionSpec {
        match self {
            Self::Fish => BashFunctionSpec::new("scripts/apps/devops.sh", "install_fish"),
            Self::Micro => BashFunctionSpec::new("scripts/apps/devops.sh", "install_micro"),
            Self::Yazi => BashFunctionSpec::new("scripts/apps/devops.sh", "install_yazi"),
            Self::Tmux => BashFunctionSpec::new("scripts/system.sh", "setup_tmux"),
            Self::Podman => BashFunctionSpec::new("scripts/apps/podman.sh", "install_podman"),
            Self::RustSdk => BashFunctionSpec::new("scripts/apps/rust.sh", "install_rust"),
            Self::Realm => BashFunctionSpec::new("scripts/apps/realm.sh", "install_realm"),
            Self::Ferron => BashFunctionSpec::new("scripts/apps/ferron.sh", "install_ferron"),
            Self::GoSdk => BashFunctionSpec::new("scripts/apps/golang.sh", "install_go"),
            Self::Caddy => BashFunctionSpec::new("scripts/apps/golang.sh", "install_caddy"),
            Self::Derper => BashFunctionSpec::new("scripts/apps/golang.sh", "install_derper"),
            Self::Xray => BashFunctionSpec::new("scripts/apps/xray.sh", "install_xray"),
            Self::Warp => BashFunctionSpec::new("scripts/apps/warp.sh", "install_warp"),
            Self::Usque => BashFunctionSpec::new("scripts/apps/warp.sh", "install_usque"),
            Self::Easytier => BashFunctionSpec::new("scripts/apps/easytier.sh", "install_easytier"),
            Self::Tailscale => {
                BashFunctionSpec::new("scripts/apps/tailscale.sh", "install_tailscale")
            }
            Self::Freship => BashFunctionSpec::new("scripts/apps/freship.sh", "install_freship"),
        }
    }

    /// Static Bash module relative path and uninstall function name.
    pub fn uninstall_target(self) -> BashFunctionSpec {
        match self {
            Self::Fish => BashFunctionSpec::new("scripts/apps/devops.sh", "uninstall_fish"),
            Self::Micro => BashFunctionSpec::new("scripts/apps/devops.sh", "uninstall_micro"),
            Self::Yazi => BashFunctionSpec::new("scripts/apps/devops.sh", "uninstall_yazi"),
            Self::Tmux => BashFunctionSpec::new("scripts/system.sh", "uninstall_tmux"),
            Self::Podman => BashFunctionSpec::new("scripts/apps/podman.sh", "uninstall_podman"),
            Self::RustSdk => BashFunctionSpec::new("scripts/apps/rust.sh", "uninstall_rust"),
            Self::Realm => BashFunctionSpec::new("scripts/apps/realm.sh", "uninstall_realm"),
            Self::Ferron => BashFunctionSpec::new("scripts/apps/ferron.sh", "uninstall_ferron"),
            Self::GoSdk => BashFunctionSpec::new("scripts/apps/golang.sh", "uninstall_go"),
            Self::Caddy => BashFunctionSpec::new("scripts/apps/golang.sh", "uninstall_caddy"),
            Self::Derper => BashFunctionSpec::new("scripts/apps/golang.sh", "uninstall_derper"),
            Self::Xray => BashFunctionSpec::new("scripts/apps/xray.sh", "uninstall_xray"),
            Self::Warp => BashFunctionSpec::new("scripts/apps/warp.sh", "uninstall_warp"),
            Self::Usque => BashFunctionSpec::new("scripts/apps/warp.sh", "uninstall_usque"),
            Self::Easytier => {
                BashFunctionSpec::new("scripts/apps/easytier.sh", "uninstall_easytier")
            }
            Self::Tailscale => {
                BashFunctionSpec::new("scripts/apps/tailscale.sh", "uninstall_tailscale")
            }
            Self::Freship => BashFunctionSpec::new("scripts/apps/freship.sh", "uninstall_freship"),
        }
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.cli_slug())
    }
}

impl FromStr for AppId {
    type Err = BridgeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fish" => Ok(Self::Fish),
            "micro" => Ok(Self::Micro),
            "yazi" => Ok(Self::Yazi),
            "tmux" => Ok(Self::Tmux),
            "podman" => Ok(Self::Podman),
            "rust-sdk" | "rust" | "rustc" => Ok(Self::RustSdk),
            "realm" => Ok(Self::Realm),
            "ferron" => Ok(Self::Ferron),
            "go-sdk" | "go" | "golang" => Ok(Self::GoSdk),
            "caddy" => Ok(Self::Caddy),
            "derper" | "derp" => Ok(Self::Derper),
            "xray" => Ok(Self::Xray),
            "warp" | "warp-cli" => Ok(Self::Warp),
            "usque" => Ok(Self::Usque),
            "easytier" | "easytier-core" => Ok(Self::Easytier),
            "tailscale" => Ok(Self::Tailscale),
            "freship" => Ok(Self::Freship),
            other => Err(BridgeError::UnknownAction(other.to_string())),
        }
    }
}

/// Compile-time static target for a Bash function invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BashFunctionSpec {
    pub module_rel_path: &'static str,
    pub function_name: &'static str,
}

impl BashFunctionSpec {
    pub const fn new(module_rel_path: &'static str, function_name: &'static str) -> Self {
        Self {
            module_rel_path,
            function_name,
        }
    }
}

/// Dispatch strategy for a `BashAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashDispatch {
    Function(BashFunctionSpec),
    Entrypoint(&'static str),
}

/// Legacy Bash actions exposed through the Strangler-Fig bridge (1:1 with `scripts/tui.sh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BashAction {
    RunBaseOptimization,
    ToggleIpForwarding,
    TogglePodmanGc,
    InstallApp(AppId),
    UninstallApp(AppId),
    ManageLego,
    ManageXray,
    GenerateWarpXray,
    ManageFreship,
    ScriptUpdate,
    ScriptUninstall,
    LegacyFullEntry,
}

impl BashAction {
    /// Returns the compile-time static dispatch specification for this action.
    pub fn dispatch(self) -> BashDispatch {
        match self {
            Self::RunBaseOptimization => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/system.sh",
                "run_base_optimization",
            )),
            Self::ToggleIpForwarding => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/system.sh",
                "toggle_ip_forwarding",
            )),
            Self::TogglePodmanGc => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/apps/podman.sh",
                "toggle_podman_gc",
            )),
            Self::InstallApp(app) => BashDispatch::Function(app.install_target()),
            Self::UninstallApp(app) => BashDispatch::Function(app.uninstall_target()),
            Self::ManageLego => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/apps/devops.sh",
                "handle_lego_submenu",
            )),
            Self::ManageXray => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/apps/xray.sh",
                "handle_xray_submenu",
            )),
            Self::GenerateWarpXray => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/apps/warp.sh",
                "generate_warp_xray",
            )),
            Self::ManageFreship => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/apps/freship.sh",
                "manage_freship",
            )),
            Self::ScriptUpdate => {
                BashDispatch::Function(BashFunctionSpec::new("scripts/common.sh", "script_update"))
            }
            Self::ScriptUninstall => BashDispatch::Function(BashFunctionSpec::new(
                "scripts/common.sh",
                "script_uninstall",
            )),
            Self::LegacyFullEntry => BashDispatch::Entrypoint("deb_optimizer.sh"),
        }
    }

    /// Whether `scripts/tui.sh` pauses for user acknowledgement after running this action.
    pub fn needs_pause_after(self) -> bool {
        matches!(
            self,
            Self::RunBaseOptimization
                | Self::ToggleIpForwarding
                | Self::TogglePodmanGc
                | Self::InstallApp(_)
                | Self::UninstallApp(_)
                | Self::GenerateWarpXray
                | Self::ScriptUpdate
        )
    }

    /// Stable CLI identifier for `debopty bridge <action>`.
    pub fn cli_name(self) -> String {
        match self {
            Self::RunBaseOptimization => "base-optimize".to_string(),
            Self::ToggleIpForwarding => "toggle-ip-forward".to_string(),
            Self::TogglePodmanGc => "toggle-podman-gc".to_string(),
            Self::InstallApp(app) => format!("install-{}", app.cli_slug()),
            Self::UninstallApp(app) => format!("uninstall-{}", app.cli_slug()),
            Self::ManageLego => "manage-lego".to_string(),
            Self::ManageXray => "manage-xray".to_string(),
            Self::GenerateWarpXray => "generate-warp-xray".to_string(),
            Self::ManageFreship => "manage-freship".to_string(),
            Self::ScriptUpdate => "script-update".to_string(),
            Self::ScriptUninstall => "script-uninstall".to_string(),
            Self::LegacyFullEntry => "legacy-entry".to_string(),
        }
    }

    /// Enumerates every supported bridge action in deterministic order.
    pub fn all_actions() -> Vec<BashAction> {
        let mut actions = vec![
            Self::RunBaseOptimization,
            Self::ToggleIpForwarding,
            Self::TogglePodmanGc,
            Self::ManageLego,
            Self::ManageXray,
            Self::GenerateWarpXray,
            Self::ManageFreship,
            Self::ScriptUpdate,
            Self::ScriptUninstall,
            Self::LegacyFullEntry,
        ];
        for &app in AppId::ALL {
            actions.push(Self::InstallApp(app));
            actions.push(Self::UninstallApp(app));
        }
        actions
    }
}

impl fmt::Display for BashAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.cli_name())
    }
}

impl FromStr for BashAction {
    type Err = BridgeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm = s.trim().to_ascii_lowercase();
        match norm.as_str() {
            "base-optimize" | "run-base-optimization" => Ok(Self::RunBaseOptimization),
            "toggle-ip-forward" | "toggle-ip-forwarding" => Ok(Self::ToggleIpForwarding),
            "toggle-podman-gc" => Ok(Self::TogglePodmanGc),
            "manage-lego" => Ok(Self::ManageLego),
            "manage-xray" => Ok(Self::ManageXray),
            "generate-warp-xray" => Ok(Self::GenerateWarpXray),
            "manage-freship" => Ok(Self::ManageFreship),
            "script-update" => Ok(Self::ScriptUpdate),
            "script-uninstall" => Ok(Self::ScriptUninstall),
            "legacy-entry" | "legacy-full-entry" => Ok(Self::LegacyFullEntry),
            other => {
                if let Some(rest) = other.strip_prefix("install-") {
                    return Ok(Self::InstallApp(AppId::from_str(rest)?));
                }
                if let Some(rest) = other.strip_prefix("uninstall-") {
                    return Ok(Self::UninstallApp(AppId::from_str(rest)?));
                }
                Err(BridgeError::UnknownAction(other.to_string()))
            }
        }
    }
}
