#!/bin/bash
# 构建 Podman 模块测试基础镜像（各 Debian 版本仅需构建一次）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
read -r -a TAGS <<< "${DEBIAN_TEST_TAGS:-bookworm bullseye}"
IMAGE_PREFIX="${PODMAN_TEST_BASE_IMAGE:-localhost/debopti-podman-test-base}"

for tag in "${TAGS[@]}"; do
    image="${IMAGE_PREFIX}:${tag}"
    if podman image exists "$image" 2>/dev/null && [[ "${PODMAN_TEST_FORCE_REBUILD:-}" != "1" ]]; then
        echo "SKIP: ${image} 已存在"
        continue
    fi
    echo "BUILD: ${image}"
    podman build \
        --build-arg "DEBIAN_TAG=${tag}" \
        -t "$image" \
        -f "${SCRIPT_DIR}/Dockerfile" \
        "${SCRIPT_DIR}" || {
        echo "WARN: 构建 ${image} 失败，重试一次..." >&2
        podman build --no-cache \
            --build-arg "DEBIAN_TAG=${tag}" \
            -t "$image" \
            -f "${SCRIPT_DIR}/Dockerfile" \
            "${SCRIPT_DIR}"
    }
done

echo "BASE IMAGES READY"
