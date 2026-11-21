use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::error::{CoreError, Result};
use crate::sys;
use crate::version;

include!(concat!(env!("OUT_DIR"), "/embedded_templates.rs"));

fn placeholder_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{\{([A-Za-z0-9_]+)\}\}").expect("valid template placeholder regex")
    })
}

fn validate_template_rel_path(rel_path: &str) -> Result<String> {
    if rel_path.is_empty() || rel_path.contains('\0') || rel_path.contains('\\') {
        return Err(CoreError::ValidationFailed(format!(
            "invalid template path: {rel_path}"
        )));
    }
    let path = Path::new(rel_path);
    if path.is_absolute() {
        return Err(CoreError::ValidationFailed(format!(
            "template path must be relative: {rel_path}"
        )));
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(os) => {
                let s = os.to_str().ok_or_else(|| {
                    CoreError::ValidationFailed(format!(
                        "non-UTF-8 component in template path: {rel_path}"
                    ))
                })?;
                parts.push(s.to_string());
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::ValidationFailed(format!(
                    "path traversal rejected in template path: {rel_path}"
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(CoreError::ValidationFailed(format!(
            "empty template path: {rel_path}"
        )));
    }
    Ok(parts.join("/"))
}

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
        let normalized = validate_template_rel_path(rel_path)?;

        if let Some(ref dir) = self.override_dir {
            let mut candidates = vec![dir.join(&normalized)];
            if let Some(stripped) = normalized.strip_prefix("templates/") {
                candidates.push(dir.join(stripped));
            }
            if let Some(file_name) = Path::new(&normalized).file_name() {
                candidates.push(dir.join(file_name));
            }

            let base_canon = fs::canonicalize(dir).ok();
            for candidate in candidates {
                if !candidate.is_file() {
                    continue;
                }
                if let (Some(base), Ok(cand_canon)) = (&base_canon, fs::canonicalize(&candidate))
                    && !cand_canon.starts_with(base)
                {
                    return Err(CoreError::ValidationFailed(format!(
                        "template override escapes base directory: {rel_path}"
                    )));
                }
                return Ok(fs::read_to_string(candidate)?);
            }
        }

        if let Ok(idx) = EMBEDDED_TEMPLATES.binary_search_by_key(&normalized.as_str(), |(k, _)| *k)
        {
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

    /// Render a template to a string using single-pass placeholder substitution.
    pub fn render_to_string(
        &self,
        rel_path: &str,
        vars: &BTreeMap<String, String>,
    ) -> Result<String> {
        let content = self.get_raw(rel_path)?;
        let mut effective = Self::builtin_vars();
        for (k, v) in vars {
            effective.insert(k.clone(), v.clone());
        }
        let rendered = placeholder_re().replace_all(&content, |caps: &regex::Captures<'_>| {
            let key = &caps[1];
            match effective.get(key) {
                Some(val) => val.clone(),
                None => caps[0].to_string(),
            }
        });
        Ok(rendered.into_owned())
    }

    /// Render a template atomically to `target`, creating parent directories and preserving existing mode
    /// (defaulting to `0o644`) when `mode` is `None`.
    pub fn render_to_file(
        &self,
        rel_path: &str,
        target: &Path,
        vars: &BTreeMap<String, String>,
        mode: Option<u32>,
    ) -> Result<()> {
        let rendered = self.render_to_string(rel_path, vars)?;
        sys::write_file_atomic(target, rendered.as_bytes(), mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
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

    #[test]
    fn template_override_dir_blocks_parent_traversal() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        fs::write(&secret, "TOP_SECRET").unwrap();
        let override_dir = tmp.path().join("override");
        fs::create_dir_all(&override_dir).unwrap();

        let engine = TemplateEngine::new(Some(override_dir));
        for bad in [
            "../secret.txt",
            "../../etc/passwd",
            "/etc/passwd",
            "templates/../../secret.txt",
            "templates/common/debopty.sh\0.txt",
            r"templates\..\secret.txt",
        ] {
            let err = engine.get_raw(bad).unwrap_err();
            assert!(
                matches!(err, CoreError::ValidationFailed(_)),
                "expected ValidationFailed for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn template_render_prevents_double_substitution_and_preserves_mode() {
        let tmp = TempDir::new().unwrap();
        let override_dir = tmp.path().join("override");
        fs::create_dir_all(&override_dir).unwrap();
        fs::write(override_dir.join("double.tmpl"), "A={{A}}|B={{B}}\n").unwrap();

        let engine = TemplateEngine::new(Some(override_dir));
        let mut vars = BTreeMap::new();
        vars.insert("A".to_string(), "{{B}}".to_string());
        vars.insert("B".to_string(), "INJECTED".to_string());

        let out = engine.render_to_string("double.tmpl", &vars).unwrap();
        assert_eq!(out, "A={{B}}|B=INJECTED\n");

        // Mode preservation when mode is None: new file defaults to 0o644, existing 0o640 preserved
        let target = tmp.path().join("rendered.conf");
        engine
            .render_to_file("double.tmpl", &target, &vars, None)
            .unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );

        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        engine
            .render_to_file("double.tmpl", &target, &vars, None)
            .unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );

        // Explicit 0o600 still honored
        engine
            .render_to_file("double.tmpl", &target, &vars, Some(0o600))
            .unwrap();
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
