#!/usr/bin/env bash
# ==============================================================================
# debopty - GitHub Sanitized Push Tool (Advanced Edition)
# ==============================================================================
# 此脚本用于一键安全推送至 GitHub：保留 governance/、tools/、tests/ 等公开协作资产，仅清理已废弃的私有历史目录（instructions/）并拦截个人隐私与密钥泄露。
# 整个重写过程在隔离的沙盒中进行，确保本地的 jj (jujutsu) 版本控制和工作区 100% 不受影响。
# ==============================================================================

set -euo pipefail

# 1. 基础配置
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="${DEBOPTY_WORKSPACE_DIR:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
SANDBOX_DIR="${WORKSPACE_DIR}/.cache/push-temp"
SSH_CONTROL_PATH="${HOME}/.cache/debopti-ssh-control"
SSH_CRED_NAME="debopti-github-ssh"
SSH_CRED_FILE="${XDG_CONFIG_HOME:-${HOME}/.config}/credstore/${SSH_CRED_NAME}.cred"
REMOTE_REPO="${DEBOPTY_REPO:-G3arB0xX/Debian-Optimizer-Script}"
GITHUB_REMOTE="${DEBOPTY_GITHUB_REMOTE:-git@github.com:${REMOTE_REPO}.git}"
BRANCH_NAME="main"
PUSH_SEMVER=""
PUSH_CALVER=""
PUSH_VERSION=""
PUSH_BUILD=""

# shellcheck source=../scripts/version.sh
source "${WORKSPACE_DIR}/scripts/version.sh"

# ================= GPG 重新签名配置 =================
# 如果您希望脱敏后的 Commit 在 GitHub 上依然带有 "Verified" 徽标，请将其设为 true。
# 注意：启用此项需要您的宿主机正在运行 gpg-agent 并且已缓存了 GPG 密钥的密码，
# 否则在重构 40+ 个 commit 时，系统会连续提示输入密码。
ENABLE_GPG_SIGNING=false
# ===================================================

FILTER_PATHS=(
    instructions
)

_ssh_agent_status() {
    # 返回值: 0=已有密钥, 1=agent 正常但无密钥, 2=无 agent 或 agent 不可用
    if [ -z "${SSH_AUTH_SOCK:-}" ]; then
        return 2
    fi
    if ssh-add -l >/dev/null 2>&1; then
        return 0
    fi
    local rc=$?
    if [ "${rc}" -eq 1 ]; then
        return 1
    fi
    return 2
}

_ensure_ssh_agent() {
    local status=0

    set +e
    _ssh_agent_status
    status=$?
    set -e

    if [ "${status}" -eq 0 ]; then
        return 0
    fi

    if [ "${status}" -eq 2 ]; then
        unset SSH_AUTH_SOCK SSH_AGENT_PID
        eval "$(ssh-agent -s)" >/dev/null
    fi
}

_resolve_github_identity() {
    local config="${HOME}/.ssh/config"
    local line host_pattern identity in_block=false

    if [ -f "${config}" ]; then
        while IFS= read -r line || [ -n "${line}" ]; do
            line="${line%%#*}"
            line="${line#"${line%%[![:space:]]*}"}"
            line="${line%"${line##*[![:space:]]}"}"
            [ -z "${line}" ] && continue

            if [[ "${line}" =~ ^[Hh]ost[[:space:]]+(.+)$ ]]; then
                host_pattern="${BASH_REMATCH[1]}"
                in_block=false
                if [[ " ${host_pattern} " == *" github.com "* ]] \
                    || [[ "${host_pattern}" == "github.com" ]]; then
                    in_block=true
                fi
                continue
            fi

            if [ "${in_block}" = true ] \
                && [[ "${line}" =~ ^[Ii]dentityFile[[:space:]]+(.+)$ ]]; then
                identity="${BASH_REMATCH[1]}"
                identity="${identity//\"/}"
                identity="${identity/#\~/${HOME}}"
                if [ -f "${identity}" ]; then
                    printf '%s' "${identity}"
                    return 0
                fi
            fi
        done < "${config}"
    fi

    for identity in \
        "${HOME}/.ssh/id_ed25519" \
        "${HOME}/.ssh/id_rsa" \
        "${HOME}/.ssh/id_ecdsa" \
        "${HOME}/.ssh/github_gbx_ed25519"; do
        if [ -f "${identity}" ]; then
            printf '%s' "${identity}"
            return 0
        fi
    done

    return 1
}

_identity_loaded() {
    local identity="$1"
    local fingerprint=""

    fingerprint="$(ssh-keygen -lf "${identity}" 2>/dev/null | awk '{print $2}')"
    [ -n "${fingerprint}" ] || return 1

    if ssh-add -l 2>/dev/null | grep -Fq "${fingerprint}"; then
        return 0
    fi
    return 1
}

_add_ssh_key_with_passphrase() {
    local identity="$1"
    local askpass_script=""

    askpass_script="$(mktemp "${TMPDIR:-/tmp}/debopti-ssh-askpass.XXXXXX")"
    cat > "${askpass_script}" << 'EOF'
#!/bin/sh
printf '%s\n' "${SSH_PASSPHRASE}"
EOF
    chmod 700 "${askpass_script}"
    # shellcheck disable=SC2064
    trap "rm -f '${askpass_script}'" RETURN

    DISPLAY="${DISPLAY:-:0}" SSH_ASKPASS="${askpass_script}" SSH_ASKPASS_REQUIRE=force \
        ssh-add "${identity}" </dev/null
}

_tty_usable() {
    [ -e /dev/tty ] 2>/dev/null || return 1
    ( exec 3</dev/tty ) 2>/dev/null || return 1
    exec 3<&-
    return 0
}

_load_passphrase_from_systemd_cred() {
    local cred_file="${SSH_CRED_FILE}"
    local pass=""

    [ -f "${cred_file}" ] || return 1
    command -v systemd-creds >/dev/null 2>&1 || return 1

    if ! pass="$(systemd-creds --user decrypt "${cred_file}" - 2>/dev/null)"; then
        return 1
    fi
    [ -n "${pass}" ] || return 1

    SSH_PASSPHRASE="${pass}"
    export SSH_PASSPHRASE
    return 0
}

_load_ssh_key() {
    local identity="$1"
    local rc=0

    if [ -n "${SSH_PASSPHRASE:-}" ]; then
        echo "🔑 使用 SSH_PASSPHRASE 加载密钥（AI 对话框模式）..."
        echo "   密钥文件: ${identity}"
        set +e
        _add_ssh_key_with_passphrase "${identity}"
        rc=$?
        set -e
        unset SSH_PASSPHRASE
        return "${rc}"
    fi

    if _load_passphrase_from_systemd_cred; then
        echo "🔑 从 systemd 用户凭证加载 SSH 密码（${SSH_CRED_FILE}）..."
        echo "   密钥文件: ${identity}"
        set +e
        _add_ssh_key_with_passphrase "${identity}"
        rc=$?
        set -e
        unset SSH_PASSPHRASE
        return "${rc}"
    fi

    if _tty_usable; then
        echo "🔑 请一次性输入 SSH 密钥密码（本次推送全程复用）..."
        echo "   密钥文件: ${identity}"
        unset SSH_ASKPASS SSH_ASKPASS_REQUIRE
        ssh-add "${identity}" </dev/tty
        return $?
    fi

    if [ "${PASSPHRASE_FROM_STDIN:-false}" = "true" ]; then
        echo "🔑 从标准输入读取 SSH 密钥密码..."
        echo "   密钥文件: ${identity}"
        if ! read -r SSH_PASSPHRASE; then
            echo "❌ 读取 SSH 密钥密码失败。"
            return 1
        fi
        export SSH_PASSPHRASE
        _load_ssh_key "${identity}"
        return $?
    fi

    echo "DEBOPTI_SSH_PASSPHRASE_REQUIRED"
    echo "❌ 当前环境无交互终端（如 AI Agent），无法直接提示输入密码。"
    echo ""
    echo "推荐一次性配置 systemd 用户凭证（加密存于 ~/.config/credstore/）："
    echo "  bash tools/github-ssh-cred-setup.sh"
    echo ""
    echo "或在对话中提供 SSH 密钥密码后，由 Coder 执行："
    echo "  SSH_PASSPHRASE='<您的密码>' bash tools/github-push.sh"
    echo ""
    echo "或在本地终端直接运行本脚本（将提示输入密码）。"
    printf '%s\n' "亦可通过管道：printf '%s\\n' '<密码>' | bash tools/github-push.sh --passphrase-stdin"
    return 1
}

setup_ssh_once() {
    local identity=""

    mkdir -p "$(dirname "${SSH_CONTROL_PATH}")"
    _ensure_ssh_agent

    identity="$(_resolve_github_identity)" || {
        echo "❌ 未找到 GitHub SSH 私钥。"
        echo "   请在 ~/.ssh/config 中为 github.com 配置 IdentityFile，或将私钥放在 ~/.ssh/id_ed25519。"
        exit 1
    }

    set +e
    _identity_loaded "${identity}"
    local loaded=$?
    set -e

    if [ "${loaded}" -ne 0 ]; then
        if ! _load_ssh_key "${identity}"; then
            echo "❌ SSH 密钥加载失败，无法继续推送。"
            exit 1
        fi
    else
        echo "🔑 已检测到 GitHub SSH 密钥已加载，跳过重复输入。"
    fi

    export GIT_SSH_COMMAND="ssh -o ControlMaster=auto -o ControlPath=${SSH_CONTROL_PATH}-%r@%h-%p -o ControlPersist=600 -o IdentitiesOnly=yes -i ${identity}"
}

build_filter_index_cmd() {
    local cmd="git rm --cached -r --ignore-unmatch"
    local path
    for path in "${FILTER_PATHS[@]}"; do
        cmd+=" ${path}"
    done
    printf '%s' "${cmd}"
}

_jj_short_id() {
    jj log -r "$1" -T 'change_id.short()' --no-graph 2>/dev/null | tr -d '[:space:]' || true
}

_jj_main_rev() {
    if jj log -r 'main@git' -n 1 --no-graph >/dev/null 2>&1; then
        printf '%s\n' 'main@git'
        return 0
    fi
    if jj log -r 'main' -n 1 --no-graph >/dev/null 2>&1; then
        printf '%s\n' 'main'
        return 0
    fi
    printf '%s\n' '@-'
}

_resolve_main_bookmark() {
    local rev
    if jj log -r 'main' -n 1 --no-graph >/dev/null 2>&1; then
        return 0
    fi
    rev="$(_jj_main_rev)"
    echo "⚠️  bookmark main 冲突或不唯一，将其固定到 ${rev}..."
    jj bookmark set main -r "${rev}" --allow-backwards
}

_fetch_published_version() {
    local text parsed
    if text="$(git show "origin/${BRANCH_NAME}:VERSION" 2>/dev/null)"; then
        if parsed="$(debopti_parse_version_text "$text")"; then
            printf '%s\n' "${parsed%%|*}"
            return 0
        fi
    fi
    if text="$(curl -fsSL --connect-timeout 10 --max-time 20 \
        "https://raw.githubusercontent.com/${REMOTE_REPO}/main/VERSION" 2>/dev/null)"; then
        if parsed="$(debopti_parse_version_text "$text")"; then
            printf '%s\n' "${parsed%%|*}"
            return 0
        fi
    fi
    return 1
}

_fold_release_files() {
    local semver=$1
    local at_cid main_cid
    if command -v jj >/dev/null 2>&1 && [ -d "${WORKSPACE_DIR}/.jj" ]; then
        _resolve_main_bookmark
        main_cid="$(_jj_short_id "$(_jj_main_rev)")"
        at_cid="$(_jj_short_id @)"
        if [ "${main_cid}" != "${at_cid}" ]; then
            echo "   正在将发布元数据折叠进 main..."
            if ! JJ_EDITOR=true jj squash --into "$(_jj_main_rev)" --use-destination-message -- \
                VERSION CHANGELOG.md scripts/common.sh 2>/dev/null; then
                JJ_EDITOR=true jj squash --into "$(_jj_main_rev)" -- VERSION CHANGELOG.md scripts/common.sh
            fi
        fi
        jj git export >/dev/null
        git -C "${WORKSPACE_DIR}" checkout "${BRANCH_NAME}" >/dev/null 2>&1 || true
    else
        git -C "${WORKSPACE_DIR}" add VERSION CHANGELOG.md scripts/common.sh
        if ! git -C "${WORKSPACE_DIR}" diff --cached --quiet; then
            git -C "${WORKSPACE_DIR}" commit -m "chore(release): v${semver}"
        fi
    fi
}

bump_release_identity() {
    local build local_full local_semver="" local_calver="" local_build=""
    local remote_version="" today_version="" version notes
    local at_cid main_cid
    local -a bump_cmd=()

    # 1. Explicit --bump or --version: delegate to tools/release.sh bump
    if [ -n "${RELEASE_BUMP}" ] || [ -n "${RELEASE_VERSION}" ]; then
        bump_cmd=(bash "${WORKSPACE_DIR}/tools/release.sh" --repo-root "${WORKSPACE_DIR}" bump)
        if [ -n "${RELEASE_BUMP}" ]; then
            bump_cmd+=(--bump "${RELEASE_BUMP}")
        fi
        if [ -n "${RELEASE_VERSION}" ]; then
            bump_cmd+=(--version "${RELEASE_VERSION}")
        fi
        if [ -n "${RELEASE_PRERELEASE}" ]; then
            bump_cmd+=(--prerelease "${RELEASE_PRERELEASE}")
        fi
        if [ "${DRY_RUN}" = "true" ]; then
            bump_cmd+=(--dry-run)
            "${bump_cmd[@]}"
            local plan_json
            local -a plan_cmd=(bash "${WORKSPACE_DIR}/tools/release.sh" --repo-root "${WORKSPACE_DIR}" plan --json)
            [ -n "${RELEASE_BUMP}" ] && plan_cmd+=(--bump "${RELEASE_BUMP}")
            [ -n "${RELEASE_VERSION}" ] && plan_cmd+=(--version "${RELEASE_VERSION}")
            [ -n "${RELEASE_PRERELEASE}" ] && plan_cmd+=(--prerelease "${RELEASE_PRERELEASE}")
            plan_json="$("${plan_cmd[@]}")"
            PUSH_SEMVER="$(printf '%s' "${plan_json}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["next_semver"])')"
            PUSH_CALVER="$(printf '%s' "${plan_json}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["next_calver"])')"
            PUSH_BUILD="$(printf '%s' "${plan_json}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["next_build"])')"
            PUSH_VERSION="${PUSH_SEMVER:-${PUSH_CALVER}}"
            return 0
        fi
        "${bump_cmd[@]}"
        local_full="$(debopti_parse_version_file_full "${WORKSPACE_DIR}/VERSION")" || {
            echo "❌ 无法解析 bump 后的 VERSION 文件。"
            return 1
        }
        IFS='|' read -r PUSH_SEMVER PUSH_CALVER PUSH_BUILD <<< "${local_full}"
        PUSH_VERSION="${PUSH_SEMVER:-${PUSH_CALVER}}"
        _fold_release_files "${PUSH_SEMVER}"
        _verify_git_identity
        return 0
    fi

    # 2. Prepared SemVer identity already in VERSION and passing verify -> preserve verbatim
    if local_full="$(debopti_parse_version_file_full "${WORKSPACE_DIR}/VERSION" 2>/dev/null)"; then
        IFS='|' read -r local_semver local_calver local_build <<< "${local_full}"
        if [ -n "${local_semver}" ] && debopti_is_semver "${local_semver}" \
            && bash "${WORKSPACE_DIR}/tools/release.sh" --repo-root "${WORKSPACE_DIR}" verify >/dev/null 2>&1; then
            PUSH_SEMVER="${local_semver}"
            PUSH_CALVER="${local_calver}"
            PUSH_VERSION="${local_semver}"
            PUSH_BUILD="${local_build}"
            echo "📌 保留已就绪的 SemVer 发布身份: ${PUSH_SEMVER} (CalVer ${PUSH_CALVER}, build ${PUSH_BUILD})"
            if [ "${DRY_RUN}" = "true" ]; then
                return 0
            fi
            if command -v jj >/dev/null 2>&1 && [ -d "${WORKSPACE_DIR}/.jj" ]; then
                _resolve_main_bookmark
                jj git export >/dev/null
                git -C "${WORKSPACE_DIR}" checkout "${BRANCH_NAME}" >/dev/null 2>&1 || true
            fi
            _verify_git_identity
            return 0
        fi
    fi

    # 3. Backward-compatible CalVer + Build fallback
    if [ "${DRY_RUN}" = "true" ] || ! command -v jj >/dev/null 2>&1 || [ ! -d "${WORKSPACE_DIR}/.jj" ]; then
        if [ "${DRY_RUN}" != "true" ]; then
            echo "⚠️  未找到 jj，跳过自动写入 VERSION。"
        fi
        if local_full="$(debopti_parse_version_file_full "${WORKSPACE_DIR}/VERSION")"; then
            IFS='|' read -r PUSH_SEMVER PUSH_CALVER PUSH_BUILD <<< "${local_full}"
            PUSH_VERSION="${PUSH_SEMVER:-${PUSH_CALVER}}"
            return 0
        fi
        echo "❌ 无法解析 VERSION 文件。"
        return 1
    fi

    _resolve_main_bookmark

    build="$(_jj_short_id "$(_jj_main_rev)")"
    if [ -z "${build}" ]; then
        echo "❌ 无法读取 main 的 Build ID。"
        return 1
    fi

    if local_full="$(debopti_parse_version_file_full "${WORKSPACE_DIR}/VERSION")"; then
        IFS='|' read -r local_semver local_calver local_build <<< "${local_full}"
    fi

    remote_version="$(_fetch_published_version || true)"
    remote_version="$(debopti_normalize_calver "${remote_version}")"
    today_version="$(debopti_next_calver "${remote_version}")"

    if [ "${local_build}" = "${build}" ] && [ "${local_calver}" = "${today_version}" ] \
        && debopti_version_file_canonical "${WORKSPACE_DIR}/VERSION"; then
        version="${local_calver}"
        echo "📌 VERSION 已对应当前 Build: ${version} (${build})"
    else
        version="${today_version}"
        echo "📌 写入版本身份: ${version} (${build})"
        debopti_write_version_file "${WORKSPACE_DIR}/VERSION" "${version}" "${build}"
        debopti_sync_common_alias "${WORKSPACE_DIR}/scripts/common.sh" "${build}"
        notes="$(jj log -r "$(_jj_main_rev)" -T 'description.first_line()' --no-graph 2>/dev/null | tr -d '\r' || true)"
        debopti_changelog_prepend "${WORKSPACE_DIR}/CHANGELOG.md" "${version}" "${build}" "${notes}"

        main_cid="$(_jj_short_id "$(_jj_main_rev)")"
        at_cid="$(_jj_short_id @)"
        if [ "${main_cid}" != "${at_cid}" ]; then
            echo "   正在将 VERSION 折叠进 main..."
            if ! JJ_EDITOR=true jj squash --into "$(_jj_main_rev)" --use-destination-message -- \
                VERSION CHANGELOG.md scripts/common.sh 2>/dev/null; then
                JJ_EDITOR=true jj squash --into "$(_jj_main_rev)" -- VERSION CHANGELOG.md scripts/common.sh
            fi
        fi
    fi

    PUSH_SEMVER=""
    PUSH_CALVER="${version}"
    PUSH_VERSION="${version}"
    PUSH_BUILD="${build}"

    jj git export >/dev/null
    git -C "${WORKSPACE_DIR}" checkout "${BRANCH_NAME}" >/dev/null 2>&1 || true
    _verify_git_identity
}

_verify_git_identity() {
    local shown parsed version build
    if ! shown="$(git show "${BRANCH_NAME}:VERSION" 2>/dev/null)"; then
        echo "❌ Git ${BRANCH_NAME} 中缺少 VERSION，拒绝推送。"
        return 1
    fi
    if ! parsed="$(debopti_parse_version_text "${shown}")"; then
        echo "❌ Git ${BRANCH_NAME} 的 VERSION 无法解析。"
        return 1
    fi
    version="${parsed%%|*}"
    build="${parsed#*|}"
    if [ "${version}" != "${PUSH_VERSION}" ] || [ "${build}" != "${PUSH_BUILD}" ]; then
        echo "❌ Git VERSION (${version} / ${build}) 与即将发布的身份 (${PUSH_VERSION} / ${PUSH_BUILD}) 不一致。"
        return 1
    fi
    echo "✅ Git ${BRANCH_NAME} 身份: ${version} (build ${build})"
}

_verify_sandbox_tree() {
    local shown parsed version build
    if git ls-tree -r --name-only HEAD | grep -Eq '(^|/)(id_ed25519|id_rsa|.*\.cred|.*\.pem|.*\.key)$'; then
        echo "❌ HEAD 包含疑似私钥或凭证文件，已中止推送。"
        return 1
    fi
    if ! shown="$(git show HEAD:VERSION 2>/dev/null)"; then
        echo "❌ 净化后的 HEAD 缺少 VERSION。"
        return 1
    fi
    parsed="$(debopti_parse_version_text "${shown}")" || {
        echo "❌ 净化树 VERSION 无法解析。"
        return 1
    }
    version="${parsed%%|*}"
    build="${parsed#*|}"
    if [ "${version}" != "${PUSH_VERSION}" ] || [ "${build}" != "${PUSH_BUILD}" ]; then
        echo "❌ 净化树身份 (${version} / ${build}) 与发布身份 (${PUSH_VERSION} / ${PUSH_BUILD}) 不一致。"
        return 1
    fi
    echo "✅ 沙盒已净化，身份 ${version} (build ${build})"
}

_resolve_push_tag() {
    local version=$1 build=$2
    if [ -n "${PUSH_SEMVER}" ]; then
        debopti_semver_tag "${PUSH_SEMVER}"
    else
        debopti_tag_name "${version}" "${build}"
    fi
}

_verify_published() {
    local version=$1 build=$2
    local tag remote_text parsed remote_ver remote_build
    local common_id
    tag="$(_resolve_push_tag "${version}" "${build}")"

    echo "🔍 正在复核 GitHub 公开身份..."
    remote_text="$(curl -fsSL --connect-timeout 10 --max-time 20 \
        "https://raw.githubusercontent.com/${REMOTE_REPO}/main/VERSION?$(date +%s)" 2>/dev/null || true)"
    if parsed="$(debopti_parse_version_text "${remote_text}" 2>/dev/null)"; then
        remote_ver="${parsed%%|*}"
        remote_build="${parsed#*|}"
        if [ "${remote_ver}" = "${version}" ] && [ "${remote_build}" = "${build}" ]; then
            echo "✅ GitHub VERSION 已同步: ${remote_ver} (build ${remote_build})"
        else
            echo "⚠️  GitHub VERSION 为 ${remote_ver} (build ${remote_build})，本地发布为 ${version} (build ${build})（CDN 可能延迟）。"
        fi
    else
        echo "⚠️  暂时无法拉取 GitHub VERSION（网络或 CDN）。"
    fi

    common_id="$(curl -fsSL --connect-timeout 10 --max-time 20 \
        "https://raw.githubusercontent.com/${REMOTE_REPO}/main/scripts/common.sh?$(date +%s)" 2>/dev/null \
        | grep '^VERSION_ID=' | head -n 1 | cut -d'"' -f2 || true)"
    if [ -n "${common_id}" ]; then
        if [ "${common_id}" = "${build}" ]; then
            echo "✅ 旧客户端别名 VERSION_ID=${common_id}"
        else
            echo "⚠️  VERSION_ID=${common_id}，期望 ${build}（CDN 可能延迟）。"
        fi
    fi

    if git ls-remote --tags github "refs/tags/${tag}" 2>/dev/null | grep -q "refs/tags/${tag}"; then
        echo "✅ 远程 tag ${tag} 存在"
    else
        echo "⚠️  远程尚未看到 tag ${tag}"
    fi
}

_git_annotated_tag() {
    local tag=$1
    local message=$2
    local target=${3:-HEAD}
    local name email
    name="$(jj log -r "$(_jj_main_rev)" -T 'author.name()' --no-graph 2>/dev/null || true)"
    email="$(jj log -r "$(_jj_main_rev)" -T 'author.email()' --no-graph 2>/dev/null || true)"
    [ -n "${name}" ] || name="$(git config user.name 2>/dev/null || true)"
    [ -n "${email}" ] || email="$(git config user.email 2>/dev/null || true)"
    [ -n "${name}" ] || name="debopty"
    [ -n "${email}" ] || email="noreply@users.noreply.github.com"
    git -c "user.name=${name}" -c "user.email=${email}" tag -a "${tag}" -m "${message}" "${target}"
}

publish_tag_and_release() {
    local version=$1
    local build=$2
    local tag notes dist_dir archive sums
    local -a gh_flags=()

    if [ -z "${version}" ]; then
        echo "⚠️  无版本号，跳过 tag / Release。"
        return 0
    fi

    tag="$(_resolve_push_tag "${version}" "${build}")"

    if git rev-parse "${tag}" >/dev/null 2>&1; then
        echo "🏷️  沙盒已有 tag ${tag}。"
    else
        _git_annotated_tag "${tag}" "debopty ${version} (${build})"
    fi

    if git ls-remote --tags github "refs/tags/${tag}" 2>/dev/null | grep -q "refs/tags/${tag}"; then
        echo "🏷️  远程已有 tag ${tag}，不改写。"
    else
        echo "🏷️  推送 tag ${tag}..."
        git push github "${tag}"
    fi

    notes=""
    if [ -n "${PUSH_SEMVER}" ] && [ -f "${WORKSPACE_DIR}/tools/release.sh" ]; then
        notes="$(bash "${WORKSPACE_DIR}/tools/release.sh" --repo-root "${WORKSPACE_DIR}" notes --version "${PUSH_SEMVER}" 2>/dev/null || true)"
    fi
    if [ -z "${notes}" ]; then
        notes="$(cd "${WORKSPACE_DIR}" && jj log -r "$(_jj_main_rev)" -T 'description' --no-graph 2>/dev/null || true)"
    fi
    if [ -z "${notes}" ]; then
        notes="debopty ${version}"
    fi

    if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
        if gh release view "${tag}" --repo "${REMOTE_REPO}" >/dev/null 2>&1; then
            echo "📦 GitHub Release ${tag} 已存在，跳过。"
        else
            echo "📦 创建 GitHub Release ${tag}..."
            if [[ "${PUSH_SEMVER}" == *-* ]]; then
                gh_flags+=(--prerelease)
            else
                gh_flags+=(--latest)
            fi
            dist_dir="$(mktemp -d "${TMPDIR:-/tmp}/debopty-dist.XXXXXX")"
            archive="${dist_dir}/debopty-${tag}.tar.gz"
            sums="${dist_dir}/SHA256SUMS"
            tar -czf "${archive}" \
                --exclude='.git' \
                --exclude='.github' \
                --exclude='.jj' \
                --exclude='.cache' \
                --exclude='tools' \
                --exclude='governance' \
                --exclude='INDEX.md' \
                --exclude='instructions' \
                --exclude='tests' \
                --exclude='Makefile' \
                --exclude='AGENTS.md' \
                --exclude='CLAUDE.md' \
                --exclude='GEMINI.md' \
                --exclude='.shellcheckrc' \
                --exclude='.editorconfig' \
                --exclude='.gitignore' \
                .
            (cd "${dist_dir}" && sha256sum "debopty-${tag}.tar.gz" > SHA256SUMS)
            gh release create "${tag}" \
                "${archive}" "${sums}" \
                --repo "${REMOTE_REPO}" \
                --title "${tag}" \
                --notes "${notes}" \
                "${gh_flags[@]}"
            rm -rf "${dist_dir}"
        fi
    else
        echo "ℹ️  未检测到已登录的 gh，仅推送 tag。"
    fi
}

echo "========================================================"
echo "🛡️  开始执行 GitHub 历史脱敏安全推送流程..."
echo "========================================================"

# 解析命令行参数
FORCE_RUN=false
PASSPHRASE_FROM_STDIN=false
TAG_ONLY=false
DRY_RUN=false
SKIP_CHECK=false
RELEASE_BUMP=""
RELEASE_VERSION=""
RELEASE_PRERELEASE=""
while [ $# -gt 0 ]; do
    case "$1" in
        -f|--force)
            FORCE_RUN=true
            shift
            ;;
        --passphrase-stdin)
            PASSPHRASE_FROM_STDIN=true
            shift
            ;;
        --tag-only)
            TAG_ONLY=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --skip-check)
            SKIP_CHECK=true
            shift
            ;;
        --bump)
            RELEASE_BUMP="${2:-}"
            shift 2
            ;;
        --bump=*)
            RELEASE_BUMP="${1#*=}"
            shift
            ;;
        --version)
            RELEASE_VERSION="${2:-}"
            shift 2
            ;;
        --version=*)
            RELEASE_VERSION="${1#*=}"
            shift
            ;;
        --prerelease)
            RELEASE_PRERELEASE="${2:-}"
            shift 2
            ;;
        --prerelease=*)
            RELEASE_PRERELEASE="${1#*=}"
            shift
            ;;
        *)
            echo "❌ 未知参数: $1" >&2
            exit 1
            ;;
    esac
done

cd "${WORKSPACE_DIR}"

if [ "${SKIP_CHECK}" != "true" ] && [ "${TAG_ONLY}" != "true" ] && [ -f "${WORKSPACE_DIR}/tools/release.sh" ]; then
    echo "🔎 运行发布前元数据一致性门禁 (tools/release.sh verify)..."
    bash "${WORKSPACE_DIR}/tools/release.sh" --repo-root "${WORKSPACE_DIR}" verify
fi

if [ "${DRY_RUN}" = "true" ]; then
    bump_release_identity
    target_tag="$(_resolve_push_tag "${PUSH_VERSION}" "${PUSH_BUILD}")"
    echo "========================================================"
    echo "🧪 [dry-run] 预演完成（未修改远端，未连接 SSH）"
    echo "ℹ️  目标仓库 : ${REMOTE_REPO}"
    echo "ℹ️  SemVer   : ${PUSH_SEMVER:-(none)}"
    echo "ℹ️  CalVer   : ${PUSH_CALVER:-(none)}"
    echo "ℹ️  Build    : ${PUSH_BUILD:-(none)}"
    echo "ℹ️  Tag      : ${target_tag}"
    echo "========================================================"
    exit 0
fi

setup_ssh_once

if [ "${TAG_ONLY}" = "true" ]; then
    echo "🏷️  仅发布 tag（不重写 main）..."
    local_full="$(debopti_parse_version_file_full "${WORKSPACE_DIR}/VERSION")" || {
        echo "❌ 无法解析本地 VERSION。"
        exit 1
    }
    IFS='|' read -r PUSH_SEMVER PUSH_CALVER PUSH_BUILD <<< "${local_full}"
    PUSH_VERSION="${PUSH_SEMVER:-${PUSH_CALVER}}"
    git remote get-url github >/dev/null 2>&1 || git remote add github "${GITHUB_REMOTE}"
    git fetch github "${BRANCH_NAME}" --quiet
    tag="$(_resolve_push_tag "${PUSH_VERSION}" "${PUSH_BUILD}")"
    if git ls-remote --tags github "refs/tags/${tag}" 2>/dev/null | grep -q "refs/tags/${tag}"; then
        echo "🏷️  远程已有 tag ${tag}，不改写。"
    else
        if git rev-parse "${tag}" >/dev/null 2>&1; then
            git tag -d "${tag}" >/dev/null 2>&1 || true
        fi
        _git_annotated_tag "${tag}" "debopty ${PUSH_VERSION} (${PUSH_BUILD})" "github/${BRANCH_NAME}"
        echo "🏷️  推送 tag ${tag}..."
        git push github "${tag}"
    fi
    _verify_published "${PUSH_VERSION}" "${PUSH_BUILD}"
    echo "========================================================"
    echo "🎉 tag 发布流程完成：${PUSH_VERSION} (build ${PUSH_BUILD}, tag ${tag})"
    echo "========================================================"
    exit 0
fi

cd "${WORKSPACE_DIR}"

# 2. 自动写入 CalVer + Build，再决定是否跳过
echo "🔍 正在检查本地与远程仓库的状态..."
git fetch origin "${BRANCH_NAME}" --quiet 2>/dev/null || true
bump_release_identity

if git rev-parse "${BRANCH_NAME}" >/dev/null 2>&1; then
    LOCAL_HASH=$(git rev-parse "${BRANCH_NAME}")
    REMOTE_HASH=$(git rev-parse "origin/${BRANCH_NAME}" 2>/dev/null || echo "")

    if [ "${LOCAL_HASH}" = "${REMOTE_HASH}" ] && [ "${FORCE_RUN}" != "true" ]; then
        echo "✅ 本地分支与远程分支完全一致，没有检测到新提交，跳过推送。"
        echo "💡 提示：若远程仓库未脱敏且需要强制重写，请运行：$0 --force"
        echo "========================================================"
        exit 0
    fi
    if [ "${FORCE_RUN}" = "true" ]; then
        echo "💡 检测到 --force 参数，强制启动脱敏流程..."
    else
        echo "💡 检测到本地有未推送的更新，准备启动脱敏流程..."
    fi
else
    echo "⚠️  无法解析本地 ${BRANCH_NAME}，将直接启动脱敏重写..."
fi

# 3. 清理与异常捕获
cleanup() {
    if [ -d "${SANDBOX_DIR}" ]; then
        echo "🧹 安全清理沙盒临时目录..."
        rm -rf "${SANDBOX_DIR}"
    fi
    rm -f "${SSH_CONTROL_PATH}"-* 2>/dev/null || true
}
trap cleanup EXIT

# 4. 创建隔离沙盒克隆
echo "📦 正在创建本地 Git 阴影克隆 (隔离沙盒)..."
git clone --no-hardlinks "${WORKSPACE_DIR}" "${SANDBOX_DIR}"

# 5. 进入沙盒并执行脱敏重写
cd "${SANDBOX_DIR}"

# 强制检出需要推送的目标分支
echo "🎋 正在切换到 ${BRANCH_NAME} 分支..."
git checkout "${BRANCH_NAME}"

if [ "${#FILTER_PATHS[@]}" -gt 0 ]; then
    FILTER_INDEX_CMD="$(build_filter_index_cmd)"
    echo "✂️ 正在重写 Git 提交历史，仅清理已废弃的私有历史目录（instructions/）..."
    export FILTER_BRANCH_SQUELCH_WARNING=1

    if [ "${ENABLE_GPG_SIGNING}" = "true" ]; then
        echo "🔑 已启用 PGP 重新签名，正在重写并对 Commit 进行加密签名..."
        git filter-branch --force \
            --index-filter "${FILTER_INDEX_CMD}" \
            --commit-filter 'git commit-tree -S "$@"' \
            "${BRANCH_NAME}"
    else
        git filter-branch --force \
            --index-filter "${FILTER_INDEX_CMD}" \
            "${BRANCH_NAME}"
    fi
else
    echo "ℹ️ 当前未配置历史目录过滤，仅执行用户隐私与密钥泄露门禁校验。"
fi

_verify_sandbox_tree

# 6. 添加 GitHub 远程源并推送
echo "🚀 正在向 GitHub 安全推送净化后的历史记录..."
git remote add github "${GITHUB_REMOTE}"

# 执行拉取以刷新本地对 github/main 的引用，避免 --force-with-lease 出现 stale info 报错
echo "📥 正在同步远程仓库状态..."
git fetch github "${BRANCH_NAME}" --quiet || true

# 使用安全强推，保护远程分支不受未知的覆盖风险
git push github "${BRANCH_NAME}" --force-with-lease

echo "🏷️  正在发布 tag / Release..."
publish_tag_and_release "${PUSH_VERSION}" "${PUSH_BUILD}"
_verify_published "${PUSH_VERSION}" "${PUSH_BUILD}"

# 7. 清理现场
echo "🧹 推送成功！正在返回工作区..."
cd "${WORKSPACE_DIR}"

echo "========================================================"
echo "🎉 恭喜！脱敏推送成功完成！"
echo "ℹ️  版本: ${PUSH_VERSION:-unknown} (build ${PUSH_BUILD:-unknown})"
echo "ℹ️  GitHub 上的提交历史已完全净化，且本地 jj 版本控制完美保持原样。"
echo "========================================================"
