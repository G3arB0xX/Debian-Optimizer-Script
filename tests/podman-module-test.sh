#!/bin/bash
# Podman 模块 Debian 容器集成测试
#
# 用法:
#   PODMAN_TEST_MODE=fast  bash tests/podman-module-test.sh   # 默认，约 2-4 分钟/版本
#   PODMAN_TEST_MODE=full  bash tests/podman-module-test.sh   # 含重装/卸载，约 5-8 分钟/版本
#   DEBIAN_TEST_TAGS=bookworm bash tests/podman-module-test.sh
#   PODMAN_TEST_PARALLEL=1   bash tests/podman-module-test.sh  # 多版本并行
#   PODMAN_TEST_SKIP_BUILD=1 bash tests/podman-module-test.sh # 跳过基础镜像构建
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOG_FILE="${REPO_ROOT}/.cursor/debug-99e7f0.log"
BASE_DIR="${REPO_ROOT}/tests/podman-test-base"
INNER_SCRIPT="${REPO_ROOT}/tests/podman-test-inner.sh"
IMAGE_PREFIX="${PODMAN_TEST_BASE_IMAGE:-localhost/debopti-podman-test-base}"
APT_CACHE_ROOT="${REPO_ROOT}/.cache/apt"

PODMAN_TEST_MODE="${PODMAN_TEST_MODE:-fast}"
# fast 模式默认只测 bookworm；full 模式默认测 bookworm + bullseye
if [[ -n "${DEBIAN_TEST_TAGS:-}" ]]; then
    read -r -a DEBIAN_TAGS <<< "$DEBIAN_TEST_TAGS"
elif [[ "$PODMAN_TEST_MODE" == "full" ]]; then
    DEBIAN_TAGS=(bookworm bullseye)
else
    DEBIAN_TAGS=(bookworm)
fi

PODMAN_TEST_PARALLEL="${PODMAN_TEST_PARALLEL:-0}"
PODMAN_TEST_KEEP="${PODMAN_TEST_KEEP:-0}"

test_log() {
    local hypo="$1" msg="$2" data="${3:-{}}"
    mkdir -p "$(dirname "$LOG_FILE")"
    printf '{"sessionId":"99e7f0","runId":"container-test","hypothesisId":"%s","location":"podman-module-test.sh","message":"%s","data":%s,"timestamp":%s}\n' \
        "$hypo" "$msg" "$data" "$(date +%s%3N 2>/dev/null || date +%s)" >> "$LOG_FILE"
}

_ensure_base_images() {
    if [[ "${PODMAN_TEST_SKIP_BUILD:-}" == "1" ]]; then
        return 0
    fi
    DEBIAN_TEST_TAGS="${DEBIAN_TAGS[*]}" bash "${BASE_DIR}/build-base.sh"
}

_resolve_image() {
    local tag="$1"
    local image="${IMAGE_PREFIX}:${tag}"
    if podman image exists "$image" 2>/dev/null; then
        echo "$image"
    else
        echo "docker.io/library/debian:${tag}"
    fi
}

run_in_container() {
    local tag="$1"
    local cname="debopti-podman-test-${tag}"
    local image apt_cache start_ts end_ts elapsed

    image=$(_resolve_image "$tag")
    apt_cache="${APT_CACHE_ROOT}/${tag}/archives"
    mkdir -p "$apt_cache"

    podman rm -f "$cname" 2>/dev/null || true
    test_log "H0" "starting_container" "{\"tag\":\"${tag}\",\"mode\":\"${PODMAN_TEST_MODE}\",\"image\":\"${image}\"}"

    start_ts=$(date +%s)

    local -a pull_opt=(--pull=never)
    if [[ "$image" == docker.io/* ]]; then
        pull_opt=(--pull=missing)
    fi

    podman run -d --replace --name "$cname" \
        "${pull_opt[@]}" \
        --privileged \
        --userns=host \
        --cgroupns=host \
        --security-opt seccomp=unconfined \
        --cap-add=ALL \
        -v "${REPO_ROOT}:/opt/debopti:Z" \
        -v "${apt_cache}:/var/cache/apt/archives:Z" \
        -e DEBIAN_FRONTEND=noninteractive \
        -e CI=true \
        -e "PODMAN_TEST_MODE=${PODMAN_TEST_MODE}" \
        -e "DEBIAN_TAG=${tag}" \
        "$image" \
        sleep infinity

    podman exec \
        -e DEBIAN_TAG="$tag" \
        -e PODMAN_TEST_MODE="$PODMAN_TEST_MODE" \
        "$cname" \
        bash /opt/debopti/tests/podman-test-inner.sh \
        2>&1 | tee "/tmp/debopti-test-${tag}.log"

    local rc=${PIPESTATUS[0]}
    end_ts=$(date +%s)
    elapsed=$((end_ts - start_ts))

    if grep -q "ALL_TESTS_PASSED" "/tmp/debopti-test-${tag}.log"; then
        test_log "H0" "container_passed" "{\"tag\":\"${tag}\",\"elapsed_sec\":${elapsed}}"
        echo "PASS: debian:${tag} (${elapsed}s)"
    else
        test_log "H0" "container_failed" "{\"tag\":\"${tag}\",\"rc\":${rc},\"elapsed_sec\":${elapsed}}"
        echo "FAIL: debian:${tag} (${elapsed}s)"
        [[ "$PODMAN_TEST_KEEP" == "1" ]] || podman rm -f "$cname" 2>/dev/null || true
        return 1
    fi

    [[ "$PODMAN_TEST_KEEP" == "1" ]] || podman rm -f "$cname" 2>/dev/null || true
}

main() {
    local total_start total_end total_elapsed failed=0

    if [[ ! -f "$INNER_SCRIPT" ]]; then
        echo "错误: 缺少 ${INNER_SCRIPT}"
        exit 1
    fi

    echo "Podman 模块测试 | 模式: ${PODMAN_TEST_MODE} | 版本: ${DEBIAN_TAGS[*]}"
    : > "$LOG_FILE" 2>/dev/null || true
    _ensure_base_images

    total_start=$(date +%s)

    if [[ "$PODMAN_TEST_PARALLEL" == "1" && ${#DEBIAN_TAGS[@]} -gt 1 ]]; then
        local pids=() tag
        for tag in "${DEBIAN_TAGS[@]}"; do
            ( run_in_container "$tag" ) &
            pids+=($!)
        done
        for pid in "${pids[@]}"; do
            wait "$pid" || failed=$((failed + 1))
        done
    else
        local tag
        for tag in "${DEBIAN_TAGS[@]}"; do
            run_in_container "$tag" || failed=$((failed + 1))
        done
    fi

    total_end=$(date +%s)
    total_elapsed=$((total_end - total_start))

    if [[ "$failed" -gt 0 ]]; then
        echo "TESTS FAILED: ${failed} (总耗时 ${total_elapsed}s)"
        exit 1
    fi
    echo "ALL DEBIAN VERSIONS PASSED (总耗时 ${total_elapsed}s)"
}

main "$@"
