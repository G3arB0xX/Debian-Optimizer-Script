#!/bin/bash
# =========================================================
# 全局网络探测与高可用下载/克隆模块 (镜像池自动化版)
# =========================================================

# ----------------- DNS 健康检查与自动修复 -----------------
# 两阶段调用:
#   initial   - global_netcheck 之前调用，地区未知，仅用 1.1.1.1 临时救活
#   calibrate - global_netcheck 之后调用，根据地区与 IP 栈完整写入最终配置
#
# IPv6 检测采用双轨策略（参照 freship.sh）：
#   第 1 轨：本地网卡 global scope 地址逐一验证连通性（绑定 --interface）
#   第 2 轨：外部 API 探测（回退兜底）
check_and_fix_dns() {
    local mode="${1:-initial}"
    local resolv="/etc/resolv.conf"
    local backup="/etc/resolv.conf.debopti.bak"
    local marker="# managed-by-debopti"

    # ---- initial 阶段：DNS 故障时用 1.1.1.1 临时救活 ----
    if [[ "$mode" == "initial" ]]; then
        # 使用 getent hosts (libc6 内置，在 curl 安装前即可用) 检测 DNS
        # getent 返回非零 = DNS 解析失败，精确可靠
        if getent hosts cloudflare.com >/dev/null 2>&1; then
            return 0  # DNS 正常，不做任何修改
        fi
        warn "检测到 DNS 解析故障，正在写入 1.1.1.1 作为临时 DNS..."
        # 备份原始 resolv.conf（仅首次）
        [[ ! -f "$backup" ]] && cp "$resolv" "$backup" 2>/dev/null || true
        printf '%s\n' "$marker" "nameserver 1.1.1.1" > "$resolv"
        info "临时 DNS 已写入，后续将根据地区完成校准。"
        return 0
    fi

    # ---- calibrate 阶段：仅当 resolv.conf 由本脚本管理时才执行校准 ----
    # 若 DNS 从未故障（无标记），则不修改用户的原始配置
    if ! grep -q "$marker" "$resolv" 2>/dev/null; then
        return 0
    fi

    info "正在根据地区校准 DNS 配置..."

    # 检测 IPv4 连通性
    local has_v4=false
    curl -4 -s -m 5 -o /dev/null "http://1.1.1.1" 2>/dev/null && has_v4=true

    # 检测 IPv6 连通性（双轨）
    local has_v6=false
    local v6_candidates=()
    mapfile -t v6_candidates < <(
        ip -6 addr show scope global 2>/dev/null \
        | grep "inet6" \
        | grep -v "temporary\|deprecated" \
        | grep -oP '(?<=inet6 )[\da-f:]+(?=/)' \
        | grep -v '^::1' \
        | grep -v '^fe80'
    )
    # 第 1 轨：绑定本地地址逐一验证
    for v6_addr in "${v6_candidates[@]}"; do
        if curl -6 -s -m 6 -o /dev/null \
               --interface "$v6_addr" \
               "https://ipv6.google.com" 2>/dev/null; then
            has_v6=true
            break
        fi
    done
    # 第 2 轨：外部 API 回退（仅在第 1 轨失败时）
    if [[ "$has_v6" == "false" ]]; then
        local v6_check
        v6_check=$(curl -6 -s -m 8 api.ip.sb/ip 2>/dev/null \
                 || curl -6 -s -m 8 icanhazip.com 2>/dev/null \
                 || echo "")
        v6_check=$(echo "$v6_check" | tr -d '[:space:]')
        [[ -n "$v6_check" ]] && has_v6=true
    fi

    # 根据地区与栈类型构建 nameserver 列表
    # 海外：8.8.8.8(v4 优先) → 1.1.1.1 | 2001:4860:... → 2606:4700:...
    # 境内：223.5.5.5 → 119.29.29.29 | 2402:4e00:: → 2400:3200::1
    local ns_list=()
    if [[ "${IS_CN_REGION:-false}" == "true" ]]; then
        [[ "$has_v4" == "true" ]] && ns_list+=("nameserver 223.5.5.5" "nameserver 119.29.29.29")
        [[ "$has_v6" == "true" ]] && ns_list+=("nameserver 2402:4e00::" "nameserver 2400:3200::1")
    else
        [[ "$has_v4" == "true" ]] && ns_list+=("nameserver 8.8.8.8" "nameserver 1.1.1.1")
        [[ "$has_v6" == "true" ]] && ns_list+=("nameserver 2001:4860:4860::8888" "nameserver 2606:4700:4700::1111")
    fi

    if [[ ${#ns_list[@]} -eq 0 ]]; then
        warn "无法确定可用 IP 栈，DNS 配置维持当前状态。"
        return 0
    fi

    # 保留原始文件中的 domain/search/options 声明，仅重写 nameserver
    local extra_lines
    extra_lines=$(grep -E '^(domain|search|options)' "$resolv" 2>/dev/null || true)

    {
        printf '%s\n' "$marker"
        [[ -n "$extra_lines" ]] && printf '%s\n' "$extra_lines"
        printf '%s\n' "${ns_list[@]}"
    } > "$resolv"

    local region_label
    [[ "${IS_CN_REGION:-false}" == "true" ]] && region_label="中国大陆" || region_label="海外"
    success "DNS 校准完成（地区: ${region_label}，v4: ${has_v4}，v6: ${has_v6}）。"
}

# ----------------- 网络环境自举 -----------------

# 采用多节点冗灾探测，确保在不同服务商网络下均能精准识别归属地
global_netcheck() {
    # 幂等保护：如果配置已加载，确保环境变量导出后直接返回
    if [[ -n "${IS_CN_REGION:-}" ]]; then
        if [[ "$IS_CN_REGION" == "true" ]]; then
            export RUSTUP_DIST_SERVER="https://rsproxy.cn"
            export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
            export GOPROXY="https://goproxy.cn,direct"
        fi
        return
    fi

    # 预检依赖：curl 是网络自举的唯一核心依赖
    if ! command -v curl >/dev/null 2>&1; then
        info "正在补齐核心网络依赖 (curl)..."
        apt-get update -yq >/dev/null 2>&1
        apt-get install -yq curl >/dev/null 2>&1
    fi

    info "正在探测服务器网络环境 (全球多节点冗灾)..."
    IS_CN_REGION="false"
    
    # 使用标准浏览器 UA 绕过基础的安全组过滤
    local UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"

    # 级联探测逻辑
    if [[ "$(curl -sL --connect-timeout 3 -A "$UA" https://ipinfo.io/country 2>/dev/null)" == *"CN"* ]]; then
        IS_CN_REGION="true"
    elif curl -sL --connect-timeout 3 -A "$UA" http://myip.ipip.net 2>/dev/null | grep -q "中国"; then
        IS_CN_REGION="true"
    elif curl -sL --connect-timeout 3 -A "$UA" https://cip.cc 2>/dev/null | grep -q "中国"; then
        IS_CN_REGION="true"
    elif curl -sL --connect-timeout 3 -A "$UA" https://api.ip.sb/geoip 2>/dev/null | grep -i -q "China"; then
        IS_CN_REGION="true"
    fi

    if [[ "$IS_CN_REGION" == "true" ]]; then
        warn "检测到服务器位于中国大陆，将全局开启镜像加速模式。"
        # 强制导出环境变量供子进程（如 go, rust 安装脚本）使用
        export RUSTUP_DIST_SERVER="https://rsproxy.cn"
        export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
        export GOPROXY="https://goproxy.cn,direct"
    else
        info "服务器位于海外地区，将保持官方直连模式。"
    fi

    # 状态持久化：写入配置文件以供后续运行参考
    save_project_config "IS_CN_REGION" "$IS_CN_REGION"
    return 0
}

# ----------------- GitHub 版本获取增强 -----------------
# 参数: user/repo
get_latest_github_release() {
    local repo=$1
    local version=""
    local UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
    
    # 优先尝试 API (大陆环境下使用 ghp.ci 代理)
    local api_url="https://api.github.com/repos/${repo}/releases/latest"
    [[ "$IS_CN_REGION" == "true" ]] && api_url="https://ghproxy.homeboyc.cn/https://api.github.com/repos/${repo}/releases/latest"
    
    version=$(curl -sL --connect-timeout 5 -A "$UA" "$api_url" 2>/dev/null | grep '"tag_name":' | head -n1 | awk -F '"' '{print $4}')
    
    # 兜底逻辑：如果 API 失败或返回格式不对，尝试通过网页爬取最新 Tag
    if [[ ! "$version" =~ ^v?[0-9] ]]; then
        local target_page="https://github.com/${repo}/releases/latest"
        [[ "$IS_CN_REGION" == "true" ]] && target_page="https://ghfast.top/https://github.com/${repo}/releases/latest"
        version=$(curl -sL --connect-timeout 5 "$target_page" 2>/dev/null | grep -oE 'tag/v?[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?' | head -n1 | cut -d/ -f2)
    fi
    
    # 最终防御性过滤
    if [[ "$version" =~ ^v?[0-9] ]]; then
        echo "$version"
    else
        echo ""
    fi
}

# ----------------- 高可用下载模块 -----------------
# 针对国内 GitHub 访问困难问题，内置镜像池轮询机制
download_with_fallback() {
    local target_file=$1
    local original_url=$2
    
    # 5秒连接超时，120秒传输上限，防止进程永久挂起
    local curl_opts="-fsSL --connect-timeout 5 --max-time 120"

    # 大陆环境下的镜像轮询策略
    if [[ "$IS_CN_REGION" == "true" ]] && [[ "$original_url" =~ github\.com|githubusercontent\.com ]]; then
        info "开启 GitHub 下载镜像轮询下载..."
        
        # 优先级：CDN (jsDelivr) -> 专用代理 (ghp.ci/ghfast) -> 教育网镜像 (nuaa) -> 老牌代理 (kkgithub)
        local mirrors=(
            "jsdelivr|"                                     
            "prefix|https://ghfast.top" 
            "prefix|https://ghproxy.homeboyc.cn"                 
            "replace|github.com|hub.nuaa.cf"                
            "replace|raw.githubusercontent.com|raw.nuaa.cf" 
            "replace|github.com|kkgithub.com"     
            "prefix|https://moeyy.cn/gh-proxy"                        
        )
        
        local success="false"
        for mirror_conf in "${mirrors[@]}"; do
            local mode="${mirror_conf%%|*}"
            local rest="${mirror_conf#*|}"
            local dl_url=""
            
            # jsDelivr 模式：通过重组 URL 利用其强大的边缘缓存
            if [[ "$mode" == "jsdelivr" ]]; then
                if [[ "$original_url" =~ ^https://raw\.githubusercontent\.com/([^/]+)/([^/]+)/([^/]+)/(.*)$ ]]; then
                    dl_url="https://cdn.jsdelivr.net/gh/${BASH_REMATCH[1]}/${BASH_REMATCH[2]}@${BASH_REMATCH[3]}/${BASH_REMATCH[4]}"
                else
                    continue 
                fi
            elif [[ "$mode" == "prefix" ]]; then
                dl_url="${rest}/${original_url}"
            elif [[ "$mode" == "replace" ]]; then
                local target="${rest%%|*}"
                local replacement="${rest#*|}"
                dl_url="${original_url/${target}/${replacement}}"
                [[ "$dl_url" == "$original_url" ]] && continue
            fi
            
            info "尝试镜像节点: $dl_url"
            # 使用 -w "%{http_code}" 捕获代理服务器返回的 502 等错误，防止 curl 误判
            if curl $curl_opts -o "$target_file" "$dl_url" -w "%{http_code}" | grep -q "^20"; then
                success="true"
                break
            else
                rm -f "$target_file" 
            fi
        done
        
        [[ "$success" == "true" ]] && return 0 || { err "GitHub 镜像池全线失效，请检查网络。"; return 1; }
    else
        # 海外环境或非 GitHub 链接直接官网下载
        info "优先执行官网直连下载: $original_url"
        if ! curl $curl_opts -o "$target_file" "$original_url"; then
            rm -f "$target_file"
            err "下载失败，请检查目标链接有效性。"
            return 1
        fi
    fi
}

# ----------------- 高可用 Git Clone 模块 -----------------
git_clone_with_fallback() {
    local target_dir=$1
    local repo_url=$2
    shift 2
    local extra_args=("$@") 

    # 安全检查：防止 rm -rf 误伤核心系统目录
    if [[ -z "$target_dir" || "$target_dir" == "/" || "$target_dir" == "/usr" || "$target_dir" == "/etc" ]]; then
        err "安全预警：尝试操作受保护的系统目录 ($target_dir)！"
        return 1
    fi

    # Git 底层断流保护：连续 10秒 速度低于 1k/s 则自动断开重试
    export GIT_HTTP_LOW_SPEED_LIMIT=1000
    export GIT_HTTP_LOW_SPEED_TIME=10
    export GIT_TERMINAL_PROMPT=0 # 禁止交互式密码弹窗

    if [[ "$IS_CN_REGION" == "true" ]] && [[ "$repo_url" =~ github\.com ]]; then
        info "开启 Git 镜像链式轮询..."
        # 强制使用 HTTP/1.1 以避开部分镜像站 HTTP/2 的帧限制问题
        git config --global http.version HTTP/1.1

        local mirrors=(
            "replace|github.com|gitclone.com/github.com"  
            "replace|github.com|hub.nuaa.cf"              
            "prefix|https://ghfast.top"                   
            "replace|github.com|kkgithub.com"             
        )

        local success="false"
        for mirror_conf in "${mirrors[@]}"; do
            local mode="${mirror_conf%%|*}"
            local rest="${mirror_conf#*|}"
            local clone_url=""

            [[ "$mode" == "prefix" ]] && clone_url="${rest}/${repo_url}"
            [[ "$mode" == "replace" ]] && clone_url="${repo_url/${rest%%|*}/${rest#*|}}"

            info "尝试从镜像站拉取源码: $clone_url"
            rm -rf "$target_dir"
            
            if git clone "${extra_args[@]}" "$clone_url" "$target_dir"; then
                success="true"
                # 克隆完成后，必须将 remote 修改回官方 GitHub，确保后续 git pull 正常
                git -C "$target_dir" remote set-url origin "$repo_url"
                break
            fi
        done
        [[ "$success" == "true" ]] && return 0 || { err "Git 镜像拉取彻底失败。"; return 1; }
    else
        info "官网直连拉取: $repo_url"
        rm -rf "$target_dir"
        git clone "${extra_args[@]}" "$repo_url" "$target_dir" || return 1
    fi
}
