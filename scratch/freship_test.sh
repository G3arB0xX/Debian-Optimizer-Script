#!/bin/bash
# FreshIP V3.1 Test Harness (Modern Logging & Logic Validation)

# Mock Environment
export REGION_CODE="JP"
export UTC_OFFSET="+9"
export BIND_IPV4="127.0.0.1"
export BIND_IPV6="::1"
export INSTANCE_MODE="v4"
export INSTALL_DIR="/tmp/freship_test"
export DATA_DIR="/tmp/freship_test/data"

mkdir -p "$INSTALL_DIR/bin" "$DATA_DIR/keywords" "$DATA_DIR/regions"

# Mock Data
echo "test keyword" > "$DATA_DIR/keywords/kw_JP.txt"
echo '{"trust_module": {"white_urls": ["https://news.google.com"], "static_urls": ["https://www.gov.jp"]}}' > "$DATA_DIR/regions/tokyo.json"
echo "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36" > "$DATA_DIR/user_agents.txt"

# Mock curl (dummy)
cat > "$INSTALL_DIR/bin/curl_mock" << 'EOF'
#!/bin/bash
echo "200"
EOF
chmod +x "$INSTALL_DIR/bin/curl_mock"

# Scenario Tester Function
test_scenario() {
    local test_date=$1
    local test_hour_utc=$2
    
    echo "================================================================"
    echo "Testing V3.1 Scenario: Date=$test_date, UTC_Hour=$test_hour_utc"
    
    /bin/bash << EOF
    # Injected environment
    UTC_OFFSET="$UTC_OFFSET"
    REGION_CODE="$REGION_CODE"
    INSTALL_DIR="$INSTALL_DIR"
    DATA_DIR="$DATA_DIR"
    INSTANCE_MODE="$INSTANCE_MODE"
    
    # Modern Log Function (V3.1)
    log() {
        local type=\$1
        local msg=\$2
        local icon="ℹ️"
        case "\$type" in
            START) icon="🚀" ;;
            INFO)  icon="📊" ;;
            SLEEP) icon="🌙" ;;
            SUCCESS) icon="✅" ;;
            ERROR) icon="❌" ;;
            ACTION) icon="🔗" ;;
        esac
        echo -e "[FreshIP] \$icon | 21:15:35 | \$INSTANCE_MODE | \$REGION_CODE | \$msg"
    }

    # Mock date within this shell
    date() {
        if [[ "\$1" == "-u" ]]; then
            local offset=\${3% hours}
            local h=\$(( $test_hour_utc + offset ))
            printf "%02d" \$(( (h + 24) % 24 ))
        elif [[ "\$1" == "+%Y%m%d" ]]; then
            echo "$test_date"
        else
            /bin/date "\$@"
        fi
    }
    
    # Night Silence
    LOCAL_HOUR=\$(date -u -d "\${UTC_OFFSET:-+0} hours" +%H)
    if [ "\$LOCAL_HOUR" -ge 1 ] && [ "\$LOCAL_HOUR" -le 6 ]; then
        log "SLEEP" "处于目标地区深夜 (\$LOCAL_HOUR:00)，进入休眠模式。"
        exit 0
    fi
    
    # Activity Level
    DAILY_SEED=\$(echo "\$(date +%Y%m%d)" | cksum | awk '{print \$1}')
    ACTIVITY_LEVEL=\$(( DAILY_SEED % 100 ))
    
    # Start Task
    log "START" "启动养护任务 (活跃度: \$ACTIVITY_LEVEL%)"
    
    # Simulated Action
    CURL_BIN="$INSTALL_DIR/bin/curl_mock"
    TLS_MODE="Mock-TLS"
    URL="https://example.com/test"
    code="200"
    log "ACTION" "[SEARCH] 响应码: \$code | TLS: \$TLS_MODE | URL: \${URL:0:40}..."
    
    log "SUCCESS" "养护流程执行完毕。"
EOF
}

# Run Scenarios
test_scenario "20260507" 18 # Night Silence (+9 offset -> 03 local)
test_scenario "20260507" 06 # Normal daytime (+9 offset -> 15 local)
