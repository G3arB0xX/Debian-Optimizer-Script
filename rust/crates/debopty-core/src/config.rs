use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::error::{CoreError, Result};
use crate::template::TemplateEngine;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/debopti/debopti.conf";

const DEFAULT_CONFIG_TEMPLATE: &str = "templates/common/debopti.conf";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub is_cn_region: String,
    pub base_optimized: bool,
    pub installed: bool,
    pub script_version: String,
    pub script_build: String,
    pub ssh_hardened: bool,
    pub editor_cmd: String,
    pub podman_gc_enabled: bool,
    pub yazi_media_preview: String,
    pub xray_geo_auto_update: String,
    pub extra: BTreeMap<String, String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            is_cn_region: String::new(),
            base_optimized: false,
            installed: false,
            script_version: String::new(),
            script_build: String::new(),
            ssh_hardened: false,
            editor_cmd: "micro".to_string(),
            podman_gc_enabled: false,
            yazi_media_preview: String::new(),
            xray_geo_auto_update: String::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl ProjectConfig {
    pub fn is_cn_region_enabled(&self) -> bool {
        self.is_cn_region == "true"
    }

    pub fn from_kv_map(map: &BTreeMap<String, String>) -> Self {
        let mut cfg = Self::default();
        for (k, v) in map {
            match k.as_str() {
                "IS_CN_REGION" => cfg.is_cn_region = v.clone(),
                "BASE_OPTIMIZED" => cfg.base_optimized = v == "true",
                "INSTALLED" => cfg.installed = v == "true",
                "SCRIPT_VERSION" => cfg.script_version = v.clone(),
                "SCRIPT_BUILD" => cfg.script_build = v.clone(),
                "SSH_HARDENED" => cfg.ssh_hardened = v == "true",
                "EDITOR_CMD" => cfg.editor_cmd = v.clone(),
                "PODMAN_GC_ENABLED" => cfg.podman_gc_enabled = v == "true",
                "YAZI_MEDIA_PREVIEW" => cfg.yazi_media_preview = v.clone(),
                "XRAY_GEO_AUTO_UPDATE" => cfg.xray_geo_auto_update = v.clone(),
                _ => {
                    cfg.extra.insert(k.clone(), v.clone());
                }
            }
        }
        cfg
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_store() -> Self {
        Self::new(DEFAULT_CONFIG_PATH)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_raw_or_default(&self) -> Result<String> {
        if self.path.is_file() {
            Ok(fs::read_to_string(&self.path)?)
        } else {
            let engine = TemplateEngine::new(None);
            engine.get_raw(DEFAULT_CONFIG_TEMPLATE)
        }
    }

    pub fn load(&self) -> Result<ProjectConfig> {
        let map = self.list()?;
        Ok(ProjectConfig::from_kv_map(&map))
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        validate_key(key)?;
        let map = self.list()?;
        Ok(map.get(key).cloned())
    }

    pub fn list(&self) -> Result<BTreeMap<String, String>> {
        let raw = self.read_raw_or_default()?;
        Ok(parse_bash_kv(&raw))
    }

    /// Update or append `KEY="value"` while preserving comments, blank lines, ordering, and unknown keys.
    /// Writes atomically via temp file + rename in the target directory.
    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        validate_key(key)?;
        if value.contains('\n') || value.contains('\r') {
            return Err(CoreError::InvalidConfig(format!(
                "config value for {key} cannot contain newlines"
            )));
        }

        let raw = self.read_raw_or_default()?;
        let formatted_line = format!("{key}=\"{}\"", escape_bash_double_quoted(value));

        let active_prefix = format!("{key}=");
        let commented_prefix = format!("#{key}=");

        let mut updated = false;
        let mut out_lines: Vec<String> = Vec::new();
        for line in raw.lines() {
            if line.starts_with(&active_prefix) || line.starts_with(&commented_prefix) {
                out_lines.push(formatted_line.clone());
                updated = true;
            } else {
                out_lines.push(line.to_string());
            }
        }
        if !updated {
            out_lines.push(formatted_line);
        }

        let mut output = out_lines.join("\n");
        output.push('\n');

        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let mut tmp = NamedTempFile::new_in(parent)?;
        tmp.write_all(output.as_bytes())?;
        tmp.flush()?;
        tmp.persist(&self.path)
            .map_err(|e| CoreError::Io(e.error))?;
        Ok(())
    }
}

fn validate_key(key: &str) -> Result<()> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err(CoreError::InvalidConfig("empty config key".to_string()));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(CoreError::InvalidConfig(format!(
            "invalid config key: {key}"
        )));
    }
    Ok(())
}

fn escape_bash_double_quoted(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for ch in val.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn unescape_bash_double_quoted(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    let mut chars = val.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    '\\' | '"' | '$' | '`' => out.push(next),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn parse_bash_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        unescape_bash_double_quoted(&trimmed[1..trimmed.len() - 1])
    } else if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn parse_bash_kv(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let key = k.trim();
            if validate_key(key).is_ok() {
                map.insert(key.to_string(), parse_bash_value(v));
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn missing_file_loads_embedded_defaults() {
        let tmp = TempDir::new().unwrap();
        let store = ConfigStore::new(tmp.path().join("debopti.conf"));
        let cfg = store.load().unwrap();
        assert_eq!(cfg.editor_cmd, "micro");
        assert!(!cfg.base_optimized);
        assert!(!cfg.installed);
        assert!(!cfg.ssh_hardened);
        assert!(!cfg.podman_gc_enabled);
        assert_eq!(cfg.is_cn_region, "");
    }

    #[test]
    fn set_preserves_comments_order_unknown_keys_and_bash_source() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("etc/debopti/debopti.conf");
        let store = ConfigStore::new(&path);

        store.set("IS_CN_REGION", "true").unwrap();
        store.set("BASE_OPTIMIZED", "true").unwrap();
        store.set("INSTALLED", "true").unwrap();
        store.set("SCRIPT_VERSION", "1.0.7").unwrap();
        store.set("SCRIPT_BUILD", "231e05eb").unwrap();
        store.set("SSH_HARDENED", "true").unwrap();
        store.set("EDITOR_CMD", "nano").unwrap();
        store.set("PODMAN_GC_ENABLED", "true").unwrap();
        store.set("YAZI_MEDIA_PREVIEW", "false").unwrap();
        store.set("XRAY_GEO_AUTO_UPDATE", "true").unwrap();
        store.set("CUSTOM_KEY", "hello world").unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# debopty (Debian Optimizer Script) 全局持久化配置文件"));
        assert!(content.contains("CUSTOM_KEY=\"hello world\""));

        let cfg = store.load().unwrap();
        assert!(cfg.is_cn_region_enabled());
        assert!(cfg.base_optimized);
        assert!(cfg.installed);
        assert_eq!(cfg.script_version, "1.0.7");
        assert_eq!(cfg.script_build, "231e05eb");
        assert!(cfg.ssh_hardened);
        assert_eq!(cfg.editor_cmd, "nano");
        assert!(cfg.podman_gc_enabled);
        assert_eq!(cfg.yazi_media_preview, "false");
        assert_eq!(cfg.xray_geo_auto_update, "true");
        assert_eq!(
            cfg.extra.get("CUSTOM_KEY").map(String::as_str),
            Some("hello world")
        );

        // Verify Bash can source the file cleanly under set -euo pipefail
        let out = Command::new("bash")
            .args([
                "-c",
                "set -euo pipefail; source \"$1\"; printf '%s|%s|%s' \"$IS_CN_REGION\" \"$EDITOR_CMD\" \"$CUSTOM_KEY\"",
                "--",
                path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "true|nano|hello world"
        );
    }

    #[test]
    fn uncomment_and_reject_invalid_keys() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("debopti.conf");
        fs::write(&path, "# header\n#FOO=\"old\"\nBAR=\"keep\"\n").unwrap();
        let store = ConfigStore::new(&path);
        store.set("FOO", "new").unwrap();

        assert_eq!(store.get("FOO").unwrap().as_deref(), Some("new"));
        assert_eq!(store.get("BAR").unwrap().as_deref(), Some("keep"));
        assert!(store.set("BAD-KEY", "x").is_err());
        assert!(store.set("FOO", "line1\nline2").is_err());
    }
}
