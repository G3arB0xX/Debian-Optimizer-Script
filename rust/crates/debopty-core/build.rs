use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let repo_root = manifest_dir
        .join("../../..")
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("../../.."));

    emit_version_env(&repo_root);
    emit_embedded_templates(&repo_root);
}

fn unquote(val: &str) -> &str {
    let trimmed = val.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn extract_kv<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(&prefix) {
            return Some(unquote(rest));
        }
    }
    None
}

fn emit_version_env(repo_root: &Path) {
    let version_path = repo_root.join("VERSION");
    println!("cargo:rerun-if-changed={}", version_path.display());

    let mut semver = String::new();
    let mut calver = String::new();
    let mut build_id = String::new();

    if let Ok(content) = fs::read_to_string(&version_path) {
        if let Some(v) = extract_kv(&content, "DEBOPTI_SEMVER") {
            semver = v.to_string();
        }
        if let Some(v) = extract_kv(&content, "DEBOPTI_CALVER") {
            calver = v.to_string();
        }
        if let Some(v) = extract_kv(&content, "DEBOPTI_BUILD") {
            build_id = v.to_string();
        }
    }

    println!("cargo:rustc-env=DEBOPTY_BUILD_SEMVER={semver}");
    println!("cargo:rustc-env=DEBOPTY_BUILD_CALVER={calver}");
    println!("cargo:rustc-env=DEBOPTY_BUILD_ID={build_id}");
}

fn emit_embedded_templates(repo_root: &Path) {
    let templates_dir = repo_root.join("templates");
    println!("cargo:rerun-if-changed={}", templates_dir.display());

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    if templates_dir.is_dir() {
        for entry in WalkDir::new(&templates_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.file_name() == "INDEX.md" {
                continue;
            }
            let abs_path = entry.into_path();
            if let Ok(rel) = abs_path.strip_prefix(repo_root) {
                let rel_posix = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                entries.push((rel_posix, abs_path));
            }
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set"));
    let out_file = out_dir.join("embedded_templates.rs");
    let mut f = fs::File::create(&out_file).expect("failed to create embedded_templates.rs");

    writeln!(f, "pub static EMBEDDED_TEMPLATES: &[(&str, &str)] = &[").unwrap();
    for (rel_posix, abs_path) in &entries {
        println!("cargo:rerun-if-changed={}", abs_path.display());
        let abs_str = abs_path.to_string_lossy();
        writeln!(f, "    ({rel_posix:?}, include_str!({abs_str:?})),").unwrap();
    }
    writeln!(f, "];").unwrap();
}
