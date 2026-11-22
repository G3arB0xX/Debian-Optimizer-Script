use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use debopty_core::error::CoreError;
use debopty_core::reconcile::{
    BackupEntry, BackupManifest, FindingStatus, ReconcileContext, ReconcileDomain, ReconcileMode,
    audit_system, reconcile_system, restore_backup,
};
use debopty_core::sys::{CmdOutput, MockCommandRunner};
use tempfile::TempDir;
use walkdir::WalkDir;

fn ok_out(stdout: &str) -> CmdOutput {
    CmdOutput {
        code: 0,
        stdout: stdout.to_string(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn err_out(stderr: &str) -> CmdOutput {
    CmdOutput {
        code: 1,
        stdout: String::new(),
        stderr: stderr.to_string(),
        timed_out: false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TreeEntry {
    Dir(u32),
    File { mode: u32, bytes: Vec<u8> },
    Symlink(String),
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, TreeEntry> {
    let mut map = BTreeMap::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let key = rel.to_string_lossy().into_owned();
        let meta = fs::symlink_metadata(entry.path()).unwrap();
        if meta.file_type().is_symlink() {
            let target = fs::read_link(entry.path())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            map.insert(key, TreeEntry::Symlink(target));
        } else if meta.is_dir() {
            map.insert(key, TreeEntry::Dir(meta.permissions().mode() & 0o777));
        } else {
            map.insert(
                key,
                TreeEntry::File {
                    mode: meta.permissions().mode() & 0o777,
                    bytes: fs::read(entry.path()).unwrap(),
                },
            );
        }
    }
    map
}

// =========================================================================
// [U11] RW-SSH-01: cloud-init + ssh.socket + invalid drop-in + F2b ignoreip
// =========================================================================

#[test]
fn rw_ssh_01_cloud_init_ssh_socket_invalid_dropin_and_f2b_ignoreip() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::create_dir_all(root.join("etc/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
    fs::create_dir_all(root.join("etc/systemd/system/ssh.socket.d")).unwrap();
    fs::create_dir_all(root.join("etc/fail2ban/jail.d")).unwrap();
    fs::create_dir_all(root.join("run/systemd/system")).unwrap();

    fs::write(
        root.join("etc/debopti/debopti.conf"),
        "SSH_HARDENED=\"true\"\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config"),
        "Port 22\nAllowUsers ops\nMatch User sftp\n    ForceCommand internal-sftp\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config.d/50-cloud-init.conf"),
        "PasswordAuthentication yes\nPermitRootLogin yes\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config.d/90-invalid-dropin.conf"),
        "PermitRootLogin \"unclosed\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config.d/60-corp-policy.conf"),
        "ClientAliveInterval 60\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/systemd/system/ssh.socket.d/override.conf"),
        "[Socket]\nListenStream=\nListenStream=52222\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/fail2ban/jail.local"),
        "[DEFAULT]\nignoreip = 198.51.100.7 10.10.0.0/16\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/fail2ban/jail.d/99-debopty.local"),
        "[DEFAULT]\nignoreip = 127.0.0.1/8 ::1\nbackend = auto\n[sshd]\nlogpath = /var/log/auth.log\n",
    )
    .unwrap();

    let mut ctx = ReconcileContext::with_root(root);
    ctx.domains = vec![ReconcileDomain::Ssh, ReconcileDomain::Firewall];
    ctx.f2b_backend_override = Some("systemd".to_string());

    // Live validation runner: sshd -t succeeds, dpkg/nft/fail2ban clean
    let runner = MockCommandRunner::new();
    runner.on_exact("sshd", &["-t"], ok_out(""));
    runner.on_exact("nft", &["list", "tables"], ok_out(""));
    runner.on_exact("dpkg", &["-s", "ufw"], err_out("not installed"));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("fail2ban-client", ok_out(""));

    let report = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
    assert!(report.changes_made >= 4);
    assert!(
        runner
            .invocations()
            .iter()
            .any(|(p, a)| p == "sshd" && a == &vec!["-t".to_string()]),
        "sshd -t must be executed on live Apply"
    );

    // Invalid drop-in quarantined to .disabled; custom policy & Match preserved
    assert!(
        !root
            .join("etc/ssh/sshd_config.d/90-invalid-dropin.conf")
            .exists()
    );
    assert!(
        root.join("etc/ssh/sshd_config.d/90-invalid-dropin.conf.disabled")
            .is_file()
    );
    assert!(
        root.join("etc/ssh/sshd_config.d/60-corp-policy.conf")
            .is_file()
    );

    let hardening =
        fs::read_to_string(root.join("etc/ssh/sshd_config.d/00-debopty-hardening.conf")).unwrap();
    assert!(hardening.contains("# managed-by-debopty"));
    assert!(hardening.contains("PasswordAuthentication no"));

    // Debian 11 / OpenSSH < 8.7 reconcile emits ChallengeResponseAuthentication (never KbdInteractiveAuthentication)
    {
        let tmp_d11 = TempDir::new().unwrap();
        let r11 = tmp_d11.path();
        fs::create_dir_all(r11.join("etc/ssh/sshd_config.d")).unwrap();
        fs::create_dir_all(r11.join("etc/debopti")).unwrap();
        fs::write(
            r11.join("etc/debopti/debopti.conf"),
            "SSH_HARDENED=\"true\"\n",
        )
        .unwrap();
        fs::write(
            r11.join("etc/ssh/sshd_config"),
            "Include /etc/ssh/sshd_config.d/*.conf\nPort 22\n",
        )
        .unwrap();

        let mut ctx11 = ReconcileContext::with_root(r11);
        ctx11.domains = vec![ReconcileDomain::Ssh];
        let run11 = MockCommandRunner::new();
        run11.on_exact(
            "sshd",
            &["-V"],
            CmdOutput {
                code: 1,
                stdout: String::new(),
                stderr: "unknown option -- V\nOpenSSH_8.4p1 Debian-5+deb11u3\n".to_string(),
                timed_out: false,
            },
        );
        run11.on_exact("sshd", &["-t"], ok_out(""));

        reconcile_system(&ctx11, ReconcileMode::Apply, &run11).unwrap();
        let dropin11 =
            fs::read_to_string(r11.join("etc/ssh/sshd_config.d/00-debopty-hardening.conf"))
                .unwrap();
        assert!(dropin11.contains("ChallengeResponseAuthentication no"));
        assert!(!dropin11.contains("KbdInteractiveAuthentication"));
    }

    let sshd_after = fs::read_to_string(root.join("etc/ssh/sshd_config")).unwrap();
    assert!(sshd_after.contains("Include /etc/ssh/sshd_config.d/*.conf"));
    assert!(sshd_after.contains("Port 52222"));
    assert!(sshd_after.contains("AllowUsers ops"));
    assert!(sshd_after.contains("Match User sftp"));

    // Fail2ban preserves operator ignoreip & switches to systemd backend
    let f2b = fs::read_to_string(root.join("etc/fail2ban/jail.d/99-debopty.local")).unwrap();
    assert!(f2b.contains("198.51.100.7"));
    assert!(f2b.contains("10.10.0.0/16"));
    assert!(f2b.contains("backend = systemd"));
    let f2b_ports =
        fs::read_to_string(root.join("etc/fail2ban/jail.d/99-debopty-ports.local")).unwrap();
    assert!(f2b_ports.contains("port = 52222"));
}

// =========================================================================
// [U11] RW-FW-01: Preserve netavark & Docker tables — NO global flush ruleset
// =========================================================================

#[test]
fn rw_fw_01_preserves_netavark_and_docker_tables_no_global_flush() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/ufw")).unwrap();
    fs::create_dir_all(root.join("etc/fail2ban/jail.d")).unwrap();

    fs::write(
        root.join("etc/nftables.conf"),
        "#!/usr/sbin/nft -f\nflush ruleset\ntable inet netavark {\n    chain postrouting {}\n}\ntable inet filter {\n    chain input {\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/nftables.d/80-corp.nft"),
        "table inet corp_custom {\n    chain input {}\n}\n",
    )
    .unwrap();
    fs::write(root.join("etc/ufw/ufw.conf"), "ENABLED=yes\n").unwrap();
    fs::write(
        root.join("etc/ufw/user.rules"),
        "-A ufw-user-input -p tcp --dport 8080 -j ACCEPT\n-A ufw-user-input -p udp --dport 51820 -j ACCEPT\n-A ufw-user-input -p tcp --dport 10000:10010 -j ACCEPT\n",
    )
    .unwrap();

    let mut ctx = ReconcileContext::with_root(root);
    ctx.domains = vec![ReconcileDomain::Firewall];
    ctx.f2b_backend_override = Some("systemd".to_string());

    let runner = MockCommandRunner::new();
    runner.on_exact(
        "nft",
        &["list", "tables"],
        ok_out("table inet netavark\ntable ip nat\ntable ip filter\n"),
    );
    runner.on_exact("dpkg", &["-s", "ufw"], err_out("not installed"));
    runner.on_prog("ufw", ok_out(""));
    runner.on_prog("nft", ok_out(""));
    runner.on_prog("systemctl", ok_out(""));
    runner.on_prog("fail2ban-client", ok_out(""));

    let report = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "FW_CONTAINER_TABLES_PRESERVED"
                && f.status == FindingStatus::PreservedUserCustom
                && f.summary.contains("ip nat")
                && f.summary.contains("inet netavark"))
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "FW_GLOBAL_FLUSH_SCOPED" && f.status == FindingStatus::Fixed)
    );

    let nft_after = fs::read_to_string(root.join("etc/nftables.conf")).unwrap();
    assert!(!nft_after.contains("flush ruleset"));
    assert!(nft_after.contains("flush table inet filter"));
    assert!(nft_after.contains("table inet netavark"));
    assert!(root.join("etc/nftables.d/80-corp.nft").is_file());

    let ufw_mig =
        fs::read_to_string(root.join("etc/nftables.d/debopti/90-ufw-migrated-ports.nft")).unwrap();
    assert!(ufw_mig.contains("8080"));
    assert!(ufw_mig.contains("10000 - 10010"));
    assert!(ufw_mig.contains("51820"));
    assert!(
        fs::read_to_string(root.join("etc/ufw/ufw.conf"))
            .unwrap()
            .contains("ENABLED=no")
    );
}

// =========================================================================
// [U11] RW-MEM-01: dpkg active/stale locks + LXC zram/sysctl + Buster/DEB822
// =========================================================================

#[test]
fn rw_mem_01_dpkg_lock_lxc_zram_and_deb822_third_party_preserved() {
    // Part A: Active dpkg lock (fuser == 0) + LXC container RO sysctl & missing zram + DEB822 + 3rd party repos
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("run")).unwrap();
        fs::write(root.join("run/.containerenv"), "").unwrap();
        fs::create_dir_all(root.join("var/lib/dpkg")).unwrap();
        let lock_file = root.join("var/lib/dpkg/lock-frontend");
        fs::write(&lock_file, "").unwrap();

        fs::create_dir_all(root.join("etc/debopti")).unwrap();
        fs::write(
            root.join("etc/debopti/debopti.conf"),
            "BASE_OPTIMIZED=\"true\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("etc/sysctl.d")).unwrap();
        fs::create_dir_all(root.join("etc/default")).unwrap();
        fs::write(root.join("etc/default/zramswap"), "ALGO=\nPERCENT=500\n").unwrap();

        fs::create_dir_all(root.join("etc/apt/sources.list.d")).unwrap();
        fs::write(root.join("etc/debian_version"), "12.8\n").unwrap();
        fs::write(
            root.join("etc/apt/sources.list"),
            "deb https://deb.debian.org/debian bookworm main\ndeb https://packages.corp.example/debian bookworm main\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/apt/sources.list.d/debian.sources"),
            "Types: deb\nURIs: https://deb.debian.org/debian\nSuites: bookworm\nComponents: main\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/apt/sources.list.d/ferron.list"),
            "deb https://pkg.ferron.sh/debian bookworm main\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/apt/sources.list.d/tailscale.sources"),
            "Types: deb\nURIs: https://pkgs.tailscale.com/stable/debian\nSuites: bookworm\nComponents: main\n",
        )
        .unwrap();

        let mut ctx = ReconcileContext::with_root(root);
        ctx.domains = vec![
            ReconcileDomain::Sysctl,
            ReconcileDomain::Swap,
            ReconcileDomain::Apt,
        ];

        let runner = MockCommandRunner::new();
        runner.on_exact(
            "sysctl",
            &["--system"],
            err_out("sysctl: setting key: Read-only file system"),
        );
        runner.on_exact(
            "modprobe",
            &["-n", "zram"],
            err_out("Module zram not found"),
        );
        runner.on_prog("fuser", ok_out("12345\n")); // Active holder!

        let report = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();

        // Active dpkg lock NEVER deleted and reported as SkippedNoSys
        assert!(lock_file.exists(), "active dpkg lock must NEVER be deleted");
        assert!(report.findings.iter().any(|f| {
            f.code == "APT_DPKG_LOCK_ACTIVE" && f.status == FindingStatus::SkippedNoSys
        }));

        // Container sysctl & zram degrade cleanly to SkippedNoSys
        assert!(report.findings.iter().any(|f| {
            f.code == "SYSCTL_CONTAINER_RESTRICTED" && f.status == FindingStatus::SkippedNoSys
        }));
        assert!(report.findings.iter().any(|f| {
            f.code == "SWAP_ZRAM_UNAVAILABLE_IN_CONTAINER"
                && f.status == FindingStatus::SkippedNoSys
        }));

        // Overlap shadowed; 3rd-party repos in both sources.list and sources.list.d preserved
        let sl = fs::read_to_string(root.join("etc/apt/sources.list")).unwrap();
        assert!(sl.contains("shadowed-by-debian.sources"));
        assert!(sl.contains("https://packages.corp.example/debian"));
        assert!(root.join("etc/apt/sources.list.d/ferron.list").is_file());
        assert!(
            root.join("etc/apt/sources.list.d/tailscale.sources")
                .is_file()
        );
    }

    // Part B: Stale dpkg lock (fuser == 1) + broken dpkg --audit triggers repair; Debian 10 Buster EOL repaired
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("var/lib/dpkg")).unwrap();
        fs::write(root.join("var/lib/dpkg/lock"), "").unwrap();
        fs::create_dir_all(root.join("etc/apt/sources.list.d")).unwrap();
        fs::write(root.join("etc/debian_version"), "10.13\n").unwrap();
        fs::write(
            root.join("etc/apt/sources.list"),
            "deb http://deb.debian.org/debian buster main\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/apt/sources.list.d/custom.list"),
            "deb https://custom.example/buster buster main\n",
        )
        .unwrap();

        let mut ctx = ReconcileContext::with_root(root);
        ctx.domains = vec![ReconcileDomain::Apt];

        let runner = MockCommandRunner::new();
        runner.on_prog("fuser", err_out("")); // No live process holds lock
        runner.on_exact(
            "dpkg",
            &["--audit"],
            ok_out("The following packages are only half configured:\n curl\n"),
        );
        runner.on_exact("dpkg", &["--configure", "-a"], ok_out(""));
        runner.on_exact("apt-get", &["-f", "install", "-yq"], ok_out(""));

        let report = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
        assert!(
            report.findings.iter().any(|f| {
                f.code == "APT_BUSTER_EOL_REPAIRED" && f.status == FindingStatus::Fixed
            })
        );
        assert!(
            report.findings.iter().any(|f| {
                f.code == "APT_DPKG_BROKEN_REPAIRED" && f.status == FindingStatus::Fixed
            })
        );
        assert!(
            fs::read_to_string(root.join("etc/apt/sources.list"))
                .unwrap()
                .contains("archive.debian.org")
        );
        assert!(root.join("etc/apt/sources.list.d/custom.list").is_file());
    }
}

// =========================================================================
// Kitchen-Sink 9-Domain Chaos: Audit Zero-Mutation, U8 Preservation,
// Idempotency, Atomic Restore & Path-Traversal Rejection
// =========================================================================

#[test]
fn kitchen_sink_nine_domain_chaos_audit_zero_mutation_apply_idempotent_and_restore() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // 0. Users + Debian 12 + corrupt BOM/NUL/CRLF/unclosed-quote/dup debopti.conf with USER_CUSTOM_VAR
    fs::create_dir_all(root.join("etc/debopti")).unwrap();
    fs::create_dir_all(root.join("home/ops/.ssh")).unwrap();
    fs::write(
        root.join("etc/passwd"),
        "root:x:0:0:root:/root:/bin/bash\nops:x:1000:1000:Ops:/home/ops:/bin/bash\n",
    )
    .unwrap();
    fs::write(root.join("etc/debian_version"), "12.8\n").unwrap();
    let corrupt_conf = b"\xEF\xBB\xBF# corp header comment\r\nSSH_HARDENED=\"true\"\r\nBASE_OPTIMIZED=\"true\"\r\nBASE_OPTIMIZED=\"true\"\r\nUSER_CUSTOM_VAR=\"preserve-me\r\n\0";
    let conf_path = root.join("etc/debopti/debopti.conf");
    fs::write(&conf_path, corrupt_conf).unwrap();
    fs::set_permissions(&conf_path, fs::Permissions::from_mode(0o600)).unwrap();

    // 1. SSH: 2 user keys with options + dup + CRLF + Match User sftp + cloud-init + broken drop-in + custom drop-in
    fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
    fs::create_dir_all(root.join("etc/systemd/system/ssh.socket.d")).unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config"),
        "Port 22\nAllowUsers ops\nMatch User sftp\n    ChrootDirectory /srv/sftp\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config.d/50-cloud-init.conf"),
        "PasswordAuthentication yes\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config.d/95-corrupt.conf"),
        "OnlyOneKeyword\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/ssh/sshd_config.d/60-user.conf"),
        "MaxAuthTries 4\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/systemd/system/ssh.socket.d/override.conf"),
        "[Socket]\nListenStream=\nListenStream=49222\n",
    )
    .unwrap();
    let auth_keys = root.join("home/ops/.ssh/authorized_keys");
    fs::write(
        &auth_keys,
        "from=\"10.0.0.0/8\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKeyAlpha111111111111111111111111111111111111 ops@bastion\r\nfrom=\"10.0.0.0/8\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKeyAlpha111111111111111111111111111111111111 dup\r\ncommand=\"/usr/local/bin/backup\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKeyBeta2222222222222222222222222222222222222 ops@backup\r\n",
    )
    .unwrap();
    fs::set_permissions(
        root.join("home/ops/.ssh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::set_permissions(&auth_keys, fs::Permissions::from_mode(0o644)).unwrap();

    // 2. Firewall + Fail2ban: netavark + global flush + custom .nft + custom jail.local + custom jail.d
    fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/fail2ban/jail.d")).unwrap();
    fs::create_dir_all(root.join("run/systemd/system")).unwrap();
    fs::write(
        root.join("etc/nftables.conf"),
        "#!/usr/sbin/nft -f\nflush ruleset\ntable inet netavark {}\ntable inet filter {\n    chain input {\n        ip6 nexthdr icmpv6 icmpv6 type echo-request limit rate 5/second accept\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/nftables.d/75-custom.nft"),
        "table inet my_custom_fw {}\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/fail2ban/jail.local"),
        "[DEFAULT]\nignoreip = 203.0.113.55\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/fail2ban/jail.d/50-nginx.local"),
        "[nginx-http-auth]\nenabled = true\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/fail2ban/jail.d/99-debopty.local"),
        "[DEFAULT]\nignoreip = 127.0.0.1/8 ::1\nbackend = auto\n",
    )
    .unwrap();

    // 3. DNS: dangling symlink
    symlink("/nonexistent/resolved.conf", root.join("etc/resolv.conf")).unwrap();

    // 4. Sysctl: conflicting BBR key + user key + 0600
    fs::create_dir_all(root.join("etc/sysctl.d")).unwrap();
    let user_sysctl = root.join("etc/sysctl.d/80-user.conf");
    fs::write(
        &user_sysctl,
        "net.ipv4.tcp_congestion_control = cubic\nvm.swappiness = 10\n",
    )
    .unwrap();
    fs::set_permissions(&user_sysctl, fs::Permissions::from_mode(0o600)).unwrap();

    // 5. Limits: broken user.conf + custom limits.d/50-postgres.conf
    fs::create_dir_all(root.join("etc/systemd")).unwrap();
    fs::create_dir_all(root.join("etc/security/limits.d")).unwrap();
    fs::write(
        root.join("etc/systemd/user.conf"),
        "DefaultTimeoutStartSec=30s\nDefaultLimitNOFILE=65536\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/security/limits.d/50-postgres.conf"),
        "postgres soft nofile 131072\n",
    )
    .unwrap();

    // 6. Swap: CRLF + missing /swapfile + /data & NFS mounts + broken zramswap
    fs::write(
        root.join("etc/fstab"),
        "UUID=abcd-1234 /data ext4 defaults 0 2\r\nnfs.internal:/export /mnt/nfs nfs4 defaults 0 0\r\n/swapfile none swap sw 0 0\r\n/swapfile none swap sw 0 0\r\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("etc/default")).unwrap();
    fs::write(
        root.join("etc/default/zramswap"),
        "ALGO=\r\nPERCENT=999\r\n",
    )
    .unwrap();

    // 7. Logrotate: conflicting weekly + misplaced daily + dateext + myapp dropin
    fs::create_dir_all(root.join("etc/logrotate.d")).unwrap();
    fs::write(
        root.join("etc/logrotate.conf"),
        "weekly\ndateext\ninclude /etc/logrotate.d\ndaily\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/logrotate.d/myapp"),
        "/var/log/myapp.log { rotate 14 }\n",
    )
    .unwrap();

    // 8. APT: duplicate + deb822 overlap + ferron.list + tailscale.sources
    fs::create_dir_all(root.join("etc/apt/sources.list.d")).unwrap();
    fs::write(
        root.join("etc/apt/sources.list"),
        "deb https://deb.debian.org/debian bookworm main\ndeb https://deb.debian.org/debian bookworm main\ndeb https://repo.corp.local/apt bookworm main\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/apt/sources.list.d/debian.sources"),
        "Types: deb\nURIs: https://deb.debian.org/debian\nSuites: bookworm\nComponents: main\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/apt/sources.list.d/ferron.list"),
        "deb https://pkg.ferron.sh/debian bookworm main\n",
    )
    .unwrap();
    fs::write(
        root.join("etc/apt/sources.list.d/tailscale.sources"),
        "Types: deb\nURIs: https://pkgs.tailscale.com/stable/debian\nSuites: bookworm\nComponents: main\n",
    )
    .unwrap();

    // 9. Legacy /opt/debopti + shortcuts + custom profile.d
    fs::create_dir_all(root.join("opt/debopti")).unwrap();
    fs::write(root.join("opt/debopti/legacy.txt"), "old-data").unwrap();
    fs::create_dir_all(root.join("usr/local/bin")).unwrap();
    fs::write(root.join("usr/local/bin/debopti"), "#!/bin/sh\n").unwrap();
    fs::create_dir_all(root.join("etc/profile.d")).unwrap();
    fs::write(root.join("etc/profile.d/debopti.sh"), "alias debopti=old\n").unwrap();
    fs::write(root.join("etc/profile.d/99-corp.sh"), "export CORP=1\n").unwrap();

    let mut ctx = ReconcileContext::with_root(root);
    ctx.no_sys = true;
    ctx.f2b_backend_override = Some("systemd".to_string());
    let runner = MockCommandRunner::new();

    // 1. Audit mutates ZERO files (exact pre/post tree snapshot equality)
    let before_audit = snapshot_tree(root);
    let audit1 = audit_system(&ctx, &runner).unwrap();
    let after_audit = snapshot_tree(root);
    assert_eq!(before_audit, after_audit, "Audit MUST mutate zero files");
    assert_eq!(audit1.changes_made, 0);
    assert!(audit1.backup_dir.is_none());
    assert!(audit1.drift_count >= 15);
    assert!(audit1.preserved_count >= 12);

    // 2. Apply fixes all 9 domains & preserves 100% of U8 user customizations
    let apply1 = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
    assert!(apply1.changes_made >= 15);
    assert!(apply1.preserved_count >= 12);
    let backup_dir = apply1.backup_dir.clone().expect("backup dir created");
    assert!(backup_dir.join("manifest.json").is_file());

    // U8 preservation assertions
    let auth_after = fs::read_to_string(&auth_keys).unwrap();
    assert_eq!(auth_after.lines().count(), 2);
    assert!(auth_after.contains("from=\"10.0.0.0/8\""));
    assert!(auth_after.contains("command=\"/usr/local/bin/backup\""));
    assert_eq!(
        fs::metadata(&auth_keys).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(root.join("home/ops/.ssh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(
        fs::read_to_string(root.join("etc/ssh/sshd_config"))
            .unwrap()
            .contains("Match User sftp")
    );
    assert!(root.join("etc/ssh/sshd_config.d/60-user.conf").is_file());
    assert!(root.join("etc/nftables.d/75-custom.nft").is_file());
    assert!(root.join("etc/fail2ban/jail.local").is_file());
    assert!(root.join("etc/fail2ban/jail.d/50-nginx.local").is_file());
    assert!(
        fs::read_to_string(root.join("etc/fail2ban/jail.d/99-debopty.local"))
            .unwrap()
            .contains("203.0.113.55")
    );
    assert!(
        fs::read_to_string(&user_sysctl)
            .unwrap()
            .contains("vm.swappiness = 10")
    );
    assert_eq!(
        fs::metadata(&user_sysctl).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert!(
        fs::read_to_string(root.join("etc/systemd/user.conf"))
            .unwrap()
            .contains("DefaultTimeoutStartSec=30s")
    );
    assert!(
        root.join("etc/security/limits.d/50-postgres.conf")
            .is_file()
    );
    let fstab_after = fs::read_to_string(root.join("etc/fstab")).unwrap();
    assert!(fstab_after.contains("/data"));
    assert!(fstab_after.contains("/mnt/nfs"));
    assert!(!fstab_after.contains('\r'));
    assert!(
        fs::read_to_string(root.join("etc/logrotate.conf"))
            .unwrap()
            .contains("dateext")
    );
    assert!(root.join("etc/logrotate.d/myapp").is_file());
    assert!(
        fs::read_to_string(root.join("etc/apt/sources.list"))
            .unwrap()
            .contains("https://repo.corp.local/apt")
    );
    assert!(root.join("etc/apt/sources.list.d/ferron.list").is_file());
    assert!(
        root.join("etc/apt/sources.list.d/tailscale.sources")
            .is_file()
    );
    assert!(root.join("etc/profile.d/99-corp.sh").is_file());
    let conf_after = fs::read_to_string(&conf_path).unwrap();
    assert!(conf_after.contains("# corp header comment"));
    assert!(conf_after.contains("USER_CUSTOM_VAR=\"preserve-me\""));

    // 3. Idempotency: 2nd Apply => 0 changes & no new backup dir; Audit => 0 drift
    let apply2 = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
    assert_eq!(apply2.changes_made, 0, "2nd apply must be idempotent");
    assert_eq!(apply2.drift_count, 0);
    assert!(apply2.backup_dir.is_none());

    let audit2 = audit_system(&ctx, &runner).unwrap();
    assert_eq!(audit2.drift_count, 0);

    // 4. Restore restores original bytes, modes, and dangling symlink
    let restored = restore_backup(&backup_dir, root).unwrap();
    assert!(restored >= 15);
    assert_eq!(fs::read(&conf_path).unwrap(), corrupt_conf);
    assert_eq!(
        fs::metadata(&conf_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        fs::symlink_metadata(root.join("etc/resolv.conf"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(root.join("etc/resolv.conf")).unwrap(),
        Path::new("/nonexistent/resolved.conf")
    );
    assert!(root.join("opt/debopti/legacy.txt").is_file());
    assert!(root.join("etc/ssh/sshd_config.d/95-corrupt.conf").is_file());
    assert!(
        !root
            .join("etc/ssh/sshd_config.d/95-corrupt.conf.disabled")
            .exists()
    );
}

#[test]
fn u8_working_corporate_dns_and_healthy_symlink_preserved_untouched() {
    // Working corporate regular resolv.conf
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc")).unwrap();
        let orig = "search corp.example\nnameserver 10.53.0.1\n";
        fs::write(root.join("etc/resolv.conf"), orig).unwrap();

        let mut ctx = ReconcileContext::with_root(root);
        ctx.domains = vec![ReconcileDomain::Dns];
        ctx.no_sys = true;
        let runner = MockCommandRunner::new();

        let rep = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
        assert_eq!(rep.changes_made, 0);
        assert!(rep.backup_dir.is_none());
        assert!(rep.findings.iter().any(|f| {
            f.code == "DNS_CUSTOM_RESOLV_PRESERVED"
                && f.status == FindingStatus::PreservedUserCustom
        }));
        assert_eq!(
            fs::read_to_string(root.join("etc/resolv.conf")).unwrap(),
            orig
        );
    }

    // Healthy systemd-resolved symlink
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::create_dir_all(root.join("run/systemd/resolve")).unwrap();
        let stub = root.join("run/systemd/resolve/stub-resolv.conf");
        fs::write(&stub, "nameserver 127.0.0.53\n").unwrap();
        symlink(&stub, root.join("etc/resolv.conf")).unwrap();

        let mut ctx = ReconcileContext::with_root(root);
        ctx.domains = vec![ReconcileDomain::Dns];
        ctx.no_sys = true;
        let runner = MockCommandRunner::new();

        let rep = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
        assert_eq!(rep.changes_made, 0);
        assert!(rep.findings.iter().any(|f| {
            f.code == "DNS_SYMLINK_PRESERVED" && f.status == FindingStatus::PreservedUserCustom
        }));
        assert!(
            fs::symlink_metadata(root.join("etc/resolv.conf"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn restore_backup_rejects_path_traversal_in_manifest() {
    let tmp = TempDir::new().unwrap();
    let run_dir = tmp.path().join("bad_backup");
    let root = tmp.path().join("target_root");
    fs::create_dir_all(&run_dir).unwrap();
    fs::create_dir_all(&root).unwrap();

    let evil_manifest = BackupManifest {
        run_id: "evil".to_string(),
        entries: vec![BackupEntry {
            original_path: "../../outside.txt".to_string(),
            backup_rel_path: None,
            existed: false,
            is_symlink: false,
            symlink_target: None,
            mode: None,
            uid: None,
            gid: None,
        }],
    };
    fs::write(
        run_dir.join("manifest.json"),
        serde_json::to_string_pretty(&evil_manifest).unwrap(),
    )
    .unwrap();

    let err = restore_backup(&run_dir, &root).unwrap_err();
    assert!(matches!(err, CoreError::ValidationFailed(_)));
}

// =========================================================================
// [U14] Red-Team Round-1 Reconcile Regressions (BE-P0-01, P1-06/07/21/22/23, P2-14)
// =========================================================================

#[test]
fn u14_reconcile_auth_keys_owner_backup_perms_key_eq_val_logrotate_ufw_buster_resolv() {
    use std::os::unix::fs::MetadataExt;

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Seed /etc/passwd with current euid/egid so ownership verification works deterministically
    let cur_uid = debopty_core::sys::current_euid();
    let cur_gid = debopty_core::sys::current_egid();
    fs::create_dir_all(root.join("etc/debopti")).unwrap();
    fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
    fs::create_dir_all(root.join("home/ops/.ssh")).unwrap();
    fs::create_dir_all(root.join("etc/apt")).unwrap();
    fs::write(
        root.join("etc/passwd"),
        format!(
            "root:x:0:0:root:/root:/bin/bash\nops:x:{cur_uid}:{cur_gid}:Ops:/home/ops:/bin/bash\n"
        ),
    )
    .unwrap();
    fs::write(
        root.join("etc/debopti/debopti.conf"),
        "SSH_HARDENED=\"true\"\n",
    )
    .unwrap();

    // BE-P0-01 & BE-P1-22: 0600 authorized_keys with CRLF + dup
    let auth_keys = root.join("home/ops/.ssh/authorized_keys");
    fs::write(
        &auth_keys,
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKeyAlpha111111111111111111111111111111111111 ops@host\r\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKeyAlpha111111111111111111111111111111111111 dup\r\n",
    )
    .unwrap();
    fs::set_permissions(&auth_keys, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(
        root.join("home/ops/.ssh"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();

    // BE-P1-07: Valid Key=Value SSH drop-in must NOT be quarantined; Key=Value weak auth IS caught
    fs::write(root.join("etc/ssh/sshd_config"), "Port=2222\n").unwrap();
    let kv_valid = root.join("etc/ssh/sshd_config.d/60-kv-valid.conf");
    fs::write(
        &kv_valid,
        "ClientAliveInterval=60\nPermitRootLogin = prohibit-password\nPubkeyAuthentication=yes\n",
    )
    .unwrap();
    let kv_weak = root.join("etc/ssh/sshd_config.d/50-kv-weak.conf");
    fs::write(&kv_weak, "PasswordAuthentication=yes\n").unwrap();

    // BE-P1-06: Logrotate wtmp/btmp { ... } block preserved verbatim
    let lr_path = root.join("etc/logrotate.conf");
    fs::write(
        &lr_path,
        "weekly\ninclude /etc/logrotate.d\n\n/var/log/wtmp {\n    monthly\n    rotate 1\n}\n",
    )
    .unwrap();

    // BE-P1-23: Debian 10 Buster APT sources.list with custom 3rd-party repo preserved
    fs::write(root.join("etc/debian_version"), "10.13\n").unwrap();
    let sl_path = root.join("etc/apt/sources.list");
    fs::write(
        &sl_path,
        "# custom comment\ndeb http://deb.debian.org/debian buster main\ndeb http://security.debian.org/debian-security buster/updates main\ndeb https://custom.corp.example/apt buster main\n",
    )
    .unwrap();

    // BE-P2-14: Tab-delimited domain/search/options in broken resolv.conf preserved
    let resolv_path = root.join("etc/resolv.conf");
    fs::write(
        &resolv_path,
        "domain\tcorp.internal\nsearch\tcorp.internal\texample.local\noptions\tedns0\ttimeout:2\n",
    )
    .unwrap();

    let mut ctx = ReconcileContext::with_root(root);
    ctx.no_sys = true;
    ctx.domains = vec![
        ReconcileDomain::Ssh,
        ReconcileDomain::Dns,
        ReconcileDomain::Logrotate,
        ReconcileDomain::Apt,
    ];
    let runner = MockCommandRunner::new();
    let rep = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
    let bak_dir = rep.backup_dir.expect("backup dir created");

    // BE-P1-22: Backup dirs are 0700, manifest is 0600, snapshotted authorized_keys stays 0600
    assert_eq!(
        fs::metadata(&ctx.backup_base_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&bak_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(bak_dir.join("manifest.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let snap_auth = bak_dir.join("files/home/ops/.ssh/authorized_keys");
    assert_eq!(
        fs::metadata(&snap_auth).unwrap().permissions().mode() & 0o777,
        0o600,
        "snapshotted 0600 authorized_keys must NOT become world-readable 0644"
    );

    // BE-P0-01: Manifest records uid/gid and normalized authorized_keys preserves uid/gid & 0600
    let manifest: BackupManifest =
        serde_json::from_str(&fs::read_to_string(bak_dir.join("manifest.json")).unwrap()).unwrap();
    let auth_entry = manifest
        .entries
        .iter()
        .find(|e| e.original_path == "home/ops/.ssh/authorized_keys")
        .expect("authorized_keys recorded in manifest");
    assert_eq!(auth_entry.uid, Some(cur_uid));
    assert_eq!(auth_entry.gid, Some(cur_gid));
    let auth_meta = fs::metadata(&auth_keys).unwrap();
    assert_eq!(auth_meta.uid(), cur_uid);
    assert_eq!(auth_meta.gid(), cur_gid);
    assert_eq!(auth_meta.permissions().mode() & 0o777, 0o600);

    // BE-P1-07: Valid Key=Value drop-in was NOT quarantined; hardening override emitted for weak KV
    assert!(
        kv_valid.is_file(),
        "valid Key=Value drop-in must NOT be quarantined"
    );
    assert!(
        !root
            .join("etc/ssh/sshd_config.d/60-kv-valid.conf.disabled")
            .exists()
    );
    assert!(
        rep.findings
            .iter()
            .any(|f| f.code == "SSH_CONFLICTING_DROPIN")
    );

    // BE-P1-06: wtmp block monthly/rotate 1 preserved
    let lr_after = fs::read_to_string(&lr_path).unwrap();
    assert!(lr_after.contains("daily\ninclude /etc/logrotate.d"));
    assert!(lr_after.contains("    monthly\n"));
    assert!(lr_after.contains("    rotate 1\n"));

    // BE-P1-23: Debian 10 custom repo preserved alongside archive.debian.org
    let sl_after = fs::read_to_string(&sl_path).unwrap();
    assert!(sl_after.contains("archive.debian.org"));
    assert!(sl_after.contains("https://custom.corp.example/apt buster main"));
    assert!(sl_after.contains("# custom comment"));

    // BE-P2-14: Tab-delimited resolv.conf directives preserved
    let resolv_after = fs::read_to_string(&resolv_path).unwrap();
    assert!(resolv_after.contains("domain\tcorp.internal"));
    assert!(resolv_after.contains("search\tcorp.internal\texample.local"));
    assert!(resolv_after.contains("options\tedns0\ttimeout:2"));

    // Restore preserves uid/gid and 0600 on authorized_keys
    restore_backup(&bak_dir, root).unwrap();
    let restored_meta = fs::metadata(&auth_keys).unwrap();
    assert_eq!(restored_meta.uid(), cur_uid);
    assert_eq!(restored_meta.gid(), cur_gid);
    assert_eq!(restored_meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn u14_be_p1_21_ufw_not_purged_before_nft_check_and_f2b_reloaded_on_jail_fix() {
    // Part A: nft -c -f failure must roll back nft changes and NEVER disable/purge UFW
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
        fs::create_dir_all(root.join("etc/ufw")).unwrap();
        fs::write(
            root.join("etc/nftables.conf"),
            "flush ruleset\ntable inet filter { chain input { tcp dport 22 accept comment \"SSH_Access_Port\" } }\n",
        )
        .unwrap();
        fs::write(root.join("etc/ufw/ufw.conf"), "ENABLED=yes\n").unwrap();
        fs::write(
            root.join("etc/ufw/user.rules"),
            "-A ufw-user-input -p tcp --dport 8080 -j ACCEPT\n",
        )
        .unwrap();

        let mut ctx = ReconcileContext::with_root(root);
        ctx.domains = vec![ReconcileDomain::Firewall];
        let runner = MockCommandRunner::new();
        runner.on_exact("dpkg", &["-s", "ufw"], ok_out("installed"));
        runner.on_prog("nft", err_out("syntax error"));
        runner.on_exact("nft", &["list", "tables"], ok_out(""));

        let err = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap_err();
        assert!(matches!(err, CoreError::Reconcile(_)));
        assert_eq!(
            fs::read_to_string(root.join("etc/ufw/ufw.conf")).unwrap(),
            "ENABLED=yes\n",
            "UFW must remain enabled when nft -c -f fails"
        );
        assert!(
            !runner
                .invocations()
                .iter()
                .any(|(p, _)| p == "ufw" || p == "apt-get"),
            "ufw disable / apt-get purge ufw must NOT run before nft -c -f succeeds"
        );
    }

    // Part B: Mutating 99-debopty.local (ignoreip/backend) reloads fail2ban even with zero port-sync drift
    {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc/fail2ban/jail.d")).unwrap();
        fs::create_dir_all(root.join("etc/nftables.d")).unwrap();
        fs::create_dir_all(root.join("run/systemd/system")).unwrap();
        fs::write(
            root.join("etc/fail2ban/jail.local"),
            "[DEFAULT]\nignoreip = 198.51.100.42\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/fail2ban/jail.d/99-debopty.local"),
            "[DEFAULT]\nignoreip = 127.0.0.1/8 ::1\nbackend = auto\n",
        )
        .unwrap();
        // Pre-populate matching 99-debopty-ports.local so f2b_sync_has_drift == false
        fs::write(
            root.join("etc/fail2ban/jail.d/99-debopty-ports.local"),
            "[sshd]\nport = 22\n[debopty-public-ports]\nenabled = false\nport = 22\n",
        )
        .unwrap();

        let mut ctx = ReconcileContext::with_root(root);
        ctx.domains = vec![ReconcileDomain::Firewall];
        ctx.f2b_backend_override = Some("systemd".to_string());

        let runner = MockCommandRunner::new();
        runner.on_exact("dpkg", &["-s", "ufw"], err_out("not installed"));
        runner.on_exact("nft", &["list", "tables"], ok_out(""));
        runner.on_exact(
            "systemctl",
            &["is-active", "--quiet", "fail2ban"],
            ok_out(""),
        );
        runner.on_exact("fail2ban-client", &["reload"], ok_out(""));

        let rep = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
        assert!(
            rep.findings
                .iter()
                .any(|f| f.code == "F2B_IGNOREIP_MERGED" && f.status == FindingStatus::Fixed)
        );
        assert!(
            runner
                .invocations()
                .iter()
                .any(|(p, a)| p == "fail2ban-client" && a == &vec!["reload".to_string()]),
            "fail2ban-client reload must run when 99-debopty.local is mutated"
        );
    }
}
