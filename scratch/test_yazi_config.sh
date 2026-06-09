#!/bin/bash
# =================================================================
# Yazi Configuration Customization and Opener Integration Test
# =================================================================
set -euo pipefail

# Define paths
MOCK_DIR="/tmp/mock_yazi_config"
ARTIFACTS_DIR="/home/user/.gemini/antigravity/brain/d71b59ff-b1ea-4992-acf1-86149da9d730"
SCRATCH_DIR="/home/user/文档/CodeDev/deboptiscript/scratch"

echo "=== Preparing Mock Yazi Config Environment ==="
rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Copy templates
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"
cp -f templates/apps/devops/yazi_keymap.toml "$MOCK_DIR/keymap.toml"

# Create dummy test files
echo -e "Line 1: Yazi Integration Test File\nLine 2: This file should open in Micro.\nLine 3: Press Ctrl+Q to return to Yazi." > "$MOCK_DIR/test_file.txt"

# Kill existing tmux session if any
tmux kill-session -t testyazi 2>/dev/null || true

echo "=== Launching Yazi in tmux session 'testyazi' ==="
# We start a session with a clean shell first, then run yazi inside it
tmux new-session -d -s testyazi -x 100 -y 30 "bash"
sleep 1

# Send command to run yazi focused on test_file.txt
tmux send-keys -t testyazi "export YAZI_CONFIG_HOME=$MOCK_DIR" C-m
tmux send-keys -t testyazi "yazi $MOCK_DIR/test_file.txt" C-m
sleep 2

echo "=== Capture 1: Yazi Started ==="
python3 "$SCRATCH_DIR/capture_yazi.py" "testyazi" "$ARTIFACTS_DIR/yazi_started.png"

echo "=== Opening test_file.txt (Pressing Enter) ==="
tmux send-keys -t testyazi "Enter"
sleep 2

echo "=== Capture 2: Micro Editor Open ==="
python3 "$SCRATCH_DIR/capture_yazi.py" "testyazi" "$ARTIFACTS_DIR/yazi_editor_open.png"

echo "=== Closing Micro Editor (Pressing Ctrl-q) ==="
tmux send-keys -t testyazi "C-q"
sleep 2

echo "=== Capture 3: Back to Yazi ==="
python3 "$SCRATCH_DIR/capture_yazi.py" "testyazi" "$ARTIFACTS_DIR/yazi_editor_closed.png"

# Clean up tmux session
tmux kill-session -t testyazi 2>/dev/null || true
echo "=== Test Completed ==="
