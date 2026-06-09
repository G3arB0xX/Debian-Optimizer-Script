#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Write corrected keymap.toml
cat > "$MOCK_DIR/keymap.toml" << 'EOF'
# =================================================================
# Yazi 快捷键配置 (Windows / Micro 风格优先)
# =================================================================

# ----------------- 文件基本操作 -----------------
[[mgr.prepend_keymap]]
on   = [ "<C-c>" ]
run  = "yank"
desc = "复制选中文件"

[[mgr.prepend_keymap]]
on   = [ "<C-x>" ]
run  = "yank --cut"
desc = "剪切选中文件"

[[mgr.prepend_keymap]]
on   = [ "<C-v>" ]
run  = "paste"
desc = "粘贴文件"

[[mgr.prepend_keymap]]
on   = [ "<Delete>" ]
run  = "remove"
desc = "将选中文件移入回收站"

[[mgr.prepend_keymap]]
on   = [ "<S-Delete>" ]
run  = "remove --permanently"
desc = "永久删除选中文件"

# ----------------- 选择与新建 -----------------
[[mgr.prepend_keymap]]
on   = [ "<C-a>" ]
run  = "select_all --state=true"
desc = "全选当前目录下文件"

[[mgr.prepend_keymap]]
on   = [ "<Esc>" ]
run  = "escape --select"
desc = "取消所有选中状态"

[[mgr.prepend_keymap]]
on   = [ "<C-n>" ]
run  = "create"
desc = "新建文件或文件夹 (加/结尾为文件夹)"

[[mgr.prepend_keymap]]
on   = [ "F2" ]
run  = "rename --cursor=before_ext"
desc = "重命名文件 (光标定位在后缀名前)"

[[mgr.prepend_keymap]]
on   = [ "<C-f>" ]
run  = "filter --smart"
desc = "开启实时搜索/过滤文件"
EOF

# Copy yazi.toml
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"

# Create dummy test files
echo "dummy file" > "$MOCK_DIR/test_file.txt"

# Helper function to run yazi, send a key, capture and kill
run_key_test() {
    local key_name="$1"
    local send_key="$2"
    echo "=============================================="
    echo "Testing Key: $key_name (Sending: $send_key)"
    echo "=============================================="
    
    tmux kill-session -t testyazi 2>/dev/null || true
    tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
    sleep 2
    
    tmux send-keys -t testyazi "$send_key"
    sleep 1
    
    tmux capture-pane -t testyazi -p
    tmux kill-session -t testyazi 2>/dev/null || true
    echo ""
}

# Run tests
run_key_test "Ctrl-n (Create File)" "C-n"
run_key_test "Ctrl-f (Filter)" "C-f"
run_key_test "F2 (Rename)" "F2"
