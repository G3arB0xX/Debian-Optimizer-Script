use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use regex::Regex;
use tempfile::NamedTempFile;

use crate::config::ConfigStore;
use crate::error::{CoreError, Result};
use crate::sys::CommandRunner;

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

const MANAGED_DNS_MARKER: &str = "# managed-by-debopti";

const CN_API_PROXIES: &[&str] = &[
    "https://gh-proxy.com",
    "https://edgeone.gh-proxy.org",
    "https://gh-proxy.org",
];

const CN_GIT_PROXIES: &[&str] = &[
    "https://ghfast.top",
    "https://gh-proxy.org",
    "https://wget.la",
    "https://gh-proxy.com",
];

const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_HTTP_LOW_SPEED_LIMIT", "1000"),
    ("GIT_HTTP_LOW_SPEED_TIME", "10"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IpPreference {
    #[default]
    Any,
    Ipv4Only,
    Ipv6Only,
}

#[derive(Debug, Clone)]
pub struct HttpRequest<'a> {
    pub url: &'a str,
    pub user_agent: Option<&'a str>,
    pub connect_timeout: Duration,
    pub max_timeout: Duration,
    pub ip_preference: IpPreference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait HttpClient: Send + Sync {
    fn get_text(&self, req: &HttpRequest<'_>) -> Result<HttpResponse>;
    fn download_to_file(&self, req: &HttpRequest<'_>, dest: &Path) -> Result<u16>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UreqHttpClient;

impl UreqHttpClient {
    pub fn new() -> Self {
        Self
    }

    fn build_agent(&self, req: &HttpRequest<'_>) -> ureq::Agent {
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::WebPki)
            .build();

        let ip_family = match req.ip_preference {
            IpPreference::Any => ureq::config::IpFamily::Any,
            IpPreference::Ipv4Only => ureq::config::IpFamily::Ipv4Only,
            IpPreference::Ipv6Only => ureq::config::IpFamily::Ipv6Only,
        };

        let ua = req.user_agent.unwrap_or(DEFAULT_USER_AGENT);

        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .ip_family(ip_family)
            .timeout_connect(Some(req.connect_timeout))
            .timeout_global(Some(req.max_timeout))
            .http_status_as_error(false)
            .user_agent(ua)
            .build();

        ureq::Agent::new_with_config(config)
    }
}

impl HttpClient for UreqHttpClient {
    fn get_text(&self, req: &HttpRequest<'_>) -> Result<HttpResponse> {
        let agent = self.build_agent(req);
        let mut resp = agent
            .get(req.url)
            .call()
            .map_err(|e| CoreError::Network(format!("GET {} failed: {e}", req.url)))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| CoreError::Network(format!("read body from {} failed: {e}", req.url)))?;
        Ok(HttpResponse { status, body })
    }

    fn download_to_file(&self, req: &HttpRequest<'_>, dest: &Path) -> Result<u16> {
        let agent = self.build_agent(req);
        let mut resp = agent
            .get(req.url)
            .call()
            .map_err(|e| CoreError::Network(format!("download {} failed: {e}", req.url)))?;
        let status = resp.status().as_u16();
        if !(200..=299).contains(&status) {
            return Err(CoreError::HttpStatus {
                url: req.url.to_string(),
                status,
            });
        }

        let parent = dest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;

        let mut tmp = NamedTempFile::new_in(parent)?;
        let mut reader = resp.body_mut().as_reader();
        let bytes_written = std::io::copy(&mut reader, &mut tmp)
            .map_err(|e| CoreError::Network(format!("stream {} failed: {e}", req.url)))?;
        if bytes_written == 0 {
            return Err(CoreError::Network(format!(
                "empty response body from {}",
                req.url
            )));
        }
        tmp.flush()?;
        tmp.persist(dest).map_err(|e| CoreError::Io(e.error))?;
        Ok(status)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub url: String,
    pub ip_preference: IpPreference,
    pub user_agent: Option<String>,
    pub is_download: bool,
}

#[derive(Debug, Clone)]
enum MockGetOutcome {
    Ok(HttpResponse),
    Err(String),
}

#[derive(Debug, Clone)]
enum MockDownloadOutcome {
    Ok { status: u16, payload: Vec<u8> },
    Err(String),
}

#[derive(Debug, Default)]
pub struct MockHttpClient {
    get_responses: Mutex<HashMap<String, MockGetOutcome>>,
    download_responses: Mutex<HashMap<String, MockDownloadOutcome>>,
    requests: Mutex<Vec<RecordedRequest>>,
}

impl MockHttpClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_get(&self, url: impl Into<String>, resp: HttpResponse) {
        self.get_responses
            .lock()
            .unwrap()
            .insert(url.into(), MockGetOutcome::Ok(resp));
    }

    pub fn on_get_err(&self, url: impl Into<String>, message: impl Into<String>) {
        self.get_responses
            .lock()
            .unwrap()
            .insert(url.into(), MockGetOutcome::Err(message.into()));
    }

    pub fn on_download(&self, url: impl Into<String>, status: u16, payload: impl Into<Vec<u8>>) {
        self.download_responses.lock().unwrap().insert(
            url.into(),
            MockDownloadOutcome::Ok {
                status,
                payload: payload.into(),
            },
        );
    }

    pub fn on_download_err(&self, url: impl Into<String>, message: impl Into<String>) {
        self.download_responses
            .lock()
            .unwrap()
            .insert(url.into(), MockDownloadOutcome::Err(message.into()));
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn requested_urls(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.url.clone())
            .collect()
    }
}

impl HttpClient for MockHttpClient {
    fn get_text(&self, req: &HttpRequest<'_>) -> Result<HttpResponse> {
        self.requests.lock().unwrap().push(RecordedRequest {
            url: req.url.to_string(),
            ip_preference: req.ip_preference,
            user_agent: req.user_agent.map(ToString::to_string),
            is_download: false,
        });

        match self.get_responses.lock().unwrap().get(req.url).cloned() {
            Some(MockGetOutcome::Ok(resp)) => Ok(resp),
            Some(MockGetOutcome::Err(msg)) => Err(CoreError::Network(msg)),
            None => Err(CoreError::Network(format!("unmocked GET {}", req.url))),
        }
    }

    fn download_to_file(&self, req: &HttpRequest<'_>, dest: &Path) -> Result<u16> {
        self.requests.lock().unwrap().push(RecordedRequest {
            url: req.url.to_string(),
            ip_preference: req.ip_preference,
            user_agent: req.user_agent.map(ToString::to_string),
            is_download: true,
        });

        match self
            .download_responses
            .lock()
            .unwrap()
            .get(req.url)
            .cloned()
        {
            Some(MockDownloadOutcome::Ok { status, payload }) => {
                if !(200..=299).contains(&status) {
                    return Err(CoreError::HttpStatus {
                        url: req.url.to_string(),
                        status,
                    });
                }
                if payload.is_empty() {
                    return Err(CoreError::Network(format!(
                        "empty response body from {}",
                        req.url
                    )));
                }
                let parent = dest
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                fs::create_dir_all(parent)?;
                let mut tmp = NamedTempFile::new_in(parent)?;
                tmp.write_all(&payload)?;
                tmp.flush()?;
                tmp.persist(dest).map_err(|e| CoreError::Io(e.error))?;
                Ok(status)
            }
            Some(MockDownloadOutcome::Err(msg)) => Err(CoreError::Network(msg)),
            None => Err(CoreError::Network(format!("unmocked download {}", req.url))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsFixMode {
    Initial,
    Calibrate,
}

#[derive(Debug, Clone)]
pub struct DnsFixOptions {
    pub resolv_path: PathBuf,
    pub backup_path: PathBuf,
    pub is_cn_region: bool,
}

impl Default for DnsFixOptions {
    fn default() -> Self {
        Self {
            resolv_path: PathBuf::from("/etc/resolv.conf"),
            backup_path: PathBuf::from("/etc/resolv.conf.debopti.bak"),
            is_cn_region: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsFixReport {
    pub modified: bool,
    pub has_v4: bool,
    pub has_v6: bool,
    pub nameservers: Vec<String>,
}

fn write_file_atomic(path: &Path, content: &str) -> Result<()> {
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

fn parse_global_ipv6_candidates(ip_output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in ip_output.lines() {
        if !line.contains("inet6") || line.contains("temporary") || line.contains("deprecated") {
            continue;
        }
        let Some(after) = line.split("inet6 ").nth(1) else {
            continue;
        };
        let Some(addr) = after.split('/').next() else {
            continue;
        };
        let addr = addr.trim();
        let lower = addr.to_ascii_lowercase();
        if addr.is_empty() || lower.starts_with("::1") || lower.starts_with("fe80") {
            continue;
        }
        if addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
            out.push(addr.to_string());
        }
    }
    out
}

/// Two-stage DNS health check and auto-repair matching `scripts/network.sh::check_and_fix_dns`.
pub fn check_and_fix_dns(
    mode: DnsFixMode,
    opts: &DnsFixOptions,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<DnsFixReport> {
    match mode {
        DnsFixMode::Initial => {
            if let Ok(out) = runner.run(
                "getent",
                &["hosts", "cloudflare.com"],
                Duration::from_secs(10),
            ) && out.success()
            {
                return Ok(DnsFixReport {
                    modified: false,
                    has_v4: false,
                    has_v6: false,
                    nameservers: Vec::new(),
                });
            }

            if !opts.backup_path.exists() && opts.resolv_path.is_file() {
                if let Some(parent) = opts.backup_path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(&opts.resolv_path, &opts.backup_path);
            }

            let content = format!("{MANAGED_DNS_MARKER}\nnameserver 1.1.1.1\n");
            write_file_atomic(&opts.resolv_path, &content)?;
            Ok(DnsFixReport {
                modified: true,
                has_v4: true,
                has_v6: false,
                nameservers: vec!["1.1.1.1".to_string()],
            })
        }
        DnsFixMode::Calibrate => {
            let Ok(existing) = fs::read_to_string(&opts.resolv_path) else {
                return Ok(DnsFixReport {
                    modified: false,
                    has_v4: false,
                    has_v6: false,
                    nameservers: Vec::new(),
                });
            };
            if !existing.contains(MANAGED_DNS_MARKER) {
                return Ok(DnsFixReport {
                    modified: false,
                    has_v4: false,
                    has_v6: false,
                    nameservers: Vec::new(),
                });
            }

            // Probe IPv4 connectivity
            let v4_req = HttpRequest {
                url: "http://1.1.1.1",
                user_agent: Some(DEFAULT_USER_AGENT),
                connect_timeout: Duration::from_secs(5),
                max_timeout: Duration::from_secs(5),
                ip_preference: IpPreference::Ipv4Only,
            };
            let has_v4 = http.get_text(&v4_req).is_ok();

            // Probe IPv6 connectivity (dual-track)
            let mut has_v6 = false;
            if let Ok(ip_out) = runner.run(
                "ip",
                &["-6", "addr", "show", "scope", "global"],
                Duration::from_secs(5),
            ) && ip_out.success()
            {
                let candidates = parse_global_ipv6_candidates(&ip_out.stdout);
                for addr in candidates {
                    match runner.run(
                        "curl",
                        &[
                            "-6",
                            "-s",
                            "-m",
                            "6",
                            "-o",
                            "/dev/null",
                            "--interface",
                            &addr,
                            "https://ipv6.google.com",
                        ],
                        Duration::from_secs(8),
                    ) {
                        Ok(out) if out.success() => {
                            has_v6 = true;
                            break;
                        }
                        Ok(_) => {}
                        Err(_) => {
                            // curl unavailable: fall back to pure-Rust IPv6-only HTTP probe
                            let v6_google_req = HttpRequest {
                                url: "https://ipv6.google.com",
                                user_agent: Some(DEFAULT_USER_AGENT),
                                connect_timeout: Duration::from_secs(6),
                                max_timeout: Duration::from_secs(6),
                                ip_preference: IpPreference::Ipv6Only,
                            };
                            if http.get_text(&v6_google_req).is_ok() {
                                has_v6 = true;
                                break;
                            }
                        }
                    }
                }
            }

            // Track 2: external IPv6 API fallback
            if !has_v6 {
                for probe_url in ["https://api.ip.sb/ip", "https://icanhazip.com"] {
                    let req = HttpRequest {
                        url: probe_url,
                        user_agent: Some(DEFAULT_USER_AGENT),
                        connect_timeout: Duration::from_secs(5),
                        max_timeout: Duration::from_secs(8),
                        ip_preference: IpPreference::Ipv6Only,
                    };
                    if let Ok(resp) = http.get_text(&req)
                        && (200..=299).contains(&resp.status)
                        && !resp.body.trim().is_empty()
                    {
                        has_v6 = true;
                        break;
                    }
                }
            }

            let mut nameservers: Vec<String> = Vec::new();
            if opts.is_cn_region {
                if has_v4 {
                    nameservers.push("223.5.5.5".to_string());
                    nameservers.push("119.29.29.29".to_string());
                }
                if has_v6 {
                    nameservers.push("2402:4e00::".to_string());
                    nameservers.push("2400:3200::1".to_string());
                }
            } else {
                if has_v4 {
                    nameservers.push("8.8.8.8".to_string());
                    nameservers.push("1.1.1.1".to_string());
                }
                if has_v6 {
                    nameservers.push("2001:4860:4860::8888".to_string());
                    nameservers.push("2606:4700:4700::1111".to_string());
                }
            }

            if nameservers.is_empty() {
                return Ok(DnsFixReport {
                    modified: false,
                    has_v4,
                    has_v6,
                    nameservers,
                });
            }

            let mut out_lines: Vec<String> = vec![MANAGED_DNS_MARKER.to_string()];
            for line in existing.lines() {
                if line.starts_with("domain")
                    || line.starts_with("search")
                    || line.starts_with("options")
                {
                    out_lines.push(line.to_string());
                }
            }
            for ns in &nameservers {
                out_lines.push(format!("nameserver {ns}"));
            }
            let mut rendered = out_lines.join("\n");
            rendered.push('\n');
            write_file_atomic(&opts.resolv_path, &rendered)?;

            Ok(DnsFixReport {
                modified: true,
                has_v4,
                has_v6,
                nameservers,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetcheckReport {
    pub is_cn_region: bool,
    pub cached: bool,
    pub env_exports: BTreeMap<String, String>,
}

fn cn_env_exports(is_cn: bool) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if is_cn {
        map.insert(
            "RUSTUP_DIST_SERVER".to_string(),
            "https://rsproxy.cn".to_string(),
        );
        map.insert(
            "RUSTUP_UPDATE_ROOT".to_string(),
            "https://rsproxy.cn/rustup".to_string(),
        );
        map.insert(
            "GOPROXY".to_string(),
            "https://goproxy.cn,direct".to_string(),
        );
    }
    map
}

/// 4-node CN region detection with persistent caching in `ConfigStore`.
pub fn global_netcheck(
    store: &ConfigStore,
    force: bool,
    http: &dyn HttpClient,
) -> Result<NetcheckReport> {
    if !force
        && let Some(cached_val) = store.get("IS_CN_REGION")?
        && !cached_val.trim().is_empty()
    {
        let is_cn = cached_val.trim() == "true";
        return Ok(NetcheckReport {
            is_cn_region: is_cn,
            cached: true,
            env_exports: cn_env_exports(is_cn),
        });
    }

    let make_req = |url: &'static str| HttpRequest {
        url,
        user_agent: Some(DEFAULT_USER_AGENT),
        connect_timeout: Duration::from_secs(3),
        max_timeout: Duration::from_secs(6),
        ip_preference: IpPreference::Any,
    };

    let mut is_cn = false;
    if let Ok(resp) = http.get_text(&make_req("https://ipinfo.io/country"))
        && resp.body.contains("CN")
    {
        is_cn = true;
    } else if let Ok(resp) = http.get_text(&make_req("http://myip.ipip.net"))
        && resp.body.contains("中国")
    {
        is_cn = true;
    } else if let Ok(resp) = http.get_text(&make_req("https://cip.cc"))
        && resp.body.contains("中国")
    {
        is_cn = true;
    } else if let Ok(resp) = http.get_text(&make_req("https://api.ip.sb/geoip"))
        && resp.body.to_ascii_lowercase().contains("china")
    {
        is_cn = true;
    }

    store.set("IS_CN_REGION", if is_cn { "true" } else { "false" })?;
    Ok(NetcheckReport {
        is_cn_region: is_cn,
        cached: false,
        env_exports: cn_env_exports(is_cn),
    })
}

fn is_valid_release_tag(tag: &str) -> bool {
    matches!(tag.as_bytes(), [b'v', b'0'..=b'9', ..] | [b'0'..=b'9', ..])
}

fn parse_tag_from_api_json(body: &str) -> Option<String> {
    let re = Regex::new(r#""tag_name"\s*:\s*"([^"]+)""#).ok()?;
    let cap = re.captures(body)?;
    let tag = cap.get(1)?.as_str();
    is_valid_release_tag(tag).then(|| tag.to_string())
}

fn parse_tag_from_release_page(body: &str) -> Option<String> {
    let re = Regex::new(r"tag/(v?[0-9]+\.[0-9]+\.[0-9]+(?:-[a-zA-Z0-9.]+)?)").ok()?;
    let cap = re.captures(body)?;
    let tag = cap.get(1)?.as_str();
    is_valid_release_tag(tag).then(|| tag.to_string())
}

fn fetch_release_tag(url: &str, api_mode: bool, http: &dyn HttpClient) -> Option<String> {
    let req = HttpRequest {
        url,
        user_agent: Some(DEFAULT_USER_AGENT),
        connect_timeout: Duration::from_secs(5),
        max_timeout: Duration::from_secs(15),
        ip_preference: IpPreference::Any,
    };
    let resp = http.get_text(&req).ok()?;
    if !(200..=299).contains(&resp.status) {
        return None;
    }
    if api_mode {
        parse_tag_from_api_json(&resp.body)
    } else {
        parse_tag_from_release_page(&resp.body)
    }
}

/// Fetch latest GitHub release tag with CN API proxy pool (strictly excluding `ghfast.top`).
pub fn get_latest_github_release(
    repo: &str,
    is_cn: bool,
    http: &dyn HttpClient,
) -> Result<Option<String>> {
    let api_path = format!("https://api.github.com/repos/{repo}/releases/latest");
    let page_path = format!("https://github.com/{repo}/releases/latest");

    if is_cn {
        for proxy in CN_API_PROXIES {
            let url = format!("{proxy}/{api_path}");
            if let Some(tag) = fetch_release_tag(&url, true, http) {
                return Ok(Some(tag));
            }
        }
        for proxy in CN_API_PROXIES {
            let url = format!("{proxy}/{page_path}");
            if let Some(tag) = fetch_release_tag(&url, false, http) {
                return Ok(Some(tag));
            }
        }
    }

    if let Some(tag) = fetch_release_tag(&api_path, true, http) {
        return Ok(Some(tag));
    }
    if let Some(tag) = fetch_release_tag(&page_path, false, http) {
        return Ok(Some(tag));
    }

    Ok(None)
}

fn rewrite_raw_to_jsdelivr(original_url: &str) -> Option<String> {
    let re =
        Regex::new(r"^https://raw\.githubusercontent\.com/([^/]+)/([^/]+)/([^/]+)/(.*)$").ok()?;
    let caps = re.captures(original_url)?;
    Some(format!(
        "https://cdn.jsdelivr.net/gh/{}/{}@{}/{}",
        &caps[1], &caps[2], &caps[3], &caps[4]
    ))
}

fn is_ghfast_forbidden_url(original_url: &str) -> bool {
    if original_url.contains("api.github.com") {
        return true;
    }
    let re = Regex::new(r"/releases/latest([?#].*)?$").unwrap();
    re.is_match(original_url)
}

/// Build the ordered candidate URL list for `download_with_fallback`.
pub fn build_download_candidates(original_url: &str, is_cn: bool) -> Vec<String> {
    let is_github =
        original_url.contains("github.com") || original_url.contains("githubusercontent.com");
    if !is_cn || !is_github {
        return vec![original_url.to_string()];
    }

    let mut candidates = Vec::with_capacity(9);
    for prefix in [
        "https://gh-proxy.com",
        "https://cdn.gh-proxy.org",
        "https://edgeone.gh-proxy.org",
        "https://git.yylx.win",
        "https://gh-proxy.org",
    ] {
        candidates.push(format!("{prefix}/{original_url}"));
    }
    if let Some(jsdelivr) = rewrite_raw_to_jsdelivr(original_url) {
        candidates.push(jsdelivr);
    }
    if !is_ghfast_forbidden_url(original_url) {
        candidates.push(format!("https://ghfast.top/{original_url}"));
    }
    candidates.push(format!("https://wget.la/{original_url}"));
    candidates.push(original_url.to_string());
    candidates
}

/// Download a file with CN GitHub 9-entry mirror pool fallback; returns the winning URL.
pub fn download_with_fallback(
    target: &Path,
    original_url: &str,
    is_cn: bool,
    http: &dyn HttpClient,
) -> Result<String> {
    let candidates = build_download_candidates(original_url, is_cn);

    for dl_url in candidates {
        let req = HttpRequest {
            url: &dl_url,
            user_agent: Some(DEFAULT_USER_AGENT),
            connect_timeout: Duration::from_secs(5),
            max_timeout: Duration::from_secs(120),
            ip_preference: IpPreference::Any,
        };
        match http.download_to_file(&req, target) {
            Ok(status)
                if (200..=299).contains(&status)
                    && target.metadata().map(|m| m.len() > 0).unwrap_or(false) =>
            {
                return Ok(dl_url);
            }
            _ => {
                let _ = fs::remove_file(target);
            }
        }
    }

    Err(CoreError::Network(format!(
        "all download mirrors failed for {original_url}"
    )))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn is_protected_dir(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return true;
    }
    let protected = [Path::new("/"), Path::new("/usr"), Path::new("/etc")];
    let lexical = normalize_lexical(path);
    if protected.iter().any(|p| lexical == *p) {
        return true;
    }
    if let Ok(canon) = fs::canonicalize(path)
        && protected.iter().any(|p| canon == *p)
    {
        return true;
    }
    false
}

fn remove_target_if_exists(target_dir: &Path) -> Result<()> {
    if target_dir.is_dir() {
        fs::remove_dir_all(target_dir)?;
    } else if target_dir.exists() {
        fs::remove_file(target_dir)?;
    }
    Ok(())
}

/// Clone a Git repository with CN mirror pool fallback, protected-dir guard, and origin reset.
pub fn git_clone_with_fallback(
    target_dir: &Path,
    repo_url: &str,
    extra_args: &[&str],
    is_cn: bool,
    runner: &dyn CommandRunner,
) -> Result<String> {
    if is_protected_dir(target_dir) {
        return Err(CoreError::ValidationFailed(format!(
            "refusing to operate on protected directory: {}",
            target_dir.display()
        )));
    }

    let target_str = target_dir.to_string_lossy().into_owned();

    if is_cn && repo_url.contains("github.com") {
        let _ = runner.run_with_env(
            "git",
            &["config", "--global", "http.version", "HTTP/1.1"],
            GIT_ENV,
            Duration::from_secs(15),
        );

        let mut candidates: Vec<String> = CN_GIT_PROXIES
            .iter()
            .map(|p| format!("{p}/{repo_url}"))
            .collect();
        candidates.push(repo_url.to_string());

        for clone_url in candidates {
            remove_target_if_exists(target_dir)?;
            let mut args: Vec<&str> = Vec::with_capacity(3 + extra_args.len());
            args.push("clone");
            args.extend_from_slice(extra_args);
            args.push(&clone_url);
            args.push(&target_str);

            if let Ok(out) = runner.run_with_env("git", &args, GIT_ENV, Duration::from_secs(300))
                && out.success()
            {
                let reset = runner.run_with_env(
                    "git",
                    &["-C", &target_str, "remote", "set-url", "origin", repo_url],
                    GIT_ENV,
                    Duration::from_secs(15),
                )?;
                if !reset.success() {
                    return Err(CoreError::CommandFailed {
                        prog: "git".to_string(),
                        message: format!("failed to reset origin url: {}", reset.stderr),
                    });
                }
                return Ok(clone_url);
            }
        }

        Err(CoreError::Network(format!(
            "all git mirrors failed for {repo_url}"
        )))
    } else {
        remove_target_if_exists(target_dir)?;
        let mut args: Vec<&str> = Vec::with_capacity(3 + extra_args.len());
        args.push("clone");
        args.extend_from_slice(extra_args);
        args.push(repo_url);
        args.push(&target_str);

        let out = runner.run_with_env("git", &args, GIT_ENV, Duration::from_secs(300))?;
        if out.success() {
            Ok(repo_url.to_string())
        } else {
            Err(CoreError::Network(format!(
                "git clone failed for {repo_url}: {}",
                out.stderr
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{CmdOutput, MockCommandRunner};
    use tempfile::TempDir;

    fn ok_cmd(stdout: &str) -> CmdOutput {
        CmdOutput {
            code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn err_cmd(code: i32) -> CmdOutput {
        CmdOutput {
            code,
            stdout: String::new(),
            stderr: "err".to_string(),
            timed_out: false,
        }
    }

    #[test]
    fn ureq_http_client_constructs_with_rustls_ring() {
        let client = UreqHttpClient::new();
        let req = HttpRequest {
            url: "https://example.invalid",
            user_agent: None,
            connect_timeout: Duration::from_secs(1),
            max_timeout: Duration::from_secs(2),
            ip_preference: IpPreference::Ipv4Only,
        };
        let _agent = client.build_agent(&req);
    }

    #[test]
    fn dns_initial_leaves_healthy_resolv_untouched() {
        let tmp = TempDir::new().unwrap();
        let resolv = tmp.path().join("resolv.conf");
        let backup = tmp.path().join("resolv.conf.bak");
        fs::write(&resolv, "nameserver 9.9.9.9\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("getent", &["hosts", "cloudflare.com"], ok_cmd("1.1.1.1"));
        let http = MockHttpClient::new();

        let opts = DnsFixOptions {
            resolv_path: resolv.clone(),
            backup_path: backup.clone(),
            is_cn_region: false,
        };
        let report = check_and_fix_dns(DnsFixMode::Initial, &opts, &http, &runner).unwrap();
        assert!(!report.modified);
        assert!(!backup.exists());
        assert_eq!(fs::read_to_string(&resolv).unwrap(), "nameserver 9.9.9.9\n");
    }

    #[test]
    fn dns_initial_writes_fallback_and_backs_up_once() {
        let tmp = TempDir::new().unwrap();
        let resolv = tmp.path().join("resolv.conf");
        let backup = tmp.path().join("resolv.conf.bak");
        fs::write(&resolv, "search corp.local\nnameserver 10.0.0.53\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("getent", &["hosts", "cloudflare.com"], err_cmd(2));
        let http = MockHttpClient::new();

        let opts = DnsFixOptions {
            resolv_path: resolv.clone(),
            backup_path: backup.clone(),
            is_cn_region: false,
        };
        let report = check_and_fix_dns(DnsFixMode::Initial, &opts, &http, &runner).unwrap();
        assert!(report.modified);
        assert_eq!(report.nameservers, vec!["1.1.1.1"]);
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "search corp.local\nnameserver 10.0.0.53\n"
        );
        assert_eq!(
            fs::read_to_string(&resolv).unwrap(),
            "# managed-by-debopti\nnameserver 1.1.1.1\n"
        );
    }

    #[test]
    fn dns_calibrate_skips_unmanaged_file() {
        let tmp = TempDir::new().unwrap();
        let resolv = tmp.path().join("resolv.conf");
        fs::write(&resolv, "nameserver 10.0.0.53\n").unwrap();

        let runner = MockCommandRunner::new();
        let http = MockHttpClient::new();
        let opts = DnsFixOptions {
            resolv_path: resolv.clone(),
            backup_path: tmp.path().join("resolv.conf.bak"),
            is_cn_region: true,
        };

        let report = check_and_fix_dns(DnsFixMode::Calibrate, &opts, &http, &runner).unwrap();
        assert!(!report.modified);
        assert!(http.requested_urls().is_empty());
    }

    #[test]
    fn dns_calibrate_cn_dual_stack_preserves_options_and_uses_track2_fallback() {
        let tmp = TempDir::new().unwrap();
        let resolv = tmp.path().join("resolv.conf");
        fs::write(
            &resolv,
            "# managed-by-debopti\nsearch lan.local\noptions edns0\nnameserver 1.1.1.1\n",
        )
        .unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact(
            "ip",
            &["-6", "addr", "show", "scope", "global"],
            ok_cmd(
                "    inet6 2001:db8::10/64 scope global temporary dynamic\n\
                 inet6 fe80::1/64 scope global\n\
                 inet6 2001:db8::20/64 scope global\n",
            ),
        );
        // Track 1 curl fails -> triggers Track 2 API fallback
        runner.on_prog("curl", err_cmd(7));

        let http = MockHttpClient::new();
        http.on_get(
            "http://1.1.1.1",
            HttpResponse {
                status: 301,
                body: String::new(),
            },
        );
        http.on_get_err("https://api.ip.sb/ip", "timeout");
        http.on_get(
            "https://icanhazip.com",
            HttpResponse {
                status: 200,
                body: "2001:db8::20\n".to_string(),
            },
        );

        let opts = DnsFixOptions {
            resolv_path: resolv.clone(),
            backup_path: tmp.path().join("resolv.conf.bak"),
            is_cn_region: true,
        };
        let report = check_and_fix_dns(DnsFixMode::Calibrate, &opts, &http, &runner).unwrap();
        assert!(report.modified);
        assert!(report.has_v4);
        assert!(report.has_v6);
        assert_eq!(
            report.nameservers,
            vec!["223.5.5.5", "119.29.29.29", "2402:4e00::", "2400:3200::1"]
        );

        let content = fs::read_to_string(&resolv).unwrap();
        assert_eq!(
            content,
            "# managed-by-debopti\n\
             search lan.local\n\
             options edns0\n\
             nameserver 223.5.5.5\n\
             nameserver 119.29.29.29\n\
             nameserver 2402:4e00::\n\
             nameserver 2400:3200::1\n"
        );
    }

    #[test]
    fn dns_calibrate_global_v4_only() {
        let tmp = TempDir::new().unwrap();
        let resolv = tmp.path().join("resolv.conf");
        fs::write(&resolv, "# managed-by-debopti\nnameserver 1.1.1.1\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("ip", &["-6", "addr", "show", "scope", "global"], ok_cmd(""));

        let http = MockHttpClient::new();
        http.on_get(
            "http://1.1.1.1",
            HttpResponse {
                status: 200,
                body: String::new(),
            },
        );

        let opts = DnsFixOptions {
            resolv_path: resolv.clone(),
            backup_path: tmp.path().join("resolv.conf.bak"),
            is_cn_region: false,
        };
        let report = check_and_fix_dns(DnsFixMode::Calibrate, &opts, &http, &runner).unwrap();
        assert!(report.modified);
        assert!(report.has_v4);
        assert!(!report.has_v6);
        assert_eq!(report.nameservers, vec!["8.8.8.8", "1.1.1.1"]);
    }

    #[test]
    fn global_netcheck_cascades_persists_and_caches() {
        let tmp = TempDir::new().unwrap();
        let store = ConfigStore::new(tmp.path().join("debopti.conf"));
        let http = MockHttpClient::new();

        http.on_get_err("https://ipinfo.io/country", "timeout");
        http.on_get(
            "http://myip.ipip.net",
            HttpResponse {
                status: 200,
                body: "当前 IP：1.2.3.4 来自于：中国 北京".to_string(),
            },
        );

        let r1 = global_netcheck(&store, false, &http).unwrap();
        assert!(r1.is_cn_region);
        assert!(!r1.cached);
        assert_eq!(
            r1.env_exports.get("RUSTUP_DIST_SERVER").map(String::as_str),
            Some("https://rsproxy.cn")
        );
        assert_eq!(
            r1.env_exports.get("GOPROXY").map(String::as_str),
            Some("https://goproxy.cn,direct")
        );
        assert_eq!(store.get("IS_CN_REGION").unwrap().as_deref(), Some("true"));

        // Second call hits cache without HTTP probes
        let http2 = MockHttpClient::new();
        let r2 = global_netcheck(&store, false, &http2).unwrap();
        assert!(r2.is_cn_region);
        assert!(r2.cached);
        assert!(http2.requested_urls().is_empty());

        // Force re-probe -> overseas
        http2.on_get(
            "https://ipinfo.io/country",
            HttpResponse {
                status: 200,
                body: "US\n".to_string(),
            },
        );
        let r3 = global_netcheck(&store, true, &http2).unwrap();
        assert!(!r3.is_cn_region);
        assert!(!r3.cached);
        assert!(r3.env_exports.is_empty());
        assert_eq!(store.get("IS_CN_REGION").unwrap().as_deref(), Some("false"));
    }

    #[test]
    fn github_release_excludes_ghfast_and_falls_back_to_page() {
        let http = MockHttpClient::new();
        http.on_get(
            "https://edgeone.gh-proxy.org/https://github.com/zhboner/realm/releases/latest",
            HttpResponse {
                status: 200,
                body: r#"<a href="/zhboner/realm/releases/tag/v2.7.0-rc.1">v2.7.0-rc.1</a>"#
                    .to_string(),
            },
        );

        let tag = get_latest_github_release("zhboner/realm", true, &http)
            .unwrap()
            .unwrap();
        assert_eq!(tag, "v2.7.0-rc.1");

        let urls = http.requested_urls();
        assert_eq!(
            urls[0],
            "https://gh-proxy.com/https://api.github.com/repos/zhboner/realm/releases/latest"
        );
        assert!(urls.iter().all(|u| !u.contains("ghfast.top")));
    }

    #[test]
    fn download_candidates_exact_order_jsdelivr_ghfast_and_no_dead_mirrors() {
        let dead_mirrors = [
            "ghproxy.homeboyc.cn",
            "hub.nuaa.cf",
            "raw.nuaa.cf",
            "kkgithub.com",
            "moeyy.cn/gh-proxy",
            "gitclone.com",
            "ghproxy.net",
            "github.akams.cn",
            "gh.llkk.cc",
            "hk.gh-proxy.org",
        ];

        let raw_url = "https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh";
        let cands = build_download_candidates(raw_url, true);
        assert_eq!(cands.len(), 9);
        assert_eq!(cands[0], format!("https://gh-proxy.com/{raw_url}"));
        assert_eq!(cands[1], format!("https://cdn.gh-proxy.org/{raw_url}"));
        assert_eq!(cands[2], format!("https://edgeone.gh-proxy.org/{raw_url}"));
        assert_eq!(cands[3], format!("https://git.yylx.win/{raw_url}"));
        assert_eq!(cands[4], format!("https://gh-proxy.org/{raw_url}"));
        assert_eq!(
            cands[5],
            "https://cdn.jsdelivr.net/gh/XTLS/Xray-install@main/install-release.sh"
        );
        assert_eq!(cands[6], format!("https://ghfast.top/{raw_url}"));
        assert_eq!(cands[7], format!("https://wget.la/{raw_url}"));
        assert_eq!(cands[8], raw_url);

        for c in &cands {
            for dead in &dead_mirrors {
                assert!(!c.contains(dead), "dead mirror {dead} found in {c}");
            }
        }

        // ghfast.top must be skipped for api.github.com and /releases/latest
        let api_cands =
            build_download_candidates("https://api.github.com/repos/a/b/releases/latest", true);
        assert!(api_cands.iter().all(|u| !u.contains("ghfast.top")));

        let latest_cands =
            build_download_candidates("https://github.com/a/b/releases/latest", true);
        assert!(latest_cands.iter().all(|u| !u.contains("ghfast.top")));

        // Actual download fallback picks jsDelivr when earlier mirrors fail
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("install.sh");
        let http = MockHttpClient::new();
        http.on_download(
            "https://cdn.jsdelivr.net/gh/XTLS/Xray-install@main/install-release.sh",
            200,
            b"#!/bin/bash\necho ok\n".to_vec(),
        );
        let winner = download_with_fallback(&dest, raw_url, true, &http).unwrap();
        assert_eq!(
            winner,
            "https://cdn.jsdelivr.net/gh/XTLS/Xray-install@main/install-release.sh"
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "#!/bin/bash\necho ok\n");
    }

    #[test]
    fn git_clone_rejects_protected_dirs_and_resets_origin_on_cn_mirror() {
        let runner = MockCommandRunner::new();
        for bad in ["", "/", "/usr", "/etc", "/etc/"] {
            let err = git_clone_with_fallback(
                Path::new(bad),
                "https://github.com/org/repo.git",
                &[],
                true,
                &runner,
            )
            .unwrap_err();
            assert!(matches!(err, CoreError::ValidationFailed(_)));
        }

        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("repo");
        let target_str = target.to_string_lossy().into_owned();
        let repo_url = "https://github.com/org/repo.git";

        runner.on_exact(
            "git",
            &["config", "--global", "http.version", "HTTP/1.1"],
            ok_cmd(""),
        );
        runner.on_exact(
            "git",
            &[
                "clone",
                "--depth=1",
                "https://ghfast.top/https://github.com/org/repo.git",
                &target_str,
            ],
            err_cmd(128),
        );
        runner.on_exact(
            "git",
            &[
                "clone",
                "--depth=1",
                "https://gh-proxy.org/https://github.com/org/repo.git",
                &target_str,
            ],
            ok_cmd(""),
        );
        runner.on_exact(
            "git",
            &["-C", &target_str, "remote", "set-url", "origin", repo_url],
            ok_cmd(""),
        );

        let winner =
            git_clone_with_fallback(&target, repo_url, &["--depth=1"], true, &runner).unwrap();
        assert_eq!(
            winner,
            "https://gh-proxy.org/https://github.com/org/repo.git"
        );

        let env_calls = runner.env_invocations();
        assert!(env_calls.iter().any(|(prog, args, env)| {
            prog == "git"
                && args.first().map(String::as_str) == Some("clone")
                && env
                    .iter()
                    .any(|(k, v)| k == "GIT_TERMINAL_PROMPT" && v == "0")
        }));
    }
}
