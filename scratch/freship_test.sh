#!/bin/bash
# FreshIP V3.0 Test Harness
# Use this to simulate different environments and scenarios

# Mock Environment
export REGION_CODE="JP"
export UTC_OFFSET="+9"
export BIND_IPV4="127.0.0.1"
export BIND_IPV6="::1"
export INSTANCE_MODE="v4"
export INSTALL_DIR="/tmp/freship_test"
export LOG_FILE="/tmp/freship_test/freship.log"
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
    local ua_type=$3 # mobile or desktop
    
    echo "------------------------------------------------"
    echo "Testing Scenario: Date=$test_date, UTC_Hour=$test_hour_utc, UA=$ua_type"
    
    # Override date command for the subshell
    date() {
        if [[ "$1" == "-u" ]]; then
            # Logic for UTC offset
            local offset=${3% hours}
            local h=$(( test_hour_utc + offset ))
            printf "%02d" $(( (h + 24) % 24 ))
        elif [[ "$1" == "+%Y%m%d" ]]; then
            echo "$test_date"
        else
            command date "$@"
        fi
    }
    export -f date

    # Run core logic (mocked)
    # We'll just run a trimmed version of the core script
    /bin/bash << EOF
    # Injected environment
    UTC_OFFSET="$UTC_OFFSET"
    REGION_CODE="$REGION_CODE"
    INSTALL_DIR="$INSTALL_DIR"
    DATA_DIR="$DATA_DIR"
    LOG_FILE="$LOG_FILE"
    
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
    
    # Mock jq and curl
    CURL_BIN="$INSTALL_DIR/bin/curl_mock"
    REGION_JSON="$DATA_DIR/regions/tokyo.json"
    
    # Logic 1: Night Silence
    LOCAL_HOUR=\$(date -u -d "\${UTC_OFFSET:-+0} hours" +%H)
    echo "Local Hour calculated: \$LOCAL_HOUR"
    if [ "\$LOCAL_HOUR" -ge 1 ] && [ "\$LOCAL_HOUR" -le 6 ]; then
        echo "RESULT: Night Silence triggered."
        exit 0
    fi
    
    # Logic 2: Activity Level
    DAILY_SEED=\$(echo "\$(date +%Y%m%d)" | cksum | awk '{print \$1}')
    ACTIVITY_LEVEL=\$(( DAILY_SEED % 100 ))
    echo "Activity Level calculated: \$ACTIVITY_LEVEL"
    # We skip randomness for deterministic test, but show the value
    
    # Logic 3: UA
    if [[ "$ua_type" == "mobile" ]]; then
        SESSION_UA="Mozilla/5.0 (Android 14; Mobile; rv:124.0) Gecko/124.0 Firefox/124.0"
    else
        SESSION_UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124.0.0.0"
    fi
    IS_MOBILE=false
    [[ "\$SESSION_UA" =~ "Android" || "\$SESSION_UA" =~ "iPhone" ]] && IS_MOBILE=true
    echo "Is Mobile: \$IS_MOBILE"
    
    # Logic 4: Weighted Action
    ROLL=\$(( RANDOM % 100 ))
    echo "Action Roll: \$ROLL"
    # ... output test results ...
EOF
}

# Run Scenarios
test_scenario "20260507" 18 "desktop" # 18 UTC + 9 = 03 local (Night)
test_scenario "20260507" 06 "desktop" # 06 UTC + 9 = 15 local (Day)
test_scenario "20260507" 06 "mobile"  # Mobile check
