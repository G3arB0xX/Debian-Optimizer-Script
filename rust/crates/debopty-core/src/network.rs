use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use tempfile::NamedTempFile;

use crate::config::ConfigStore;
use crate::error::{CoreError, Result};
use crate::sys::{self, CommandRunner};

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
        sys::persist_preserving_mode(tmp, dest, None)?;
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
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.into(), MockGetOutcome::Ok(resp));
    }

    pub fn on_get_err(&self, url: impl Into<String>, message: impl Into<String>) {
        self.get_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.into(), MockGetOutcome::Err(message.into()));
    }

    pub fn on_download(&self, url: impl Into<String>, status: u16, payload: impl Into<Vec<u8>>) {
        self.download_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
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
            .unwrap_or_else(|e| e.into_inner())
            .insert(url.into(), MockDownloadOutcome::Err(message.into()));
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn requested_urls(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|r| r.url.clone())
            .collect()
    }
}

impl HttpClient for MockHttpClient {
    fn get_text(&self, req: &HttpRequest<'_>) -> Result<HttpResponse> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(RecordedRequest {
                url: req.url.to_string(),
                ip_preference: req.ip_preference,
                user_agent: req.user_agent.map(ToString::to_string),
                is_download: false,
            });

        match self
            .get_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(req.url)
            .cloned()
        {
            Some(MockGetOutcome::Ok(resp)) => Ok(resp),
            Some(MockGetOutcome::Err(msg)) => Err(CoreError::Network(msg)),
            None => Err(CoreError::Network(format!("unmocked GET {}", req.url))),
        }
    }

    fn download_to_file(&self, req: &HttpRequest<'_>, dest: &Path) -> Result<u16> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(RecordedRequest {
                url: req.url.to_string(),
                ip_preference: req.ip_preference,
                user_agent: req.user_agent.map(ToString::to_string),
                is_download: true,
            });

        match self
            .download_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
                sys::persist_preserving_mode(tmp, dest, None)?;
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

fn write_resolv_atomic(path: &Path, content: &str) -> Result<()> {
    match sys::write_file_atomic(path, content.as_bytes(), Some(0o644)) {
        Ok(()) => Ok(()),
        Err(CoreError::Io(ref e))
            if matches!(e.raw_os_error(), Some(libc::EXDEV) | Some(libc::EBUSY)) =>
        {
            let mut f = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .create(true)
                .open(path)?;
            f.write_all(content.as_bytes())?;
            f.flush()?;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o644));
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn extract_preserved_resolv_lines(existing: &str) -> Vec<String> {
    existing
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("domain ")
                || trimmed.starts_with("domain\t")
                || trimmed.starts_with("search ")
                || trimmed.starts_with("search\t")
                || trimmed.starts_with("options ")
                || trimmed.starts_with("options\t")
        })
        .map(ToString::to_string)
        .collect()
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

            let existing = fs::read_to_string(&opts.resolv_path).unwrap_or_default();

            if !opts.backup_path.exists() && opts.resolv_path.is_file() {
                if let Some(parent) = opts.backup_path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::copy(&opts.resolv_path, &opts.backup_path);
            }

            let mut out_lines: Vec<String> = vec![MANAGED_DNS_MARKER.to_string()];
            out_lines.extend(extract_preserved_resolv_lines(&existing));
            out_lines.push("nameserver 1.1.1.1".to_string());
            let mut content = out_lines.join("\n");
            content.push('\n');
            write_resolv_atomic(&opts.resolv_path, &content)?;
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
            out_lines.extend(extract_preserved_resolv_lines(&existing));
            for ns in &nameservers {
                out_lines.push(format!("nameserver {ns}"));
            }
            let mut rendered = out_lines.join("\n");
            rendered.push('\n');
            write_resolv_atomic(&opts.resolv_path, &rendered)?;

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

    let mut detected: Option<bool> = None;

    // Probe 1: ipinfo.io/country returns a strict 2-letter ISO country code on 2xx
    if let Ok(resp) = http.get_text(&make_req("https://ipinfo.io/country"))
        && (200..=299).contains(&resp.status)
    {
        let code = resp.body.trim();
        if code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()) {
            detected = Some(code.eq_ignore_ascii_case("CN"));
        }
    }

    // Probe 2: myip.ipip.net
    if detected.is_none()
        && let Ok(resp) = http.get_text(&make_req("http://myip.ipip.net"))
        && (200..=299).contains(&resp.status)
        && !resp.body.trim().is_empty()
    {
        detected = Some(resp.body.contains("中国"));
    }

    // Probe 3: cip.cc
    if detected.is_none()
        && let Ok(resp) = http.get_text(&make_req("https://cip.cc"))
        && (200..=299).contains(&resp.status)
        && !resp.body.trim().is_empty()
    {
        detected = Some(resp.body.contains("中国"));
    }

    // Probe 4: api.ip.sb/geoip
    if detected.is_none()
        && let Ok(resp) = http.get_text(&make_req("https://api.ip.sb/geoip"))
        && (200..=299).contains(&resp.status)
        && !resp.body.trim().is_empty()
    {
        detected = Some(resp.body.to_ascii_lowercase().contains("china"));
    }

    let is_cn = detected.ok_or_else(|| {
        CoreError::Network("all region detection probes failed; refusing to cache".to_string())
    })?;

    store.set("IS_CN_REGION", if is_cn { "true" } else { "false" })?;
    Ok(NetcheckReport {
        is_cn_region: is_cn,
        cached: false,
        env_exports: cn_env_exports(is_cn),
    })
}

fn valid_release_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^v?[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")
            .expect("valid release tag regex")
    })
}

fn api_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""tag_name"\s*:\s*"([^"]+)""#).expect("valid api tag regex"))
}

fn release_page_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"tag/(v?[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)")
            .expect("valid release page tag regex")
    })
}

fn raw_github_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^https://raw\.githubusercontent\.com/([^/]+)/([^/]+)/([^/]+)/(.*)$")
            .expect("valid raw github regex")
    })
}

fn ghfast_latest_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/releases/latest([?#].*)?$").expect("valid ghfast latest regex"))
}

fn extract_url_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    if host_port.starts_with('[') {
        host_port.split(']').next().map(|s| &s[1..])
    } else {
        host_port.split(':').next()
    }
    .filter(|h| !h.is_empty())
}

fn is_github_host(url: &str) -> bool {
    let Some(host) = extract_url_host(url) else {
        return false;
    };
    let lower = host.to_ascii_lowercase();
    lower == "github.com"
        || lower.ends_with(".github.com")
        || lower == "githubusercontent.com"
        || lower.ends_with(".githubusercontent.com")
}

fn is_api_github_host(url: &str) -> bool {
    extract_url_host(url).is_some_and(|h| h.eq_ignore_ascii_case("api.github.com"))
}

fn is_valid_release_tag(tag: &str) -> bool {
    valid_release_tag_re().is_match(tag)
}

fn parse_tag_from_api_json(body: &str) -> Option<String> {
    let cap = api_tag_re().captures(body)?;
    let tag = cap.get(1)?.as_str();
    is_valid_release_tag(tag).then(|| tag.to_string())
}

fn parse_tag_from_release_page(body: &str) -> Option<String> {
    let cap = release_page_tag_re().captures(body)?;
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
    let caps = raw_github_re().captures(original_url)?;
    Some(format!(
        "https://cdn.jsdelivr.net/gh/{}/{}@{}/{}",
        &caps[1], &caps[2], &caps[3], &caps[4]
    ))
}

fn is_ghfast_forbidden_url(original_url: &str) -> bool {
    if is_api_github_host(original_url) {
        return true;
    }
    ghfast_latest_re().is_match(original_url)
}

/// Build the ordered candidate URL list for `download_with_fallback`.
pub fn build_download_candidates(original_url: &str, is_cn: bool) -> Vec<String> {
    if !is_cn || !is_github_host(original_url) {
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
        if let Ok(status) = http.download_to_file(&req, target)
            && (200..=299).contains(&status)
            && target.metadata().map(|m| m.len() > 0).unwrap_or(false)
        {
            return Ok(dl_url);
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

const PROTECTED_DIRS: &[&str] = &[
    "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib32", "/lib64", "/libx32", "/media",
    "/mnt", "/opt", "/proc", "/root", "/run", "/sbin", "/srv", "/sys", "/tmp", "/usr", "/var",
];

fn is_protected_dir(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return true;
    }
    let lexical = normalize_lexical(path);
    if lexical.as_os_str().is_empty() {
        return true;
    }
    if PROTECTED_DIRS.iter().any(|p| lexical == Path::new(p)) {
        return true;
    }
    if let Ok(canon) = fs::canonicalize(path)
        && PROTECTED_DIRS.iter().any(|p| canon == Path::new(p))
    {
        return true;
    }
    false
}

fn remove_target_if_exists(target_dir: &Path) -> Result<()> {
    match fs::symlink_metadata(target_dir) {
        Ok(meta) => {
            let ft = meta.file_type();
            if ft.is_symlink() || !ft.is_dir() {
                fs::remove_file(target_dir)?;
            } else {
                fs::remove_dir_all(target_dir)?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CoreError::Io(e)),
    }
}

fn validate_git_repo_url(repo_url: &str) -> Result<()> {
    if repo_url.is_empty()
        || repo_url.starts_with('-')
        || repo_url.contains(['\r', '\n', '\0'])
        || !(repo_url.starts_with("https://")
            || repo_url.starts_with("http://")
            || repo_url.starts_with("ssh://")
            || repo_url.starts_with("git@"))
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid git repository URL: {repo_url}"
        )));
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
    validate_git_repo_url(repo_url)?;
    if is_protected_dir(target_dir) {
        return Err(CoreError::ValidationFailed(format!(
            "refusing to operate on protected directory: {}",
            target_dir.display()
        )));
    }

    let target_str = target_dir.to_string_lossy().into_owned();

    if is_cn && is_github_host(repo_url) {
        let mut candidates: Vec<String> = CN_GIT_PROXIES
            .iter()
            .map(|p| format!("{p}/{repo_url}"))
            .collect();
        candidates.push(repo_url.to_string());

        for clone_url in candidates {
            remove_target_if_exists(target_dir)?;
            let mut args: Vec<&str> = Vec::with_capacity(6 + extra_args.len());
            args.push("-c");
            args.push("http.version=HTTP/1.1");
            args.push("clone");
            args.extend_from_slice(extra_args);
            args.push("--");
            args.push(&clone_url);
            args.push(&target_str);

            if let Ok(out) = runner.run_with_env("git", &args, GIT_ENV, Duration::from_secs(300))
                && out.success()
            {
                let reset = runner.run_with_env(
                    "git",
                    &[
                        "-C",
                        &target_str,
                        "remote",
                        "set-url",
                        "--",
                        "origin",
                        repo_url,
                    ],
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
        let mut args: Vec<&str> = Vec::with_capacity(4 + extra_args.len());
        args.push("clone");
        args.extend_from_slice(extra_args);
        args.push("--");
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
            "# managed-by-debopti\nsearch corp.local\nnameserver 1.1.1.1\n"
        );
        assert_eq!(
            fs::metadata(&resolv).unwrap().permissions().mode() & 0o777,
            0o644
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
        for bad in [
            "", "/", "/usr", "/etc", "/etc/", "/opt", "/home", "/var", "/root", "/tmp", "/bin",
            "/sbin", "/lib", "/lib64", "/boot", "/dev", "/proc", "/sys", ".", "..",
        ] {
            let err = git_clone_with_fallback(
                Path::new(bad),
                "https://github.com/org/repo.git",
                &[],
                true,
                &runner,
            )
            .unwrap_err();
            assert!(
                matches!(err, CoreError::ValidationFailed(_)),
                "expected ValidationFailed for protected dir {bad:?}"
            );
        }

        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("repo");
        let target_str = target.to_string_lossy().into_owned();
        let repo_url = "https://github.com/org/repo.git";

        runner.on_exact(
            "git",
            &[
                "-c",
                "http.version=HTTP/1.1",
                "clone",
                "--depth=1",
                "--",
                "https://ghfast.top/https://github.com/org/repo.git",
                &target_str,
            ],
            err_cmd(128),
        );
        runner.on_exact(
            "git",
            &[
                "-c",
                "http.version=HTTP/1.1",
                "clone",
                "--depth=1",
                "--",
                "https://gh-proxy.org/https://github.com/org/repo.git",
                &target_str,
            ],
            ok_cmd(""),
        );
        runner.on_exact(
            "git",
            &[
                "-C",
                &target_str,
                "remote",
                "set-url",
                "--",
                "origin",
                repo_url,
            ],
            ok_cmd(""),
        );

        let winner =
            git_clone_with_fallback(&target, repo_url, &["--depth=1"], true, &runner).unwrap();
        assert_eq!(
            winner,
            "https://gh-proxy.org/https://github.com/org/repo.git"
        );

        let env_calls = runner.env_invocations();
        // Must NEVER mutate global ~/.gitconfig
        assert!(
            !env_calls
                .iter()
                .any(|(prog, args, _)| prog == "git" && args.iter().any(|a| a == "--global"))
        );
        assert!(env_calls.iter().any(|(prog, args, env)| {
            prog == "git"
                && args.iter().any(|a| a == "http.version=HTTP/1.1")
                && args.iter().any(|a| a == "--")
                && env
                    .iter()
                    .any(|(k, v)| k == "GIT_TERMINAL_PROMPT" && v == "0")
        }));
    }

    #[test]
    fn global_netcheck_rejects_non_2xx_and_does_not_poison_cache_offline() {
        let tmp = TempDir::new().unwrap();
        let store = ConfigStore::new(tmp.path().join("debopti.conf"));

        // Case 1: 429 error page containing "CN" must NOT detect CN; all other 3 fail -> Err & unset cache
        let http = MockHttpClient::new();
        http.on_get(
            "https://ipinfo.io/country",
            HttpResponse {
                status: 429,
                body: "CDN CN error rate limit".to_string(),
            },
        );
        http.on_get_err("http://myip.ipip.net", "offline");
        http.on_get_err("https://cip.cc", "offline");
        http.on_get_err("https://api.ip.sb/geoip", "offline");

        let err = global_netcheck(&store, false, &http).unwrap_err();
        assert!(matches!(err, CoreError::Network(_)));
        assert_eq!(store.get("IS_CN_REGION").unwrap().as_deref(), Some(""));

        // Case 2: 200 OK with "US\n" stops cascading immediately (1 probe total)
        let http_us = MockHttpClient::new();
        http_us.on_get(
            "https://ipinfo.io/country",
            HttpResponse {
                status: 200,
                body: "US\n".to_string(),
            },
        );
        let rep = global_netcheck(&store, true, &http_us).unwrap();
        assert!(!rep.is_cn_region);
        assert_eq!(http_us.requested_urls().len(), 1);
    }

    #[test]
    fn dns_initial_then_calibrate_preserves_search_options_and_0644_mode() {
        let tmp = TempDir::new().unwrap();
        let resolv = tmp.path().join("resolv.conf");
        let backup = tmp.path().join("resolv.conf.bak");
        fs::write(
            &resolv,
            "domain corp.local\nsearch corp.local\noptions edns0\nnameserver 10.0.0.53\n",
        )
        .unwrap();
        fs::set_permissions(&resolv, fs::Permissions::from_mode(0o644)).unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("getent", &["hosts", "cloudflare.com"], err_cmd(2));
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
            backup_path: backup,
            is_cn_region: false,
        };

        let init_rep = check_and_fix_dns(DnsFixMode::Initial, &opts, &http, &runner).unwrap();
        assert!(init_rep.modified);
        let after_init = fs::read_to_string(&resolv).unwrap();
        assert!(after_init.contains("domain corp.local"));
        assert!(after_init.contains("search corp.local"));
        assert!(after_init.contains("options edns0"));
        assert_eq!(
            fs::metadata(&resolv).unwrap().permissions().mode() & 0o777,
            0o644
        );

        let cal_rep = check_and_fix_dns(DnsFixMode::Calibrate, &opts, &http, &runner).unwrap();
        assert!(cal_rep.modified);
        let after_cal = fs::read_to_string(&resolv).unwrap();
        assert!(after_cal.contains("domain corp.local"));
        assert!(after_cal.contains("search corp.local"));
        assert!(after_cal.contains("options edns0"));
        assert!(after_cal.contains("nameserver 8.8.8.8"));
        assert_eq!(
            fs::metadata(&resolv).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn download_preserves_existing_file_on_failure_and_strict_github_host_check() {
        // Strict host parsing: non-GitHub URLs with `github.com` in query/path/userinfo must NOT be proxied
        for non_gh in [
            "https://evil.test/?github.com",
            "https://evilgithub.com/org/repo",
            "https://evil.test/github.com/a/b",
            "https://github.com@evil.test/a/b",
        ] {
            let cands = build_download_candidates(non_gh, true);
            assert_eq!(cands, vec![non_gh.to_string()]);
        }

        // Pre-existing file survives total mirror failure
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("existing.bin");
        fs::write(&dest, b"keep-existing-content").unwrap();
        let http = MockHttpClient::new();
        let err = download_with_fallback(&dest, "https://example.invalid/file", false, &http)
            .unwrap_err();
        assert!(matches!(err, CoreError::Network(_)));
        assert_eq!(fs::read(&dest).unwrap(), b"keep-existing-content");

        // Release tag parsing preserves hyphenated prereleases and rejects malformed tags
        assert_eq!(
            parse_tag_from_release_page(
                r#"<a href="/org/repo/releases/tag/v1.0.0-alpha-1">v1.0.0-alpha-1</a>"#
            )
            .as_deref(),
            Some("v1.0.0-alpha-1")
        );
        assert!(!is_valid_release_tag("v1.0.0/evil"));
        assert!(!is_valid_release_tag("v1.0.0 bad"));
        assert!(is_valid_release_tag("v1.0.0-alpha-1+build.2"));
    }

    #[test]
    fn git_clone_blocks_dash_arg_injection_and_cleans_dangling_symlink() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("repo");
        let runner = MockCommandRunner::new();

        for bad_url in [
            "--upload-pack=sh",
            "-c core.sshCommand=id",
            "file:///etc/passwd",
            "https://github.com/org/repo\n-c",
        ] {
            let err = git_clone_with_fallback(&target, bad_url, &[], true, &runner).unwrap_err();
            assert!(
                matches!(err, CoreError::ValidationFailed(_)),
                "expected ValidationFailed for {bad_url:?}"
            );
        }

        // Create a dangling symlink at target and verify clone removes it cleanly
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("nonexistent-target"), &target).unwrap();
        assert!(fs::symlink_metadata(&target).is_ok());
        assert!(!target.exists());

        let target_str = target.to_string_lossy().into_owned();
        let repo_url = "https://gitlab.example/org/repo.git";
        runner.on_exact("git", &["clone", "--", repo_url, &target_str], ok_cmd(""));

        let winner = git_clone_with_fallback(&target, repo_url, &[], true, &runner).unwrap();
        assert_eq!(winner, repo_url);
        assert!(fs::symlink_metadata(&target).is_err());
    }
}
