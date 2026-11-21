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
    mkdir -p "$d/governance/constraints" "$d/tests"

    cat > "$d/AGENTS.md" <<'EOF'
---
name: sample-agents
description: Sample root entrypoint.
zh_summary: 示例根目录总入口规范。
tags: [entrypoint, governance]
index: [governance/CONSTRAINTS.md, governance/constraints/INDEX.md]
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

    cat > "$d/governance/CONSTRAINTS.md" <<'EOF'
---
name: sample-constraints
description: Constraints router.
zh_summary: 约束总路由索引。
tags: [governance]
index: [governance/constraints/INDEX.md, governance/constraints/README.md]
---

<constraints_index>
ROUTER:
  general: governance/constraints/00_general.md
</constraints_index>
EOF

    cat > "$d/governance/constraints/INDEX.md" <<'EOF'
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

    cat > "$d/governance/constraints/README.md" <<'EOF'
# 约束目录

Agent 路由：[INDEX.md](INDEX.md) | 通用规则：[00_general.md](00_general.md)
EOF

    cat > "$d/governance/constraints/00_general.md" <<'EOF'
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
sed -i 's|governance/CONSTRAINTS.md|governance/nope.md|' "$FIX/AGENTS.md"
expect_fmt_fail "missing index path rejected" "$FIX" "missing on disk.*governance/nope.md"

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
index: [governance/CONSTRAINTS.md]
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
index: [governance/CONSTRAINTS.md]
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
printf '\n<guide>\n' >> "$FIX/governance/constraints/00_general.md"
expect_fmt_fail "agent XML tag in human doc rejected" "$FIX" "agent XML boundary tag forbidden"

# 9. Missing bidirectional link (governance/constraints/README.md -> INDEX.md) rejected
seed_valid_repo "$FIX"
cat > "$FIX/governance/constraints/README.md" <<'EOF'
# 约束目录

仅链接：[00_general.md](00_general.md)
EOF
expect_fmt_fail "missing bidirectional governance/constraints README->INDEX link rejected" "$FIX" "missing bidirectional link"

# 10. ISO date in governance/constraints/00_general.md rejected
seed_valid_repo "$FIX"
printf '\n更新日期：2026-11-21\n' >> "$FIX/governance/constraints/00_general.md"
expect_fmt_fail "ISO date in constraint file rejected" "$FIX" "date '2026-11-21' forbidden"

# 11. Banned separator in governance/constraints/00_general.md rejected
seed_valid_repo "$FIX"
printf '\n规则 A · 规则 B\n' >> "$FIX/governance/constraints/00_general.md"
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

# 17. Live-repo assertion
if python3 -B "$FMT_TOOL" >/dev/null 2>&1; then
    pass "live repo passes verify_agent_fmt"
else
    python3 -B "$FMT_TOOL" || true
    fail "live repo passes verify_agent_fmt"
fi

echo
echo "agent-fmt-test: ${PASS} passed, ${FAIL} failed"
[[ "$FAIL" -eq 0 ]]
