#!/bin/bash
# Micro 剪贴板 Debian 容器测试
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INNER_SCRIPT="${REPO_ROOT}/tests/micro-clipboard-inner.sh"
IMAGE_PREFIX="${PODMAN_TEST_BASE_IMAGE:-localhost/debopti-podman-test-base}"
APT_CACHE_ROOT="${REPO_ROOT}/.cache/apt"
TAG="${DEBIAN_TEST_TAGS:-bookworm}"
CNAME="debopti-micro-clip-test-${TAG}"

_resolve_image() {
    local image="${IMAGE_PREFIX}:${TAG}"
    if podman image exists "$image" 2>/dev/null; then
        echo "$image"
    else
        echo "docker.io/library/debian:${TAG}"
    fi
}

image=$(_resolve_image)
apt_cache="${APT_CACHE_ROOT}/${TAG}/archives"
mkdir -p "$apt_cache"
chmod +x "$INNER_SCRIPT"

podman rm -f "$CNAME" 2>/dev/null || true
echo "Starting debian:${TAG} image=${image}"

pull_opt=(--pull=never)
if [[ "$image" == docker.io/* ]]; then
    pull_opt=(--pull=missing)
fi

podman run -d --replace --name "$CNAME" \
    "${pull_opt[@]}" \
    -v "${REPO_ROOT}:/opt/debopti:Z" \
    -v "${apt_cache}:/var/cache/apt/archives:Z" \
    -e DEBIAN_FRONTEND=noninteractive \
    -e CI=true \
    -e "DEBIAN_TAG=${TAG}" \
    "$image" \
    sleep infinity

set +e
podman exec -e DEBIAN_TAG="$TAG" "$CNAME" bash /opt/debopti/tests/micro-clipboard-inner.sh \
    2>&1 | tee "/tmp/debopti-micro-clip-test-${TAG}.log"
rc=${PIPESTATUS[0]}
set -e

if grep -q "ALL_TESTS_PASSED" "/tmp/debopti-micro-clip-test-${TAG}.log"; then
    echo "PASS: debian:${TAG}"
    podman rm -f "$CNAME" >/dev/null
    exit 0
fi

echo "FAIL: debian:${TAG} rc=${rc}"
echo "Container kept: $CNAME  log: /tmp/debopti-micro-clip-test-${TAG}.log"
exit 1
