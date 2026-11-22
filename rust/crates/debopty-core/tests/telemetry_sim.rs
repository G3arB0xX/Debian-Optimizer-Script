use std::collections::VecDeque;
use std::fs;
use std::path::Path;

use debopty_core::telemetry::{
    CpuJiffies, HISTORY_CAP, LiveSample, SystemSpecs, TelemetryCollector, TelemetryHistory,
    compute_cpu_pct, format_braille_curve, format_bytes_rate, format_count_compact,
    format_kib_compact, format_uptime_compact, parse_proc_loadavg, parse_proc_meminfo,
    parse_proc_net_dev, parse_proc_stat_cpu, parse_proc_uptime, probe_system_specs,
    probe_system_specs_with_overrides, sanitize_config_excerpt, shorten_cpu_model,
};
use tempfile::TempDir;

fn write_mock(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel.trim_start_matches('/'));
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, content).unwrap();
}

#[test]
fn probe_system_specs_reads_mock_rootfs_and_module_details() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    write_mock(root, "/proc/sys/kernel/hostname", "edge-node-02\n");
    write_mock(root, "/proc/sys/kernel/osrelease", "6.12.9-cloud-amd64\n");
    write_mock(
        root,
        "/etc/os-release",
        "PRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\nNAME=\"Debian GNU/Linux\"\nVERSION_ID=\"12\"\n",
    );
    write_mock(
        root,
        "/proc/cpuinfo",
        "processor\t: 0\nmodel name\t: AMD EPYC 7763 64-Core Processor\n\nprocessor\t: 1\nmodel name\t: AMD EPYC 7763 64-Core Processor\n",
    );
    write_mock(
        root,
        "/proc/meminfo",
        "MemTotal:        4194304 kB\nMemFree:         1000000 kB\nMemAvailable:    2097152 kB\nSwapTotal:       2097152 kB\nSwapFree:        2000000 kB\n",
    );
    write_mock(root, "/proc/sys/net/ipv4/tcp_congestion_control", "bbr\n");
    write_mock(root, "/proc/sys/net/core/default_qdisc", "fq\n");
    write_mock(root, "/proc/sys/net/ipv4/ip_forward", "1\n");
    write_mock(root, "/proc/sys/net/ipv6/conf/all/forwarding", "1\n");
    write_mock(root, "/proc/sys/fs/file-max", "1048576\n");
    write_mock(root, "/proc/sys/vm/swappiness", "10\n");

    write_mock(
        root,
        "/etc/ssh/sshd_config.d/00-debopty-hardening.conf",
        "Port 2222\nPasswordAuthentication no\n",
    );
    write_mock(
        root,
        "/etc/ssh/sshd_config",
        "Port 22\nPasswordAuthentication yes\n",
    );

    write_mock(
        root,
        "/etc/nftables.conf",
        "table inet filter {\n  chain input {\n    tcp dport { 80, 443, 2222 } accept\n  }\n}\n",
    );
    write_mock(
        root,
        "/etc/nftables.d/debopti/tailscale.nft",
        "# tailscale mesh\nudp dport 41641 accept\n",
    );

    write_mock(
        root,
        "/etc/fail2ban/jail.d/99-debopty.local",
        "[DEFAULT]\nbackend = systemd\nbanaction = nftables-multiport\n",
    );
    write_mock(
        root,
        "/etc/systemd/system/debopty-f2b-port-sync.timer",
        "[Timer]\nOnBootSec=2m\n",
    );

    write_mock(
        root,
        "/etc/sysctl.d/99-debopti-optimize.conf",
        "# base tuning\nnet.core.default_qdisc = fq\nnet.ipv4.tcp_congestion_control = bbr\n",
    );
    write_mock(
        root,
        "/etc/sysctl.d/99-debopti-forwarding.conf",
        "net.ipv4.ip_forward = 1\nnet.ipv6.conf.all.forwarding = 1\n",
    );
    write_mock(root, "/usr/bin/ferron", "#!/bin/sh\n");
    write_mock(
        root,
        "/etc/ferron/config.kdl",
        "# ferron edge\nexample.com {\n  proxy \"http://127.0.0.1:8080\"\n}\n",
    );

    let specs = probe_system_specs_with_overrides(root, Some("x86_64"), Some((184, 500)));
    assert_eq!(specs.hostname, "edge-node-02");
    assert_eq!(specs.os_name, "Debian 12");
    assert_eq!(specs.arch, "x86_64");
    assert_eq!(specs.kernel, "6.12.9-cloud-amd64");
    assert_eq!(specs.cpu_model, "EPYC 7763");
    assert_eq!(specs.cpu_cores, 2);
    assert_eq!(specs.mem_total_kib, 4_194_304);
    assert_eq!(specs.swap_total_kib, 2_097_152);
    assert_eq!(specs.disk_root_used_gib_x10, Some(184));
    assert_eq!(specs.disk_root_total_gib_x10, Some(500));
    assert_eq!(specs.tcp_cc, "bbr");
    assert_eq!(specs.default_qdisc, "fq");
    assert!(specs.ipv4_forward);
    assert!(specs.ipv6_forward);
    assert_eq!(specs.file_max, 1_048_576);
    assert_eq!(specs.swappiness, Some(10));
    assert_eq!(specs.ssh_port, 2222);
    assert_eq!(specs.ssh_password_auth, Some(false));
    assert!(specs.nftables_configured);
    assert_eq!(specs.fw_open_ports, vec![80, 443, 2222, 41641]);
    assert!(specs.fail2ban_configured);
    assert_eq!(specs.fail2ban_backend, "systemd");

    let base = specs.module_details.get("base").unwrap();
    assert_eq!(base.service_state, "configured");
    assert!(base.config_exists);
    assert_eq!(base.config_excerpt.len(), 2);

    let f2b = specs.module_details.get("f2b_sync").unwrap();
    assert_eq!(f2b.service_state, "timer");

    let ferron = specs.module_details.get("ferron").unwrap();
    assert_eq!(ferron.service_state, "active");
    assert!(ferron.ports.contains(&"443/tcp".to_string()));

    let defaults = SystemSpecs::default();
    assert_eq!(defaults.ssh_port, 22);
    assert_eq!(defaults.cpu_cores, 1);

    // Non-override probe on root should execute statvfs cleanly without panicking.
    let direct = probe_system_specs(root);
    assert!(direct.disk_root_total_gib_x10.is_some());
}

#[test]
fn os_release_compacts_ubuntu_and_debian_trixie() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    write_mock(
        root,
        "/etc/os-release",
        "PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n",
    );
    assert_eq!(probe_system_specs(root).os_name, "Ubuntu 24.04");

    write_mock(
        root,
        "/etc/os-release",
        "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\n",
    );
    assert_eq!(probe_system_specs(root).os_name, "Debian 13");
}

#[test]
fn sanitize_config_excerpt_strips_comments_truncates_and_redacts_runtime_secrets() {
    let fake_pass = ["synthetic", "pass", "9876"].join("_");
    let fake_tok = ["syn", "tok", "alpha"].join("-");
    let fake_uuid = ["11111111", "2222", "3333", "4444", "555555555555"].join("-");
    let fake_bearer = ["Bea", "rer ", "ephemeral_xyz"].join("");
    let fake_url_cred = ["https://", "demo", ":", "pw_value", "@example.com/hook"].join("");

    let raw = format!(
        "\
# comment line
// cpp style comment
; ini comment

listen_addr = \"0.0.0.0:443\"
password = {fake_pass}
token = {fake_tok}
uuid = {fake_uuid}
secret_key = synthetic_sec
private_key = synthetic_pk
credential = synthetic_cred
authorization = {fake_bearer}
webhook = {fake_url_cred}
very_long_directive = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"
extra_line_should_be_dropped = true
"
    );

    let lines = sanitize_config_excerpt(&raw, 3, 54);
    assert_eq!(lines.len(), 3);
    for line in &lines {
        assert!(line.chars().count() <= 54);
        assert!(!line.contains(&fake_pass));
        assert!(!line.contains(&fake_tok));
        assert!(!line.contains(&fake_uuid));
        assert!(!line.contains("ephemeral_xyz"));
        assert!(!line.contains("pw_value"));
        assert!(!line.contains("synthetic_sec"));
        assert!(!line.contains("synthetic_pk"));
        assert!(!line.contains("synthetic_cred"));
    }
    assert!(lines[0].contains("listen_addr"));
    assert!(lines[1].contains("[REDACTED]"));
    assert!(lines[2].ends_with('…'));
}

#[test]
fn proc_parsers_and_collector_handle_deltas_wraps_zero_dt_and_lo_exclusion() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    write_mock(
        root,
        "/proc/stat",
        "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 100 0 50 800 50 0 0 0\n",
    );
    write_mock(root, "/proc/loadavg", "0.45 0.30 0.25 2/180 4321\n");
    write_mock(
        root,
        "/proc/meminfo",
        "MemTotal: 1000 kB\nMemFree: 200 kB\nBuffers: 100 kB\nCached: 300 kB\nSwapTotal: 500 kB\nSwapFree: 400 kB\n",
    );
    write_mock(
        root,
        "/proc/net/dev",
        "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 999999 10 0 0 0 0 0 0 888888 10 0 0 0 0 0 0
  eth0: 1000 10 0 0 0 0 0 0 2000 10 0 0 0 0 0 0
",
    );
    write_mock(root, "/proc/uptime", "1234.56 4000.00\n");

    let mut collector = TelemetryCollector::new(root);
    let s1 = collector.sample_with_elapsed(2.0);
    assert_eq!(s1.cpu_pct, 0.0);
    assert_eq!(s1.net_rx_bps, 0.0);
    assert_eq!(s1.net_tx_bps, 0.0);
    assert_eq!(s1.mem_used_kib, 400); // 1000 - (200 + 100 + 300) fallback
    assert_eq!(s1.mem_total_kib, 1000);
    assert_eq!(s1.swap_used_kib, 100);
    assert_eq!(s1.swap_total_kib, 500);
    assert!((s1.mem_pct - 40.0).abs() < 0.01);
    assert_eq!(s1.uptime_secs, 1234);
    assert_eq!(s1.procs_running, 2);
    assert_eq!(s1.procs_total, 180);

    // Step 2: CPU busy +150, idle +50 => +200 total => 75.0%; net eth0 +4000 rx, +2000 tx over 2.0s => 2000 / 1000 bps.
    write_mock(root, "/proc/stat", "cpu  200 0 100 850 50 0 0 0 0 0\n");
    write_mock(
        root,
        "/proc/net/dev",
        "    lo: 9999999 1 0 0 0 0 0 0 9999999 1 0 0 0 0 0 0\n  eth0: 5000 20 0 0 0 0 0 0 4000 20 0 0 0 0 0 0\n",
    );
    let s2 = collector.sample_with_elapsed(2.0);
    assert!((s2.cpu_pct - 75.0).abs() < 0.01);
    assert!((s2.net_rx_bps - 2000.0).abs() < 0.01);
    assert!((s2.net_tx_bps - 1000.0).abs() < 0.01);

    // Step 3: zero dt must produce 0 network rates safely.
    write_mock(
        root,
        "/proc/net/dev",
        "  eth0: 9000 30 0 0 0 0 0 0 8000 30 0 0 0 0 0 0\n",
    );
    let s3 = collector.sample_with_elapsed(0.0);
    assert_eq!(s3.net_rx_bps, 0.0);
    assert_eq!(s3.net_tx_bps, 0.0);

    // Step 4: counter wrap must not underflow or yield negative/huge rates.
    write_mock(root, "/proc/stat", "cpu  10 0 5 100 0 0 0 0 0 0\n");
    write_mock(
        root,
        "/proc/net/dev",
        "  eth0: 100 1 0 0 0 0 0 0 50 1 0 0 0 0 0 0\n",
    );
    let s4 = collector.sample_with_elapsed(2.0);
    assert_eq!(s4.cpu_pct, 0.0);
    assert_eq!(s4.net_rx_bps, 0.0);
    assert_eq!(s4.net_tx_bps, 0.0);

    // Direct parser spot checks
    assert!(parse_proc_stat_cpu("cpu 1 2 3").is_none());
    assert_eq!(
        compute_cpu_pct(
            CpuJiffies { busy: 50, total: 100 },
            CpuJiffies { busy: 50, total: 100 }
        ),
        0.0
    );
    assert_eq!(parse_proc_loadavg("bad"), (0.0, 0.0, 0.0, 0, 0));
    assert_eq!(parse_proc_meminfo(""), (0, 0, 0, 0));
    assert_eq!(parse_proc_net_dev("  lo: 500 0 0 0 0 0 0 0 500").rx_bytes, 0);
    assert_eq!(parse_proc_uptime("99.9 10.0"), 99);
}

#[test]
fn braille_curve_and_compact_formatters_work_accurately() {
    let empty = VecDeque::new();
    let blank = format_braille_curve(&empty, 6, 0.0, 100.0);
    assert_eq!(blank.chars().count(), 6);
    assert!(blank.chars().all(|c| c == '\u{2800}'));

    let mut q = VecDeque::from(vec![0.0, 100.0, 33.0, 66.0]);
    let curve = format_braille_curve(&q, 4, 0.0, 100.0);
    assert_eq!(curve.chars().count(), 4);
    let chars: Vec<char> = curve.chars().collect();
    // First 2 cells are padded (absent samples -> U+2800).
    assert_eq!(chars[0], '\u{2800}');
    assert_eq!(chars[1], '\u{2800}');
    // Cell 2 has left=0.0 (floor dot 0x40) and right=100.0 (all right dots 0xB8) => 0xF8.
    assert_eq!(chars[2] as u32, 0x2800 + 0x40 + 0x80 + 0x20 + 0x10 + 0x08);
    assert!(chars.iter().all(|&c| ('\u{2800}'..='\u{28FF}').contains(&c)));

    q.push_back(50.0);
    assert_eq!(format_braille_curve(&q, 2, 0.0, 100.0).chars().count(), 2);

    assert_eq!(format_bytes_rate(0.0), "0 B/s");
    assert_eq!(format_bytes_rate(512.0), "512 B/s");
    assert_eq!(format_bytes_rate(2048.0), "2.0 KB/s");
    assert_eq!(format_bytes_rate(3.0 * 1024.0 * 1024.0), "3.0 MB/s");

    assert_eq!(format_kib_compact(0), "0K");
    assert_eq!(format_kib_compact(512 * 1024), "512M");
    assert_eq!(format_kib_compact(4 * 1024 * 1024), "4.0G");

    assert_eq!(format_uptime_compact(45), "45s");
    assert_eq!(format_uptime_compact(1200), "20m");
    assert_eq!(format_uptime_compact(3660), "1h1m");
    assert_eq!(format_uptime_compact(2 * 86_400 + 3 * 3600), "2d3h");

    assert_eq!(format_count_compact(512), "512");
    assert_eq!(format_count_compact(1500), "1.5K");
    assert_eq!(format_count_compact(1_048_576), "1.0M");

    assert_eq!(
        shorten_cpu_model("AMD EPYC 7763 64-Core Processor"),
        "EPYC 7763"
    );
    assert_eq!(
        shorten_cpu_model("Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz"),
        "Xeon E5-2680 v4"
    );
}

#[test]
fn demo_fixture_is_deterministic_phase_shifted_and_bounded() {
    let (specs_a, hist_a) = TelemetryHistory::demo_fixture(0);
    let (specs_b, hist_b) = TelemetryHistory::demo_fixture(0);
    let (_specs_c, hist_c) = TelemetryHistory::demo_fixture(3);

    assert_eq!(specs_a, specs_b);
    assert_eq!(hist_a, hist_b);
    assert_eq!(hist_a.visual_fingerprint(), hist_b.visual_fingerprint());
    assert_ne!(hist_a.visual_fingerprint(), hist_c.visual_fingerprint());

    assert_eq!(hist_a.cpu_pct.len(), 40);
    assert_eq!(specs_a.hostname, "deb-edge-01");
    assert_eq!(specs_a.ssh_port, 2222);
    assert_eq!(specs_a.ssh_password_auth, Some(false));
    assert_eq!(specs_a.fw_open_ports, vec![80, 443, 2222, 41641]);
    for key in [
        "base",
        "ip_forward",
        "podman",
        "xray",
        "ferron",
        "tailscale",
        "devops",
        "reconcile",
        "f2b_sync",
    ] {
        assert!(specs_a.module_details.contains_key(key), "missing {key}");
    }

    let mut bounded = TelemetryHistory::default();
    for i in 0..(HISTORY_CAP + 12) {
        bounded.push_sample(LiveSample {
            cpu_pct: i as f64,
            ..LiveSample::default()
        });
    }
    assert_eq!(bounded.cpu_pct.len(), HISTORY_CAP);
    assert_eq!(bounded.cpu_pct.front().copied(), Some(12.0));
}
