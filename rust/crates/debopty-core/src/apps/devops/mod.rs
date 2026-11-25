pub mod cli_deps;
pub mod fish;
pub mod lego;
pub mod micro;
pub mod yazi;

pub const MODULE_ID: &str = "devops";

pub use cli_deps::{ensure_resvg, ensure_sendme, install_devops_shared_cli_deps};
pub use fish::{deploy_fish_yazi_wrapper, install_fish, uninstall_fish};
pub use lego::{
    LegoDomainSpec, LegoDomainStatus, LegoFlag, add_lego_domain, build_lego_troubleshoot_cmd,
    install_lego, list_lego_domains, remove_lego_domain, run_lego_ferron_hook, run_lego_once,
    toggle_lego_flag, uninstall_lego, validate_primary_domain, validate_san_domain,
};
pub use micro::{install_micro, patch_micro_omni_session_lua, uninstall_micro};
pub use yazi::{YaziInstallOptions, install_yazi, uninstall_yazi};
