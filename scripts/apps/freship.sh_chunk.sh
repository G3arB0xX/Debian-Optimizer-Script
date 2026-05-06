    # 5. 资产同步与核心脚本去痕迹化 Patch
    info "同步全球养护资产并执行核心逻辑 Patch..."
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    
    # 首次数据拉取 (原子操作模式由 updater 负责后续维护)
    local region_target="${opt_dir}/data/regions/${city_id//\//.}.json"
    curl -sL "${repo_raw}/data/regions/${country_id}/${city_id}.json" -o "$region_target" 2>/dev/null || \
    curl -sL "${repo_raw}/data/regions/${country_id}/$(basename "$city_id").json" -o "$region_target" 2>/dev/null
    curl -sL "${repo_raw}/data/keywords/${kw_filename}" -o "${opt_dir}/data/keywords/${kw_filename}"
    curl -sL "${repo_raw}/data/user_agents.txt" -o "${opt_dir}/data/user_agents.txt"

    # 复制并深度 Patch 核心脚本
    local core_script="${opt_dir}/core/freship_core.sh"
    info "正在拉取核心逻辑引擎..."
    download_with_fallback "$core_script" "${repo_raw}/sentinel.sh" || return 1
    
    # 执行品牌重置与路径校准
    sed -i "s|IP-Sentinel|FreshIP|g; s|ip_sentinel|freship|g; s|sentinel|freship|g; s|Sentinel|FreshIP|g" "$core_script"
    sed -i "s|INSTALL_DIR=\"/opt/ip_sentinel\"|INSTALL_DIR=\"${opt_dir}\"|g" "$core_script"
    sed -i "s|CONFIG_FILE=\"\${INSTALL_DIR}/config.conf\"|CONFIG_FILE=\"${config_file}\"|g" "$core_script"
    sed -i "s|LOG_FILE=\"\${INSTALL_DIR}/logs/sentinel.log\"|LOG_FILE=\"${log_dir}/freship.log\"|g" "$core_script"
    sed -i "s|LOCK_FILE=\"/tmp/ip_sentinel.lock\"|LOCK_FILE=\"/tmp/freship_\\\${INSTANCE_MODE:-global}.lock\"|g" "$core_script"
