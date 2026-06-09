#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"
ARTIFACTS_DIR="/home/user/.gemini/antigravity/brain/d71b59ff-b1ea-4992-acf1-86149da9d730"
SCRATCH_DIR="/home/user/文档/CodeDev/deboptiscript/scratch"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Copy templates
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"
cp -f templates/apps/devops/yazi_keymap.toml "$MOCK_DIR/keymap.toml"

# Modify headers from [[mgr. to [[manager.
sed -i 's/\[\[mgr\./\[\[manager\./g' "$MOCK_DIR/keymap.toml"
sed -i 's/run  = "copy"/run  = "yank"/g' "$MOCK_DIR/keymap.toml"

# Create dummy test files
echo "dummy file" > "$MOCK_DIR/test_file.txt"

# Kill existing tmux session
tmux kill-session -t testyazi 2>/dev/null || true

# Start tmux running Yazi directly
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
sleep 2

# Send key 'c' to trigger creation
echo "=== Sending key 'c' ==="
tmux send-keys -t testyazi c
sleep 1

# Capture screen
python3 "$SCRATCH_DIR/capture_yazi.py" "testyazi" "$ARTIFACTS_DIR/yazi_c.png"

# Kill session
tmux kill-session -t testyazi 2>/dev/null || true
echo "=== Test Completed ==="
