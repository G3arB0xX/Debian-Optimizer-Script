#!/usr/bin/env bash
# Machine verification gate (`make check`).
# Runs syntax, shellcheck, host unit tests, agent/doc format, repo hygiene, release verify, and Rust checks.
# Supports optional `--slice <path>` (repeatable/comma-separated) or `DEBOPTY_SLICE`.
set -uo pipefail
shopt -s nullglob
export PYTHONDONTWRITEBYTECODE=1

cd "$(dirname "$0")/.." || exit 1

strict_shellcheck=0
strict_rust=0
check_commits=0
check_commits_range=""
jobs="${DEBOPTY_JOBS:-auto}"
log_dir="${DEBOPTY_LOG_DIR:-${TMPDIR:-/tmp}/debopty_logs}"
mkdir -p "$log_dir"

declare -a slice_paths=()
cli_slice=0
_add_slice_arg() {
    local raw="$1" item
    local -a parts=()
    IFS=',' read -r -a parts <<< "$raw"
    for item in "${parts[@]}"; do
        item="${item#./}"
        item="${item%/}"
        [[ -n "$item" ]] && slice_paths+=("$item")
    done
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --strict-shellcheck) strict_shellcheck=1; shift ;;
        --strict-rust) strict_rust=1; shift ;;
        --check-commits)
            check_commits=1
            if [[ $# -ge 2 && "${2:-}" != --* ]]; then
                check_commits_range="$2"
                shift 2
            else
                shift
            fi
            ;;
        --check-commits=*)
            check_commits=1
            check_commits_range="${1#*=}"
            shift
            ;;
        --jobs) jobs="${2:-auto}"; shift 2 ;;
        --jobs=*) jobs="${1#*=}"; shift ;;
        --slice) cli_slice=1; _add_slice_arg "${2:-}"; shift 2 ;;
        --slice=*) cli_slice=1; _add_slice_arg "${1#*=}"; shift ;;
        *) shift ;;
    esac
done

if [[ "$cli_slice" -eq 0 && -n "${DEBOPTY_SLICE:-}" ]]; then
    _add_slice_arg "$DEBOPTY_SLICE"
fi

mode="full"
if [[ "${#slice_paths[@]}" -gt 0 ]]; then
    mode="slice"
fi

_path_in_slice() {
    local candidate="${1#./}" s
    for s in "${slice_paths[@]}"; do
        if [[ "$candidate" == "$s" || "$candidate" == "$s/"* || "$s" == "$candidate/"* ]]; then
            return 0
        fi
    done
    return 1
}

fail=0
step() {
    echo
    echo "== $1"
}

tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/debopty_check_XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT

run_syntax() {
    local -a dirs=() files=() py_files=()
    local d f rc=0
    if [[ "$mode" == "slice" ]]; then
        for d in "${slice_paths[@]}"; do
            if [[ -d "$d" ]]; then
                while IFS= read -r -d '' f; do
                    files+=("$f")
                done < <(find "$d" -type d -name target -prune -o -type f -name '*.sh' -print0 | sort -z)
                while IFS= read -r -d '' f; do
                    py_files+=("$f")
                done < <(find "$d" -type d -name target -prune -o -type f -name '*.py' -print0 | sort -z)
            elif [[ -f "$d" ]]; then
                case "$d" in
                    *.sh) files+=("$d") ;;
                    *.py) py_files+=("$d") ;;
                esac
            fi
        done
    else
        for d in install.sh archive/legacy-bash templates tools tests; do
            [[ -e "$d" ]] && dirs+=("$d")
        done
        if [[ "${#dirs[@]}" -gt 0 ]]; then
            while IFS= read -r -d '' f; do
                files+=("$f")
            done < <(find "${dirs[@]}" -type f -name '*.sh' -print0 | sort -z)
        fi
        for f in tools/*.py; do
            [[ -f "$f" ]] && py_files+=("$f")
        done
    fi
    for f in "${files[@]}"; do
        if ! bash -n "$f"; then
            rc=1
        fi
    done
    if [[ "${#py_files[@]}" -gt 0 ]]; then
        if ! PYTHONPYCACHEPREFIX="$tmpdir/pycache" python3 -B -m py_compile "${py_files[@]}"; then
            rc=1
        fi
    fi
    if [[ "${#files[@]}" -eq 0 && "${#py_files[@]}" -eq 0 ]]; then
        echo "SKIP: no .sh or .py targets (${mode})"
        return 0
    fi
    if [[ "$rc" -eq 0 ]]; then
        echo "syntax: PASS (${#files[@]} .sh, ${#py_files[@]} .py)"
    else
        echo "syntax: FAIL"
    fi
    return "$rc"
}

run_shellcheck() {
    local -a all_sc=() sc_files=()
    local f
    for f in install.sh archive/legacy-bash/deb_optimizer.sh archive/legacy-bash/scripts/*.sh archive/legacy-bash/scripts/apps/*.sh tools/*.sh; do
        [[ -f "$f" ]] && all_sc+=("$f")
    done
    if [[ "$mode" == "slice" ]]; then
        for f in "${all_sc[@]}"; do
            if _path_in_slice "$f"; then
                sc_files+=("$f")
            fi
        done
    else
        sc_files=("${all_sc[@]}")
    fi
    if ! command -v shellcheck >/dev/null 2>&1; then
        echo "SKIP (shellcheck not installed)"
        return 0
    fi
    if [[ "${#sc_files[@]}" -eq 0 ]]; then
        echo "SKIP: no target shell scripts (${mode})"
        return 0
    fi
    shellcheck "${sc_files[@]}"
}

run_unit_tests() {
    if [[ ! -d tests ]]; then
        echo "SKIP: tests/ directory not present"
        return 0
    fi
    local -a fast_host_tests=(
        "tests/version-test.sh"
        "tests/release-test.sh"
        "tests/install-bootstrap-test.sh"
        "tests/tmux-test.sh"
        "tests/fail2ban-sync-test.sh"
        "tests/agent-fmt-test.sh"
    )
    local -a selected=()
    local t s ran=0 rc=0
    if [[ "$mode" == "slice" ]]; then
        declare -A seen=()
        _select_test() {
            local cand="$1"
            if [[ -f "$cand" && -z "${seen[$cand]:-}" ]]; then
                seen["$cand"]=1
                selected+=("$cand")
            fi
        }
        for s in "${slice_paths[@]}"; do
            case "$s" in
                tests/rust-parity-test.sh) ;;
                tests/*-test.sh) _select_test "$s" ;;
                tests|tests/*)
                    for t in "${fast_host_tests[@]}"; do _select_test "$t"; done
                    ;;
                scripts/version.sh|archive/legacy-bash/scripts/version.sh|VERSION|VERSION_ID)
                    _select_test "tests/version-test.sh"
                    ;;
                tools/release.sh|tools/release_helper.py|CHANGELOG.md)
                    _select_test "tests/release-test.sh"
                    ;;
                install.sh|deb_optimizer.sh|archive/legacy-bash/deb_optimizer.sh|.github/workflows/release.yml)
                    _select_test "tests/install-bootstrap-test.sh"
                    ;;
                scripts/security.sh|archive/legacy-bash/scripts/security.sh|templates/security|templates/security/*)
                    _select_test "tests/fail2ban-sync-test.sh"
                    ;;
                scripts/system.sh|archive/legacy-bash/scripts/system.sh|templates/system|templates/system/*)
                    _select_test "tests/tmux-test.sh"
                    ;;
                tools/verify_agent_fmt.py|tools/verify_repo_hygiene.py|tools/check.sh)
                    _select_test "tests/agent-fmt-test.sh"
                    ;;
            esac
        done
    else
        selected=("${fast_host_tests[@]}")
    fi
    for t in "${selected[@]}"; do
        if [[ -f "$t" ]]; then
            echo "-- running $t"
            if ! bash "$t"; then
                rc=1
            fi
            ran=$((ran + 1))
        fi
    done
    if [[ "$ran" -eq 0 ]]; then
        echo "SKIP: no mapped fast host unit tests (${mode})"
        return 0
    fi
    return "$rc"
}

run_rust_parity() {
    local rust_parity_test="tests/rust-parity-test.sh"
    if [[ ! -f "$rust_parity_test" ]]; then
        return 0
    fi
    if [[ "$mode" == "slice" ]]; then
        local run_it=0 s
        for s in "${slice_paths[@]}"; do
            case "$s" in
                tests|tests/rust-parity-test.sh|rust|rust/*|templates|templates/*|VERSION)
                    run_it=1
                    break
                    ;;
            esac
        done
        if [[ "$run_it" -eq 0 ]]; then
            return 0
        fi
    fi
    echo "-- running ${rust_parity_test}"
    bash "$rust_parity_test"
}

run_agent_fmt() {
    if [[ "$mode" == "slice" ]]; then
        local run_it=0 s
        for s in "${slice_paths[@]}"; do
            case "$s" in
                *.md|governance|governance/*|docs|docs/*|tests|tests/*|rust|rust/*|archive|archive/*|tools/verify_agent_fmt.py|Makefile)
                    run_it=1
                    break
                    ;;
            esac
        done
        if [[ "$run_it" -eq 0 ]]; then
            echo "SKIP: no agent_fmt targets (${mode})"
            return 0
        fi
    fi
    python3 -B tools/verify_agent_fmt.py
}

run_hygiene() {
    python3 -B tools/verify_repo_hygiene.py
}

run_release() {
    if [[ "$mode" == "slice" ]]; then
        local run_it=0 s
        for s in "${slice_paths[@]}"; do
            case "$s" in
                VERSION|VERSION_ID|CHANGELOG.md|tools/release.sh|tools/release_helper.py|scripts/version.sh|archive/legacy-bash/scripts/version.sh|archive/legacy-bash/scripts/common.sh)
                    run_it=1
                    break
                    ;;
            esac
        done
        if [[ "$run_it" -eq 0 ]]; then
            echo "SKIP: no release targets (${mode})"
            return 0
        fi
    fi
    if [[ "$check_commits" -eq 1 ]]; then
        if [[ -n "$check_commits_range" ]]; then
            bash tools/release.sh verify --check-commits "$check_commits_range"
        else
            bash tools/release.sh verify --check-commits
        fi
    else
        bash tools/release.sh verify
    fi
}

run_rust() {
    if [[ ! -f rust/Cargo.toml ]]; then
        echo "SKIP: rust/Cargo.toml not present"
        return 0
    fi
    if [[ "$mode" == "slice" ]]; then
        local run_it=0 s
        for s in "${slice_paths[@]}"; do
            case "$s" in
                rust|rust/*|templates|templates/*|VERSION)
                    run_it=1
                    break
                    ;;
            esac
        done
        if [[ "$run_it" -eq 0 ]]; then
            echo "SKIP: no rust targets (${mode})"
            return 0
        fi
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        if [[ "$strict_rust" -eq 1 || "${CI:-}" == "true" ]]; then
            echo "FAIL: cargo not installed (required by --strict-rust / CI=true)"
            return 1
        fi
        echo "SKIP (cargo not installed)"
        return 0
    fi

    local -a offline_args=()
    local host_triple=""
    if command -v rustc >/dev/null 2>&1; then
        host_triple="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
    fi
    if [[ "${DEBOPTY_CARGO_OFFLINE:-0}" == "1" ]] || {
        [[ -n "$host_triple" ]] && cargo metadata --manifest-path rust/Cargo.toml --locked --offline --filter-platform "$host_triple" --format-version 1 >/dev/null 2>&1
    } || cargo metadata --manifest-path rust/Cargo.toml --locked --offline --format-version 1 >/dev/null 2>&1; then
        offline_args=(--offline)
    fi

    local rc=0
    echo "-- cargo fmt --check"
    if ! cargo fmt --manifest-path rust/Cargo.toml --all --check; then
        rc=1
    fi
    echo "-- cargo clippy ratatui-ui (${offline_args[*]:-online})"
    if ! cargo clippy --manifest-path rust/Cargo.toml --locked "${offline_args[@]}" --features ratatui-ui --all-targets -- -D warnings; then
        rc=1
    fi
    echo "-- cargo test ratatui-ui (${offline_args[*]:-online})"
    if ! cargo test --manifest-path rust/Cargo.toml --locked "${offline_args[@]}" --features ratatui-ui; then
        rc=1
    fi
    echo "-- cargo clippy default (${offline_args[*]:-online})"
    if ! cargo clippy --manifest-path rust/Cargo.toml --locked "${offline_args[@]}" --all-targets -- -D warnings; then
        rc=1
    fi
    echo "-- cargo test default (${offline_args[*]:-online})"
    if ! cargo test --manifest-path rust/Cargo.toml --locked "${offline_args[@]}"; then
        rc=1
    fi
    if [[ "$rc" -eq 0 ]]; then
        echo "rust: PASS"
    else
        echo "rust: FAIL"
    fi
    return "$rc"
}

if [[ "$jobs" == "1" ]]; then
    run_syntax > "$tmpdir/syntax.out" 2>&1
    echo $? > "$tmpdir/syntax.rc"

    run_shellcheck > "$tmpdir/shellcheck.out" 2>&1
    echo $? > "$tmpdir/shellcheck.rc"

    run_unit_tests > "$tmpdir/unit_tests.out" 2>&1
    echo $? > "$tmpdir/unit_tests.rc"

    run_agent_fmt > "$tmpdir/agent_fmt.out" 2>&1
    echo $? > "$tmpdir/agent_fmt.rc"

    run_hygiene > "$tmpdir/hygiene.out" 2>&1
    echo $? > "$tmpdir/hygiene.rc"

    run_release > "$tmpdir/release.out" 2>&1
    echo $? > "$tmpdir/release.rc"

    run_rust > "$tmpdir/rust.out" 2>&1
    echo $? > "$tmpdir/rust.rc"

    run_rust_parity >> "$tmpdir/unit_tests.out" 2>&1
    parity_rc=$?
    if [[ "$parity_rc" -ne 0 ]]; then
        echo 1 > "$tmpdir/unit_tests.rc"
    fi
else
    (
        run_syntax > "$tmpdir/syntax.out" 2>&1
        echo $? > "$tmpdir/syntax.rc"
    ) &
    pid_syntax=$!

    (
        run_shellcheck > "$tmpdir/shellcheck.out" 2>&1
        echo $? > "$tmpdir/shellcheck.rc"
    ) &
    pid_shellcheck=$!

    (
        run_unit_tests > "$tmpdir/unit_tests.out" 2>&1
        echo $? > "$tmpdir/unit_tests.rc"
    ) &
    pid_unit_tests=$!

    (
        run_agent_fmt > "$tmpdir/agent_fmt.out" 2>&1
        echo $? > "$tmpdir/agent_fmt.rc"
    ) &
    pid_agent_fmt=$!

    (
        run_hygiene > "$tmpdir/hygiene.out" 2>&1
        echo $? > "$tmpdir/hygiene.rc"
    ) &
    pid_hygiene=$!

    (
        run_release > "$tmpdir/release.out" 2>&1
        echo $? > "$tmpdir/release.rc"
    ) &
    pid_release=$!

    (
        run_rust > "$tmpdir/rust.out" 2>&1
        echo $? > "$tmpdir/rust.rc"
    ) &
    pid_rust=$!

    wait "$pid_syntax" 2>/dev/null || true
    wait "$pid_shellcheck" 2>/dev/null || true
    wait "$pid_unit_tests" 2>/dev/null || true
    wait "$pid_agent_fmt" 2>/dev/null || true
    wait "$pid_hygiene" 2>/dev/null || true
    wait "$pid_release" 2>/dev/null || true
    wait "$pid_rust" 2>/dev/null || true

    # Run rust-parity-test.sh strictly after run_rust finishes to avoid Cargo rust/target race.
    run_rust_parity >> "$tmpdir/unit_tests.out" 2>&1
    parity_rc=$?
    if [[ "$parity_rc" -ne 0 ]]; then
        echo 1 > "$tmpdir/unit_tests.rc"
    fi
fi

for name in syntax shellcheck unit_tests agent_fmt hygiene release rust; do
    cp "$tmpdir/${name}.out" "$log_dir/${name}.log" 2>/dev/null || true
done

syntax_rc=$(cat "$tmpdir/syntax.rc" 2>/dev/null || echo 1)
shellcheck_rc=$(cat "$tmpdir/shellcheck.rc" 2>/dev/null || echo 1)
unit_tests_rc=$(cat "$tmpdir/unit_tests.rc" 2>/dev/null || echo 1)
agent_fmt_rc=$(cat "$tmpdir/agent_fmt.rc" 2>/dev/null || echo 1)
hygiene_rc=$(cat "$tmpdir/hygiene.rc" 2>/dev/null || echo 1)
release_rc=$(cat "$tmpdir/release.rc" 2>/dev/null || echo 1)
rust_rc=$(cat "$tmpdir/rust.rc" 2>/dev/null || echo 1)

_show_step_log() {
    local file="$1" rc="$2" ok_lines="$3"
    if [[ "$rc" -ne 0 ]]; then
        tail -120 "$file"
    else
        tail -"$ok_lines" "$file"
    fi
}

step "bash -n syntax (${mode})"
_show_step_log "$tmpdir/syntax.out" "$syntax_rc" 10
[[ "$syntax_rc" -eq 0 ]] || fail=1

step "shellcheck (${mode})"
_show_step_log "$tmpdir/shellcheck.out" "$shellcheck_rc" 15
if [[ "$shellcheck_rc" -ne 0 ]]; then
    if [[ "$strict_shellcheck" -eq 1 || "${CI:-}" == "true" ]]; then
        fail=1
    else
        echo "WARN: shellcheck reported issues (pass --strict-shellcheck or CI=true to block)"
    fi
fi

step "host unit_tests (${mode})"
_show_step_log "$tmpdir/unit_tests.out" "$unit_tests_rc" 20
[[ "$unit_tests_rc" -eq 0 ]] || fail=1

step "verify_agent_fmt.py (${mode})"
_show_step_log "$tmpdir/agent_fmt.out" "$agent_fmt_rc" 15
[[ "$agent_fmt_rc" -eq 0 ]] || fail=1

step "verify_repo_hygiene.py (${mode})"
_show_step_log "$tmpdir/hygiene.out" "$hygiene_rc" 15
[[ "$hygiene_rc" -eq 0 ]] || fail=1

step "release verify (${mode})"
_show_step_log "$tmpdir/release.out" "$release_rc" 15
[[ "$release_rc" -eq 0 ]] || fail=1

step "rust checks (${mode})"
_show_step_log "$tmpdir/rust.out" "$rust_rc" 20
[[ "$rust_rc" -eq 0 ]] || fail=1

printf '{"ts":"%s","tool":"check.sh","mode":"%s","syntax_rc":%s,"shellcheck_rc":%s,"unit_tests_rc":%s,"agent_fmt_rc":%s,"hygiene_rc":%s,"release_rc":%s,"rust_rc":%s,"fail":%s}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$mode" "$syntax_rc" "$shellcheck_rc" \
    "$unit_tests_rc" "$agent_fmt_rc" "$hygiene_rc" "$release_rc" "$rust_rc" "$fail" >> "$log_dir/check_summary.jsonl"

echo
if [[ "$fail" -eq 0 ]]; then
    echo "check: PASS (${mode})"
else
    echo "check: FAIL (${mode})"
fi
exit "$fail"
