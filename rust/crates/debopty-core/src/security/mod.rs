pub mod f2b_sync;
pub mod fail2ban;
pub mod firewall;
pub mod ssh;

use std::path::{Path, PathBuf};

use crate::config::DEFAULT_CONFIG_PATH;
use crate::security::f2b_sync::F2bSyncOptions;
use crate::template::TemplateEngine;

pub(crate) use crate::sys::write_file_atomic;

pub const DEFAULT_NFT_BASE_D: &str = "/etc/nftables.d";
pub const DEFAULT_NFT_CONF_DIR: &str = "/etc/nftables.d/debopti";
pub const DEFAULT_NFT_MAIN_CONF: &str = "/etc/nftables.conf";

#[derive(Debug, Clone)]
pub struct SecurityContext {
    pub root_prefix: PathBuf,
    pub nft_base_d: PathBuf,
    pub nft_conf_dir: PathBuf,
    pub nft_main_conf: PathBuf,
    pub no_sys: bool,
    pub non_interactive: bool,
    /// Explicit test-only opt-in permitting ephemeral `/opt/debopty/test_key` generation
    /// and default test password fallback in non-interactive mode. Always `false` by default.
    pub allow_test_ephemeral_auth: bool,
    pub config_path: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub f2b_backend_override: Option<String>,
    pub binary_path: Option<PathBuf>,
}

impl Default for SecurityContext {
    fn default() -> Self {
        Self {
            root_prefix: PathBuf::from("/"),
            nft_base_d: PathBuf::from(DEFAULT_NFT_BASE_D),
            nft_conf_dir: PathBuf::from(DEFAULT_NFT_CONF_DIR),
            nft_main_conf: PathBuf::from(DEFAULT_NFT_MAIN_CONF),
            no_sys: false,
            non_interactive: false,
            allow_test_ephemeral_auth: false,
            config_path: PathBuf::from(DEFAULT_CONFIG_PATH),
            template_dir: None,
            f2b_backend_override: None,
            binary_path: std::env::current_exe().ok(),
        }
    }
}

impl SecurityContext {
    /// Construct a `SecurityContext` scoped cleanly under `root` for unprivileged tests or `--root`.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let nft_base_d = prefixed_path(&root, DEFAULT_NFT_BASE_D);
        let nft_conf_dir = nft_base_d.join("debopti");
        let nft_main_conf = prefixed_path(&root, DEFAULT_NFT_MAIN_CONF);
        let config_path = prefixed_path(&root, DEFAULT_CONFIG_PATH);
        Self {
            root_prefix: root,
            nft_base_d,
            nft_conf_dir,
            nft_main_conf,
            no_sys: false,
            non_interactive: false,
            allow_test_ephemeral_auth: false,
            config_path,
            template_dir: None,
            f2b_backend_override: None,
            binary_path: std::env::current_exe().ok(),
        }
    }

    pub fn is_custom_root(&self) -> bool {
        !self.root_prefix.as_os_str().is_empty() && self.root_prefix != Path::new("/")
    }

    pub fn path(&self, abs: &str) -> PathBuf {
        prefixed_path(&self.root_prefix, abs)
    }

    pub fn effective_nft_base_d(&self) -> PathBuf {
        if self.is_custom_root() && self.nft_base_d == Path::new(DEFAULT_NFT_BASE_D) {
            self.path(DEFAULT_NFT_BASE_D)
        } else {
            self.nft_base_d.clone()
        }
    }

    pub fn effective_nft_conf_dir(&self) -> PathBuf {
        if self.is_custom_root() && self.nft_conf_dir == Path::new(DEFAULT_NFT_CONF_DIR) {
            self.effective_nft_base_d().join("debopti")
        } else {
            self.nft_conf_dir.clone()
        }
    }

    pub fn effective_nft_main_conf(&self) -> PathBuf {
        if self.is_custom_root() && self.nft_main_conf == Path::new(DEFAULT_NFT_MAIN_CONF) {
            self.path(DEFAULT_NFT_MAIN_CONF)
        } else {
            self.nft_main_conf.clone()
        }
    }

    pub fn effective_config_path(&self) -> PathBuf {
        if self.is_custom_root() && self.config_path == Path::new(DEFAULT_CONFIG_PATH) {
            self.path(DEFAULT_CONFIG_PATH)
        } else {
            self.config_path.clone()
        }
    }

    pub fn template_engine(&self) -> TemplateEngine {
        TemplateEngine::new(self.template_dir.clone()).with_root_prefix(&self.root_prefix)
    }

    pub fn f2b_sync_options(&self, force_reload: bool) -> F2bSyncOptions {
        F2bSyncOptions {
            root_prefix: self.root_prefix.clone(),
            no_sys: self.no_sys,
            force_reload,
            print_ports: false,
            backend_override: self.f2b_backend_override.clone(),
            template_dir: self.template_dir.clone(),
            ss_file: None,
        }
    }
}

pub(crate) fn prefixed_path(root: &Path, abs: &str) -> PathBuf {
    let clean = abs.trim_start_matches('/');
    if root.as_os_str().is_empty() || root == Path::new("/") {
        PathBuf::from("/").join(clean)
    } else {
        root.join(clean)
    }
}

pub(crate) fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}
