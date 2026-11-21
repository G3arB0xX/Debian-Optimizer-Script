#!/bin/bash
# =========================================================
# Debopty Fail2ban Public Open Ports Auto-Sync Helper
# Standalone script installed at /usr/local/bin/debopty-f2b-sync
# =========================================================
set -euo pipefail

ROOT_PREFIX="${DEBOPTY_F2B_ROOT:-}"
ROOT_PREFIX="${ROOT_PREFIX%/}"
NO_SYS="${DEBOPTY_F2B_NO_SYS:-0}"

NFT_CONF="${ROOT_PREFIX}/etc/nftables.conf"
NFT_D_DIR="${ROOT_PREFIX}/etc/nftables.d"
NFT_GUARD_FILE="${NFT_D_DIR}/55-debopty-f2b-guard.nft"
F2B_JAIL_DIR="${ROOT_PREFIX}/etc/fail2ban/jail.d"
F2B_PORTS_FILE="${F2B_JAIL_DIR}/99-debopty-ports.local"
SSHD_CONF="${ROOT_PREFIX}/etc/ssh/sshd_config"
SSH_SOCKET_OVERRIDE="${ROOT_PREFIX}/etc/systemd/system/ssh.socket.d/override.conf"

FORCE_RELOAD="false"
PRINT_PORTS="false"

for arg in "$@"; do
    case "$arg" in
        --force-reload) FORCE_RELOAD="true" ;;
        --print-ports) PRINT_PORTS="true" ;;
        *)
            printf 'Unknown option: %s\n' "$arg" >&2
            exit 2
            ;;
    esac
done

resolve_template_dir() {
    if [[ -n "${DEBOPTY_F2B_TEMPLATE_DIR:-}" && -d "${DEBOPTY_F2B_TEMPLATE_DIR}" ]]; then
        printf '%s\n' "${DEBOPTY_F2B_TEMPLATE_DIR}"
        return 0
    fi
    local installed_dir="${ROOT_PREFIX}/usr/local/share/debopty/templates/security"
    if [[ -d "$installed_dir" ]]; then
        printf '%s\n' "$installed_dir"
        return 0
    fi
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    if [[ -f "${script_dir}/fail2ban_ports.local" ]]; then
        printf '%s\n' "$script_dir"
        return 0
    fi
    return 1
}

render_simple_template() {
    local tmpl="$1"
    local out="$2"
    shift 2
    local content
    content="$(cat "$tmpl")"
    local pair key val
    for pair in "$@"; do
        key="${pair%%=*}"
        val="${pair#*=}"
        content="${content//\{\{${key}\}\}/$val}"
    done
    mkdir -p "$(dirname "$out")"
    printf '%s\n' "$content" > "$out"
}

extract_ssh_port() {
    local port=""

    if [[ -f "$NFT_CONF" ]]; then
        port="$(awk '
            /SSH_Access_Port/ {
                line = $0
                sub(/#.*/, "", line)
                if (match(line, /tcp[ \t]+dport[ \t]+\{?[ \t]*[0-9]+/)) {
                    seg = substr(line, RSTART, RLENGTH)
                    sub(/^.*dport[ \t]+\{?[ \t]*/, "", seg)
                    print seg
                    exit
                }
            }
        ' "$NFT_CONF" 2>/dev/null || true)"
    fi

    if [[ -z "$port" && -f "$SSH_SOCKET_OVERRIDE" ]]; then
        port="$(awk -F= '
            /^[ \t]*ListenStream[ \t]*=/ {
                val = $2
                gsub(/^[ \t]+|[ \t]+$/, "", val)
                if (val != "") {
                    sub(/^.*:/, "", val)
                    if (val ~ /^[0-9]+$/) p = val
                }
            }
            END { if (p != "") print p }
        ' "$SSH_SOCKET_OVERRIDE" 2>/dev/null || true)"
    fi

    if [[ -z "$port" && -f "$SSHD_CONF" ]]; then
        port="$(awk '
            /^[ \t]*Port[ \t]+[0-9]+/ {
                print $2
                exit
            }
        ' "$SSHD_CONF" 2>/dev/null || true)"
    fi

    if [[ -z "$port" && "$NO_SYS" != "1" ]] && command -v sshd >/dev/null 2>&1; then
        port="$(sshd -T 2>/dev/null | awk '/^port / {print $2; exit}' || true)"
    fi

    if [[ ! "$port" =~ ^[0-9]+$ ]] || (( port < 1 || port > 65535 )); then
        port="22"
    fi
    printf '%s\n' "$port"
}

nft_has_policy_drop() {
    [[ -f "$NFT_CONF" ]] || return 1
    awk '
        {
            sub(/#.*/, "")
            if ($0 ~ /policy[ \t]+drop/) { found = 1; exit }
        }
        END { exit(found ? 0 : 1) }
    ' "$NFT_CONF"
}

parse_nft_files() {
    local files=()
    [[ -f "$NFT_CONF" ]] && files+=("$NFT_CONF")
    if [[ -d "$NFT_D_DIR" ]]; then
        local f
        while IFS= read -r f; do
            [[ -n "$f" ]] && files+=("$f")
        done < <(find "$NFT_D_DIR" -type f -name '*.nft' ! -name '55-debopty-f2b-guard.nft' 2>/dev/null | sort)
    fi
    [[ ${#files[@]} -eq 0 ]] && return 0

    awk '
        {
            sub(/#.*/, "")
            if ($0 !~ /(^|[^A-Za-z0-9_])accept([^A-Za-z0-9_]|$)/) next
            line = $0
            while (match(line, /(tcp|udp)[ \t]+dport[ \t]+(\{[^}]*\}|[0-9]+[ \t]*[-:][ \t]*[0-9]+|[0-9]+)/)) {
                seg = substr(line, RSTART, RLENGTH)
                line = substr(line, RSTART + RLENGTH)
                proto = (seg ~ /^tcp/) ? "tcp" : "udp"
                sub(/^(tcp|udp)[ \t]+dport[ \t]+/, "", seg)
                gsub(/[{}]/, "", seg)
                n = split(seg, items, ",")
                for (i = 1; i <= n; i++) {
                    item = items[i]
                    gsub(/^[ \t]+|[ \t]+$/, "", item)
                    if (item ~ /^[0-9]+$/) {
                        p = item + 0
                        if (p >= 1 && p <= 65535) print proto, p
                    } else if (item ~ /^[0-9]+[ \t]*[-:][ \t]*[0-9]+$/) {
                        gsub(/[ \t]*[-:][ \t]*/, ":", item)
                        split(item, r, ":")
                        p1 = r[1] + 0
                        p2 = r[2] + 0
                        if (p1 >= 1 && p2 <= 65535 && p1 <= p2) {
                            print proto, p1 ":" p2
                        }
                    }
                }
            }
        }
    ' "${files[@]}" 2>/dev/null || true
}

parse_ss_public_ports() {
    local ss_data=""
    if [[ -n "${DEBOPTY_F2B_SS_FILE:-}" && -r "${DEBOPTY_F2B_SS_FILE}" ]]; then
        ss_data="$(cat "${DEBOPTY_F2B_SS_FILE}")"
    elif [[ "$NO_SYS" != "1" ]] && command -v ss >/dev/null 2>&1; then
        ss_data="$(ss -tulnH 2>/dev/null || true)"
    fi
    [[ -z "$ss_data" ]] && return 0

    printf '%s\n' "$ss_data" | awk '
        {
            netid = $1
            local_addr = $5
            if (netid ~ /^tcp/) proto = "tcp"
            else if (netid ~ /^udp/) proto = "udp"
            else next

            # Split host and port at last colon
            if (!match(local_addr, /:[0-9]+$/)) next
            port = substr(local_addr, RSTART + 1) + 0
            host = substr(local_addr, 1, RSTART - 1)
            gsub(/^\[|\]$/, "", host)
            sub(/%.*$/, "", host)

            # Exclude loopback and link-local addresses
            if (host ~ /^127\./ || host == "::1" || host ~ /^169\.254\./ || tolower(host) ~ /^fe80:/) next
            if (port >= 1 && port <= 65535) print proto, port
        }
    '
}

discover_public_open_ports() {
    {
        parse_nft_files
        if ! nft_has_policy_drop; then
            parse_ss_public_ports
        fi
    } | sort -u -t' ' -k1,1 -k2,2V
}

format_nft_port_expr() {
    local items=("$@")
    local count=${#items[@]}
    if (( count == 0 )); then
        printf ''
        return 0
    fi
    local formatted=()
    local item
    for item in "${items[@]}"; do
        formatted+=("${item/:/ - }")
    done
    local joined=""
    local i
    for (( i = 0; i < count; i++ )); do
        if (( i > 0 )); then
            joined+=", "
        fi
        joined+="${formatted[$i]}"
    done
    if (( count > 1 )) || [[ "$joined" == *" - "* ]]; then
        printf '{ %s }' "$joined"
    else
        printf '%s' "$joined"
    fi
}

format_f2b_port_list() {
    local items=("$@")
    local joined=""
    local i
    for (( i = 0; i < ${#items[@]}; i++ )); do
        if (( i > 0 )); then
            joined+=","
        fi
        joined+="${items[$i]}"
    done
    printf '%s' "$joined"
}

detect_public_guard_backend() {
    if [[ -n "${DEBOPTY_F2B_BACKEND:-}" ]]; then
        if [[ "${DEBOPTY_F2B_BACKEND}" == "systemd" ]]; then
            printf 'backend = systemd\njournalmatch = _TRANSPORT=kernel'
        else
            printf 'logpath = /var/log/kern.log'
        fi
        return 0
    fi

    if [[ -d /run/systemd/system ]] && { python3 -c 'import systemd.journal' >/dev/null 2>&1 || dpkg -s python3-systemd >/dev/null 2>&1; }; then
        printf 'backend = systemd\njournalmatch = _TRANSPORT=kernel'
    else
        printf 'logpath = /var/log/kern.log'
    fi
}

main() {
    local ssh_port
    ssh_port="$(extract_ssh_port)"

    local tcp_ports=()
    local udp_ports=()
    local all_ports_raw=()
    local proto port
    while read -r proto port; do
        [[ -z "$proto" || -z "$port" ]] && continue
        if [[ "$proto" == "tcp" ]]; then
            tcp_ports+=("$port")
        elif [[ "$proto" == "udp" ]]; then
            udp_ports+=("$port")
        fi
        all_ports_raw+=("$port")
    done < <(discover_public_open_ports)

    local all_ports=()
    if (( ${#all_ports_raw[@]} > 0 )); then
        while IFS= read -r port; do
            [[ -n "$port" ]] && all_ports+=("$port")
        done < <(printf '%s\n' "${all_ports_raw[@]}" | sort -u -V)
    fi

    local tcp_nft=""
    local udp_nft=""
    local all_f2b=""
    (( ${#tcp_ports[@]} > 0 )) && tcp_nft="$(format_nft_port_expr "${tcp_ports[@]}")"
    (( ${#udp_ports[@]} > 0 )) && udp_nft="$(format_nft_port_expr "${udp_ports[@]}")"
    (( ${#all_ports[@]} > 0 )) && all_f2b="$(format_f2b_port_list "${all_ports[@]}")"

    if [[ "$PRINT_PORTS" == "true" ]]; then
        printf 'SSH_PORT=%s\n' "$ssh_port"
        printf 'TCP_PORTS_NFT=%s\n' "$tcp_nft"
        printf 'UDP_PORTS_NFT=%s\n' "$udp_nft"
        printf 'ALL_PUBLIC_PORTS_F2B=%s\n' "$all_f2b"
        exit 0
    fi

    local tmpl_dir
    if ! tmpl_dir="$(resolve_template_dir)"; then
        printf 'debopty-f2b-sync: template directory not found\n' >&2
        exit 1
    fi

    local tmp_dir
    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "${tmp_dir:-}"' EXIT

    local has_ports="false"
    local public_guard_enabled="false"
    local f2b_ports_value="$ssh_port"
    local guard_rules=""

    if (( ${#all_ports[@]} > 0 )); then
        has_ports="true"
        public_guard_enabled="true"
        f2b_ports_value="$all_f2b"

        if [[ -n "$tcp_nft" ]]; then
            guard_rules+="        ct state new tcp dport ${tcp_nft} update @abuse_v4 { ip saddr limit rate over 60/minute burst 30 packets } log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second"
            guard_rules+=$'\n'
            guard_rules+="        ct state new tcp dport ${tcp_nft} update @abuse_v6 { ip6 saddr limit rate over 60/minute burst 30 packets } log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second"
        fi
        if [[ -n "$udp_nft" ]]; then
            [[ -n "$guard_rules" ]] && guard_rules+=$'\n'
            guard_rules+="        ct state new udp dport ${udp_nft} update @abuse_v4 { ip saddr limit rate over 60/minute burst 30 packets } log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second"
            guard_rules+=$'\n'
            guard_rules+="        ct state new udp dport ${udp_nft} update @abuse_v6 { ip6 saddr limit rate over 60/minute burst 30 packets } log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second"
        fi
    fi

    local public_guard_backend
    public_guard_backend="$(detect_public_guard_backend)"

    local tmp_jail="${tmp_dir}/99-debopty-ports.local"
    local tmp_guard="${tmp_dir}/55-debopty-f2b-guard.nft"

    render_simple_template "${tmpl_dir}/fail2ban_ports.local" "$tmp_jail" \
        "SSH_PORT=${ssh_port}" \
        "PUBLIC_GUARD_ENABLED=${public_guard_enabled}" \
        "ALL_PUBLIC_PORTS_F2B=${f2b_ports_value}" \
        "PUBLIC_GUARD_BACKEND=${public_guard_backend}"

    local guard_valid="true"
    if [[ "$has_ports" == "true" ]]; then
        render_simple_template "${tmpl_dir}/f2b_public_ports_guard.nft" "$tmp_guard" \
            "GUARD_RULES=${guard_rules}"

        if [[ "$NO_SYS" != "1" && "${EUID:-$(id -u)}" -eq 0 ]] && command -v nft >/dev/null 2>&1; then
            if ! nft -c -f "$tmp_guard" >/dev/null 2>&1; then
                printf 'debopty-f2b-sync: nft syntax validation failed for generated guard table\n' >&2
                guard_valid="false"
            fi
        fi
    fi

    local guard_changed="false"
    local jail_changed="false"

    if [[ "$has_ports" == "true" ]]; then
        if [[ "$guard_valid" == "true" ]]; then
            if [[ ! -f "$NFT_GUARD_FILE" ]] || ! cmp -s "$tmp_guard" "$NFT_GUARD_FILE"; then
                mkdir -p "$NFT_D_DIR"
                cp "$tmp_guard" "$NFT_GUARD_FILE"
                chmod 644 "$NFT_GUARD_FILE"
                guard_changed="true"
            fi
        fi
    else
        if [[ -f "$NFT_GUARD_FILE" ]]; then
            rm -f "$NFT_GUARD_FILE"
            guard_changed="true"
        fi
    fi

    if [[ ! -f "$F2B_PORTS_FILE" ]] || ! cmp -s "$tmp_jail" "$F2B_PORTS_FILE"; then
        mkdir -p "$F2B_JAIL_DIR"
        cp "$tmp_jail" "$F2B_PORTS_FILE"
        chmod 644 "$F2B_PORTS_FILE"
        jail_changed="true"
    fi

    if [[ "$guard_changed" == "false" && "$jail_changed" == "false" && "$FORCE_RELOAD" == "false" ]]; then
        exit 0
    fi

    if [[ "$NO_SYS" != "1" && "${EUID:-$(id -u)}" -eq 0 ]]; then
        if [[ "$guard_changed" == "true" || "$FORCE_RELOAD" == "true" ]] && command -v nft >/dev/null 2>&1; then
            if [[ "$has_ports" == "true" && "$guard_valid" == "true" ]]; then
                nft -f "$NFT_GUARD_FILE" >/dev/null 2>&1 || true
            elif [[ "$has_ports" == "false" ]]; then
                nft delete table inet debopty_f2b_guard >/dev/null 2>&1 || true
            fi
        fi

        if [[ "$jail_changed" == "true" || "$FORCE_RELOAD" == "true" ]]; then
            if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet fail2ban 2>/dev/null; then
                if command -v fail2ban-client >/dev/null 2>&1; then
                    fail2ban-client reload >/dev/null 2>&1 || systemctl reload-or-restart fail2ban >/dev/null 2>&1 || true
                else
                    systemctl reload-or-restart fail2ban >/dev/null 2>&1 || true
                fi
            fi
        fi
    fi
}

main
