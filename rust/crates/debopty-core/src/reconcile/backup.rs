use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::sys;

static RUN_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupEntry {
    pub original_path: String,
    pub backup_rel_path: Option<String>,
    pub existed: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub run_id: String,
    pub entries: Vec<BackupEntry>,
}

fn validate_rel_path(rel: &str) -> Result<String> {
    if rel.is_empty() || rel.contains('\0') || rel.contains('\\') {
        return Err(CoreError::ValidationFailed(format!(
            "invalid backup relative path: '{rel}'"
        )));
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(CoreError::ValidationFailed(format!(
            "backup manifest path must be relative: '{rel}'"
        )));
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(os) => {
                let s = os.to_str().ok_or_else(|| {
                    CoreError::ValidationFailed(format!(
                        "non-UTF-8 path component in backup manifest: '{rel}'"
                    ))
                })?;
                parts.push(s.to_string());
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(CoreError::ValidationFailed(format!(
                    "path traversal rejected in backup manifest: '{rel}'"
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(CoreError::ValidationFailed(format!(
            "empty backup relative path: '{rel}'"
        )));
    }
    Ok(parts.join("/"))
}

pub(crate) fn prefixed(root: &Path, abs: &str) -> PathBuf {
    let clean = abs.trim_start_matches('/');
    if root.as_os_str().is_empty() || root == Path::new("/") {
        PathBuf::from("/").join(clean)
    } else {
        root.join(clean)
    }
}

fn generate_run_id() -> String {
    let ts = diag::utc_rfc3339_now().replace(['-', ':'], "");
    let seq = RUN_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{ts}-p{pid}-{seq}")
}

/// Lazy atomic backup session created only on first actual mutation.
pub(crate) struct BackupSession {
    root_prefix: PathBuf,
    backup_base_dir: PathBuf,
    run_id: String,
    run_dir: Option<PathBuf>,
    entries: Vec<BackupEntry>,
    recorded: BTreeSet<String>,
}

impl BackupSession {
    pub(crate) fn new(root_prefix: PathBuf, backup_base_dir: PathBuf) -> Self {
        Self {
            root_prefix,
            backup_base_dir,
            run_id: generate_run_id(),
            run_dir: None,
            entries: Vec::new(),
            recorded: BTreeSet::new(),
        }
    }

    pub(crate) fn run_dir(&self) -> Option<&Path> {
        self.run_dir.as_deref()
    }

    fn ensure_run_dir(&mut self) -> Result<PathBuf> {
        if let Some(ref dir) = self.run_dir {
            return Ok(dir.clone());
        }
        fs::create_dir_all(&self.backup_base_dir)?;
        fs::set_permissions(&self.backup_base_dir, fs::Permissions::from_mode(0o700))?;
        let dir = self.backup_base_dir.join(&self.run_id);
        fs::create_dir_all(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        self.run_dir = Some(dir.clone());
        diag::log_event(
            LogLevel::Info,
            "debopty::reconcile::backup",
            None,
            "created reconcile backup run directory",
            None,
            &[("backup_dir", &dir.to_string_lossy())],
        );
        Ok(dir)
    }

    fn write_manifest(&self, run_dir: &Path) -> Result<()> {
        let manifest = BackupManifest {
            run_id: self.run_id.clone(),
            entries: self.entries.clone(),
        };
        let mut json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| CoreError::Reconcile(format!("serialize backup manifest failed: {e}")))?;
        json.push('\n');
        sys::write_file_atomic(&run_dir.join("manifest.json"), json.as_bytes(), Some(0o600))
    }

    /// Snapshot `abs_logical_path` once per run before mutation or quarantine.
    ///
    /// Returns the backup relative path inside `<run_dir>` if file bytes were copied.
    pub(crate) fn snapshot_path(&mut self, abs_logical_path: &str) -> Result<Option<String>> {
        let rel = validate_rel_path(abs_logical_path.trim_start_matches('/'))?;
        if self.recorded.contains(&rel) {
            return Ok(self
                .entries
                .iter()
                .find(|e| e.original_path == rel)
                .and_then(|e| e.backup_rel_path.clone()));
        }

        let run_dir = self.ensure_run_dir()?;
        let target = prefixed(&self.root_prefix, abs_logical_path);

        let entry = match fs::symlink_metadata(&target) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => BackupEntry {
                original_path: rel.clone(),
                backup_rel_path: None,
                existed: false,
                is_symlink: false,
                symlink_target: None,
                mode: None,
                uid: None,
                gid: None,
            },
            Err(e) => return Err(CoreError::Io(e)),
            Ok(meta) if meta.file_type().is_symlink() => {
                let link_target = fs::read_link(&target)?;
                BackupEntry {
                    original_path: rel.clone(),
                    backup_rel_path: None,
                    existed: true,
                    is_symlink: true,
                    symlink_target: Some(link_target.to_string_lossy().into_owned()),
                    mode: None,
                    uid: Some(meta.uid()),
                    gid: Some(meta.gid()),
                }
            }
            Ok(meta) if meta.is_file() => {
                let mode = meta.permissions().mode() & 0o7777;
                let uid = meta.uid();
                let gid = meta.gid();
                let backup_rel = format!("files/{rel}");
                let dest = run_dir.join(&backup_rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                let bytes = fs::read(&target)?;
                // Preserve exact source permission mode (e.g. 0600 for authorized_keys/secrets).
                sys::write_file_atomic(&dest, &bytes, Some(mode))?;
                BackupEntry {
                    original_path: rel.clone(),
                    backup_rel_path: Some(backup_rel),
                    existed: true,
                    is_symlink: false,
                    symlink_target: None,
                    mode: Some(mode),
                    uid: Some(uid),
                    gid: Some(gid),
                }
            }
            Ok(meta) => {
                let mode = meta.permissions().mode() & 0o7777;
                BackupEntry {
                    original_path: rel.clone(),
                    backup_rel_path: None,
                    existed: true,
                    is_symlink: false,
                    symlink_target: None,
                    mode: Some(mode),
                    uid: Some(meta.uid()),
                    gid: Some(meta.gid()),
                }
            }
        };

        let ret = entry.backup_rel_path.clone();
        self.recorded.insert(rel);
        self.entries.push(entry);
        self.write_manifest(&run_dir)?;
        Ok(ret)
    }

    /// Restore a subset of logical paths captured in this session (for transactional rollback).
    pub(crate) fn restore_paths(&self, abs_logical_paths: &[String]) -> Result<()> {
        let Some(ref run_dir) = self.run_dir else {
            return Ok(());
        };
        for abs in abs_logical_paths {
            let rel = abs.trim_start_matches('/');
            if let Some(entry) = self.entries.iter().find(|e| e.original_path == rel) {
                restore_single_entry(entry, run_dir, &self.root_prefix)?;
            }
        }
        Ok(())
    }
}

fn restore_single_entry(entry: &BackupEntry, run_dir: &Path, root_prefix: &Path) -> Result<()> {
    let clean_orig = validate_rel_path(&entry.original_path)?;
    let target = prefixed(root_prefix, &clean_orig);

    if !entry.existed {
        if let Ok(meta) = fs::symlink_metadata(&target) {
            if meta.is_dir() && !meta.file_type().is_symlink() {
                let _ = fs::remove_dir_all(&target);
            } else {
                fs::remove_file(&target)?;
            }
        }
        return Ok(());
    }

    if entry.is_symlink {
        let Some(ref link_target) = entry.symlink_target else {
            return Err(CoreError::Reconcile(format!(
                "missing symlink target in backup manifest for '{}'",
                entry.original_path
            )));
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::symlink_metadata(&target).is_ok() {
            let _ = fs::remove_file(&target);
        }
        symlink(link_target, &target)?;
        return Ok(());
    }

    let owner = entry.uid.zip(entry.gid);
    if let Some(ref rel_bak) = entry.backup_rel_path {
        let clean_bak = validate_rel_path(rel_bak)?;
        let src = run_dir.join(&clean_bak);
        let bytes = fs::read(&src)?;
        if let Ok(meta) = fs::symlink_metadata(&target)
            && meta.file_type().is_symlink()
        {
            fs::remove_file(&target)?;
        }
        sys::write_file_atomic_with_owner(
            &target,
            &bytes,
            Some(entry.mode.unwrap_or(0o644)),
            owner,
        )?;
    } else if target.exists() {
        if let Some((uid, gid)) = owner {
            sys::chown_path(&target, uid, gid)?;
        }
        if let Some(mode) = entry.mode {
            fs::set_permissions(&target, fs::Permissions::from_mode(mode))?;
        }
    }

    Ok(())
}

/// Create an atomic point-in-time snapshot of `logical_paths` beneath `root_prefix` inside
/// a new `0700` run directory under `backup_base_dir` with a `0600` `manifest.json`.
///
/// Always creates the run directory and manifest even when `logical_paths` is empty.
/// Rejects path traversal strictly before mutating disk.
pub fn create_snapshot(
    root_prefix: &Path,
    backup_base_dir: &Path,
    logical_paths: &[String],
) -> Result<(String, PathBuf, BackupManifest)> {
    for raw in logical_paths {
        validate_rel_path(raw.trim_start_matches('/'))?;
    }

    let mut session = BackupSession::new(root_prefix.to_path_buf(), backup_base_dir.to_path_buf());
    let run_dir = session.ensure_run_dir()?;
    for raw in logical_paths {
        session.snapshot_path(raw)?;
    }
    session.write_manifest(&run_dir)?;

    let manifest = BackupManifest {
        run_id: session.run_id.clone(),
        entries: session.entries,
    };
    Ok((session.run_id, run_dir, manifest))
}

/// Restore all files, modes, and symlinks recorded in `<backup_run_dir>/manifest.json`
/// back onto `root_prefix`. Rejects path traversal strictly.
pub fn restore_backup(backup_run_dir: &Path, root_prefix: &Path) -> Result<usize> {
    let manifest_path = backup_run_dir.join("manifest.json");
    if !manifest_path.is_file() {
        return Err(CoreError::Reconcile(format!(
            "backup manifest not found: {}",
            manifest_path.display()
        )));
    }

    let raw = fs::read_to_string(&manifest_path)?;
    let manifest: BackupManifest = serde_json::from_str(&raw)
        .map_err(|e| CoreError::Reconcile(format!("invalid backup manifest JSON: {e}")))?;

    // Validate all entries before mutating anything on disk.
    for entry in &manifest.entries {
        validate_rel_path(&entry.original_path)?;
        if let Some(ref rel_bak) = entry.backup_rel_path {
            validate_rel_path(rel_bak)?;
        }
    }

    let mut restored = 0usize;
    for entry in &manifest.entries {
        restore_single_entry(entry, backup_run_dir, root_prefix)?;
        restored += 1;
    }

    diag::log_event(
        LogLevel::Info,
        "debopty::reconcile::backup",
        None,
        "restored reconcile backup manifest",
        None,
        &[
            ("backup_dir", &backup_run_dir.to_string_lossy()),
            ("restored_count", &restored.to_string()),
        ],
    );

    Ok(restored)
}
