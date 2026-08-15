#!/bin/bash
# =========================================================
# CalVer + Build identity helpers
# Version: YYYY.MM.DD.N   Build: opaque short id
# Production copy must not mention VCS internals.
# =========================================================

DEBOPTI_VERSION_REGEX='^[0-9]{4}\.[0-9]{2}\.[0-9]{2}\.[0-9]+$'
DEBOPTI_BUILD_REGEX='^[a-z0-9]+$'

_debopti_unquote() {
    local v=$1
    v="${v#\"}"
    v="${v%\"}"
    v="${v#\'}"
    v="${v%\'}"
    printf '%s' "$v"
}

_debopti_kv() {
    local text=$1 key=$2
    local line
    line="$(printf '%s\n' "$text" | grep "^${key}=" | head -n 1 || true)"
    [[ -n "$line" ]] || return 1
    _debopti_unquote "${line#*=}"
}

# Parse VERSION file. Prints: "<version>|<build>"
debopti_parse_version_file() {
    local file=$1
    [[ -f "$file" ]] || return 1
    debopti_parse_version_text "$(cat "$file")"
}

# Parse VERSION file text or legacy common.sh VERSION_ID= line.
# Prints: "<version>|<build>"  (version may be empty for legacy)
debopti_parse_version_text() {
    local text=$1
    local version="" build=""

    version="$(_debopti_kv "$text" "DEBOPTI_VERSION" || true)"
    build="$(_debopti_kv "$text" "DEBOPTI_BUILD" || true)"
    if [[ -z "$build" ]]; then
        build="$(_debopti_kv "$text" "VERSION_ID" || true)"
    fi

    if [[ -n "$version" && ! "$version" =~ $DEBOPTI_VERSION_REGEX ]]; then
        return 1
    fi
    if [[ -z "$build" || ! "$build" =~ $DEBOPTI_BUILD_REGEX ]]; then
        return 1
    fi
    printf '%s|%s\n' "$version" "$build"
}

# Next CalVer. $1=existing (may be empty) $2=today as YYYY.MM.DD (optional)
debopti_next_calver() {
    local existing=${1:-}
    local today=${2:-}
    [[ -n "$today" ]] || today="$(date +%Y.%m.%d)"

    local existing_date existing_n
    if [[ "$existing" =~ $DEBOPTI_VERSION_REGEX ]]; then
        existing_date="${existing%.*}"
        existing_n="${existing##*.}"
        if [[ "$existing_date" == "$today" ]]; then
            printf '%s.%s\n' "$today" "$((existing_n + 1))"
            return 0
        fi
    fi
    printf '%s.1\n' "$today"
}

# Return 0 when an update should be offered (remote build present and differs).
debopti_needs_update() {
    local local_build=${1:-}
    local remote_build=${2:-}
    [[ -n "$remote_build" ]] || return 1
    [[ "$local_build" != "$remote_build" ]]
}

debopti_format_display() {
    local version=$1 build=$2
    if [[ -n "$version" ]]; then
        printf '%s (%s)\n' "$version" "$build"
    else
        printf '%s\n' "$build"
    fi
}

debopti_format_prompt() {
    local version=$1 build=$2
    if [[ -n "$version" ]]; then
        printf '%s (build %s)\n' "$version" "$build"
    else
        printf 'build %s\n' "$build"
    fi
}

debopti_tag_name() {
    printf 'v%s\n' "$1"
}

debopti_write_version_file() {
    local path=$1 version=$2 build=$3
    mkdir -p "$(dirname "$path")"
    cat > "$path" <<EOF
DEBOPTI_VERSION="${version}"
DEBOPTI_BUILD="${build}"
EOF
}

# Keep scripts/common.sh VERSION_ID= in sync for older clients that grep the file.
debopti_sync_common_alias() {
    local common=$1 build=$2
    [[ -f "$common" ]] || return 1
    if grep -q '^VERSION_ID=' "$common"; then
        sed -i "s/^VERSION_ID=.*/VERSION_ID=\"${build}\"/" "$common"
    fi
}

# Source identity from <root>/VERSION into DEBOPTI_VERSION / DEBOPTI_BUILD / VERSION_ID.
debopti_load_version() {
    local root=$1
    local parsed version build
    DEBOPTI_VERSION=""
    DEBOPTI_BUILD=""
    if parsed="$(debopti_parse_version_file "${root}/VERSION")"; then
        version="${parsed%%|*}"
        build="${parsed#*|}"
        DEBOPTI_VERSION="$version"
        DEBOPTI_BUILD="$build"
    fi
    VERSION_ID="${DEBOPTI_BUILD:-${VERSION_ID:-}}"
}

# Verify extracted tree VERSION matches the identity we intended to install.
debopti_verify_tree() {
    local dir=$1 expected_version=$2 expected_build=$3
    local parsed version="" build=""
    if [[ -f "${dir}/VERSION" ]]; then
        parsed="$(debopti_parse_version_file "${dir}/VERSION")" || return 1
    elif [[ -f "${dir}/scripts/common.sh" ]]; then
        parsed="$(debopti_parse_version_text "$(cat "${dir}/scripts/common.sh")")" || return 1
    else
        return 1
    fi
    version="${parsed%%|*}"
    build="${parsed#*|}"
    [[ "$build" == "$expected_build" ]] || return 1
    if [[ -n "$expected_version" && -n "$version" ]]; then
        [[ "$version" == "$expected_version" ]] || return 1
    fi
    return 0
}

debopti_changelog_prepend() {
    local file=$1 version=$2 build=$3 notes=$4
    local tmp existing=""
    tmp="$(mktemp)"
    [[ -f "$file" ]] && existing="$(cat "$file")"
    {
        echo "# Changelog"
        echo ""
        echo "## ${version}"
        echo ""
        echo "- Build: ${build}"
        if [[ -n "$notes" ]]; then
            printf '%s\n' "$notes" | sed 's/^/- /'
        fi
        echo ""
        if [[ -n "$existing" ]]; then
            printf '%s\n' "$existing" | sed '/^# Changelog$/d' | sed '/^$/N;/^\n$/d'
        fi
    } > "$tmp"
    mkdir -p "$(dirname "$file")"
    mv "$tmp" "$file"
}
