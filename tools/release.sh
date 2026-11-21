#!/usr/bin/env bash
# Agent-managed release CLI (SemVer 2.0.0 + Conventional Commits 1.0.0 + Keep a Changelog 1.1.0).
# Delegates to stdlib-only tools/release_helper.py.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/release_helper.py"

if [[ ! -f "$HELPER" ]]; then
    echo "ERROR: missing release helper: ${HELPER}" >&2
    exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required by tools/release.sh" >&2
    exit 1
fi

exec python3 -B "$HELPER" "$@"
