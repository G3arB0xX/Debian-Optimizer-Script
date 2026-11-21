#!/bin/bash
# =========================================================
# SemVer 2.0.0 + backward-compatible CalVer / Build helpers
# Primary: SemVer X.Y.Z[-prerelease]
# Compat:  CalVer YY.MM.DD + Build opaque short id
# Production copy must not mention VCS internals.
# =========================================================

DEBOPTI_VERSION_REGEX='^[0-9]{2}\.[0-9]{2}\.[0-9]{2}$'
DEBOPTI_CALVER_DATE_REGEX='^[2-9][0-9]\.(0[1-9]|1[0-2])\.(0[1-9]|[12][0-9]|3[01])$'
DEBOPTI_SEMVER_REGEX='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-((0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*)(\.(0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9a-zA-Z-]*))*))?(\+[0-9a-zA-Z-]+(\.[0-9a-zA-Z-]+)*)?$'
DEBOPTI_BUILD_REGEX='^[a-z0-9]+$'

# Return 0 iff $1 is a strict SemVer 2.0.0 string.
debopti_is_semver() {
    local v=${1:-}
    [[ -n "$v" && "$v" =~ $DEBOPTI_SEMVER_REGEX ]]
}

# Return 0 iff $1 matches YY.MM.DD CalVer date range (20..99, 01..12, 01..31).
debopti_is_calver() {
    local v=${1:-}
    [[ -n "$v" && "$v" =~ $DEBOPTI_CALVER_DATE_REGEX ]]
}

# Validate and emit canonical SemVer tag "vX.Y.Z[-prerelease]".
debopti_semver_tag() {
    local v=${1:-}
    v="${v#v}"
    debopti_is_semver "$v" || return 1
    printf 'v%s\n' "$v"
}

# Compare two strict SemVer 2.0.0 strings per SemVer 2.0.0 section 11.
# Prints -1 (v1 < v2), 0 (v1 == v2), or 1 (v1 > v2). Build metadata (+...) is ignored.
debopti_semver_compare() {
    local v1=${1:-} v2=${2:-}
    debopti_is_semver "$v1" || return 1
    debopti_is_semver "$v2" || return 1

    v1="${v1%%+*}"
    v2="${v2%%+*}"

    local core1="$v1" pre1="" core2="$v2" pre2=""
    if [[ "$v1" == *-* ]]; then
        core1="${v1%%-*}"
        pre1="${v1#*-}"
    fi
    if [[ "$v2" == *-* ]]; then
        core2="${v2%%-*}"
        pre2="${v2#*-}"
    fi

    local maj1 min1 pat1 maj2 min2 pat2
    IFS='.' read -r maj1 min1 pat1 <<< "$core1"
    IFS='.' read -r maj2 min2 pat2 <<< "$core2"

    if (( 10#$maj1 != 10#$maj2 )); then
        (( 10#$maj1 < 10#$maj2 )) && printf '%s\n' "-1" || printf '%s\n' "1"
        return 0
    fi
    if (( 10#$min1 != 10#$min2 )); then
        (( 10#$min1 < 10#$min2 )) && printf '%s\n' "-1" || printf '%s\n' "1"
        return 0
    fi
    if (( 10#$pat1 != 10#$pat2 )); then
        (( 10#$pat1 < 10#$pat2 )) && printf '%s\n' "-1" || printf '%s\n' "1"
        return 0
    fi

    if [[ -z "$pre1" && -z "$pre2" ]]; then
        printf '0\n'
        return 0
    fi
    if [[ -z "$pre1" ]]; then
        printf '1\n'
        return 0
    fi
    if [[ -z "$pre2" ]]; then
        printf '%s\n' "-1"
        return 0
    fi

    local -a p1=() p2=()
    IFS='.' read -r -a p1 <<< "$pre1"
    IFS='.' read -r -a p2 <<< "$pre2"
    local len1=${#p1[@]} len2=${#p2[@]}
    local max_len=$(( len1 < len2 ? len1 : len2 ))
    local i id1 id2 is_num1 is_num2
    local LC_ALL=C

    for (( i = 0; i < max_len; i++ )); do
        id1="${p1[$i]}"
        id2="${p2[$i]}"
        [[ "$id1" == "$id2" ]] && continue
        is_num1=0
        is_num2=0
        [[ "$id1" =~ ^[0-9]+$ ]] && is_num1=1
        [[ "$id2" =~ ^[0-9]+$ ]] && is_num2=1
        if (( is_num1 && is_num2 )); then
            (( 10#$id1 < 10#$id2 )) && printf '%s\n' "-1" || printf '%s\n' "1"
            return 0
        elif (( is_num1 && ! is_num2 )); then
            printf '%s\n' "-1"
            return 0
        elif (( ! is_num1 && is_num2 )); then
            printf '1\n'
            return 0
        else
            if [[ "$id1" < "$id2" ]]; then
                printf '%s\n' "-1"
            else
                printf '1\n'
            fi
            return 0
        fi
    done

    if (( len1 < len2 )); then
        printf '%s\n' "-1"
    elif (( len1 > len2 )); then
        printf '1\n'
    else
        printf '0\n'
    fi
}

# Compute next SemVer 2.0.0 string: debopti_next_semver <current> <bump> [prerelease]
# bump in: major | minor | patch | prerelease
debopti_next_semver() {
    local current=${1:-}
    local bump=${2:-}
    local pre=${3:-}

    case "$bump" in
        major|minor|patch|prerelease) ;;
        *) return 1 ;;
    esac

    if [[ -n "$pre" ]]; then
        debopti_is_semver "1.0.0-${pre}" || return 1
    fi

    if [[ -z "$current" ]]; then
        if [[ -n "$pre" ]]; then
            printf '1.0.0-%s\n' "$pre"
        elif [[ "$bump" == "prerelease" ]]; then
            printf '1.0.0-rc.1\n'
        else
            printf '1.0.0\n'
        fi
        return 0
    fi

    debopti_is_semver "$current" || return 1
    local base="${current%%+*}"
    local core="$base" cur_pre=""
    if [[ "$base" == *-* ]]; then
        core="${base%%-*}"
        cur_pre="${base#*-}"
    fi

    local maj min pat
    IFS='.' read -r maj min pat <<< "$core"

    case "$bump" in
        major)
            maj=$((10#$maj + 1))
            min=0
            pat=0
            ;;
        minor)
            min=$((10#$min + 1))
            pat=0
            ;;
        patch)
            pat=$((10#$pat + 1))
            ;;
        prerelease)
            if [[ -z "$cur_pre" ]]; then
                pat=$((10#$pat + 1))
                pre="${pre:-rc.1}"
            elif [[ -z "$pre" ]]; then
                if [[ "$cur_pre" =~ ^(.*)\.([0-9]+)$ ]]; then
                    pre="${BASH_REMATCH[1]}.$((10#${BASH_REMATCH[2]} + 1))"
                else
                    pre="${cur_pre}.1"
                fi
            fi
            ;;
    esac

    if [[ -n "$pre" ]]; then
        printf '%s.%s.%s-%s\n' "$maj" "$min" "$pat" "$pre"
    else
        printf '%s.%s.%s\n' "$maj" "$min" "$pat"
    fi
}

# Map any stored calendar string to YY.MM.DD. Empty input stays empty.
debopti_normalize_calver() {
    local v=${1:-}
    [[ -n "$v" ]] || return 0
    if [[ "$v" =~ ^([0-9]{4})\.([0-9]{2})\.([0-9]{2})(\.[0-9]+)?$ ]]; then
        printf '%s.%s.%s\n' "${BASH_REMATCH[1]:2:2}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}"
        return 0
    fi
    printf '%s\n' "$v"
}

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

# Parse VERSION file text or legacy common.sh VERSION_ID= line.
# Prints: "<semver>|<calver>|<build>"
debopti_parse_version_full() {
    local text=$1
    local semver="" calver="" legacy_ver="" build=""

    semver="$(_debopti_kv "$text" "DEBOPTI_SEMVER" || true)"
    calver="$(_debopti_kv "$text" "DEBOPTI_CALVER" || true)"
    legacy_ver="$(_debopti_kv "$text" "DEBOPTI_VERSION" || true)"
    build="$(_debopti_kv "$text" "DEBOPTI_BUILD" || true)"
    if [[ -z "$build" ]]; then
        build="$(_debopti_kv "$text" "VERSION_ID" || true)"
    fi

    if [[ -n "$semver" ]] && ! debopti_is_semver "$semver"; then
        return 1
    fi

    if [[ -z "$calver" && -n "$legacy_ver" ]]; then
        calver="$legacy_ver"
    fi
    if [[ -n "$calver" ]]; then
        calver="$(debopti_normalize_calver "$calver")"
        if [[ ! "$calver" =~ $DEBOPTI_VERSION_REGEX ]]; then
            return 1
        fi
    fi

    if [[ -z "$build" || ! "$build" =~ $DEBOPTI_BUILD_REGEX ]]; then
        return 1
    fi
    printf '%s|%s|%s\n' "$semver" "$calver" "$build"
}

# Parse VERSION file. Prints: "<semver>|<calver>|<build>"
debopti_parse_version_file_full() {
    local file=$1
    [[ -f "$file" ]] || return 1
    debopti_parse_version_full "$(cat "$file")"
}

# Parse VERSION file. Prints: "<version>|<build>"
debopti_parse_version_file() {
    local file=$1
    [[ -f "$file" ]] || return 1
    debopti_parse_version_text "$(cat "$file")"
}

# Parse VERSION file text or legacy common.sh VERSION_ID= line.
# Prints: "<version>|<build>"  (prefers DEBOPTI_SEMVER over DEBOPTI_CALVER)
debopti_parse_version_text() {
    local text=$1
    local full semver calver build primary
    full="$(debopti_parse_version_full "$text")" || return 1
    IFS='|' read -r semver calver build <<< "$full"
    primary="${semver:-$calver}"
    printf '%s|%s\n' "$primary" "$build"
}

# Today's calendar version as YY.MM.DD. Same-day releases share this date
# and are distinguished by Build. $1 unused (kept for callers). $2=today override.
debopti_next_calver() {
    local today=${2:-}
    [[ -n "$today" ]] || today="$(date +%y.%m.%d)"
    printf '%s\n' "$today"
}

# Return 0 when an update should be offered (remote build present and differs).
debopti_needs_update() {
    local local_build=${1:-}
    local remote_build=${2:-}
    [[ -n "$remote_build" ]] || return 1
    [[ "$local_build" != "$remote_build" ]]
}

debopti_format_display() {
    local version build
    version="$(debopti_normalize_calver "$1")"
    build=$2
    if [[ -n "$version" ]]; then
        printf '%s (%s)\n' "$version" "$build"
    else
        printf '%s\n' "$build"
    fi
}

debopti_format_prompt() {
    local version build
    version="$(debopti_normalize_calver "$1")"
    build=$2
    if [[ -n "$version" ]]; then
        printf '%s (build %s)\n' "$version" "$build"
    else
        printf 'build %s\n' "$build"
    fi
}

debopti_tag_name() {
    local version=$1
    local build=${2:-}
    if [[ "$version" =~ $DEBOPTI_VERSION_REGEX ]]; then
        if debopti_is_calver "$version" || [[ "$version" =~ \.(0[0-9]) ]]; then
            if [[ -n "$build" ]]; then
                printf 'v%s-%s\n' "$version" "$build"
            else
                printf 'v%s\n' "$version"
            fi
            return 0
        fi
    fi
    if debopti_is_semver "$version"; then
        printf 'v%s\n' "$version"
        return 0
    fi
    if [[ -n "$build" ]]; then
        printf 'v%s-%s\n' "$version" "$build"
    else
        printf 'v%s\n' "$version"
    fi
}

debopti_write_version_file() {
    local path=$1 version=$2 build=$3 semver=${4:-}
    version="$(debopti_normalize_calver "$version")"
    if [[ -n "$semver" ]]; then
        debopti_is_semver "$semver" || return 1
    fi
    mkdir -p "$(dirname "$path")"
    # Older updaters (YYYY.MM.DD.N only) reject DEBOPTI_VERSION=YY.MM.DD and
    # abort extract checks. Omit that key; they ignore DEBOPTI_SEMVER and
    # DEBOPTI_CALVER and update by Build. Loaded in-process as DEBOPTI_VERSION.
    if [[ -n "$semver" ]]; then
        cat > "$path" <<EOF
DEBOPTI_SEMVER="${semver}"
DEBOPTI_CALVER="${version}"
DEBOPTI_BUILD="${build}"
EOF
    else
        cat > "$path" <<EOF
DEBOPTI_CALVER="${version}"
DEBOPTI_BUILD="${build}"
EOF
    fi
}

# 0 when VERSION uses the on-disk format older updaters can extract-check.
debopti_version_file_canonical() {
    local file=$1
    local semver=""
    [[ -f "$file" ]] || return 1
    grep -q '^DEBOPTI_CALVER=' "$file" || return 1
    grep -q '^DEBOPTI_VERSION=' "$file" && return 1
    grep -q '^DEBOPTI_BUILD=' "$file" || return 1
    if grep -q '^DEBOPTI_SEMVER=' "$file"; then
        semver="$(_debopti_kv "$(cat "$file")" "DEBOPTI_SEMVER" || true)"
        debopti_is_semver "$semver" || return 1
    fi
    return 0
}

# Keep scripts/common.sh VERSION_ID= in sync for older clients that grep the file.
debopti_sync_common_alias() {
    local common=$1 build=$2
    [[ -f "$common" ]] || return 1
    if grep -q '^VERSION_ID=' "$common"; then
        sed -i "s/^VERSION_ID=.*/VERSION_ID=\"${build}\"/" "$common"
    fi
}

# Source identity from <root>/VERSION into DEBOPTI_SEMVER / DEBOPTI_CALVER /
# DEBOPTI_VERSION / DEBOPTI_BUILD / VERSION_ID.
debopti_load_version() {
    local root=$1
    local full semver="" calver="" build=""
    DEBOPTI_SEMVER=""
    DEBOPTI_CALVER=""
    DEBOPTI_VERSION=""
    DEBOPTI_BUILD=""
    if full="$(debopti_parse_version_file_full "${root}/VERSION")"; then
        IFS='|' read -r semver calver build <<< "$full"
        DEBOPTI_SEMVER="$semver"
        DEBOPTI_CALVER="$calver"
        DEBOPTI_VERSION="${semver:-$calver}"
        DEBOPTI_BUILD="$build"
    fi
    VERSION_ID="${DEBOPTI_BUILD:-${VERSION_ID:-}}"
}

# Verify extracted tree VERSION matches the identity we intended to install.
debopti_verify_tree() {
    local dir=$1 expected_version=$2 expected_build=$3
    local full semver="" calver="" build="" norm_expected=""
    if [[ -f "${dir}/VERSION" ]]; then
        full="$(debopti_parse_version_file_full "${dir}/VERSION")" || return 1
    elif [[ -f "${dir}/scripts/common.sh" ]]; then
        full="$(debopti_parse_version_full "$(cat "${dir}/scripts/common.sh")")" || return 1
    else
        return 1
    fi
    IFS='|' read -r semver calver build <<< "$full"
    [[ "$build" == "$expected_build" ]] || return 1
    if [[ -n "$expected_version" && ( -n "$semver" || -n "$calver" ) ]]; then
        norm_expected="$(debopti_normalize_calver "$expected_version")"
        if [[ -n "$semver" && "$expected_version" == "$semver" ]]; then
            return 0
        fi
        if [[ -n "$calver" && "$norm_expected" == "$calver" ]]; then
            return 0
        fi
        return 1
    fi
    return 0
}

# Extract Markdown body under "## [<version>]" up to next "## " or ref link.
debopti_changelog_extract() {
    local file=$1
    local version=${2:-}
    version="${version#v}"
    [[ -f "$file" && -n "$version" ]] || return 1

    local line found=0
    local -a body=()
    while IFS= read -r line || [[ -n "$line" ]]; do
        if (( ! found )); then
            if [[ "$line" == "## [${version}]" || "$line" == "## [${version}] "* ]]; then
                found=1
            fi
            continue
        fi
        if [[ "$line" == "## "* ]] || [[ "$line" =~ ^\[[^]]+\]:[[:space:]]+https?:// ]]; then
            break
        fi
        body+=("$line")
    done < "$file"

    (( found )) || return 1

    local start=0 end=$(( ${#body[@]} - 1 ))
    while (( start <= end )) && [[ -z "${body[$start]//[[:space:]]/}" ]]; do
        start=$((start + 1))
    done
    while (( end >= start )) && [[ -z "${body[$end]//[[:space:]]/}" ]]; do
        end=$((end - 1))
    done

    local i
    for (( i = start; i <= end; i++ )); do
        printf '%s\n' "${body[$i]}"
    done
}

debopti_changelog_prepend() {
    local file=$1 version=$2 build=$3 notes=$4
    local tmp existing=""
    tmp="$(mktemp)"
    [[ -f "$file" ]] && existing="$(cat "$file")"
    {
        echo "# Changelog"
        echo ""
        echo "## ${version} (${build})"
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
