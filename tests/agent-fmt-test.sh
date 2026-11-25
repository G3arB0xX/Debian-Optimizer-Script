#!/usr/bin/env bash
# Offline host unit tests for tools/verify_agent_fmt.py, tools/verify_repo_hygiene.py,
# and tools/check.sh --slice.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FMT_TOOL="${ROOT}/tools/verify_agent_fmt.py"
HYG_TOOL="${ROOT}/tools/verify_repo_hygiene.py"
CHECK_TOOL="${ROOT}/tools/check.sh"

PASS=0
FAIL=0

pass() {
    echo "PASS: $1"
    PASS=$((PASS + 1))
}

fail() {
    echo "FAIL: $1"
    FAIL=$((FAIL + 1))
}

TMP="$(mktemp -d /tmp/debopty-agent-fmt-test.XXXXXX)"
cleanup() {
    rm -rf "$TMP"
}
trap cleanup EXIT

seed_valid_repo() {
    local d="$1"
    local shim_a="CLAUDE" shim_b="GEMINI"
    rm -rf "$d"
    mkdir -p "$d/.agents/constraints" "$d/tests"

    cat > "$d/AGENTS.md" <<'EOF'
---
name: sample-agents
description: Sample root entrypoint.
zh_summary: 示例根目录总入口规范。
tags: [entrypoint, agents]
index: [.agents/CONSTRAINTS.md, .agents/constraints/INDEX.md]
---

<guide>
PURPOSE:
  project: debopty | pure bash optimizer

COMMANDS:
  verify: make check | placeholder `<id>` allowed
</guide>
EOF

    cat > "$d/${shim_a}.md" <<'EOF'
---
name: sample-compat-1
description: Compatibility shim.
zh_summary: 兼容入口文件。
tags: [entrypoint]
index: [AGENTS.md]
---

<guide>
ENTRYPOINT:
  target: AGENTS.md
</guide>
EOF

    cat > "$d/${shim_b}.md" <<'EOF'
---
name: sample-compat-2
description: Compatibility shim.
zh_summary: 兼容入口文件。
tags: [entrypoint]
index: [AGENTS.md]
---

<guide>
ENTRYPOINT:
  target: AGENTS.md
</guide>
EOF

    cat > "$d/README.md" <<'EOF'
# Sample Project

[中文说明](README_CN.md)
EOF

    cat > "$d/README_CN.md" <<'EOF'
# 示例项目

[English](README.md)
EOF

    cat > "$d/.agents/CONSTRAINTS.md" <<'EOF'
---
name: sample-constraints
description: Constraints router.
zh_summary: 约束总路由索引。
tags: [agents]
index: [.agents/constraints/INDEX.md, .agents/constraints/README.md]
---

<constraints_index>
ROUTER:
  general: .agents/constraints/00_general.md
</constraints_index>
EOF

    cat > "$d/.agents/constraints/INDEX.md" <<'EOF'
---
name: sample-constraints-index
description: Constraints directory index.
zh_summary: 约束目录索引文件。
tags: [index]
index: [./README.md, ./00_general.md]
---

<index>
LAYERS:
  general: 00_general.md | mandatory
</index>
EOF

    cat > "$d/.agents/constraints/README.md" <<'EOF'
# 约束目录

Agent 路由：[INDEX.md](INDEX.md) | 通用规则：[00_general.md](00_general.md)
EOF

    cat > "$d/.agents/constraints/00_general.md" <<'EOF'
# 通用规则

人类可读文档，可以使用行内代码 `<guide>` 进行说明。
EOF

    cat > "$d/tests/INDEX.md" <<'EOF'
---
name: sample-tests-index
description: Tests directory index.
zh_summary: 测试目录索引文件。
tags: [tests, index]
index: [./README.md]
---

<index>
SUITES:
  fmt: agent-fmt-test.sh
</index>
EOF

    cat > "$d/tests/README.md" <<'EOF'
# 测试说明

Agent 路由：[INDEX.md](INDEX.md)
EOF
}

expect_fmt_fail() {
    local label="$1" dir="$2" pattern="$3"
    local out rc=0
    out="$(python3 -B "$FMT_TOOL" --root "$dir" 2>&1)" || rc=$?
    if [[ "$rc" -ne 0 ]] && grep -qE "$pattern" <<< "$out"; then
        pass "$label"
    else
        echo "  expected rc!=0 and pattern /$pattern/, got rc=$rc"
        echo "  output: $out"
        fail "$label"
    fi
}

expect_fmt_pass() {
    local label="$1" dir="$2"
    local out rc=0
    out="$(python3 -B "$FMT_TOOL" --root "$dir" 2>&1)" || rc=$?
    if [[ "$rc" -eq 0 ]]; then
        pass "$label"
    else
        echo "  expected rc=0, got rc=$rc"
        echo "  output: $out"
        fail "$label"
    fi
}

# 0. Positive case
FIX="${TMP}/valid"
seed_valid_repo "$FIX"
expect_fmt_pass "valid fixture repo passes verify_agent_fmt" "$FIX"

# 1. Non-CJK zh_summary rejected
seed_valid_repo "$FIX"
sed -i 's/示例根目录总入口规范。/English only summary./' "$FIX/AGENTS.md"
expect_fmt_fail "non-CJK zh_summary rejected" "$FIX" "zh_summary.*CJK"

# 2. Scalar tags: foo (non-list) rejected
seed_valid_repo "$FIX"
sed -i 's/^tags: .*/tags: foo/' "$FIX/AGENTS.md"
expect_fmt_fail "scalar tags rejected" "$FIX" "'tags' must be a non-empty list"

# 3. Missing index target path rejected
seed_valid_repo "$FIX"
sed -i 's|.agents/CONSTRAINTS.md|.agents/nope.md|' "$FIX/AGENTS.md"
expect_fmt_fail "missing index path rejected" "$FIX" "missing on disk.*\.agents/nope.md"

# 4. Unbackticked inline nested XML rejected; backticked accepted
seed_valid_repo "$FIX"
sed -i 's|`<id>`|<foo>|' "$FIX/AGENTS.md"
expect_fmt_fail "unbackticked inline nested XML rejected" "$FIX" "nested XML tag <foo>"
seed_valid_repo "$FIX"
sed -i 's|`<id>`|`<foo>`|' "$FIX/AGENTS.md"
expect_fmt_pass "backticked inline XML placeholder accepted" "$FIX"

# 5. Missing blank line before second SECTION: header rejected
seed_valid_repo "$FIX"
cat > "$FIX/AGENTS.md" <<'EOF'
---
name: sample-agents
description: Sample root entrypoint.
zh_summary: 示例根目录总入口规范。
tags: [entrypoint]
index: [.agents/CONSTRAINTS.md]
---

<guide>
PURPOSE:
  project: debopty
COMMANDS:
  verify: make check
</guide>
EOF
expect_fmt_fail "missing blank line before second SECTION rejected" "$FIX" "missing blank line before section"

# 6. Lowercase section header rejected
seed_valid_repo "$FIX"
sed -i 's/^COMMANDS:/commands:/' "$FIX/AGENTS.md"
expect_fmt_fail "lowercase section header rejected" "$FIX" "ALL-CAPS SECTION"

# 7. Markdown table separator inside <guide> rejected
seed_valid_repo "$FIX"
cat > "$FIX/AGENTS.md" <<'EOF'
---
name: sample-agents
description: Sample root entrypoint.
zh_summary: 示例根目录总入口规范。
tags: [entrypoint]
index: [.agents/CONSTRAINTS.md]
---

<guide>
PURPOSE:
  | col1 | col2 |
  | --- | --- |
</guide>
EOF
expect_fmt_fail "markdown table separator rejected" "$FIX" "Markdown table forbidden"

# 8. Agent XML tag outside code blocks in human doc rejected; backticked accepted
seed_valid_repo "$FIX"
printf '\n<guide>\n' >> "$FIX/.agents/constraints/00_general.md"
expect_fmt_fail "agent XML tag in human doc rejected" "$FIX" "agent XML boundary tag forbidden"

# 9. Missing bidirectional link (.agents/constraints/README.md -> INDEX.md) rejected
seed_valid_repo "$FIX"
cat > "$FIX/.agents/constraints/README.md" <<'EOF'
# 约束目录

仅链接：[00_general.md](00_general.md)
EOF
expect_fmt_fail "missing bidirectional .agents/constraints README->INDEX link rejected" "$FIX" "missing bidirectional link"

# 10. ISO date in .agents/constraints/00_general.md rejected
seed_valid_repo "$FIX"
printf '\n更新日期：2026-11-21\n' >> "$FIX/.agents/constraints/00_general.md"
expect_fmt_fail "ISO date in constraint file rejected" "$FIX" "date '2026-11-21' forbidden"

# 11. Banned separator in .agents/constraints/00_general.md rejected
seed_valid_repo "$FIX"
printf '\n规则 A · 规则 B\n' >> "$FIX/.agents/constraints/00_general.md"
expect_fmt_fail "banned separator in constraint file rejected" "$FIX" "banned separator"

# 12. verify_repo_hygiene --root: clean passes, __pycache__ & *.pyc caught
seed_valid_repo "$FIX"
if python3 -B "$HYG_TOOL" --root "$FIX" >/dev/null 2>&1; then
    pass "verify_repo_hygiene passes on clean fixture"
else
    fail "verify_repo_hygiene passes on clean fixture"
fi
mkdir -p "$FIX/tools/__pycache__"
touch "$FIX/tools/__pycache__/foo.cpython-311.pyc"
hyg_out=""
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$FIX" 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] && grep -q "__pycache__" <<< "$hyg_out"; then
    pass "verify_repo_hygiene catches __pycache__"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene catches __pycache__"
fi
rm -rf "$FIX/tools/__pycache__"
touch "$FIX/stray.pyc"
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$FIX" 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] && grep -q "stray.pyc" <<< "$hyg_out"; then
    pass "verify_repo_hygiene catches stray .pyc"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene catches stray .pyc"
fi

# 13. Missing README.md <-> README_CN.md link rejected
seed_valid_repo "$FIX"
cat > "$FIX/README.md" <<'EOF'
# Sample Project

No link to Chinese README.
EOF
expect_fmt_fail "missing root README.md -> README_CN.md link rejected" "$FIX" "missing bilingual link"

# 14. Missing tests/README.md <-> tests/INDEX.md link rejected
seed_valid_repo "$FIX"
cat > "$FIX/tests/README.md" <<'EOF'
# 测试说明

无 INDEX 链接。
EOF
expect_fmt_fail "missing tests/README.md -> tests/INDEX.md link rejected" "$FIX" "missing bidirectional link.*tests/INDEX.md"

# 15. check.sh --slice AGENTS.md runs fast scoped gate and creates no __pycache__
slice_log_dir="${TMP}/slice_logs"
if DEBOPTY_SLICE="" DEBOPTY_LOG_DIR="$slice_log_dir" bash "$CHECK_TOOL" --slice AGENTS.md >/dev/null 2>&1 \
    && grep -q '"mode":"slice"' "$slice_log_dir/check_summary.jsonl" \
    && [[ ! -d "${ROOT}/tools/__pycache__" ]]; then
    pass "tools/check.sh --slice AGENTS.md passes with mode=slice and 0 __pycache__"
else
    fail "tools/check.sh --slice AGENTS.md passes with mode=slice and 0 __pycache__"
fi

# 16. target/ directory (e.g. rust/target/debug/) ignored by verify_agent_fmt & verify_repo_hygiene
seed_valid_repo "$FIX"
mkdir -p "$FIX/rust/target/debug"
printf 'invalid agent format\n' > "$FIX/rust/target/debug/INDEX.md"
touch "$FIX/rust/target/debug/foo.pyc"
expect_fmt_pass "verify_agent_fmt ignores rust/target/" "$FIX"
if python3 -B "$HYG_TOOL" --root "$FIX" >/dev/null 2>&1; then
    pass "verify_repo_hygiene ignores rust/target/"
else
    fail "verify_repo_hygiene ignores rust/target/"
fi

# 17. verify_repo_hygiene secret scanning (working-tree + benign patterns + --scan-git-history)
seed_valid_repo "$FIX"
p_pem="-----BEG""IN OPENSSH PRIVATE KEY-----"
p_ts="tskey""-auth-k999999CNTRL-abcdef123456"
p_gh="ghp""_1234567890abcdefghij123456"
p_benign_re="-----BEG""IN [A-Z0-9 ]*PRIVATE KEY-----"

printf '%s\n' "$p_pem" > "$FIX/leak_pem.txt"
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$FIX" 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] && grep -q "pem_private_key" <<< "$hyg_out"; then
    pass "verify_repo_hygiene catches working-tree pem_private_key"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene catches working-tree pem_private_key"
fi
rm -f "$FIX/leak_pem.txt"

printf 'ts=%s\ngh=%s\n' "$p_ts" "$p_gh" > "$FIX/leak_tokens.txt"
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$FIX" 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] && grep -q "tailscale_key" <<< "$hyg_out" && grep -q "github_token" <<< "$hyg_out"; then
    pass "verify_repo_hygiene catches working-tree tailscale_key and github_token"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene catches working-tree tailscale_key and github_token"
fi
rm -f "$FIX/leak_tokens.txt"

cat > "$FIX/benign_patterns.txt" <<EOF
regex: ${p_benign_re}
fido2_a: sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29t
fido2_b: sk-ecdsa-sha2-nistp256@openssh.com AAAAInNrLWVjZHNh
redacted_url: https://alice:[REDACTED]@proxy.example.com:8443/path
EOF
if python3 -B "$HYG_TOOL" --root "$FIX" >/dev/null 2>&1; then
    pass "verify_repo_hygiene has 0 false positives on regex/FIDO2/redacted URL"
else
    python3 -B "$HYG_TOOL" --root "$FIX" || true
    fail "verify_repo_hygiene has 0 false positives on regex/FIDO2/redacted URL"
fi
rm -f "$FIX/benign_patterns.txt"

GIT_FIX="${TMP}/git_hist_fixture"
seed_valid_repo "$GIT_FIX"
git -C "$GIT_FIX" init -b main >/dev/null 2>&1
git -C "$GIT_FIX" config user.name "test"
git -C "$GIT_FIX" config user.email "test@example.com"
git -C "$GIT_FIX" config commit.gpgsign false
printf '%s\n' "$p_pem" > "$GIT_FIX/old_secret.txt"
git -C "$GIT_FIX" add -A
git -C "$GIT_FIX" commit -m "add synthetic secret" >/dev/null 2>&1
rm -f "$GIT_FIX/old_secret.txt"
git -C "$GIT_FIX" add -A
git -C "$GIT_FIX" commit -m "remove synthetic secret" >/dev/null 2>&1
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$GIT_FIX" --scan-git-history HEAD 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] && grep -q "git-blob:.*old_secret.txt.*pem_private_key" <<< "$hyg_out"; then
    pass "verify_repo_hygiene --scan-git-history catches historical secret removed from HEAD"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene --scan-git-history catches historical secret removed from HEAD"
fi

GIT_CLEAN="${TMP}/git_clean_fixture"
seed_valid_repo "$GIT_CLEAN"
git -C "$GIT_CLEAN" init -b main >/dev/null 2>&1
git -C "$GIT_CLEAN" config user.name "test"
git -C "$GIT_CLEAN" config user.email "test@example.com"
git -C "$GIT_CLEAN" config commit.gpgsign false
git -C "$GIT_CLEAN" add -A
git -C "$GIT_CLEAN" commit -m "clean commit" >/dev/null 2>&1
if python3 -B "$HYG_TOOL" --root "$GIT_CLEAN" --scan-git-history HEAD >/dev/null 2>&1; then
    pass "verify_repo_hygiene --scan-git-history passes on clean git repo"
else
    python3 -B "$HYG_TOOL" --root "$GIT_CLEAN" --scan-git-history HEAD || true
    fail "verify_repo_hygiene --scan-git-history passes on clean git repo"
fi

# 18. Provisioned VPS agent workspace validation (verify_agent_fmt + --check-workspace-perms)
WS_FIX="${TMP}/vps_workspace"
mkdir -p "$WS_FIX/.agents/constraints" "$WS_FIX/.agents/memory" "$WS_FIX/.agents/logs" "$WS_FIX/.agents/checkpoints" "$WS_FIX/.agents/credentials"
cat > "$WS_FIX/AGENTS.md" <<'EOF'
---
name: vps-agent-root
description: VPS agent workspace root entrypoint.
zh_summary: VPS 管理 Agent 工作目录总入口。
tags: [vps, agent, entrypoint]
index: [.agents/INDEX.md, .agents/CONSTRAINTS.md, MEMORY.md]
---

<guide>
WORKSPACE:
  role: vps-admin | debopty-managed
</guide>
EOF
cat > "$WS_FIX/README.md" <<'EOF'
# VPS Agent 工作环境

[AGENTS.md](AGENTS.md) | [约束索引](.agents/constraints/README.md) | [MEMORY.md](MEMORY.md)
EOF
cat > "$WS_FIX/MEMORY.md" <<'EOF'
# 运维记忆

- 初始化日期：2026-11-22
EOF
cat > "$WS_FIX/.agents/INDEX.md" <<'EOF'
---
name: vps-agents-index
description: VPS .agents router.
zh_summary: VPS `.agents/` 目录路由。
tags: [vps, index]
index: [AGENTS.md, .agents/CONSTRAINTS.md, .agents/constraints/INDEX.md, .agents/memory/INDEX.md]
---

<index>
DIR:
  path: .agents
</index>
EOF
cat > "$WS_FIX/.agents/CONSTRAINTS.md" <<'EOF'
---
name: vps-constraints-index
description: VPS constraints index.
zh_summary: VPS 约束总索引。
tags: [vps, constraints]
index: [.agents/constraints/INDEX.md, .agents/constraints/README.md]
---

<constraints_index>
ROUTER:
  dir: .agents/constraints/
</constraints_index>
EOF
cat > "$WS_FIX/.agents/constraints/INDEX.md" <<'EOF'
---
name: vps-constraints-router
description: VPS constraints directory router.
zh_summary: VPS 约束目录路由。
tags: [vps, index]
index: [./README.md, ./00_vps_general_and_docs.md]
---

<index>
LAYERS:
  general: 00_vps_general_and_docs.md
</index>
EOF
cat > "$WS_FIX/.agents/constraints/README.md" <<'EOF'
# VPS 约束目录

[INDEX.md](INDEX.md) | [00_vps_general_and_docs.md](00_vps_general_and_docs.md)
EOF
cat > "$WS_FIX/.agents/constraints/00_vps_general_and_docs.md" <<'EOF'
# 00 通用与文档规范

遵循双受众与渐进式披露。
EOF
cat > "$WS_FIX/.agents/memory/INDEX.md" <<'EOF'
---
name: vps-memory-index
description: VPS memory directory router.
zh_summary: VPS 记忆目录路由。
tags: [vps, memory, index]
index: [./README.md, ./host-profile.md]
---

<index>
MEMORY:
  profile: host-profile.md
</index>
EOF
cat > "$WS_FIX/.agents/memory/README.md" <<'EOF'
# 主机记忆

[INDEX.md](INDEX.md) | [host-profile.md](host-profile.md)
EOF
cat > "$WS_FIX/.agents/memory/host-profile.md" <<'EOF'
# 主机画像

- 更新时间：2026-11-22
EOF
printf '{"ts":"2026-11-22T00:00:00Z","action":"init","status":"ok"}\n' > "$WS_FIX/.agents/logs/audit.jsonl"
p_cred_val="ghp""_9876543210abcdefghij987654"
printf '%s\n' "$p_cred_val" > "$WS_FIX/.agents/credentials/api.cred"
chmod 0700 "$WS_FIX/.agents/credentials" "$WS_FIX/.agents/checkpoints" "$WS_FIX/.agents/logs" "$WS_FIX/.agents/memory"
chmod 0600 "$WS_FIX/.agents/credentials/api.cred" "$WS_FIX/.agents/logs/audit.jsonl" "$WS_FIX/MEMORY.md" "$WS_FIX/.agents/memory/host-profile.md"

expect_fmt_pass "provisioned VPS workspace passes verify_agent_fmt --root" "$WS_FIX"
if python3 -B "$HYG_TOOL" --root "$WS_FIX" --check-workspace-perms >/dev/null 2>&1; then
    pass "provisioned VPS workspace passes verify_repo_hygiene --check-workspace-perms"
else
    python3 -B "$HYG_TOOL" --root "$WS_FIX" --check-workspace-perms || true
    fail "provisioned VPS workspace passes verify_repo_hygiene --check-workspace-perms"
fi

# Broken .agents/memory README->INDEX link rejected
printf '# 主机记忆\n\n缺少双向链接。\n' > "$WS_FIX/.agents/memory/README.md"
expect_fmt_fail "VPS workspace missing .agents/memory README->INDEX link rejected" "$WS_FIX" "missing bidirectional link.*\.agents/memory/INDEX\.md"
printf '# 主机记忆\n\n[INDEX.md](INDEX.md) | [host-profile.md](host-profile.md)\n' > "$WS_FIX/.agents/memory/README.md"

# Lax perms in --check-workspace-perms rejected
chmod 0755 "$WS_FIX/.agents/credentials"
chmod 0644 "$WS_FIX/.agents/credentials/api.cred"
chmod 0644 "$WS_FIX/MEMORY.md"
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$WS_FIX" --check-workspace-perms 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] \
    && grep -q ".agents/credentials: expected directory mode 0o700" <<< "$hyg_out" \
    && grep -q ".agents/credentials/api.cred: expected credential file mode 0o600" <<< "$hyg_out" \
    && grep -q "MEMORY.md: expected file mode 0o600" <<< "$hyg_out"; then
    pass "verify_repo_hygiene --check-workspace-perms rejects lax 0755/0644 perms"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene --check-workspace-perms rejects lax 0755/0644 perms"
fi
chmod 0700 "$WS_FIX/.agents/credentials"
chmod 0600 "$WS_FIX/.agents/credentials/api.cred" "$WS_FIX/MEMORY.md"

# Secret leaked into MEMORY.md caught even with --check-workspace-perms
printf '\ntoken=%s\n' "$p_cred_val" >> "$WS_FIX/MEMORY.md"
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$WS_FIX" --check-workspace-perms 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] && grep -q "MEMORY.md:.*github_token" <<< "$hyg_out"; then
    pass "verify_repo_hygiene --check-workspace-perms catches secret leaked into MEMORY.md"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene --check-workspace-perms catches secret leaked into MEMORY.md"
fi

# 19. U29 forbidden credential files, new secret patterns, and systemd template hygiene
seed_valid_repo "$FIX"
mkdir -p "$FIX/templates/apps/lego"
printf 'CF_DNS_API_TOKEN={{CF_TOKEN}}\n' > "$FIX/templates/apps/lego/lego.env"
if python3 -B "$HYG_TOOL" --root "$FIX" >/dev/null 2>&1; then
    pass "U29 allows templates/apps/lego/lego.env without false positive"
else
    python3 -B "$HYG_TOOL" --root "$FIX" || true
    fail "U29 allows templates/apps/lego/lego.env without false positive"
fi

for bad_cred in ".env" ".env.prod" "token.cred" "id_ed25519" "secrets.yaml"; do
    touch "$FIX/$bad_cred"
    hyg_rc=0
    hyg_out="$(python3 -B "$HYG_TOOL" --root "$FIX" 2>&1)" || hyg_rc=$?
    if [[ "$hyg_rc" -ne 0 ]] && grep -q "forbidden credential file" <<< "$hyg_out"; then
        pass "U29 rejects forbidden credential file $bad_cred"
    else
        echo "  output: $hyg_out"
        fail "U29 rejects forbidden credential file $bad_cred"
    fi
    rm -f "$FIX/$bad_cred"
done

# U29 new secret patterns (all runtime-assembled)
p_cf="CLOUDFLARE""_DNS_API_TOKEN=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789"
p_age="AGE-SECRET""-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ"
p_tg="123456789"":AAAbCdEfGhIjKlMnOpQrStUvWxYz0123456"
p_st="rk""_live_AbCdEfGhIjKlMnOpQrStUvWx"
p_br="Authorization: Bear""er AbCdEfGhIjKlMnOpQrStUvWxYz"
cat > "$FIX/u29_leaks.txt" <<EOF
${p_cf}
${p_age}
${p_tg}
${p_st}
${p_br}
EOF
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$FIX" 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] \
    && grep -q "cloudflare_api_token" <<< "$hyg_out" \
    && grep -q "age_secret_key" <<< "$hyg_out" \
    && grep -q "telegram_bot_token" <<< "$hyg_out" \
    && grep -q "stripe_live_key" <<< "$hyg_out" \
    && grep -q "bearer_token_literal" <<< "$hyg_out"; then
    pass "U29 catches all 5 new secret patterns"
else
    echo "  output: $hyg_out"
    fail "U29 catches all 5 new secret patterns"
fi
rm -f "$FIX/u29_leaks.txt"

cat > "$FIX/u29_benign.txt" <<'EOF'
CLOUDFLARE_API_TOKEN={{CF_TOKEN}}
Authorization: Bearer [REDACTED]
Authorization: Bearer ${API_TOKEN}
EOF
if python3 -B "$HYG_TOOL" --root "$FIX" >/dev/null 2>&1; then
    pass "U29 has 0 false positives on redacted/placeholder tokens"
else
    python3 -B "$HYG_TOOL" --root "$FIX" || true
    fail "U29 has 0 false positives on redacted/placeholder tokens"
fi
rm -f "$FIX/u29_benign.txt"

# U29 systemd template credential hygiene
mkdir -p "$FIX/templates/test"
cat > "$FIX/templates/test/good.service" <<'EOF'
[Service]
LoadCredentialEncrypted=api_token:/var/lib/debopty/agent/.agents/credentials/api_token.cred
Environment="API_KEY={{API_KEY_PLACEHOLDER}}"
EOF
if python3 -B "$HYG_TOOL" --root "$FIX" >/dev/null 2>&1; then
    pass "U29 systemd template check allows LoadCredentialEncrypted and placeholder Environment"
else
    python3 -B "$HYG_TOOL" --root "$FIX" || true
    fail "U29 systemd template check allows LoadCredentialEncrypted and placeholder Environment"
fi

cat > "$FIX/templates/test/bad.service" <<'EOF'
[Service]
SetCredential=db_pass:hardcoded_literal_val
Environment="MY_SECRET=hardcoded_secret_val"
EOF
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$FIX" 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] \
    && grep -q "plaintext SetCredential literal" <<< "$hyg_out" \
    && grep -q "plaintext secret in systemd Environment=MY_SECRET" <<< "$hyg_out"; then
    pass "U29 systemd template check rejects inline SetCredential and Environment secrets"
else
    echo "  output: $hyg_out"
    fail "U29 systemd template check rejects inline SetCredential and Environment secrets"
fi
rm -rf "$FIX/templates/test"

# 20. v2 VPS workspace validation (.agents/MEMORY.md, sub-sharded memory, schemas, overrides, docs/agent/ hub + decisions.md, 90_project_legacy_ssot, v2 perms)
WS2="${TMP}/vps_workspace_v2"
mkdir -p \
    "$WS2/docs/agent/constraints" \
    "$WS2/.agents/memory/security" \
    "$WS2/.agents/memory/archive" \
    "$WS2/.agents/schemas" \
    "$WS2/.agents/overrides" \
    "$WS2/.agents/logs" \
    "$WS2/.agents/checkpoints" \
    "$WS2/.agents/credentials" \
    "$WS2/.agents/cache" \
    "$WS2/.agents/tmp"

cat > "$WS2/AGENTS.md" <<'EOF'
---
name: vps-agent-v2-root
description: VPS agent v2 root entrypoint.
zh_summary: VPS v2 工作环境总入口。
tags: [vps, agent, v2]
index: [.agents/INDEX.md, docs/agent/INDEX.md, docs/agent/CONSTRAINTS.md, .agents/MEMORY.md]
---

<guide>
<!-- managed-by-debopty:start -->
WORKSPACE:
  role: vps-admin | schema-v2
<!-- managed-by-debopty:end -->
</guide>
EOF

cat > "$WS2/README.md" <<'EOF'
# VPS Agent v2 工作环境

[AGENTS.md](AGENTS.md) | [docs/agent/README.md](docs/agent/README.md) | [.agents/README.md](.agents/README.md) | [.agents/MEMORY.md](.agents/MEMORY.md)
EOF

cat > "$WS2/.agents/INDEX.md" <<'EOF'
---
name: vps-v2-agents-index
description: VPS .agents v2 router.
zh_summary: VPS v2 `.agents/` 路由索引。
tags: [vps, index]
index: [AGENTS.md, ./README.md, docs/agent/INDEX.md, docs/agent/CONSTRAINTS.md, .agents/MEMORY.md, docs/agent/constraints/INDEX.md, .agents/memory/INDEX.md, .agents/schemas/INDEX.md, .agents/overrides/INDEX.md]
---

<index>
DIR:
  path: .agents
</index>
EOF

cat > "$WS2/.agents/README.md" <<'EOF'
# `.agents/` 目录说明

[INDEX.md](INDEX.md) | [MEMORY.md](MEMORY.md) | [docs/agent](../docs/agent/README.md)
EOF

cat > "$WS2/docs/agent/INDEX.md" <<'EOF'
---
name: vps-v2-docs-agent-index
description: VPS docs/agent control hub router.
zh_summary: VPS v2 `docs/agent/` 治理控制中心路由。
tags: [vps, index, governance]
index: [AGENTS.md, ./README.md, ./CONSTRAINTS.md, ./decisions.md, ./constraints/INDEX.md]
---

<index>
DIR:
  path: docs/agent
</index>
EOF

cat > "$WS2/docs/agent/README.md" <<'EOF'
# `docs/agent/` 治理控制中心

[INDEX.md](INDEX.md) | [CONSTRAINTS.md](CONSTRAINTS.md) | [decisions.md](decisions.md) | [constraints](constraints/README.md)
EOF

cat > "$WS2/docs/agent/decisions.md" <<'EOF'
# 架构决策台账（ADR）

[README.md](README.md) | [constraints](constraints/README.md)

- `2026-11-24`：初始化 `docs/agent/` 控制中心布局。
EOF

cat > "$WS2/docs/agent/CONSTRAINTS.md" <<'EOF'
---
name: vps-v2-constraints
description: VPS v2 constraints index.
zh_summary: VPS v2 约束总索引。
tags: [vps, constraints]
index: [docs/agent/constraints/INDEX.md, docs/agent/constraints/README.md]
---

<constraints_index>
ROUTER:
  dir: docs/agent/constraints/
</constraints_index>
EOF

cat > "$WS2/.agents/MEMORY.md" <<'EOF'
# 运维记忆目录

<!-- managed-by-debopty:start -->
- [firewall-ports](memory/security/firewall-ports.md)
<!-- managed-by-debopty:end -->
EOF

cat > "$WS2/.agents/workspace.json" <<'EOF'
{"schema_version":"2","wire_epoch":"auto-swarm/v1","memory_tracking":"local"}
EOF

cat > "$WS2/docs/agent/constraints/INDEX.md" <<'EOF'
---
name: vps-v2-constraints-router
description: VPS v2 constraints router.
zh_summary: VPS v2 约束目录路由。
tags: [vps, index]
index: [./README.md, ./00_vps_general_and_docs.md]
---

<index>
LAYERS:
  general: 00_vps_general_and_docs.md
</index>
EOF

cat > "$WS2/docs/agent/constraints/README.md" <<'EOF'
# VPS v2 约束目录

[INDEX.md](INDEX.md) | [00_vps_general_and_docs.md](00_vps_general_and_docs.md)
EOF

cat > "$WS2/docs/agent/constraints/00_vps_general_and_docs.md" <<'EOF'
# 00 通用与文档规范

遵循双受众、平台中立与渐进式披露。
EOF

cat > "$WS2/docs/agent/constraints/90_project_legacy_ssot.md" <<'EOF'
# 90 遗留项目规则归档

<!-- managed-by-debopty:start -->
```markdown
<!-- legacy-source-sha256:0000000000000000000000000000000000000000000000000000000000000000 -->
Historical rule written on 2024-05-01.
```
<!-- managed-by-debopty:end -->
EOF

cat > "$WS2/.agents/memory/INDEX.md" <<'EOF'
---
name: vps-v2-memory-index
description: VPS v2 memory router.
zh_summary: VPS v2 记忆目录路由。
tags: [vps, memory, index]
index: [./README.md, ./host-profile.md]
---

<index>
MEMORY:
  profile: host-profile.md
</index>
EOF

cat > "$WS2/.agents/memory/README.md" <<'EOF'
# 主机记忆

[INDEX.md](INDEX.md) | [host-profile.md](host-profile.md)
EOF

cat > "$WS2/.agents/memory/host-profile.md" <<'EOF'
# 主机画像

- 更新时间：2026-11-24
EOF

cat > "$WS2/.agents/memory/security/firewall-ports.md" <<'EOF'
---
id: firewall-ports
title: 防火墙放行策略
type: procedural
status: active
scope: security
created_at: 2026-11-24T00:00:00Z
updated_at: 2026-11-24T00:00:00Z
tags: [firewall]
sources: [cli:agent-memory]
confidence: 0.9
---

# 防火墙放行策略

仅放行必需公网端口。
EOF

cat > "$WS2/.agents/schemas/INDEX.md" <<'EOF'
---
name: vps-v2-schemas-index
description: VPS v2 schemas router.
zh_summary: VPS v2 Schema 契约目录路由。
tags: [vps, schemas, index]
index: [./README.md, ./repo_blueprint.json]
---

<index>
SCHEMAS:
  blueprint: repo_blueprint.json
</index>
EOF

cat > "$WS2/.agents/schemas/README.md" <<'EOF'
# Schema 契约

[INDEX.md](INDEX.md)
EOF

cat > "$WS2/.agents/overrides/INDEX.md" <<'EOF'
---
name: vps-v2-overrides-index
description: VPS v2 overrides router.
zh_summary: VPS v2 覆盖槽位目录路由。
tags: [vps, overrides, index]
index: [./README.md, ./_template.md]
---

<index>
OVERRIDES:
  template: _template.md
</index>
EOF

cat > "$WS2/.agents/overrides/README.md" <<'EOF'
# 覆盖槽位说明

[INDEX.md](INDEX.md) | [_template.md](_template.md)
EOF

cat > "$WS2/.agents/overrides/_template.md" <<'EOF'
# Harness 覆盖模板

按需声明独立覆盖参数。
EOF

v2_shim_name="CLAUDE"".md"
cat > "$WS2/.agents/schemas/repo_blueprint.json" <<EOF
{"blueprint_schema_version":2,"managed_projections":{"${v2_shim_name}":{"sha256":"deadbeef","ref_count":1}}}
EOF

cat > "$WS2/${v2_shim_name}" <<'EOF'
---
name: managed-shim
description: Managed harness shim.
zh_summary: 托管适配垫片。
tags: [entrypoint]
index: [AGENTS.md]
---

<guide>
ENTRYPOINT:
  target: AGENTS.md
</guide>
EOF

cat > "$WS2/.agents/overrides/custom.md" <<EOF
# 自定义覆盖

允许在覆盖槽位提及 ${v2_shim_name}。
EOF

printf '{"ts":"2026-11-24T00:00:00Z","action":"init","status":"ok"}\n' > "$WS2/.agents/logs/audit.jsonl"
printf '{"version":1,"docs":{}}\n' > "$WS2/.agents/cache/memory_index.json"
chmod 0700 \
    "$WS2/.agents/credentials" \
    "$WS2/.agents/checkpoints" \
    "$WS2/.agents/logs" \
    "$WS2/.agents/memory" \
    "$WS2/.agents/memory/archive" \
    "$WS2/.agents/cache" \
    "$WS2/.agents/tmp"
chmod 0600 \
    "$WS2/.agents/MEMORY.md" \
    "$WS2/.agents/workspace.json" \
    "$WS2/.agents/logs/audit.jsonl" \
    "$WS2/.agents/memory/host-profile.md" \
    "$WS2/.agents/memory/security/firewall-ports.md" \
    "$WS2/.agents/cache/memory_index.json"

expect_fmt_pass "v2 VPS workspace passes verify_agent_fmt (incl. 90_project_legacy_ssot fenced date)" "$WS2"
if python3 -B "$HYG_TOOL" --root "$WS2" --check-workspace-perms >/dev/null 2>&1; then
    pass "v2 VPS workspace passes verify_repo_hygiene --check-workspace-perms"
else
    python3 -B "$HYG_TOOL" --root "$WS2" --check-workspace-perms || true
    fail "v2 VPS workspace passes verify_repo_hygiene --check-workspace-perms"
fi

# Unfenced date in 90_project_legacy_ssot.md must still be rejected
printf '\n手工日期：2026-11-24\n' >> "$WS2/docs/agent/constraints/90_project_legacy_ssot.md"
expect_fmt_fail "unfenced ISO date in 90_project_legacy_ssot.md rejected" "$WS2" "90_project_legacy_ssot.md:.*date '2026-11-24' forbidden"
sed -i '/手工日期：2026-11-24/d' "$WS2/docs/agent/constraints/90_project_legacy_ssot.md"

# Canonical constraint date still rejected
printf '\n日期：2026-11-24\n' >> "$WS2/docs/agent/constraints/00_vps_general_and_docs.md"
expect_fmt_fail "ISO date in canonical v2 constraint rejected" "$WS2" "00_vps_general_and_docs.md:.*date '2026-11-24' forbidden"
sed -i '/日期：2026-11-24/d' "$WS2/docs/agent/constraints/00_vps_general_and_docs.md"

# Lax v2 perms on .agents/MEMORY.md, .agents/memory/, topic shard, workspace.json, cache, tmp rejected
chmod 0644 "$WS2/.agents/MEMORY.md" "$WS2/.agents/workspace.json" "$WS2/.agents/memory/security/firewall-ports.md" "$WS2/.agents/cache/memory_index.json"
chmod 0755 "$WS2/.agents/memory" "$WS2/.agents/cache" "$WS2/.agents/tmp"
hyg_rc=0
hyg_out="$(python3 -B "$HYG_TOOL" --root "$WS2" --check-workspace-perms 2>&1)" || hyg_rc=$?
if [[ "$hyg_rc" -ne 0 ]] \
    && grep -q ".agents/MEMORY.md: expected file mode 0o600" <<< "$hyg_out" \
    && grep -q ".agents/workspace.json: expected file mode 0o600" <<< "$hyg_out" \
    && grep -q ".agents/memory/security/firewall-ports.md: expected memory file mode 0o600" <<< "$hyg_out" \
    && grep -q ".agents/cache/memory_index.json: expected file mode 0o600" <<< "$hyg_out" \
    && grep -q ".agents/memory: expected directory mode 0o700" <<< "$hyg_out" \
    && grep -q ".agents/cache: expected directory mode 0o700" <<< "$hyg_out" \
    && grep -q ".agents/tmp: expected directory mode 0o700" <<< "$hyg_out"; then
    pass "verify_repo_hygiene --check-workspace-perms rejects lax v2 0755/0644 perms"
else
    echo "  output: $hyg_out"
    fail "verify_repo_hygiene --check-workspace-perms rejects lax v2 0755/0644 perms"
fi

# 21. Live-repo assertion
if python3 -B "$FMT_TOOL" >/dev/null 2>&1; then
    pass "live repo passes verify_agent_fmt"
else
    python3 -B "$FMT_TOOL" || true
    fail "live repo passes verify_agent_fmt"
fi

echo
echo "agent-fmt-test: ${PASS} passed, ${FAIL} failed"
[[ "$FAIL" -eq 0 ]]
