#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"
ARTIFACTS_DIR="/home/user/.gemini/antigravity/brain/d71b59ff-b1ea-4992-acf1-86149da9d730"
SCRATCH_DIR="/home/user/文档/CodeDev/deboptiscript/scratch"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Write a simple keymap.toml to test mapping 'n' to 'create' under [manager]
cat > "$MOCK_DIR/keymap.toml" << 'EOF'
[manager]
prepend_keymap = [
    { on = [ "n" ], run = "create", desc = "Test Create with n" },
    { on = [ "<C-n>" ], run = "create", desc = "Test Create with Ctrl-n" }
]
EOF

# Write yazi.toml
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"

# Create dummy test files
echo "dummy file" > "$MOCK_DIR/test_file.txt"

# Test 1: Send 'n'
tmux kill-session -t testyazi 2>/dev/null || true
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
sleep 2
echo "=== Sending key 'n' ==="
tmux send-keys -t testyazi n
sleep 1
python3 "$SCRATCH_DIR/capture_yazi.py" "testyazi" "$ARTIFACTS_DIR/yazi_test_n.png"
tmux kill-session -t testyazi 2>/dev/null || true

# Test 2: Send 'Ctrl-n'
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
sleep 2
echo "=== Sending key 'Ctrl-n' ==="
tmux send-keys -t testyazi C-n
sleep 1
python3 "$SCRATCH_DIR/capture_yazi.py" "testyazi" "$ARTIFACTS_DIR/yazi_test_ctrl_n.png"
tmux kill-session -t testyazi 2>/dev/null || true

echo "=== Test Completed ==="
