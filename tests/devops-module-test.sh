#!/bin/bash
# DevOps 模块 Debian 容器集成测试
#
# 用法:
#   DEVOPS_TEST_MODE=fast  bash tests/devops-module-test.sh
#   DEVOPS_TEST_MODE=full  bash tests/devops-module-test.sh
#   DEBIAN_TEST_TAGS=bookworm bash tests/devops-module-test.sh
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INNER_SCRIPT="${REPO_ROOT}/tests/devops-test-inner.sh"
IMAGE_PREFIX="${PODMAN_TEST_BASE_IMAGE:-localhost/debopti-podman-test-base}"
APT_CACHE_ROOT="${REPO_ROOT}/.cache/apt"

DEVOPS_TEST_MODE="${DEVOPS_TEST_MODE:-fast}"
if [[ -n "${DEBIAN_TEST_TAGS:-}" ]]; then
    read -r -a DEBIAN_TAGS <<< "$DEBIAN_TEST_TAGS"
elif [[ "$DEVOPS_TEST_MODE" == "full" ]]; then
    DEBIAN_TAGS=(bookworm bullseye)
else
    DEBIAN_TAGS=(bookworm)
fi

DEVOPS_TEST_KEEP="${DEVOPS_TEST_KEEP:-0}"

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
    local cname="debopti-devops-test-${tag}"
    local image apt_cache start_ts end_ts elapsed

    image=$(_resolve_image "$tag")
    apt_cache="${APT_CACHE_ROOT}/${tag}/archives"
    mkdir -p "$apt_cache"

    podman rm -f "$cname" 2>/dev/null || true
    echo "Starting debian:${tag} (mode=${DEVOPS_TEST_MODE}, image=${image})"

    start_ts=$(date +%s)

    local -a pull_opt=(--pull=never)
    if [[ "$image" == docker.io/* ]]; then
        pull_opt=(--pull=missing)
    fi

    podman run -d --replace --name "$cname" \
        "${pull_opt[@]}" \
        -v "${REPO_ROOT}:/opt/debopti:Z" \
        -v "${apt_cache}:/var/cache/apt/archives:Z" \
        -e DEBIAN_FRONTEND=noninteractive \
        -e CI=true \
        -e "DEVOPS_TEST_MODE=${DEVOPS_TEST_MODE}" \
        -e "DEBIAN_TAG=${tag}" \
        "$image" \
        sleep infinity

    podman exec \
        -e DEBIAN_TAG="$tag" \
        -e DEVOPS_TEST_MODE="$DEVOPS_TEST_MODE" \
        "$cname" \
        bash /opt/debopti/tests/devops-test-inner.sh \
        2>&1 | tee "/tmp/debopti-devops-test-${tag}.log"

    local rc=${PIPESTATUS[0]}
    end_ts=$(date +%s)
    elapsed=$((end_ts - start_ts))

    if grep -q "ALL_TESTS_PASSED" "/tmp/debopti-devops-test-${tag}.log"; then
        echo "PASS: debian:${tag} (${elapsed}s)"
    else
        echo "FAIL: debian:${tag} (${elapsed}s)"
        [[ "$DEVOPS_TEST_KEEP" == "1" ]] || podman rm -f "$cname" 2>/dev/null || true
        return 1
    fi

    [[ "$DEVOPS_TEST_KEEP" == "1" ]] || podman rm -f "$cname" 2>/dev/null || true
}

main() {
    local failed=0 tag

    if [[ ! -f "$INNER_SCRIPT" ]]; then
        echo "错误: 缺少 ${INNER_SCRIPT}"
        exit 1
    fi
    chmod +x "$INNER_SCRIPT"

    echo "DevOps 模块测试 | 模式: ${DEVOPS_TEST_MODE} | 版本: ${DEBIAN_TAGS[*]}"

    for tag in "${DEBIAN_TAGS[@]}"; do
        run_in_container "$tag" || failed=$((failed + 1))
    done

    if [[ "$failed" -gt 0 ]]; then
        echo "TESTS FAILED: ${failed}"
        exit 1
    fi
    echo "ALL DEBIAN VERSIONS PASSED"
}

main "$@"
