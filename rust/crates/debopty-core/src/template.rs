use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

use crate::error::{CoreError, Result};
use crate::sys;
use crate::version;

include!(concat!(env!("OUT_DIR"), "/embedded_templates.rs"));

#[derive(Debug, Clone)]
pub struct TemplateEngine {
    override_dir: Option<PathBuf>,
}

impl TemplateEngine {
    pub fn new(override_dir: Option<PathBuf>) -> Self {
        Self { override_dir }
    }

    /// Returns all embedded template relative paths (sorted, never includes `INDEX.md`).
    pub fn list(&self) -> Vec<&'static str> {
        EMBEDDED_TEMPLATES.iter().map(|(k, _)| *k).collect()
    }

    /// Fetch raw template contents, checking `override_dir` first before falling back to embedded SSOT.
    pub fn get_raw(&self, rel_path: &str) -> Result<String> {
        let normalized = rel_path.trim_start_matches("./");

        if let Some(ref dir) = self.override_dir {
            let candidates = [
                dir.join(normalized),
                normalized
                    .strip_prefix("templates/")
                    .map(|s| dir.join(s))
                    .unwrap_or_else(|| dir.join(normalized)),
                Path::new(normalized)
                    .file_name()
                    .map(|f| dir.join(f))
                    .unwrap_or_else(|| dir.join(normalized)),
            ];
            for candidate in candidates {
                if candidate.is_file() {
                    return Ok(fs::read_to_string(candidate)?);
                }
            }
        }

        if let Ok(idx) = EMBEDDED_TEMPLATES.binary_search_by_key(&normalized, |(k, _)| *k) {
            return Ok(EMBEDDED_TEMPLATES[idx].1.to_string());
        }

        Err(CoreError::TemplateNotFound(rel_path.to_string()))
    }

    fn builtin_vars() -> BTreeMap<String, String> {
        let id = version::compiled_identity();
        let debopti_version = env::var("DEBOPTI_VERSION")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| id.primary().map(str::to_string))
            .unwrap_or_default();
        let debopti_build = env::var("DEBOPTI_BUILD")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| env::var("VERSION_ID").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| id.build.clone());
        let version_id = env::var("VERSION_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| debopti_build.clone());
        let is_cn_region = env::var("IS_CN_REGION")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "false".to_string());

        let passwd_entries = fs::read_to_string("/etc/passwd")
            .map(|t| sys::parse_passwd(&t))
            .unwrap_or_default();
        let sot_user = sys::sot_user(&passwd_entries);
        let initial_user = sys::initial_user(env::var("SUDO_USER").ok().as_deref());
        let install_dir = env::var("INSTALL_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/opt/debopty".to_string());
        let bin_link = env::var("BIN_LINK")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/usr/local/bin/debopty".to_string());
        let nft_base_d = "/etc/nftables.d".to_string();

        let mut map = BTreeMap::new();
        map.insert("DEBOPTI_VERSION".to_string(), debopti_version);
        map.insert("DEBOPTI_BUILD".to_string(), debopti_build);
        map.insert("VERSION_ID".to_string(), version_id);
        map.insert("IS_CN_REGION".to_string(), is_cn_region);
        map.insert("SOT_USER".to_string(), sot_user);
        map.insert("INITIAL_USER".to_string(), initial_user);
        map.insert("INSTALL_DIR".to_string(), install_dir);
        map.insert("BIN_LINK".to_string(), bin_link);
        map.insert("NFT_BASE_D".to_string(), nft_base_d);
        map
    }

    /// Render a template to a string, substituting built-in globals and explicit `vars` overrides.
    pub fn render_to_string(
        &self,
        rel_path: &str,
        vars: &BTreeMap<String, String>,
    ) -> Result<String> {
        let mut content = self.get_raw(rel_path)?;
        let mut effective = Self::builtin_vars();
        for (k, v) in vars {
            effective.insert(k.clone(), v.clone());
        }
        for (k, v) in effective {
            let placeholder = format!("{{{{{k}}}}}");
            if content.contains(&placeholder) {
                content = content.replace(&placeholder, &v);
            }
        }
        Ok(content)
    }

    /// Render a template atomically to `target`, creating parent directories and optionally applying `chmod`.
    pub fn render_to_file(
        &self,
        rel_path: &str,
        target: &Path,
        vars: &BTreeMap<String, String>,
        mode: Option<u32>,
    ) -> Result<()> {
        let rendered = self.render_to_string(rel_path, vars)?;
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let mut tmp = NamedTempFile::new_in(parent)?;
        tmp.write_all(rendered.as_bytes())?;
        tmp.flush()?;
        if let Some(m) = mode {
            fs::set_permissions(tmp.path(), fs::Permissions::from_mode(m))?;
        }
        tmp.persist(target).map_err(|e| CoreError::Io(e.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn embedded_templates_count_and_no_index_md() {
        let engine = TemplateEngine::new(None);
        let list = engine.list();
        assert!(
            list.len() >= 85,
            "expected at least 85 embedded templates, got {}",
            list.len()
        );
        for item in &list {
            assert!(
                !item.ends_with("INDEX.md"),
                "INDEX.md must never be embedded: {item}"
            );
        }
        assert!(list.contains(&"templates/common/debopti.conf"));
        assert!(list.contains(&"templates/common/debopty.sh"));
        assert!(list.contains(&"templates/security/nftables.conf"));
    }

    #[test]
    fn render_substitution_override_and_atomic_write() {
        let tmp = TempDir::new().unwrap();
        let engine = TemplateEngine::new(None);

        let mut vars = BTreeMap::new();
        vars.insert("SSH_PORT".to_string(), "42222".to_string());
        vars.insert("NFT_BASE_D".to_string(), "/tmp/custom-nft.d".to_string());

        let out_path = tmp.path().join("etc/nftables.conf");
        engine
            .render_to_file(
                "templates/security/nftables.conf",
                &out_path,
                &vars,
                Some(0o644),
            )
            .unwrap();
        let rendered = fs::read_to_string(&out_path).unwrap();
        assert!(rendered.contains("42222"));
        assert!(rendered.contains("/tmp/custom-nft.d"));
        assert!(!rendered.contains("{{SSH_PORT}}"));
        assert!(!rendered.contains("{{NFT_BASE_D}}"));

        let mode = fs::metadata(&out_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);

        // Missing template returns TemplateNotFound
        let err = engine
            .render_to_string("templates/does-not-exist.conf", &BTreeMap::new())
            .unwrap_err();
        assert!(matches!(err, CoreError::TemplateNotFound(_)));

        // Override directory takes precedence over embedded
        let override_dir = tmp.path().join("override");
        fs::create_dir_all(override_dir.join("templates/common")).unwrap();
        fs::write(
            override_dir.join("templates/common/debopty.sh"),
            "OVERRIDE {{INSTALL_DIR}}\n",
        )
        .unwrap();
        let override_engine = TemplateEngine::new(Some(override_dir));
        let mut ov_vars = BTreeMap::new();
        ov_vars.insert("INSTALL_DIR".to_string(), "/opt/custom".to_string());
        let ov_rendered = override_engine
            .render_to_string("templates/common/debopty.sh", &ov_vars)
            .unwrap();
        assert_eq!(ov_rendered, "OVERRIDE /opt/custom\n");
    }
}
