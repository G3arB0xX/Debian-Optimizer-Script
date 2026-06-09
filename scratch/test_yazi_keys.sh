#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"
ARTIFACTS_DIR="/home/user/.gemini/antigravity/brain/d71b59ff-b1ea-4992-acf1-86149da9d730"
SCRATCH_DIR="/home/user/文档/CodeDev/deboptiscript/scratch"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Copy templates
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"
# We'll use our updated keymap.toml template
cp -f templates/apps/devops/yazi_keymap.toml "$MOCK_DIR/keymap.toml"

# Change the layer prefix from [[mgr. to [[manager.
sed -i 's/\[\[mgr\./\[\[manager\./g' "$MOCK_DIR/keymap.toml"

# Fix the invalid run = "copy" in the mock keymap to run = "yank" for correct copy behavior
sed -i 's/run  = "copy"/run  = "yank"/g' "$MOCK_DIR/keymap.toml"
# Also fix undo since undo command doesn't exist natively. We can map C-z to something else or comment it out, but let's see.

# Create dummy test files
echo "dummy file" > "$MOCK_DIR/test_file.txt"

# Kill existing tmux session
tmux kill-session -t testyazi 2>/dev/null || true

# Start tmux
tmux new-session -d -s testyazi -x 100 -y 30 "bash"
sleep 1

# Launch yazi
tmux send-keys -t testyazi "export YAZI_CONFIG_HOME=$MOCK_DIR" C-m
tmux send-keys -t testyazi "yazi $MOCK_DIR" C-m
sleep 2

# Send C-n (Ctrl+n) to trigger creation
echo "=== Sending Ctrl+n ==="
tmux send-keys -t testyazi C-n
sleep 1

# Capture screen
python3 "$SCRATCH_DIR/capture_yazi.py" "testyazi" "$ARTIFACTS_DIR/yazi_ctrl_n.png"

# Kill session
tmux kill-session -t testyazi 2>/dev/null || true
echo "=== Test Completed ==="
