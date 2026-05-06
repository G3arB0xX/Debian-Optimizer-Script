#!/bin/bash
# =========================================================
# 全局网络探测与高可用下载/克隆模块 (镜像池自动化版)
# =========================================================

# ----------------- 网络环境自举 -----------------
# 采用多节点冗灾探测，确保在不同服务商网络下均能精准识别归属地
global_netcheck() {
    # 幂等保护：如果配置已加载，跳过探测以节省启动时间 (约 2-3秒)
    if [[ -n "${IS_CN_REGION:-}" ]]; then
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

    # 级联探测逻辑：ipinfo -> ipip.net -> cip.cc -> ip.sb
    # 只要任意一个节点明确返回 CN 或中国，即判定为大陆环境
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
    else
        info "服务器位于海外地区，将保持官方直连模式。"
    fi

    # 状态持久化：写入配置文件以供后续运行参考
    save_project_config "IS_CN_REGION" "$IS_CN_REGION"
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
            "prefix|https://ghp.ci"                         
            "prefix|https://ghfast.top"                     
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
