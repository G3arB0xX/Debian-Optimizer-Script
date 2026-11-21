use std::cmp::Ordering;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionIdentity {
    pub semver: Option<String>,
    pub calver: Option<String>,
    pub build: String,
}

impl VersionIdentity {
    /// Returns the primary version string (prefers `semver` over `calver`).
    pub fn primary(&self) -> Option<&str> {
        self.semver
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.calver.as_deref().filter(|s| !s.is_empty()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BumpKind {
    Major,
    Minor,
    Patch,
    Prerelease,
}

fn semver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-((0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*)(\.(0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*))*))?(\+[0-9a-zA-Z-]+(\.[0-9a-zA-Z-]+)*)?$",
        )
        .expect("valid semver regex")
    })
}

fn calver_date_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[2-9][0-9]\.(0[1-9]|1[0-2])\.(0[1-9]|[12][0-9]|3[01])$")
            .expect("valid calver date regex")
    })
}

fn version_shape_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[0-9]{2}\.[0-9]{2}\.[0-9]{2}$").expect("valid shape regex"))
}

fn legacy_calver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^([0-9]{4})\.([0-9]{2})\.([0-9]{2})(\.[0-9]+)?$")
            .expect("valid legacy calver regex")
    })
}

fn build_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z0-9]+$").expect("valid build regex"))
}

fn zero_padded_segment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\.(0[0-9])").expect("valid zero-padded segment regex"))
}

fn pre_numeric_suffix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(.*)\.([0-9]+)$").expect("valid prerelease suffix regex"))
}

/// Returns `true` iff `v` is a strict SemVer 2.0.0 string.
pub fn is_semver(v: &str) -> bool {
    !v.is_empty() && semver_re().is_match(v)
}

/// Returns `true` iff `v` matches `YY.MM.DD` CalVer date range (`20..99`, `01..12`, `01..31`).
pub fn is_calver(v: &str) -> bool {
    !v.is_empty() && calver_date_re().is_match(v)
}

/// Validate and emit canonical SemVer tag `vX.Y.Z[-prerelease]`.
pub fn semver_tag(v: &str) -> Result<String> {
    let stripped = v.strip_prefix('v').unwrap_or(v);
    if !is_semver(stripped) {
        return Err(CoreError::InvalidVersion(format!(
            "not a valid SemVer 2.0.0 string: {v}"
        )));
    }
    Ok(format!("v{stripped}"))
}

/// Compare two strict SemVer 2.0.0 strings per SemVer 2.0.0 section 11.
/// Build metadata (`+...`) is ignored.
pub fn semver_compare(v1: &str, v2: &str) -> Result<Ordering> {
    if !is_semver(v1) {
        return Err(CoreError::InvalidVersion(format!(
            "not a valid SemVer 2.0.0 string: {v1}"
        )));
    }
    if !is_semver(v2) {
        return Err(CoreError::InvalidVersion(format!(
            "not a valid SemVer 2.0.0 string: {v2}"
        )));
    }

    let base1 = v1.split('+').next().unwrap_or(v1);
    let base2 = v2.split('+').next().unwrap_or(v2);

    let (core1, pre1) = match base1.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (base1, None),
    };
    let (core2, pre2) = match base2.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (base2, None),
    };

    let mut c1 = core1.split('.');
    let mut c2 = core2.split('.');
    for _ in 0..3 {
        let n1: u64 = c1
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| CoreError::InvalidVersion(v1.to_string()))?;
        let n2: u64 = c2
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| CoreError::InvalidVersion(v2.to_string()))?;
        match n1.cmp(&n2) {
            Ordering::Equal => {}
            ord => return Ok(ord),
        }
    }

    match (pre1, pre2) {
        (None, None) => Ok(Ordering::Equal),
        (None, Some(_)) => Ok(Ordering::Greater),
        (Some(_), None) => Ok(Ordering::Less),
        (Some(p1), Some(p2)) => {
            let parts1: Vec<&str> = p1.split('.').collect();
            let parts2: Vec<&str> = p2.split('.').collect();
            for (id1, id2) in parts1.iter().zip(parts2.iter()) {
                if id1 == id2 {
                    continue;
                }
                let num1 = id1.bytes().all(|b| b.is_ascii_digit());
                let num2 = id2.bytes().all(|b| b.is_ascii_digit());
                match (num1, num2) {
                    (true, true) => {
                        let n1: u128 = id1.parse().unwrap_or(0);
                        let n2: u128 = id2.parse().unwrap_or(0);
                        return Ok(n1.cmp(&n2));
                    }
                    (true, false) => return Ok(Ordering::Less),
                    (false, true) => return Ok(Ordering::Greater),
                    (false, false) => return Ok(id1.as_bytes().cmp(id2.as_bytes())),
                }
            }
            Ok(parts1.len().cmp(&parts2.len()))
        }
    }
}

/// Compute next SemVer 2.0.0 string matching `scripts/version.sh::debopti_next_semver`.
pub fn next_semver(
    current: Option<&str>,
    bump: BumpKind,
    prerelease: Option<&str>,
) -> Result<String> {
    let mut pre = prerelease.filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(ref p) = pre
        && !is_semver(&format!("1.0.0-{p}"))
    {
        return Err(CoreError::InvalidBump(format!(
            "invalid prerelease identifier: {p}"
        )));
    }

    let cur = current.filter(|s| !s.is_empty());
    let Some(cur_str) = cur else {
        if let Some(p) = pre {
            return Ok(format!("1.0.0-{p}"));
        }
        return if bump == BumpKind::Prerelease {
            Ok("1.0.0-rc.1".to_string())
        } else {
            Ok("1.0.0".to_string())
        };
    };

    if !is_semver(cur_str) {
        return Err(CoreError::InvalidVersion(format!(
            "not a valid SemVer 2.0.0 string: {cur_str}"
        )));
    }

    let base = cur_str.split('+').next().unwrap_or(cur_str);
    let (core, cur_pre) = match base.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (base, None),
    };

    let mut segs = core.split('.');
    let mut maj: u64 = segs
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CoreError::InvalidVersion(cur_str.to_string()))?;
    let mut min: u64 = segs
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CoreError::InvalidVersion(cur_str.to_string()))?;
    let mut pat: u64 = segs
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CoreError::InvalidVersion(cur_str.to_string()))?;

    match bump {
        BumpKind::Major => {
            maj += 1;
            min = 0;
            pat = 0;
        }
        BumpKind::Minor => {
            min += 1;
            pat = 0;
        }
        BumpKind::Patch => {
            pat += 1;
        }
        BumpKind::Prerelease => {
            if cur_pre.is_none() {
                pat += 1;
                if pre.is_none() {
                    pre = Some("rc.1".to_string());
                }
            } else if pre.is_none() {
                let cp = cur_pre.unwrap_or_default();
                if let Some(caps) = pre_numeric_suffix_re().captures(cp) {
                    let prefix = &caps[1];
                    let num: u64 = caps[2].parse().unwrap_or(0);
                    pre = Some(format!("{prefix}.{}", num + 1));
                } else {
                    pre = Some(format!("{cp}.1"));
                }
            }
        }
    }

    match pre {
        Some(p) => Ok(format!("{maj}.{min}.{pat}-{p}")),
        None => Ok(format!("{maj}.{min}.{pat}")),
    }
}

/// Map legacy four-digit calendar strings (`YYYY.MM.DD[.N]`) to `YY.MM.DD`.
pub fn normalize_calver(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }
    if let Some(caps) = legacy_calver_re().captures(v) {
        let yy = &caps[1][2..4];
        let mm = &caps[2];
        let dd = &caps[3];
        return format!("{yy}.{mm}.{dd}");
    }
    v.to_string()
}

fn unquote(val: &str) -> String {
    let mut s = val.trim();
    if let Some(rest) = s.strip_prefix('"') {
        s = rest;
    }
    if let Some(rest) = s.strip_suffix('"') {
        s = rest;
    }
    if let Some(rest) = s.strip_prefix('\'') {
        s = rest;
    }
    if let Some(rest) = s.strip_suffix('\'') {
        s = rest;
    }
    s.to_string()
}

fn extract_kv(text: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&prefix) {
            return Some(unquote(rest));
        }
    }
    None
}

/// Parse `VERSION` file text or legacy `scripts/common.sh` `VERSION_ID=` line.
pub fn parse_version_text(text: &str) -> Result<VersionIdentity> {
    let semver_raw = extract_kv(text, "DEBOPTI_SEMVER").unwrap_or_default();
    let mut calver_raw = extract_kv(text, "DEBOPTI_CALVER").unwrap_or_default();
    let legacy_ver = extract_kv(text, "DEBOPTI_VERSION").unwrap_or_default();
    let mut build = extract_kv(text, "DEBOPTI_BUILD").unwrap_or_default();
    if build.is_empty() {
        build = extract_kv(text, "VERSION_ID").unwrap_or_default();
    }

    if !semver_raw.is_empty() && !is_semver(&semver_raw) {
        return Err(CoreError::InvalidVersion(format!(
            "invalid DEBOPTI_SEMVER: {semver_raw}"
        )));
    }

    if calver_raw.is_empty() && !legacy_ver.is_empty() {
        calver_raw = legacy_ver;
    }
    if !calver_raw.is_empty() {
        calver_raw = normalize_calver(&calver_raw);
        if !version_shape_re().is_match(&calver_raw) {
            return Err(CoreError::InvalidVersion(format!(
                "invalid DEBOPTI_CALVER: {calver_raw}"
            )));
        }
    }

    if build.is_empty() || !build_re().is_match(&build) {
        return Err(CoreError::InvalidVersion(format!(
            "invalid or missing build id: {build}"
        )));
    }

    Ok(VersionIdentity {
        semver: if semver_raw.is_empty() {
            None
        } else {
            Some(semver_raw)
        },
        calver: if calver_raw.is_empty() {
            None
        } else {
            Some(calver_raw)
        },
        build,
    })
}

/// Parse a `VERSION` file from disk.
pub fn parse_version_file(path: &Path) -> Result<VersionIdentity> {
    let text = fs::read_to_string(path)?;
    parse_version_text(&text)
}

/// Write canonical `VERSION` file atomically (strictly omits `DEBOPTI_VERSION=`).
pub fn write_version_file(path: &Path, id: &VersionIdentity) -> Result<()> {
    if let Some(ref sv) = id.semver
        && !sv.is_empty()
        && !is_semver(sv)
    {
        return Err(CoreError::InvalidVersion(format!(
            "invalid DEBOPTI_SEMVER: {sv}"
        )));
    }
    if id.build.is_empty() || !build_re().is_match(&id.build) {
        return Err(CoreError::InvalidVersion(format!(
            "invalid DEBOPTI_BUILD: {}",
            id.build
        )));
    }

    let calver = id
        .calver
        .as_deref()
        .map(normalize_calver)
        .unwrap_or_default();

    let mut content = String::new();
    if let Some(ref sv) = id.semver
        && !sv.is_empty()
    {
        content.push_str(&format!("DEBOPTI_SEMVER=\"{sv}\"\n"));
    }
    content.push_str(&format!("DEBOPTI_CALVER=\"{calver}\"\n"));
    content.push_str(&format!("DEBOPTI_BUILD=\"{}\"\n", id.build));

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(content.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| CoreError::Io(e.error))?;
    Ok(())
}

/// Returns `true` iff `path` uses the canonical on-disk `VERSION` format
/// (has `DEBOPTI_CALVER=` and `DEBOPTI_BUILD=`, omits `DEBOPTI_VERSION=`, and valid `DEBOPTI_SEMVER=` if present).
pub fn is_canonical_version_file(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let has_calver = text.lines().any(|l| l.starts_with("DEBOPTI_CALVER="));
    let has_legacy_version = text.lines().any(|l| l.starts_with("DEBOPTI_VERSION="));
    let has_build = text.lines().any(|l| l.starts_with("DEBOPTI_BUILD="));
    if !has_calver || has_legacy_version || !has_build {
        return false;
    }
    if let Some(sv) = extract_kv(&text, "DEBOPTI_SEMVER")
        && !is_semver(&sv)
    {
        return false;
    }
    true
}

/// Emit canonical git tag name, disambiguating CalVer `YY.MM.DD` from SemVer `X.Y.Z`.
pub fn tag_name(version: &str, build: Option<&str>) -> String {
    let build = build.filter(|b| !b.is_empty());
    if version_shape_re().is_match(version)
        && (is_calver(version) || zero_padded_segment_re().is_match(version))
    {
        return match build {
            Some(b) => format!("v{version}-{b}"),
            None => format!("v{version}"),
        };
    }
    if is_semver(version) {
        return format!("v{version}");
    }
    match build {
        Some(b) => format!("v{version}-{b}"),
        None => format!("v{version}"),
    }
}

/// Returns `true` when an update should be offered (remote build present and differs).
pub fn needs_update(local_build: &str, remote_build: &str) -> bool {
    !remote_build.is_empty() && local_build != remote_build
}

/// Format display string (`"<primary> (<build>)"` or `"<build>"`).
pub fn format_display(id: &VersionIdentity) -> String {
    if let Some(primary) = id.primary() {
        let norm = normalize_calver(primary);
        if !norm.is_empty() {
            return format!("{norm} ({})", id.build);
        }
    }
    id.build.clone()
}

/// Format prompt string (`"<primary> (build <build>)"` or `"build <build>"`).
pub fn format_prompt(id: &VersionIdentity) -> String {
    if let Some(primary) = id.primary() {
        let norm = normalize_calver(primary);
        if !norm.is_empty() {
            return format!("{norm} (build {})", id.build);
        }
    }
    format!("build {}", id.build)
}

/// Verify an extracted release tree's `VERSION` (or legacy `scripts/common.sh`) matches expected identity.
pub fn verify_tree(dir: &Path, expected_version: Option<&str>, expected_build: &str) -> bool {
    let version_file = dir.join("VERSION");
    let common_file = dir.join("scripts/common.sh");

    let parsed = if version_file.is_file() {
        parse_version_file(&version_file)
    } else if common_file.is_file() {
        fs::read_to_string(&common_file)
            .map_err(CoreError::from)
            .and_then(|t| parse_version_text(&t))
    } else {
        return false;
    };

    let Ok(id) = parsed else {
        return false;
    };
    if id.build != expected_build {
        return false;
    }

    if let Some(exp_ver) = expected_version.filter(|s| !s.is_empty())
        && (id.semver.is_some() || id.calver.is_some())
    {
        let norm_exp = normalize_calver(exp_ver);
        if id.semver.as_deref() == Some(exp_ver) {
            return true;
        }
        if id.calver.as_deref() == Some(norm_exp.as_str()) {
            return true;
        }
        return false;
    }
    true
}

/// Returns the version identity compiled into the binary from `VERSION` via `build.rs`.
pub fn compiled_identity() -> VersionIdentity {
    let semver = option_env!("DEBOPTY_BUILD_SEMVER")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let calver = option_env!("DEBOPTY_BUILD_CALVER")
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let build = option_env!("DEBOPTY_BUILD_ID")
        .unwrap_or_default()
        .to_string();
    VersionIdentity {
        semver,
        calver,
        build,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn semver_validation() {
        assert!(is_semver("1.0.0"));
        assert!(is_semver("0.1.0-rc.1+build.42"));
        assert!(!is_semver("01.0.0"));
        assert!(!is_semver("1.0.0-01"));
        assert!(!is_semver("26.08.15"));
        assert!(!is_semver(""));
    }

    #[test]
    fn calver_validation_and_normalization() {
        assert!(is_calver("26.11.21"));
        assert!(is_calver("26.08.15"));
        assert!(!is_calver("1.0.0"));
        assert!(!is_calver("26.13.01"));
        assert_eq!(normalize_calver("2026.08.16.1"), "26.08.16");
        assert_eq!(normalize_calver("26.08.16"), "26.08.16");
        assert_eq!(normalize_calver(""), "");
    }

    #[test]
    fn semver_section_11_ordering() {
        assert_eq!(semver_compare("1.0.0", "1.0.0").unwrap(), Ordering::Equal);
        assert_eq!(
            semver_compare("1.0.0+build.1", "1.0.0+build.2").unwrap(),
            Ordering::Equal
        );
        assert_eq!(semver_compare("1.0.0", "2.0.0").unwrap(), Ordering::Less);
        assert_eq!(semver_compare("1.2.0", "1.1.9").unwrap(), Ordering::Greater);
        assert_eq!(semver_compare("1.0.1", "1.0.2").unwrap(), Ordering::Less);
        assert_eq!(
            semver_compare("1.0.0-alpha", "1.0.0").unwrap(),
            Ordering::Less
        );
        assert_eq!(
            semver_compare("1.0.0", "1.0.0-rc.1").unwrap(),
            Ordering::Greater
        );
        assert_eq!(
            semver_compare("1.0.0-alpha.1", "1.0.0-alpha.beta").unwrap(),
            Ordering::Less
        );
        assert_eq!(
            semver_compare("1.0.0-alpha.2", "1.0.0-alpha.10").unwrap(),
            Ordering::Less
        );
        assert_eq!(
            semver_compare("1.0.0-alpha", "1.0.0-alpha.1").unwrap(),
            Ordering::Less
        );
    }

    #[test]
    fn next_semver_bumps() {
        assert_eq!(next_semver(None, BumpKind::Minor, None).unwrap(), "1.0.0");
        assert_eq!(
            next_semver(Some(""), BumpKind::Minor, Some("rc.1")).unwrap(),
            "1.0.0-rc.1"
        );
        assert_eq!(
            next_semver(None, BumpKind::Prerelease, None).unwrap(),
            "1.0.0-rc.1"
        );
        assert_eq!(
            next_semver(Some("1.2.3"), BumpKind::Patch, None).unwrap(),
            "1.2.4"
        );
        assert_eq!(
            next_semver(Some("1.2.3"), BumpKind::Minor, None).unwrap(),
            "1.3.0"
        );
        assert_eq!(
            next_semver(Some("1.2.3"), BumpKind::Major, None).unwrap(),
            "2.0.0"
        );
        assert_eq!(
            next_semver(Some("1.2.3-rc.1"), BumpKind::Minor, None).unwrap(),
            "1.3.0"
        );
        assert_eq!(
            next_semver(Some("1.2.3"), BumpKind::Minor, Some("rc.1")).unwrap(),
            "1.3.0-rc.1"
        );
        assert_eq!(
            next_semver(Some("1.2.3-rc.1"), BumpKind::Prerelease, None).unwrap(),
            "1.2.3-rc.2"
        );
    }

    #[test]
    fn tag_disambiguation_and_needs_update() {
        assert_eq!(semver_tag("1.2.3").unwrap(), "v1.2.3");
        assert_eq!(semver_tag("v1.2.3-rc.1").unwrap(), "v1.2.3-rc.1");
        assert_eq!(tag_name("1.2.3", Some("abcd1234")), "v1.2.3");
        assert_eq!(tag_name("1.2.3-rc.1", Some("abcd1234")), "v1.2.3-rc.1");
        assert_eq!(tag_name("26.11.21", Some("abcd1234")), "v26.11.21-abcd1234");
        assert_eq!(tag_name("26.08.15", Some("uxvkolop")), "v26.08.15-uxvkolop");

        assert!(needs_update("aaa", "bbb"));
        assert!(!needs_update("aaa", "aaa"));
        assert!(!needs_update("aaa", ""));
        assert!(needs_update("", "bbb"));
    }

    #[test]
    fn parse_write_canonical_and_verify_tree() {
        let tmp = TempDir::new().unwrap();
        let ver_path = tmp.path().join("VERSION");

        let id = VersionIdentity {
            semver: Some("1.2.0".to_string()),
            calver: Some("26.11.21".to_string()),
            build: "deadbeef".to_string(),
        };
        write_version_file(&ver_path, &id).unwrap();

        let written = fs::read_to_string(&ver_path).unwrap();
        assert!(written.contains("DEBOPTI_SEMVER=\"1.2.0\""));
        assert!(written.contains("DEBOPTI_CALVER=\"26.11.21\""));
        assert!(written.contains("DEBOPTI_BUILD=\"deadbeef\""));
        assert!(!written.contains("DEBOPTI_VERSION="));
        assert!(is_canonical_version_file(&ver_path));

        let parsed = parse_version_file(&ver_path).unwrap();
        assert_eq!(parsed, id);
        assert_eq!(format_display(&parsed), "1.2.0 (deadbeef)");
        assert_eq!(format_prompt(&parsed), "1.2.0 (build deadbeef)");

        assert!(verify_tree(tmp.path(), Some("1.2.0"), "deadbeef"));
        assert!(verify_tree(tmp.path(), Some("26.11.21"), "deadbeef"));
        assert!(!verify_tree(tmp.path(), Some("9.9.9"), "deadbeef"));
        assert!(!verify_tree(tmp.path(), Some("1.2.0"), "wrongbld"));

        // Legacy VERSION_ID= fallback
        let legacy = parse_version_text("VERSION_ID=\"legacyid\"\n").unwrap();
        assert_eq!(legacy.semver, None);
        assert_eq!(legacy.calver, None);
        assert_eq!(legacy.build, "legacyid");
        assert_eq!(format_display(&legacy), "legacyid");

        // Reject DEBOPTI_VERSION= in canonical check
        let non_canon = tmp.path().join("VERSION_BAD");
        fs::write(
            &non_canon,
            "DEBOPTI_VERSION=\"26.08.16\"\nDEBOPTI_BUILD=\"abcd1234\"\n",
        )
        .unwrap();
        assert!(!is_canonical_version_file(&non_canon));
    }

    #[test]
    fn compiled_identity_loaded_from_repo_version() {
        let id = compiled_identity();
        assert!(!id.build.is_empty());
        assert!(id.semver.is_some() || id.calver.is_some());
    }
}
