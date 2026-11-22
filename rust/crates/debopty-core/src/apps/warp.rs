use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::{NamedTempFile, TempDir};
use walkdir::WalkDir;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, DEFAULT_USER_AGENT, HttpClient, HttpRequest, IpPreference};
use crate::sys::{self, CommandRunner};

pub const MODULE_ID: &str = "warp";

const DIAG_TARGET: &str = "debopty::apps::warp";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];
const WARP_PGP_URL: &str = "https://pkg.cloudflareclient.com/pubkey.gpg";
const WARP_KEYRING_ABS: &str = "/usr/share/keyrings/cloudflare-warp-archive-keyring.gpg";
const USQUE_API_BASE: &str = "https://api.github.com/repos/Diniboy1123/usque/releases/latest";
const WGCF_VERSION: &str = "2.2.22";

/// Official Cloudflare WARP WireGuard peer public key.
pub const WARP_PUBLIC_KEY: &str = "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=";
/// Official Cloudflare WARP WireGuard endpoint.
pub const WARP_ENDPOINT: &str = "162.159.192.1:2408";

/// CN API proxies supporting `api.github.com` (strictly excluding `ghfast.top`).
const CN_API_PROXIES: &[&str] = &[
    "https://gh-proxy.com",
    "https://edgeone.gh-proxy.org",
    "https://gh-proxy.org",
];

fn app_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: MODULE_ID.to_string(),
        message: message.into(),
    }
}

fn map_go_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" | "amd64" => Ok("amd64"),
        "aarch64" | "arm64" => Ok("arm64"),
        other => Err(app_err(format!("unsupported architecture: '{other}'"))),
    }
}

fn warp_ipv4_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"172\.\d+\.\d+\.\d+").expect("valid warp ipv4 regex"))
}

/// Structured Xray WireGuard outbound configuration generated from Cloudflare WARP (`wgcf`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarpXrayOutbound {
    pub secret_key: String,
    pub ipv4: String,
    pub public_key: String,
    pub endpoint: String,
    pub reserved: [u8; 3],
    pub json: String,
}

/// Compute the 3-byte Cloudflare WARP `reserved` array from a base64 `device_id`.
pub fn compute_warp_reserved(device_id: &str) -> Result<[u8; 3]> {
    let decoded = shared::decode_base64(device_id)
        .map_err(|e| app_err(format!("failed to decode WARP device_id base64: {e}")))?;
    if decoded.len() < 3 {
        return Err(app_err(format!(
            "decoded WARP device_id has only {} byte(s); expected at least 3",
            decoded.len()
        )));
    }
    Ok([decoded[0], decoded[1], decoded[2]])
}

/// Render the canonical Xray WireGuard outbound JSON matching the Bash WARP generator schema.
pub fn build_warp_xray_outbound_json(
    secret_key: &str,
    ipv4: &str,
    reserved: [u8; 3],
) -> Result<String> {
    if secret_key.trim().is_empty() {
        return Err(app_err("WARP WireGuard secretKey must not be empty"));
    }
    if ipv4.trim().is_empty() {
        return Err(app_err("WARP WireGuard IPv4 address must not be empty"));
    }
    let val = serde_json::json!({
        "tag": "warp",
        "protocol": "wireguard",
        "settings": {
            "secretKey": secret_key,
            "address": [format!("{ipv4}/32")],
            "peers": [{
                "publicKey": WARP_PUBLIC_KEY,
                "endpoint": WARP_ENDPOINT
            }],
            "reserved": [reserved[0], reserved[1], reserved[2]]
        }
    });
    serde_json::to_string_pretty(&val)
        .map_err(|e| app_err(format!("failed to serialize WARP Xray outbound JSON: {e}")))
}

fn extract_toml_string_field(content: &str, key: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(val_part) = rest.strip_prefix('=') {
                let v = val_part.trim();
                let unquoted = v
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''))
                    .or_else(|| v.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                    .unwrap_or(v);
                if !unquoted.is_empty() {
                    return Some(unquoted.to_string());
                }
            }
        }
    }
    None
}

fn extract_wgcf_ipv4(profile_conf: &str) -> Option<String> {
    for line in profile_conf.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Address")
            && let Some(m) = warp_ipv4_re().find(trimmed)
        {
            return Some(m.as_str().to_string());
        }
    }
    None
}

/// Install or update Cloudflare WARP CLI with U8 registration/mode preservation and systemd override.
pub fn install_warp(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let is_update = ctx.is_cmd_present("warp-cli", runner);
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting Cloudflare WARP installation",
        None,
        &[("is_update", if is_update { "true" } else { "false" })],
    );

    if !ctx.no_sys {
        sys::safe_apt_install(&["lsb-release", "gnupg"], runner)
            .map_err(|e| app_err(format!("failed to install WARP prerequisites: {e}")))?;
    }

    let codename = shared::detect_debian_codename(ctx, runner)
        .map_err(|e| app_err(format!("failed to detect Debian codename: {e}")))?;
    let repo_line = format!(
        "deb [signed-by={WARP_KEYRING_ABS}] https://pkg.cloudflareclient.com/ {codename} main"
    );
    let repo_list = shared::install_apt_keyring_and_repo(
        ctx,
        "cloudflare-client",
        WARP_PGP_URL,
        WARP_KEYRING_ABS,
        &repo_line,
        http,
        runner,
    )
    .map_err(|e| {
        app_err(format!(
            "failed to configure Cloudflare APT repository: {e}"
        ))
    })?;

    if !ctx.no_sys {
        let _ = runner.run_with_env(
            "apt-get",
            &["update", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
        sys::safe_apt_install(&["cloudflare-warp"], runner)
            .map_err(|e| app_err(format!("failed to install cloudflare-warp package: {e}")))?;

        let _ = runner.run(
            "systemctl",
            &["enable", "--now", "warp-svc"],
            Duration::from_secs(30),
        );

        let mut ready = false;
        for attempt in 0..10u8 {
            if let Ok(out) = runner.run(
                "warp-cli",
                &["--accept-tos", "status"],
                Duration::from_secs(10),
            ) && out.success()
            {
                ready = true;
                break;
            }
            if attempt < 9 && !ctx.no_sys && !ctx.is_custom_root() {
                thread::sleep(Duration::from_secs(1));
            }
        }
        if !ready {
            return Err(app_err(
                "WARP daemon timed out waiting for 'warp-cli --accept-tos status'",
            ));
        }

        if !is_update {
            diag::log_event(
                LogLevel::Info,
                DIAG_TARGET,
                None,
                "registering new WARP device and configuring WireGuard Socks5 proxy",
                None,
                &[],
            );
            let reg = runner.run(
                "warp-cli",
                &["--accept-tos", "registration", "new"],
                Duration::from_secs(30),
            )?;
            if !reg.success() {
                return Err(app_err(format!(
                    "WARP registration failed (code {}): {}",
                    reg.code,
                    reg.stderr.trim()
                )));
            }
            for args in [
                &["--accept-tos", "tunnel", "protocol", "set", "WireGuard"][..],
                &["--accept-tos", "dns", "families", "off"][..],
                &["--accept-tos", "mode", "proxy"][..],
                &["--accept-tos", "proxy", "port", "40000"][..],
                &["--accept-tos", "connect"][..],
            ] {
                let _ = runner.run("warp-cli", args, Duration::from_secs(20));
            }
        }
    }

    let override_path = shared::inject_service_override(
        ctx,
        "warp-svc",
        "templates/apps/warp/warp-svc.service.override.conf",
        &BTreeMap::new(),
        runner,
    )
    .map_err(|e| app_err(format!("failed to inject warp-svc systemd override: {e}")))?;

    let mut report = AppReport::new(
        MODULE_ID,
        if is_update { "update" } else { "install" },
        true,
        "Cloudflare WARP deployed (Socks5 proxy 127.0.0.1:40000)",
    )
    .with_detail("codename", codename)
    .with_detail("repo_list", repo_list.to_string_lossy().into_owned())
    .with_detail(
        "override_path",
        override_path.to_string_lossy().into_owned(),
    )
    .with_detail("proxy_endpoint", "127.0.0.1:40000");

    if is_update {
        report = report.with_preserved(
            "existing WARP registration, protocol, mode, and proxy port preserved on update",
        );
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "Cloudflare WARP deployment finished",
        None,
        &[("proxy", "127.0.0.1:40000")],
    );
    Ok(report)
}

/// Uninstall Cloudflare WARP CLI, repository list, systemd override, and state directory.
pub fn uninstall_warp(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Cloudflare WARP",
        None,
        &[],
    );

    if !ctx.no_sys {
        let _ = runner.run(
            "warp-cli",
            &["--accept-tos", "disconnect"],
            Duration::from_secs(15),
        );
        let _ = runner.run(
            "warp-cli",
            &["--accept-tos", "registration", "delete"],
            Duration::from_secs(15),
        );
        let _ = runner.run_with_env(
            "apt-get",
            &["purge", "-yq", "cloudflare-warp"],
            APT_ENV,
            Duration::from_secs(180),
        );
    }

    for file_abs in [
        "/etc/apt/sources.list.d/cloudflare-client.list",
        WARP_KEYRING_ABS,
    ] {
        let f = ctx.path(file_abs);
        if f.exists() || f.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&f);
        }
    }
    let override_dir = ctx.path("/etc/systemd/system/warp-svc.service.d");
    if override_dir.exists() {
        let _ = fs::remove_dir_all(&override_dir);
    }
    let opt_dir = ctx.path("/opt/cloudflare-warp");
    if opt_dir.exists() {
        let _ = fs::remove_dir_all(&opt_dir);
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    Ok(AppReport::new(
        MODULE_ID,
        "uninstall",
        true,
        "Cloudflare WARP removed",
    ))
}

fn resolve_usque_download_url(arch: &str, is_cn: bool, http: &dyn HttpClient) -> Result<String> {
    let mut candidates: Vec<String> = Vec::new();
    if is_cn {
        for proxy in CN_API_PROXIES {
            candidates.push(format!("{proxy}/{USQUE_API_BASE}"));
        }
    }
    candidates.push(USQUE_API_BASE.to_string());

    let asset_needle = format!("linux_{arch}.zip");
    for api_url in &candidates {
        let req = HttpRequest {
            url: api_url,
            user_agent: Some(DEFAULT_USER_AGENT),
            connect_timeout: Duration::from_secs(10),
            max_timeout: Duration::from_secs(20),
            ip_preference: IpPreference::Any,
        };
        let Ok(resp) = http.get_text(&req) else {
            continue;
        };
        if !(200..=299).contains(&resp.status) {
            continue;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&resp.body) else {
            continue;
        };
        if let Some(assets) = json.get("assets").and_then(serde_json::Value::as_array) {
            for asset in assets {
                let name = asset
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let dl = asset
                    .get("browser_download_url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if name.contains(&asset_needle) && !dl.is_empty() {
                    return Ok(dl.to_string());
                }
            }
        }
    }

    Err(app_err(format!(
        "failed to resolve Usque release asset matching '{asset_needle}' from GitHub API"
    )))
}

/// Install Usque (MASQUE) Socks5 proxy client using pure-Rust GitHub API JSON parsing.
pub fn install_usque(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting Usque (MASQUE) installation",
        None,
        &[],
    );

    if !ctx.no_sys {
        sys::safe_apt_install(&["unzip"], runner)
            .map_err(|e| app_err(format!("failed to install unzip for Usque: {e}")))?;
    }

    let go_arch = map_go_arch(&ctx.arch())?;
    let is_cn = ctx.effective_is_cn();
    let dl_url = resolve_usque_download_url(go_arch, is_cn, http)?;

    let opt_dir = ctx.path("/opt/usque");
    fs::create_dir_all(&opt_dir)?;

    let tmp_zip = NamedTempFile::new()?;
    let tmp_zip_path = tmp_zip.path().to_path_buf();
    network::download_with_fallback(&tmp_zip_path, &dl_url, is_cn, http)
        .map_err(|e| app_err(format!("failed to download Usque archive: {e}")))?;

    // BE-P1-15 & BE-P2-11: extract into staging dir, require `usque` binary (`E4004`),
    // and atomically rename into `/opt/usque/usque` to prevent `ETXTBSY`.
    let stage_dir = TempDir::new()?;
    shared::extract_zip(&tmp_zip_path, stage_dir.path(), runner)
        .map_err(|e| app_err(format!("failed to extract Usque archive: {e}")))?;

    let extracted_bin = WalkDir::new(stage_dir.path())
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|e| e.file_type().is_file() && e.file_name().to_str() == Some("usque"))
        .map(|e| e.into_path())
        .ok_or_else(|| app_err("extracted Usque archive did not contain 'usque' binary"))?;

    let bin_path = opt_dir.join("usque");
    shared::install_binary_atomic_from_file(&extracted_bin, &bin_path)?;

    shared::create_system_user(ctx, "usque", runner)
        .map_err(|e| app_err(format!("failed to create usque system user: {e}")))?;

    if !ctx.no_sys && !ctx.is_custom_root() {
        let dir_str = opt_dir.to_string_lossy();
        let _ = runner.run(
            "chown",
            &["-R", "usque:usque", &dir_str],
            Duration::from_secs(15),
        );
    }

    let svc_path = shared::deploy_systemd_service(
        ctx,
        "usque",
        "templates/apps/warp/usque.service",
        &BTreeMap::new(),
        true,
        runner,
    )
    .map_err(|e| app_err(format!("failed to deploy usque systemd service: {e}")))?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "Usque deployment finished",
        None,
        &[("arch", go_arch), ("proxy", "127.0.0.1:40001")],
    );

    Ok(AppReport::new(
        MODULE_ID,
        "install-usque",
        true,
        "Usque (MASQUE) deployed on 127.0.0.1:40001",
    )
    .with_detail("arch", go_arch)
    .with_detail("download_url", dl_url)
    .with_detail("service_path", svc_path.to_string_lossy().into_owned())
    .with_detail("proxy_endpoint", "127.0.0.1:40001"))
}

/// Uninstall Usque systemd service, `/opt/usque`, and `usque` system user.
pub fn uninstall_usque(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Usque",
        None,
        &[],
    );

    shared::remove_systemd_service(ctx, "usque", runner)
        .map_err(|e| app_err(format!("failed to remove usque systemd service: {e}")))?;

    let opt_dir = ctx.path("/opt/usque");
    if opt_dir.exists() {
        fs::remove_dir_all(&opt_dir)?;
    }

    let _ = shared::remove_system_user(ctx, "usque", false, runner)?;

    Ok(AppReport::new(
        MODULE_ID,
        "uninstall-usque",
        true,
        "Usque removed",
    ))
}

/// Generate a WireGuard-based Xray WARP outbound inside a caller-supplied directory.
///
/// The private key and device_id are never emitted to diagnostic logs (`U10` secret redaction).
pub fn generate_warp_xray_in_dir(
    ctx: &AppContext,
    work_dir: &Path,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<WarpXrayOutbound> {
    let go_arch = map_go_arch(&ctx.arch())?;
    let is_cn = ctx.effective_is_cn();
    fs::create_dir_all(work_dir)?;

    let wgcf_url = format!(
        "https://github.com/ViRb3/wgcf/releases/download/v{WGCF_VERSION}/wgcf_{WGCF_VERSION}_linux_{go_arch}"
    );
    let wgcf_bin = work_dir.join("wgcf");
    network::download_with_fallback(&wgcf_bin, &wgcf_url, is_cn, http)
        .map_err(|e| app_err(format!("failed to download wgcf binary: {e}")))?;
    fs::set_permissions(&wgcf_bin, fs::Permissions::from_mode(0o755))?;

    let account_path = work_dir.join("wgcf-account.toml");
    let profile_path = work_dir.join("wgcf-profile.conf");
    let wgcf_str = wgcf_bin.to_string_lossy().into_owned();
    let account_str = account_path.to_string_lossy().into_owned();
    let profile_str = profile_path.to_string_lossy().into_owned();

    let reg_out = runner.run(
        &wgcf_str,
        &["register", "--accept-tos", "--config", &account_str],
        Duration::from_secs(60),
    )?;
    if !reg_out.success() {
        return Err(app_err(format!(
            "wgcf register failed (code {}): {}",
            reg_out.code,
            reg_out.stderr.trim()
        )));
    }

    let gen_out = runner.run(
        &wgcf_str,
        &[
            "generate",
            "--config",
            &account_str,
            "--profile",
            &profile_str,
        ],
        Duration::from_secs(60),
    )?;
    if !gen_out.success() {
        return Err(app_err(format!(
            "wgcf generate failed (code {}): {}",
            gen_out.code,
            gen_out.stderr.trim()
        )));
    }

    let account_toml = fs::read_to_string(&account_path)
        .map_err(|e| app_err(format!("failed to read wgcf-account.toml: {e}")))?;
    let profile_conf = fs::read_to_string(&profile_path)
        .map_err(|e| app_err(format!("failed to read wgcf-profile.conf: {e}")))?;

    let private_key = extract_toml_string_field(&account_toml, "private_key")
        .ok_or_else(|| app_err("missing 'private_key' in wgcf-account.toml"))?;
    let device_id = extract_toml_string_field(&account_toml, "device_id")
        .ok_or_else(|| app_err("missing 'device_id' in wgcf-account.toml"))?;
    let ipv4 = extract_wgcf_ipv4(&profile_conf)
        .ok_or_else(|| app_err("missing 172.x.x.x Address in wgcf-profile.conf"))?;

    let reserved = compute_warp_reserved(&device_id)?;
    let json = build_warp_xray_outbound_json(&private_key, &ipv4, reserved)?;

    // U10: never log secret_key or device_id to stderr.
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "generated WireGuard Xray WARP outbound configuration",
        None,
        &[("arch", go_arch), ("ipv4", &ipv4), ("secret", "[REDACTED]")],
    );

    Ok(WarpXrayOutbound {
        secret_key: private_key,
        ipv4,
        public_key: WARP_PUBLIC_KEY.to_string(),
        endpoint: WARP_ENDPOINT.to_string(),
        reserved,
        json,
    })
}

/// Download `wgcf` into an auto-cleaned temporary directory, register a WARP account,
/// and return the structured WireGuard Xray outbound configuration.
pub fn generate_warp_xray(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<WarpXrayOutbound> {
    let tmp_dir = TempDir::new()?;
    generate_warp_xray_in_dir(ctx, tmp_dir.path(), http, runner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_warp_reserved_decodes_base64_prefix_bytes() {
        // [0x2a, 0x0b, 0xff, 0x10] -> base64 "Kgv/EA=="
        assert_eq!(compute_warp_reserved("Kgv/EA==").unwrap(), [42, 11, 255]);
        // Unpadded base64 also accepted
        assert_eq!(compute_warp_reserved("Kgv/").unwrap(), [42, 11, 255]);
        // Short input rejected with E4004 App error
        let err = compute_warp_reserved("QQ==").unwrap_err();
        assert_eq!(err.error_code(), "E4004");
    }

    #[test]
    fn build_warp_xray_outbound_json_matches_expected_schema() {
        let json_str =
            build_warp_xray_outbound_json("SECRET_KEY_VAL", "172.16.0.2", [10, 20, 30]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["tag"], "warp");
        assert_eq!(parsed["protocol"], "wireguard");
        assert_eq!(parsed["settings"]["secretKey"], "SECRET_KEY_VAL");
        assert_eq!(parsed["settings"]["address"][0], "172.16.0.2/32");
        assert_eq!(parsed["settings"]["peers"][0]["publicKey"], WARP_PUBLIC_KEY);
        assert_eq!(parsed["settings"]["peers"][0]["endpoint"], WARP_ENDPOINT);
        assert_eq!(
            parsed["settings"]["reserved"],
            serde_json::json!([10, 20, 30])
        );
    }
}
