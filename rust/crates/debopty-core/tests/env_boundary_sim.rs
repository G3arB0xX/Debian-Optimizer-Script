use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::time::{Duration, Instant};

use debopty_core::config::ConfigStore;
use debopty_core::diag::{
    DiagConfig, DiagRecord, LogFormat, LogLevel, emit_to, format_cli_error, format_record,
    redact_sensitive, resolve_format, resolve_level, utc_rfc3339_now,
};
use debopty_core::error::CoreError;
use debopty_core::network::{
    DnsFixMode, DnsFixOptions, HttpResponse, MockHttpClient, build_download_candidates,
    check_and_fix_dns, download_with_fallback, get_latest_github_release, git_clone_with_fallback,
    global_netcheck,
};
use debopty_core::security::SecurityContext;
use debopty_core::security::fail2ban::{detect_fail2ban_backend, detect_ssh_port, setup_fail2ban};
use debopty_core::security::firewall::{
    add_fw_rule, format_nft_port, update_ssh_firewall_ports, validate_fw_comment, validate_fw_port,
};
use debopty_core::sys::{
    CmdOutput, CommandRunner, MockCommandRunner, RealCommandRunner, detect_debian_major,
    safe_apt_install, set_conf_value, write_file_atomic,
};
use debopty_core::system::{
    MAX_SWAP_MB, MemoryOptions, SystemContext, setup_base, setup_kernel, setup_limits,
    setup_logrotate, setup_low_memory_optimization, setup_memory, setup_sysctl, setup_timezone,
    toggle_ip_forwarding,
};
use debopty_core::template::TemplateEngine;
use debopty_core::version::{BumpKind, next_semver, semver_compare};
use tempfile::TempDir;

fn ok_cmd(stdout: &str) -> CmdOutput {
    CmdOutput {
        code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn err_cmd(code: i32, stderr: &str) -> CmdOutput {
    CmdOutput {
        code,
        stdout: String::new(),
        stderr: stderr.to_string(),
        timed_out: false,
    }
}

fn synth_pem(label: &str, body: &str) -> String {
    let dash = "-".repeat(5);
    let beg = ["BEG", "IN"].concat();
    let end = ["EN", "D"].concat();
    format!("{dash}{beg} {label}{dash}\n{body}\n{dash}{end} {label}{dash}")
}

// =========================================================================
// 1. Debian 10-13 / sid / forky matrix & setup_base across APT formats
// =========================================================================

#[test]
fn debian_major_matrix_covers_10_to_14_codenames_and_garbage() {
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("debian_version");

    let cases = [
        ("10.13\n", Some(10)),
        ("11.11\n", Some(11)),
        ("12.8\n", Some(12)),
        ("13.0\n", Some(13)),
        ("buster/sid\n", Some(10)),
        ("bullseye\n", Some(11)),
        ("bookworm/sid\n", Some(12)),
        ("trixie/sid\n", Some(13)),
        ("sid\n", Some(13)),
        ("testing\n", Some(13)),
        ("unstable\n", Some(13)),
        ("forky/sid\n", Some(14)),
        ("", None),
        ("   \n", None),
        ("garbage-distro-xyz\n", None),
    ];

    for (raw, expected) in cases {
        fs::write(&p, raw).unwrap();
        assert_eq!(
            detect_debian_major(&p),
            expected,
            "unexpected detect_debian_major for {raw:?}"
        );
    }

    assert_eq!(detect_debian_major(&tmp.path().join("missing")), None);
}

#[test]
fn setup_base_matrix_debian_10_eol_11_12_and_13_deb822() {
    // (a) Debian 10 Buster EOL Global vs CN
    for (is_cn, expected_host) in [
        (false, "archive.debian.org"),
        (true, "mirrors.tuna.tsinghua.edu.cn"),
    ] {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc/apt")).unwrap();
        fs::write(root.join("etc/debian_version"), "10.13\n").unwrap();
        fs::write(
            root.join("etc/apt/sources.list"),
            "deb http://deb.debian.org/debian buster main\n",
        )
        .unwrap();

        let mut ctx = SystemContext::with_root(root);
        ctx.is_cn_region = is_cn;
        ctx.no_sys = true;
        let runner = MockCommandRunner::new();
        setup_base(&ctx, &runner).unwrap();

        let sources = fs::read_to_string(root.join("etc/apt/sources.list")).unwrap();
        assert!(
            sources.contains(expected_host),
            "buster is_cn={is_cn}: {sources}"
        );
        assert!(root.join("etc/apt/sources.list.bak").is_file());
        if is_cn {
            assert!(!sources.contains("http://"));
        }
    }

    // (b) Debian 11 Bullseye CN rewrites sources.list to TUNA HTTPS & installs dnsutils
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc/apt")).unwrap();
        fs::write(root.join("etc/debian_version"), "11.11\n").unwrap();
        fs::write(
            root.join("etc/apt/sources.list"),
            "deb http://deb.debian.org/debian bullseye main\ndeb http://security.debian.org/debian-security bullseye-security main\n",
        )
        .unwrap();
        fs::set_permissions(
            root.join("etc/apt/sources.list"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let mut ctx = SystemContext::with_root(root);
        ctx.is_cn_region = true;
        let runner = MockCommandRunner::new();
        runner.on_prog("dpkg-query", err_cmd(1, "not installed"));
        runner.on_prog("apt-get", ok_cmd(""));

        let installed = setup_base(&ctx, &runner).unwrap();
        assert!(installed.contains(&"dnsutils".to_string()));
        assert!(!installed.contains(&"bind9-dnsutils".to_string()));

        let sources = fs::read_to_string(root.join("etc/apt/sources.list")).unwrap();
        assert!(sources.contains("https://mirrors.tuna.tsinghua.edu.cn/debian bullseye"));
        assert!(!sources.contains("http://"));
        assert_eq!(
            fs::metadata(root.join("etc/apt/sources.list"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    // (c) Debian 12 Bookworm & Debian 13 trixie/sid DEB822 debian.sources + bind9-dnsutils on 13
    for (ver_str, expected_dns_pkg) in [("12.8\n", "dnsutils"), ("trixie/sid\n", "bind9-dnsutils")]
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc/apt/sources.list.d")).unwrap();
        fs::write(root.join("etc/debian_version"), ver_str).unwrap();
        fs::write(
            root.join("etc/apt/sources.list.d/debian.sources"),
            "Types: deb\nURIs: http://deb.debian.org/debian\nSuites: trixie\nComponents: main\n",
        )
        .unwrap();

        let mut ctx = SystemContext::with_root(root);
        ctx.is_cn_region = true;
        let runner = MockCommandRunner::new();
        runner.on_prog("dpkg-query", err_cmd(1, "not installed"));
        runner.on_prog("apt-get", ok_cmd(""));

        let installed = setup_base(&ctx, &runner).unwrap();
        assert!(installed.contains(&expected_dns_pkg.to_string()));

        let deb822 =
            fs::read_to_string(root.join("etc/apt/sources.list.d/debian.sources")).unwrap();
        assert!(deb822.contains("https://mirrors.tuna.tsinghua.edu.cn/debian"));
        assert!(root.join("etc/apt/debian.sources.bak").is_file());
    }
}

// =========================================================================
// 2. Container, Non-Systemd & no_sys=true Isolation
// =========================================================================

#[test]
fn container_non_systemd_f2b_backend_and_no_sys_zero_exec() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Isolated root WITHOUT /run/systemd/system must return "auto" regardless of host PID 1
    let sec_ctx = SecurityContext::with_root(root);
    let runner = MockCommandRunner::new();
    runner.on_prog("python3", ok_cmd(""));
    assert_eq!(detect_fail2ban_backend(&sec_ctx, &runner), "auto");

    // With /run/systemd/system + working python3-systemd -> "systemd"
    fs::create_dir_all(root.join("run/systemd/system")).unwrap();
    assert_eq!(detect_fail2ban_backend(&sec_ctx, &runner), "systemd");

    // With /run/systemd/system but NO python3-systemd -> "auto"
    let no_py_runner = MockCommandRunner::new();
    no_py_runner.on_prog("python3", err_cmd(1, "ModuleNotFoundError"));
    no_py_runner.on_prog("dpkg-query", err_cmd(1, "not installed"));
    assert_eq!(detect_fail2ban_backend(&sec_ctx, &no_py_runner), "auto");

    // End-to-end no_sys=true executes ZERO host commands across system & security ops
    let mut sys_ctx = SystemContext::with_root(root);
    sys_ctx.no_sys = true;
    fs::create_dir_all(root.join("etc/systemd")).unwrap();
    fs::create_dir_all(root.join("etc/apt")).unwrap();
    fs::create_dir_all(root.join("proc")).unwrap();
    fs::write(root.join("etc/debian_version"), "12.8\n").unwrap();
    fs::write(root.join("etc/systemd/logind.conf"), "#NAutoVTs=6\n").unwrap();
    fs::write(root.join("etc/systemd/journald.conf"), "#SystemMaxUse=\n").unwrap();
    fs::write(root.join("proc/meminfo"), "MemTotal:       262144 kB\n").unwrap();

    let strict_runner = MockCommandRunner::new();
    setup_base(&sys_ctx, &strict_runner).unwrap();
    assert!(!setup_kernel(&sys_ctx, Some(true), &strict_runner).unwrap());
    setup_sysctl(&sys_ctx, &strict_runner).unwrap();
    setup_limits(&sys_ctx).unwrap();
    setup_logrotate(&sys_ctx).unwrap();
    setup_memory(&sys_ctx, &MemoryOptions::default(), &strict_runner).unwrap();
    setup_low_memory_optimization(&sys_ctx, true, &strict_runner).unwrap();
    setup_timezone(&sys_ctx, &strict_runner).unwrap();
    assert!(toggle_ip_forwarding(&sys_ctx, &strict_runner).unwrap());

    let mut f2b_ctx = SecurityContext::with_root(root);
    f2b_ctx.no_sys = true;
    f2b_ctx.f2b_backend_override = Some("auto".to_string());
    setup_fail2ban(&f2b_ctx, Some(2222), &strict_runner).unwrap();

    let mutating_progs = [
        "apt-get",
        "systemctl",
        "nft",
        "timedatectl",
        "fallocate",
        "dd",
        "mkswap",
        "swapon",
        "sysctl",
    ];
    let bad_calls: Vec<_> = strict_runner
        .invocations()
        .into_iter()
        .filter(|(prog, _)| mutating_progs.contains(&prog.as_str()))
        .collect();
    assert!(
        bad_calls.is_empty(),
        "no_sys=true must execute zero mutating commands, got: {bad_calls:?}"
    );
}

// =========================================================================
// 3. Network, Dual-Stack DNS, Offline, CN 429/502, Download & Git Injection
// =========================================================================

#[test]
fn global_netcheck_rejects_429_cn_false_positive_and_offline_never_poisons_config() {
    let tmp = TempDir::new().unwrap();
    let conf_path = tmp.path().join("debopti.conf");
    let store = ConfigStore::new(&conf_path);

    // HTTP 429 with body containing "CN" must NOT trigger CN detection; falls through to Probe 4 ("United States")
    let http_429 = MockHttpClient::new();
    http_429.on_get(
        "https://ipinfo.io/country",
        HttpResponse {
            status: 429,
            body: "CDN CN error: rate limited".to_string(),
        },
    );
    http_429.on_get_err("http://myip.ipip.net", "timeout");
    http_429.on_get(
        "https://cip.cc",
        HttpResponse {
            status: 502,
            body: "502 Bad Gateway 中国".to_string(),
        },
    );
    http_429.on_get(
        "https://api.ip.sb/geoip",
        HttpResponse {
            status: 200,
            body: r#"{"country":"United States"}"#.to_string(),
        },
    );

    let rep = global_netcheck(&store, true, &http_429).unwrap();
    assert!(!rep.is_cn_region);
    assert_eq!(store.get("IS_CN_REGION").unwrap().as_deref(), Some("false"));

    // All 4 probes offline / HTTP 502 must error and NEVER poison/overwrite config
    let fresh_conf = tmp.path().join("offline.conf");
    let offline_store = ConfigStore::new(&fresh_conf);
    let http_offline = MockHttpClient::new();
    http_offline.on_get_err("https://ipinfo.io/country", "dns failed");
    http_offline.on_get(
        "http://myip.ipip.net",
        HttpResponse {
            status: 502,
            body: "Bad Gateway".to_string(),
        },
    );
    http_offline.on_get_err("https://cip.cc", "connection refused");
    http_offline.on_get(
        "https://api.ip.sb/geoip",
        HttpResponse {
            status: 503,
            body: String::new(),
        },
    );

    let err = global_netcheck(&offline_store, true, &http_offline).unwrap_err();
    assert!(matches!(err, CoreError::Network(_)));
    assert_eq!(err.error_code(), "E3001");
    assert!(
        !fresh_conf.exists(),
        "offline netcheck must not write config"
    );
}

#[test]
fn dns_dual_stack_ipv4_only_ipv6_only_offline_preserves_directives_and_0644() {
    let cases = [
        (
            "ipv4-global",
            false,
            true,
            false,
            vec!["8.8.8.8", "1.1.1.1"],
        ),
        (
            "ipv6-only-global",
            false,
            false,
            true,
            vec!["2001:4860:4860::8888", "2606:4700:4700::1111"],
        ),
        (
            "dual-stack-cn",
            true,
            true,
            true,
            vec!["223.5.5.5", "119.29.29.29", "2402:4e00::", "2400:3200::1"],
        ),
    ];

    for (label, is_cn, has_v4, has_v6, expected_ns) in cases {
        let tmp = TempDir::new().unwrap();
        let resolv = tmp.path().join("resolv.conf");
        let backup = tmp.path().join("resolv.conf.debopti.bak");
        fs::write(
            &resolv,
            "search corp.internal example.local\noptions edns0 timeout:2\nnameserver 192.0.2.1\n",
        )
        .unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("getent", &["hosts", "cloudflare.com"], err_cmd(2, "fail"));
        runner.on_exact(
            "ip",
            &["-6", "addr", "show", "scope", "global"],
            if has_v6 {
                ok_cmd("    inet6 2001:db8::10/64 scope global\n")
            } else {
                ok_cmd("")
            },
        );
        if has_v6 {
            runner.on_prog("curl", ok_cmd(""));
        }

        let http = MockHttpClient::new();
        if has_v4 {
            http.on_get(
                "http://1.1.1.1",
                HttpResponse {
                    status: 200,
                    body: "ok".to_string(),
                },
            );
        } else {
            http.on_get_err("http://1.1.1.1", "network unreachable");
        }

        let opts = DnsFixOptions {
            resolv_path: resolv.clone(),
            backup_path: backup.clone(),
            is_cn_region: is_cn,
        };

        let init = check_and_fix_dns(DnsFixMode::Initial, &opts, &http, &runner).unwrap();
        assert!(init.modified, "{label} initial should modify");
        assert!(backup.is_file());

        let cal = check_and_fix_dns(DnsFixMode::Calibrate, &opts, &http, &runner).unwrap();
        assert!(cal.modified, "{label} calibrate should modify");
        assert_eq!(cal.has_v4, has_v4, "{label}");
        assert_eq!(cal.has_v6, has_v6, "{label}");
        assert_eq!(
            cal.nameservers,
            expected_ns
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>(),
            "{label}"
        );

        let final_txt = fs::read_to_string(&resolv).unwrap();
        assert!(final_txt.contains("search corp.internal example.local"));
        assert!(final_txt.contains("options edns0 timeout:2"));
        assert_eq!(
            fs::metadata(&resolv).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }
}

#[test]
fn download_and_git_clone_security_boundaries_and_mirror_safety() {
    let tmp = TempDir::new().unwrap();

    // 1. Query string `?github.com` must NOT be mis-proxied as a GitHub host
    let non_gh_url = "https://mirror.example.org/file.tar.gz?ref=github.com";
    assert_eq!(
        build_download_candidates(non_gh_url, true),
        vec![non_gh_url.to_string()]
    );

    // 2. CN API/latest excludes ghfast.top
    let latest_url = "https://github.com/owner/repo/releases/latest";
    let cands = build_download_candidates(latest_url, true);
    assert!(!cands.iter().any(|u| u.contains("ghfast.top")));

    let http_api = MockHttpClient::new();
    http_api.on_get(
        "https://gh-proxy.com/https://api.github.com/repos/foo/bar/releases/latest",
        HttpResponse {
            status: 502,
            body: "Bad Gateway".to_string(),
        },
    );
    http_api.on_get(
        "https://edgeone.gh-proxy.org/https://api.github.com/repos/foo/bar/releases/latest",
        HttpResponse {
            status: 200,
            body: r#"{"tag_name": "v2.4.1"}"#.to_string(),
        },
    );
    assert_eq!(
        get_latest_github_release("foo/bar", true, &http_api)
            .unwrap()
            .as_deref(),
        Some("v2.4.1")
    );
    assert!(
        !http_api
            .requested_urls()
            .iter()
            .any(|u| u.contains("ghfast.top"))
    );

    // 3. Total download failure (HTTP 502 across all mirrors) preserves pre-existing destination
    let dest = tmp.path().join("existing.bin");
    fs::write(&dest, b"ORIGINAL_GOOD_BYTES").unwrap();
    let http_502 = MockHttpClient::new();
    for url in build_download_candidates("https://github.com/a/b/releases/download/v1/x", true) {
        http_502.on_download(url, 502, b"Bad Gateway".to_vec());
    }
    let dl_err = download_with_fallback(
        &dest,
        "https://github.com/a/b/releases/download/v1/x",
        true,
        &http_502,
    )
    .unwrap_err();
    assert!(matches!(dl_err, CoreError::Network(_)));
    assert_eq!(fs::read(&dest).unwrap(), b"ORIGINAL_GOOD_BYTES");

    // 4. git_clone_with_fallback rejects flag injection & all FHS protected dirs
    let runner = MockCommandRunner::new();
    let safe_dest = tmp.path().join("repo_dest");
    for bad_url in [
        "--upload-pack=touch /tmp/pwn",
        "-u evil",
        "file:///etc/passwd",
        "https://github.com/a/b\n--upload-pack=id",
        "",
    ] {
        let err = git_clone_with_fallback(&safe_dest, bad_url, &[], true, &runner).unwrap_err();
        assert!(
            matches!(err, CoreError::ValidationFailed(_)),
            "bad_url={bad_url:?}"
        );
    }

    for protected in [
        "/",
        "/opt",
        "/home",
        "/var",
        "/root",
        "/etc",
        "/usr",
        "/tmp",
        "/opt/sub/..",
    ] {
        let err = git_clone_with_fallback(
            Path::new(protected),
            "https://github.com/a/b.git",
            &[],
            false,
            &runner,
        )
        .unwrap_err();
        assert!(
            matches!(err, CoreError::ValidationFailed(_)),
            "protected={protected}"
        );
    }
    assert!(runner.invocations().is_empty());

    // 5. Cleans pre-existing dangling symlink & uses `-c http.version=HTTP/1.1` + `--` + origin reset
    let symlink_dest = tmp.path().join("dangling_repo");
    symlink("/nonexistent/broken_target", &symlink_dest).unwrap();
    let clone_runner = MockCommandRunner::new();
    clone_runner.on_prog("git", ok_cmd(""));
    let winner = git_clone_with_fallback(
        &symlink_dest,
        "https://github.com/owner/repo.git",
        &["--depth=1"],
        true,
        &clone_runner,
    )
    .unwrap();
    assert!(winner.starts_with("https://ghfast.top/"));
    assert!(fs::symlink_metadata(&symlink_dest).is_err());

    let calls = clone_runner.invocations();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "git");
    assert_eq!(&calls[0].1[0..3], &["-c", "http.version=HTTP/1.1", "clone"]);
    assert!(calls[0].1.contains(&"--".to_string()));
    assert_eq!(calls[1].1[2..5], ["remote", "set-url", "--"]);
}

// =========================================================================
// 4. RAM/Swap Cap, Kernel Purge Guard, dpkg rc, Process Timeout, Config, SemVer, Templates & Security
// =========================================================================

#[test]
fn ram_swap_cap_fallocate_fallback_kernel_guards_dpkg_rc_and_grandchild_timeout() {
    // (a) 256MB RAM -> 512MB swap with fallocate failure falling back to dd
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("proc")).unwrap();
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(root.join("proc/meminfo"), "MemTotal:       262144 kB\n").unwrap();
        fs::write(root.join("proc/swaps"), "Filename\tType\tSize\n").unwrap();

        let ctx = SystemContext::with_root(root);
        let runner = MockCommandRunner::new();
        runner.on_prog("fallocate", err_cmd(1, "Operation not supported"));
        runner.on_prog("dd", ok_cmd(""));
        runner.on_prog("chmod", ok_cmd(""));
        runner.on_prog("mkswap", ok_cmd(""));
        runner.on_prog("swapon", ok_cmd(""));

        let rep = setup_memory(
            &ctx,
            &MemoryOptions {
                enable_zram: false,
                enable_swap: true,
            },
            &runner,
        )
        .unwrap();
        assert!(rep.swap_created);
        assert_eq!(rep.swap_size_mb, 512);
        assert!(runner.invocations().iter().any(|(p, _)| p == "dd"));
    }

    // (b) 128GB RAM -> capped strictly at MAX_SWAP_MB (8192MB)
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("proc")).unwrap();
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(root.join("proc/meminfo"), "MemTotal:     134217728 kB\n").unwrap();
        fs::write(root.join("proc/swaps"), "").unwrap();

        let ctx = SystemContext::with_root(root);
        let runner = MockCommandRunner::new();
        runner.on_prog("fallocate", ok_cmd(""));
        runner.on_prog("chmod", ok_cmd(""));
        runner.on_prog("mkswap", ok_cmd(""));
        runner.on_prog("swapon", ok_cmd(""));

        let rep = setup_memory(
            &ctx,
            &MemoryOptions {
                enable_zram: false,
                enable_swap: true,
            },
            &runner,
        )
        .unwrap();
        assert!(rep.swap_created);
        assert_eq!(rep.swap_size_mb, MAX_SWAP_MB);
    }

    // (c) setup_kernel: empty uname -r OR failed update-grub MUST abort old-kernel purge
    for (uname_out, grub_out) in [
        (ok_cmd(""), ok_cmd("")),
        (ok_cmd("6.1.0-20-amd64\n"), err_cmd(1, "grub failed")),
    ] {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());
        let runner = MockCommandRunner::new();
        runner.on_exact("uname", &["-r"], uname_out);
        runner.on_exact("uname", &["-m"], ok_cmd("x86_64\n"));
        runner.on_prog("apt-get", ok_cmd(""));
        runner.on_exact("update-grub", &[], grub_out);
        runner.on_exact(
            "dpkg",
            &["-l"],
            ok_cmd("ii  linux-image-6.1.0-18-amd64 6.1.0 amd64 old\n"),
        );

        assert!(setup_kernel(&ctx, Some(true), &runner).unwrap());
        assert!(
            !runner.invocations().iter().any(
                |(p, args)| p == "apt-get" && args.first().map(String::as_str) == Some("purge")
            ),
            "must NOT purge old kernels when uname -r empty or update-grub failed"
        );
    }

    // (d) safe_apt_install reinstalls `deinstall ok config-files` (rc) packages
    {
        let runner = MockCommandRunner::new();
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "kept-pkg"],
            ok_cmd("install ok installed"),
        );
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "rc-pkg"],
            ok_cmd("deinstall ok config-files"),
        );
        runner.on_prog("apt-get", ok_cmd(""));

        let installed = safe_apt_install(&["kept-pkg", "rc-pkg"], &runner).unwrap();
        assert_eq!(installed, vec!["rc-pkg".to_string()]);
    }

    // (e) RealCommandRunner grandchild pipe-holding timeout returns promptly (< 4s)
    {
        let runner = RealCommandRunner;
        let started = Instant::now();
        let out = runner
            .run(
                "sh",
                &["-c", "(sleep 30) & sleep 30"],
                Duration::from_millis(150),
            )
            .unwrap();
        let elapsed = started.elapsed();
        assert!(out.timed_out);
        assert!(!out.success());
        assert!(
            elapsed < Duration::from_secs(4),
            "grandchild pipe-holding timeout took too long: {elapsed:?}"
        );
    }
}

#[test]
fn config_semver_template_mode_and_security_boundaries() {
    let tmp = TempDir::new().unwrap();

    // 1. ConfigStore CRLF / inline comments / duplicates / NUL rejection / 0644
    let cfg_path = tmp.path().join("debopti.conf");
    fs::write(
        &cfg_path,
        "# note\r\nFOO=\"first\" # inline\r\nFOO=\"second\"\r\nBAR='keep # hash' # cmt\r\n",
    )
    .unwrap();
    fs::set_permissions(&cfg_path, fs::Permissions::from_mode(0o644)).unwrap();
    let store = ConfigStore::new(&cfg_path);
    assert_eq!(store.get("FOO").unwrap().as_deref(), Some("second"));
    assert_eq!(store.get("BAR").unwrap().as_deref(), Some("keep # hash"));
    store.set("FOO", "updated").unwrap();
    assert_eq!(
        fs::read_to_string(&cfg_path)
            .unwrap()
            .matches("FOO=")
            .count(),
        1
    );
    assert!(store.set("FOO", "bad\0nul").is_err());
    assert!(store.set("FOO", "bad\rcr").is_err());
    assert!(store.set("FOO", "bad\nlf").is_err());
    assert_eq!(
        fs::metadata(&cfg_path).unwrap().permissions().mode() & 0o777,
        0o644
    );

    // 2. SemVer u64::MAX overflow returns InvalidBump (no panic) & > u128::MAX prerelease ordering
    let max_u64 = u64::MAX.to_string();
    for (cur, bump) in [
        (format!("{max_u64}.0.0"), BumpKind::Major),
        (format!("0.{max_u64}.0"), BumpKind::Minor),
        (format!("0.0.{max_u64}"), BumpKind::Patch),
        (format!("1.0.0-rc.{max_u64}"), BumpKind::Prerelease),
    ] {
        let err = next_semver(Some(&cur), bump, None).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidBump(_)),
            "expected InvalidBump for {cur} {bump:?}, got {err:?}"
        );
        assert_eq!(err.error_code(), "E2002");
    }

    // 2^128 = 340282366920938463463374607431768211456
    let huge_a = "1.0.0-340282366920938463463374607431768211456";
    let huge_b = "1.0.0-340282366920938463463374607431768211457";
    let huge_c = "1.0.0-99999999999999999999999999999999999999999";
    assert_eq!(semver_compare(huge_a, huge_b).unwrap(), Ordering::Less);
    assert_eq!(semver_compare(huge_b, huge_a).unwrap(), Ordering::Greater);
    assert_eq!(
        semver_compare(huge_c, "1.0.0-1").unwrap(),
        Ordering::Greater
    );
    assert_eq!(semver_compare("1.0.0-9", huge_a).unwrap(), Ordering::Less);

    // 3. TemplateEngine traversal & double-substitution prevention + 0644 mode
    let ov_dir = tmp.path().join("tmpl_override");
    fs::create_dir_all(&ov_dir).unwrap();
    fs::write(ov_dir.join("test.tmpl"), "X={{X}};Y={{Y}}\n").unwrap();
    let engine = TemplateEngine::new(Some(ov_dir));
    assert!(matches!(
        engine.get_raw("../debopti.conf").unwrap_err(),
        CoreError::ValidationFailed(_)
    ));
    let mut vars = BTreeMap::new();
    vars.insert("X".to_string(), "{{Y}}".to_string());
    vars.insert("Y".to_string(), "BOOM".to_string());
    assert_eq!(
        engine.render_to_string("test.tmpl", &vars).unwrap(),
        "X={{Y}};Y=BOOM\n"
    );

    let conf_file = tmp.path().join("sys_mode.conf");
    write_file_atomic(&conf_file, b"K=1\n", None).unwrap();
    set_conf_value(&conf_file, "K", "2", "=").unwrap();
    assert_eq!(
        fs::metadata(&conf_file).unwrap().permissions().mode() & 0o777,
        0o644
    );

    // 4. SSH port precedence (ssh.socket > sshd_config.d > sshd_config above Match) + nft boundaries
    let sec_root = tmp.path().join("sec_root");
    fs::create_dir_all(sec_root.join("etc/ssh/sshd_config.d")).unwrap();
    fs::create_dir_all(sec_root.join("etc/systemd/system/ssh.socket.d")).unwrap();
    fs::create_dir_all(sec_root.join("etc/nftables.d/debopti")).unwrap();
    fs::write(
        sec_root.join("etc/ssh/sshd_config"),
        "Match User backup\n    Port 60000\n",
    )
    .unwrap();
    let runner = MockCommandRunner::new();
    let mut sec_ctx = SecurityContext::with_root(&sec_root);
    sec_ctx.no_sys = true;
    // Port inside Match block must be ignored -> falls back to 22
    assert_eq!(detect_ssh_port(&sec_ctx, &runner), 22);
    fs::write(
        sec_root.join("etc/ssh/sshd_config.d/50-cloud-init.conf"),
        "Port 44444\n",
    )
    .unwrap();
    assert_eq!(detect_ssh_port(&sec_ctx, &runner), 44444);
    fs::write(
        sec_root.join("etc/systemd/system/ssh.socket.d/override.conf"),
        "[Socket]\nListenStream=\nListenStream=55555\n",
    )
    .unwrap();
    assert_eq!(detect_ssh_port(&sec_ctx, &runner), 55555);

    // nft braced ports, validation, and rollback on nft -c -f failure
    assert_eq!(format_nft_port("80,443"), "{ 80,443 }");
    assert_eq!(format_nft_port("10000:10010"), "{ 10000 - 10010 }");
    assert!(validate_fw_port("{ 80, 443 }").is_ok());
    assert!(validate_fw_port("{ 80, { 443 } }").is_err());
    assert!(validate_fw_port("0").is_err());
    assert!(validate_fw_comment("../escape").is_err());

    let nft_conf = sec_root.join("etc/nftables.conf");
    let orig_nft = "table inet filter {\n    chain input {\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n";
    fs::write(&nft_conf, orig_nft).unwrap();
    let mut live_ctx = SecurityContext::with_root(&sec_root);
    live_ctx.no_sys = false;
    let fail_nft_runner = MockCommandRunner::new();
    fail_nft_runner.on_prog("nft", err_cmd(1, "syntax error"));
    let nft_err = update_ssh_firewall_ports(&live_ctx, &[22, 55555], &fail_nft_runner).unwrap_err();
    assert!(matches!(nft_err, CoreError::Security(_)));
    assert_eq!(fs::read_to_string(&nft_conf).unwrap(), orig_nft);

    // add_fw_rule cleans up created .nft file if dry-run fails
    let add_err = add_fw_rule(&live_ctx, "8080", "tcp", "BadRule", &fail_nft_runner).unwrap_err();
    assert!(matches!(add_err, CoreError::Security(_)));
    assert!(!sec_root.join("etc/nftables.d/debopti/BadRule.nft").exists());
}

// =========================================================================
// 5. [U10] Diagnostic Taxonomy, Level Filtering, JSON/Text & Redaction
// =========================================================================

#[test]
fn u10_diag_taxonomy_levels_formats_and_secret_redaction() {
    let variants: Vec<(CoreError, &str)> = vec![
        (CoreError::Io(io::Error::other("io")), "E1001"),
        (CoreError::ValidationFailed("v".into()), "E1003"),
        (CoreError::InvalidVersion("v".into()), "E2001"),
        (CoreError::InvalidBump("b".into()), "E2002"),
        (CoreError::InvalidConfig("c".into()), "E2003"),
        (CoreError::TemplateNotFound("t".into()), "E2004"),
        (CoreError::Network("n".into()), "E3001"),
        (
            CoreError::HttpStatus {
                url: "https://example.invalid".into(),
                status: 502,
            },
            "E3002",
        ),
        (CoreError::System("s".into()), "E4001"),
        (
            CoreError::CommandFailed {
                prog: "nft".into(),
                message: "err".into(),
            },
            "E4002",
        ),
        (
            CoreError::CommandTimeout {
                prog: "apt-get".into(),
                secs: 60,
            },
            "E4003",
        ),
        (CoreError::Security("sec".into()), "E5001"),
        (CoreError::Reconcile("rec".into()), "E5002"),
    ];

    for (err, expected_code) in variants {
        assert_eq!(err.error_code(), expected_code);
        let hint = err.remediation_hint();
        assert!(!hint.trim().is_empty());
        assert!(
            hint.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
            "hint for {expected_code} must contain actionable Simplified Chinese: {hint}"
        );
    }

    // Level resolution & ordering
    assert!(LogLevel::Error < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Info);
    assert!(LogLevel::Info < LogLevel::Debug);
    assert!(LogLevel::Debug < LogLevel::Trace);
    assert_eq!(
        resolve_level(None, 0, false, Some("debopty=trace")).unwrap(),
        LogLevel::Trace
    );
    assert_eq!(
        resolve_level(None, 2, false, Some("error")).unwrap(),
        LogLevel::Debug
    );
    assert_eq!(resolve_level(None, 3, true, None).unwrap(), LogLevel::Error);
    assert_eq!(
        resolve_format(Some("json"), Some("text")).unwrap(),
        LogFormat::Json
    );

    // Emit filtering + RFC 3339 + JSON & Text formatting + secret redaction
    let bearer_tok = ["tok_", "live_999"].concat();
    let pem_fixture = synth_pem(&["PRIVATE ", "KEY"].concat(), "MIIE_SECRET_DATA");
    let rec = DiagRecord {
        ts: utc_rfc3339_now(),
        level: LogLevel::Info,
        target: "debopty::sim".to_string(),
        code: Some("E3001".to_string()),
        msg: format!("auth failed password=TopSecret42 Authorization: Bearer {bearer_tok}"),
        hint: Some("请检查凭据".to_string()),
        fields: BTreeMap::from([("pem".to_string(), pem_fixture)]),
    };

    let mut filtered = Vec::new();
    emit_to(
        &mut filtered,
        &DiagConfig {
            level: LogLevel::Warn,
            format: LogFormat::Json,
            no_color: true,
        },
        &rec,
    );
    assert!(filtered.is_empty());

    let mut json_sink = Vec::new();
    emit_to(
        &mut json_sink,
        &DiagConfig {
            level: LogLevel::Info,
            format: LogFormat::Json,
            no_color: true,
        },
        &rec,
    );
    let json_line = String::from_utf8(json_sink).unwrap();
    let parsed: DiagRecord = serde_json::from_str(json_line.trim()).unwrap();
    assert_eq!(parsed.ts.len(), 20);
    assert!(parsed.ts.ends_with('Z'));
    assert_eq!(parsed.code.as_deref(), Some("E3001"));
    assert!(!parsed.msg.contains("TopSecret42"));
    assert!(!parsed.msg.contains(&bearer_tok));
    assert_eq!(
        parsed.fields.get("pem").map(String::as_str),
        Some("[REDACTED_PRIVATE_KEY]")
    );

    let text_line = format_record(&rec, LogFormat::Text);
    assert!(text_line.contains("[INFO][E3001] debopty::sim:"));
    assert!(text_line.contains("  hint: 请检查凭据"));
    assert!(!text_line.contains("TopSecret42"));

    let cli_err = format_cli_error(
        "E5001",
        "failed with --password SuperPass123",
        "请检查 SSH 配置",
        LogFormat::Text,
    );
    assert!(cli_err.starts_with("[ERROR][E5001] debopty: failed with --password [REDACTED]"));
    assert!(cli_err.contains("  hint: 请检查 SSH 配置"));
    let sk_val = ["sk", "_123456"].concat();
    let api_kv = format!("api_key={sk_val}");
    assert!(!redact_sensitive(&api_kv).contains(&sk_val));
}

// =========================================================================
// 6. [U14] Red-Team Round-1 Backend Regression Suite (sys/diag/system/sec/version)
// =========================================================================

#[test]
fn u14_be_p0_03_security_context_defaults_to_current_exe_multicall() {
    let cur = std::env::current_exe().unwrap();
    assert_eq!(SecurityContext::default().binary_path.as_ref(), Some(&cur));

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let sec_root = SecurityContext::with_root(root);
    assert_eq!(sec_root.binary_path.as_ref(), Some(&cur));

    let mut sys_ctx = SystemContext::with_root(root);
    sys_ctx.no_sys = true;
    let derived = sys_ctx.to_security_context();
    assert_eq!(derived.binary_path.as_ref(), Some(&cur));

    let mut f2b_ctx = derived;
    f2b_ctx.f2b_backend_override = Some("systemd".to_string());
    let runner = MockCommandRunner::new();
    setup_fail2ban(&f2b_ctx, Some(2222), &runner).unwrap();

    let sync_bin = root.join("usr/local/bin/debopty-f2b-sync");
    assert!(
        sync_bin
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink(),
        "setup_fail2ban must preserve Rust multicall symlink instead of overwriting with legacy bash"
    );
    assert_eq!(fs::read_link(&sync_bin).unwrap(), cur);
}

#[test]
fn u14_be_p0_04_and_p1_24_and_p2_08_nftables_scoped_flush_pmtu_and_rollback() {
    use debopty_core::security::fail2ban::setup_security_with_port;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let mut ctx = SecurityContext::with_root(root);
    ctx.no_sys = false;
    ctx.f2b_backend_override = Some("systemd".to_string());

    // Seed existing nftables.conf with container table + custom rule
    let nft_conf = ctx.effective_nft_main_conf();
    fs::create_dir_all(nft_conf.parent().unwrap()).unwrap();
    let orig_nft = "table inet netavark {}\ntable inet filter {\n    chain input {\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n";
    fs::write(&nft_conf, orig_nft).unwrap();

    // 1. If nft -c -f fails in setup_security_with_port, orig nftables.conf is untouched and UFW is NOT purged
    let fail_runner = MockCommandRunner::new();
    fail_runner.on_exact("dpkg", &["-s", "ufw"], ok_cmd("installed"));
    fail_runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "nftables"],
        ok_cmd("install ok installed"),
    );
    fail_runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "fail2ban"],
        ok_cmd("install ok installed"),
    );
    fail_runner.on_prog("nft", err_cmd(1, "syntax error"));
    let err = setup_security_with_port(&ctx, Some(2222), &fail_runner).unwrap_err();
    assert!(matches!(err, CoreError::Security(_)));
    assert_eq!(fs::read_to_string(&nft_conf).unwrap(), orig_nft);
    assert!(
        !fail_runner
            .invocations()
            .iter()
            .any(|(p, _)| p == "ufw" || p == "apt-get"),
        "UFW must NOT be disabled/purged when nft -c -f fails"
    );

    // 2. Successful setup_security_with_port renders table-scoped flush (NO global `flush ruleset`) & `packet-too-big`
    let ok_runner = MockCommandRunner::new();
    ok_runner.on_exact("dpkg", &["-s", "ufw"], ok_cmd("installed"));
    ok_runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "nftables"],
        ok_cmd("install ok installed"),
    );
    ok_runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "fail2ban"],
        ok_cmd("install ok installed"),
    );
    ok_runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "python3-systemd"],
        ok_cmd("install ok installed"),
    );
    ok_runner.on_exact(
        "fail2ban-client",
        &["--version"],
        ok_cmd("Fail2Ban v1.0.2\n"),
    );
    ok_runner.on_prog("ufw", ok_cmd(""));
    ok_runner.on_prog("apt-get", ok_cmd(""));
    ok_runner.on_prog("systemctl", ok_cmd(""));
    ok_runner.on_prog("nft", ok_cmd(""));
    setup_security_with_port(&ctx, Some(2222), &ok_runner).unwrap();

    let rendered = fs::read_to_string(&nft_conf).unwrap();
    assert!(
        !rendered.contains("flush ruleset"),
        "nftables.conf must NEVER contain global flush ruleset"
    );
    assert!(rendered.contains("table inet filter {}"));
    assert!(rendered.contains("flush table inet filter"));
    assert!(
        rendered.contains("packet-too-big"),
        "ICMPv6 allowlist must include packet-too-big for tunnel PMTU discovery"
    );

    // 3. BE-P1-24: add_fw_rule preserves previous valid rule on validation failure & reloads scoped nftables.conf
    let rule_path = add_fw_rule(&ctx, "443", "tcp", "Web_HTTPS", &ok_runner).unwrap();
    let valid_rule_bytes = fs::read_to_string(&rule_path).unwrap();
    let nft_load_calls: Vec<_> = ok_runner
        .invocations()
        .into_iter()
        .filter(|(p, a)| p == "nft" && a.first().map(String::as_str) == Some("-f"))
        .collect();
    assert!(
        nft_load_calls
            .last()
            .is_some_and(|(_, a)| a.get(1).is_some_and(|p| p.ends_with("nftables.conf"))),
        "add_fw_rule must reload table-scoped nftables.conf instead of appending duplicate rules"
    );

    // Updating the same rule when nft -c -f fails must keep valid_rule_bytes intact!
    let bad_nft_runner = MockCommandRunner::new();
    bad_nft_runner.on_prog("nft", err_cmd(1, "invalid rule"));
    let upd_err = add_fw_rule(&ctx, "8443", "tcp", "Web_HTTPS", &bad_nft_runner).unwrap_err();
    assert!(matches!(upd_err, CoreError::Security(_)));
    assert_eq!(
        fs::read_to_string(&rule_path).unwrap(),
        valid_rule_bytes,
        "failed add_fw_rule update must preserve previous working rule file"
    );
}

#[test]
fn u14_be_p0_05_ssh_lockdown_template_kbd_interactive_key_eq_val_and_rollback() {
    use debopty_core::security::ssh::{
        NonInteractivePrompter, check_ssh_security_with_port, lockdown_ssh_system,
    };

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let mut ctx = SecurityContext::with_root(root);
    ctx.no_sys = false;
    ctx.non_interactive = true;
    ctx.f2b_backend_override = Some("systemd".to_string());

    fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
    fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
    let orig_sshd = "Include /etc/ssh/sshd_config.d/*.conf\nPort=22\nPermitRootLogin=yes\nPasswordAuthentication=yes\nKbdInteractiveAuthentication=yes\n";
    let sshd_path = root.join("etc/ssh/sshd_config");
    fs::write(&sshd_path, orig_sshd).unwrap();
    fs::write(
        root.join("etc/passwd"),
        "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
    )
    .unwrap();
    fs::write(root.join("etc/group"), "sudo:x:27:alice\n").unwrap();
    fs::write(
        root.join("etc/nftables.conf"),
        "table inet filter {}\nflush table inet filter\ntable inet filter {\n    chain input {\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n",
    )
    .unwrap();

    // 1. lockdown_ssh_system rolls back sshd_config AND drop-in when systemctl restart ssh fails
    let fail_restart = MockCommandRunner::new();
    fail_restart.on_exact("sshd", &["-t"], ok_cmd(""));
    fail_restart.on_exact(
        "systemctl",
        &["is-active", "--quiet", "ssh.socket"],
        err_cmd(3, "inactive"),
    );
    fail_restart.on_exact(
        "systemctl",
        &["restart", "ssh"],
        err_cmd(1, "failed to start ssh.service"),
    );
    let err = lockdown_ssh_system(&ctx, &fail_restart).unwrap_err();
    assert!(matches!(err, CoreError::Security(_)));
    assert_eq!(fs::read_to_string(&sshd_path).unwrap(), orig_sshd);
    assert!(
        !root
            .join("etc/ssh/sshd_config.d/00-debopty-hardening.conf")
            .exists(),
        "hardening drop-in must be rolled back on ssh restart failure"
    );

    // 2. check_ssh_security_with_port rolls back port + ssh.socket + firewall when passwd -l root fails
    let fail_passwd = MockCommandRunner::new();
    fail_passwd.on_exact(
        "systemctl",
        &["is-active", "--quiet", "ssh.socket"],
        ok_cmd(""),
    );
    fail_passwd.on_prog("chown", ok_cmd(""));
    fail_passwd.on_prog("sshd", ok_cmd(""));
    fail_passwd.on_prog("systemctl", ok_cmd(""));
    fail_passwd.on_prog("nft", ok_cmd(""));
    fail_passwd.on_exact("passwd", &["-l", "root"], err_cmd(1, "passwd failed"));

    let prompter = NonInteractivePrompter {
        username: None,
        password: None,
        ssh_pubkey: Some(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl ops@test"
                .to_string(),
        ),
    };
    let err2 =
        check_ssh_security_with_port(&ctx, &prompter, &fail_passwd, Some(53333)).unwrap_err();
    assert!(matches!(err2, CoreError::Security(_)));
    assert!(
        fs::read_to_string(root.join("etc/nftables.conf"))
            .unwrap()
            .contains(r#"tcp dport 22 accept comment "SSH_Access_Port""#),
        "firewall must roll back to port 22 if lockdown fails"
    );
    assert!(
        !root
            .join("etc/systemd/system/ssh.socket.d/override.conf")
            .exists(),
        "ssh.socket override must roll back if lockdown fails"
    );

    // 3. Successful lockdown replaces Key=Value directives & renders template with KbdInteractiveAuthentication no
    let ok_runner = MockCommandRunner::new();
    ok_runner.on_exact("sshd", &["-t"], ok_cmd(""));
    ok_runner.on_exact(
        "systemctl",
        &["is-active", "--quiet", "ssh.socket"],
        ok_cmd(""),
    );
    ok_runner.on_exact("systemctl", &["restart", "ssh"], err_cmd(1, "socket unit"));
    ok_runner.on_exact("systemctl", &["daemon-reload"], ok_cmd(""));
    ok_runner.on_exact("systemctl", &["restart", "ssh.socket"], ok_cmd(""));
    ok_runner.on_exact("passwd", &["-l", "root"], ok_cmd(""));

    lockdown_ssh_system(&ctx, &ok_runner).unwrap();
    let dropin_txt =
        fs::read_to_string(root.join("etc/ssh/sshd_config.d/00-debopty-hardening.conf")).unwrap();
    assert!(dropin_txt.contains("# managed-by-debopty"));
    assert!(dropin_txt.contains("PermitRootLogin no"));
    assert!(dropin_txt.contains("PasswordAuthentication no"));
    assert!(dropin_txt.contains("KbdInteractiveAuthentication no"));
    assert!(dropin_txt.contains("PubkeyAuthentication yes"));

    let sshd_after = fs::read_to_string(&sshd_path).unwrap();
    assert!(!sshd_after.contains("PermitRootLogin=yes"));
    assert!(!sshd_after.contains("PasswordAuthentication=yes"));
    assert!(!sshd_after.contains("KbdInteractiveAuthentication=yes"));
    assert!(sshd_after.contains("PermitRootLogin no"));
    assert!(sshd_after.contains("PasswordAuthentication no"));
    assert!(sshd_after.contains("KbdInteractiveAuthentication no"));
}

#[test]
fn u14_be_p1_01_diag_full_secret_redaction() {
    let ts_auth_1 = ["tskey", "-auth-", "k123456CNTRL-abcdef123456"].concat();
    let ts_auth_2 = ["tskey", "-auth-", "xyz987"].concat();
    let proxy_url = format!(
        "{}://{}:{}@proxy.example.com:8443/path",
        "https", "alice", "urlPass999"
    );
    let jwt_hdr = ["eyJ", "hbGciOiJIUzI1NiJ9"].concat();
    let raw = format!(
        "CF_DNS_API_TOKEN=cf_live_secret_999 CF_API_KEY=cf_key_888\n\
         ALICLOUD_SECRET_KEY=ali_sec_777 AWS_SECRET_ACCESS_KEY=aws_sec_666\n\
         TS_AUTHKEY={ts_auth_1} WARP_LICENSE_KEY=warp-lic-555\n\
         PRIVATE_KEY=priv_raw_444 private_key = \"quoted_priv_333\"\n\
         X-API-Key: header_api_222 X-Auth-Key: header_auth_111\n\
         Authorization: Basic dXNlcjpwYXNzMTIz\n\
         proxy={proxy_url}\n\
         cmd --token tok_cli_1 --secret sec_cli_2 --api-key ak_cli_3 --authkey {ts_auth_2} --network-secret ns_cli_4 --cf-token cf_cli_5\n\
         echo \"bob:My Spaced Pass 42\" | chpasswd\n\
         bypass=true\n"
    );
    let redacted = redact_sensitive(&raw);
    for leaked in [
        "cf_live_secret_999",
        "cf_key_888",
        "ali_sec_777",
        "aws_sec_666",
        "k123456CNTRL",
        "abcdef123456",
        "warp-lic-555",
        "priv_raw_444",
        "quoted_priv_333",
        "header_api_222",
        "header_auth_111",
        "dXNlcjpwYXNzMTIz",
        "urlPass999",
        "tok_cli_1",
        "sec_cli_2",
        "ak_cli_3",
        "xyz987",
        "ns_cli_4",
        "cf_cli_5",
        "My Spaced Pass 42",
    ] {
        assert!(
            !redacted.contains(leaked),
            "secret '{leaked}' leaked in redacted output:\n{redacted}"
        );
    }
    assert!(redacted.contains("https://alice:[REDACTED]@proxy.example.com:8443/path"));
    assert!(redacted.contains("Authorization: Basic [REDACTED]"));
    assert!(redacted.contains("bypass=true"));

    // Structured fields with sensitive keys must be unconditionally redacted
    let rec = DiagRecord {
        ts: utc_rfc3339_now(),
        level: LogLevel::Warn,
        target: "debopty::test".to_string(),
        code: None,
        msg: "ok".to_string(),
        hint: None,
        fields: BTreeMap::from([
            ("token".to_string(), jwt_hdr),
            ("cf_token".to_string(), "raw_cf_val".to_string()),
            ("network_secret".to_string(), "raw_net_sec".to_string()),
            ("target".to_string(), "/etc/ssh/sshd_config".to_string()),
        ]),
    };
    let json = format_record(&rec, LogFormat::Json);
    let parsed: DiagRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed.fields.get("token").map(String::as_str),
        Some("[REDACTED]")
    );
    assert_eq!(
        parsed.fields.get("cf_token").map(String::as_str),
        Some("[REDACTED]")
    );
    assert_eq!(
        parsed.fields.get("network_secret").map(String::as_str),
        Some("[REDACTED]")
    );
    assert_eq!(
        parsed.fields.get("target").map(String::as_str),
        Some("/etc/ssh/sshd_config")
    );
}

#[test]
fn u14_be_p1_06_p1_17_p1_20_p2_05_p2_07_system_version_sys_f2b_sync() {
    use debopty_core::security::f2b_sync::{F2bSyncOptions, run_f2b_sync};
    use debopty_core::system::run_base_optimization;
    use debopty_core::version::{VersionIdentity, write_version_file};

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // 1. BE-P1-17: write_version_file writes mode 0644
    let ver_path = root.join("opt/debopty/VERSION");
    let id = VersionIdentity {
        semver: Some("1.2.3".to_string()),
        calver: Some("26.11.22".to_string()),
        build: "abcd123".to_string(),
    };
    write_version_file(&ver_path, &id).unwrap();
    assert_eq!(
        fs::metadata(&ver_path).unwrap().permissions().mode() & 0o777,
        0o644
    );

    // 2. BE-P1-06: setup_logrotate preserves { ... } blocks (wtmp/btmp/custom) untouched
    let mut sys_ctx = SystemContext::with_root(root);
    sys_ctx.no_sys = false;
    fs::create_dir_all(root.join("etc/systemd")).unwrap();
    fs::create_dir_all(root.join("etc/apt")).unwrap();
    fs::write(root.join("etc/debian_version"), "12.8\n").unwrap();
    let lr_path = root.join("etc/logrotate.conf");
    fs::write(
        &lr_path,
        "weekly\nrotate 4\n#compress\ninclude /etc/logrotate.d\n\n/var/log/wtmp {\n    missingok\n    monthly\n    create 0664 root utmp\n    rotate 1\n}\n",
    )
    .unwrap();
    setup_logrotate(&sys_ctx).unwrap();
    let lr = fs::read_to_string(&lr_path).unwrap();
    assert!(lr.contains("daily\n"));
    assert!(lr.contains("rotate 7\n"));
    assert!(lr.contains("compress\n"));
    assert!(
        lr.contains("    monthly\n"),
        "wtmp block monthly stripped:\n{lr}"
    );
    assert!(
        lr.contains("    rotate 1\n"),
        "wtmp block rotate 1 stripped:\n{lr}"
    );

    // 3. BE-P1-20: local HTTP APT mirror preserved + no systemd-logind restart + [Login]/[Journal] headers + offline run_base_optimization
    fs::write(
        root.join("etc/apt/sources.list"),
        "deb http://deb.debian.org/debian bookworm main\ndeb http://apt.corp.internal/debian bookworm main\n",
    )
    .unwrap();
    let apt_runner = MockCommandRunner::new();
    apt_runner.on_exact(
        "dpkg-query",
        &["-W", "-f=${Status}", "ca-certificates"],
        ok_cmd("install ok installed"),
    );
    apt_runner.on_prog("dpkg-query", ok_cmd("install ok installed"));
    apt_runner.on_prog("apt-get", ok_cmd(""));
    setup_base(&sys_ctx, &apt_runner).unwrap();
    let sl = fs::read_to_string(root.join("etc/apt/sources.list")).unwrap();
    assert!(sl.contains("https://deb.debian.org/debian"));
    assert!(
        sl.contains("http://apt.corp.internal/debian"),
        "private HTTP mirror must NOT be blindly rewritten to HTTPS:\n{sl}"
    );

    fs::write(root.join("etc/systemd/logind.conf"), "#NAutoVTs=6\n").unwrap();
    fs::write(root.join("etc/systemd/journald.conf"), "#SystemMaxUse=\n").unwrap();
    let lowmem_runner = MockCommandRunner::new();
    lowmem_runner.on_prog("dpkg-query", err_cmd(1, "not installed"));
    lowmem_runner.on_prog("systemctl", ok_cmd(""));
    setup_low_memory_optimization(&sys_ctx, false, &lowmem_runner).unwrap();
    assert!(
        !lowmem_runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "systemctl" && a.iter().any(|x| x == "systemd-logind")),
        "setup_low_memory_optimization must NEVER restart systemd-logind"
    );
    let logind_txt = fs::read_to_string(root.join("etc/systemd/logind.conf")).unwrap();
    assert!(logind_txt.starts_with("[Login]\n"));
    let journald_txt =
        fs::read_to_string(root.join("etc/systemd/journald.conf.d/99-debopty-retention.conf"))
            .unwrap();
    assert!(journald_txt.contains("[Journal]\n"));
    assert!(journald_txt.contains("SystemMaxUse="));

    // Offline run_base_optimization must NOT abort on geoip failure
    let offline_tmp = TempDir::new().unwrap();
    let mut off_ctx = SystemContext::with_root(offline_tmp.path());
    off_ctx.no_sys = true;
    fs::create_dir_all(off_ctx.path("/etc/apt")).unwrap();
    fs::write(off_ctx.path("/etc/debian_version"), "12.8\n").unwrap();
    let offline_http = MockHttpClient::new();
    offline_http.on_get_err("https://ipinfo.io/country", "offline");
    offline_http.on_get_err("http://myip.ipip.net", "offline");
    offline_http.on_get_err("https://cip.cc", "offline");
    offline_http.on_get_err("https://api.ip.sb/geoip", "offline");
    let off_rep =
        run_base_optimization(&off_ctx, &offline_http, &MockCommandRunner::new()).unwrap();
    assert!(off_rep.base_optimized);

    // 4. BE-P2-05: RealCommandRunner returns promptly on normal exit even when background daemon inherits pipes
    let real = RealCommandRunner;
    let t0 = Instant::now();
    let out = real
        .run(
            "sh",
            &["-c", "printf 'hello'; (sleep 15) &"],
            Duration::from_secs(5),
        )
        .unwrap();
    let elapsed = t0.elapsed();
    assert!(out.success());
    assert!(out.stdout.contains("hello"));
    assert!(
        elapsed < Duration::from_secs(4),
        "normal exit hung waiting on daemon-inherited stdout/stderr pipe: {elapsed:?}"
    );

    // 5. BE-P2-07: f2b_sync parses Port=2222, braced SSH port sets, and disables public guard when nft -c -f fails
    let f2b_tmp = TempDir::new().unwrap();
    let f2b_root = f2b_tmp.path();
    fs::create_dir_all(f2b_root.join("etc/ssh")).unwrap();
    fs::write(f2b_root.join("etc/ssh/sshd_config"), "Port=2222\n").unwrap();
    let sec_eq = SecurityContext::with_root(f2b_root);
    assert_eq!(detect_ssh_port(&sec_eq, &MockCommandRunner::new()), 2222);

    fs::write(
        f2b_root.join("etc/nftables.conf"),
        "table inet filter {\n    chain input {\n        tcp dport { 22, 51234 } accept comment \"SSH_Access_Port\"\n    }\n}\n",
    )
    .unwrap();
    let f2b_opts = F2bSyncOptions {
        root_prefix: f2b_root.to_path_buf(),
        no_sys: false,
        force_reload: false,
        print_ports: false,
        backend_override: Some("systemd".to_string()),
        template_dir: None,
        ss_file: None,
    };
    let bad_guard_runner = MockCommandRunner::new();
    bad_guard_runner.on_prog("nft", err_cmd(1, "unsupported meter"));
    let engine = TemplateEngine::new(None);
    let rep = run_f2b_sync(&f2b_opts, &engine, &bad_guard_runner).unwrap();
    assert_eq!(rep.ssh_port, 51234);
    let ports_local =
        fs::read_to_string(f2b_root.join("etc/fail2ban/jail.d/99-debopty-ports.local")).unwrap();
    assert!(
        ports_local.contains("port = 22,51234"),
        "expected multi-port SSH_Access_Port set in jail:\n{ports_local}"
    );
    assert!(
        ports_local.contains("enabled = false"),
        "PUBLIC_GUARD_ENABLED must be forced false when nft -c -f fails:\n{ports_local}"
    );
    assert!(
        !f2b_root
            .join("etc/nftables.d/55-debopty-f2b-guard.nft")
            .exists()
    );
}
