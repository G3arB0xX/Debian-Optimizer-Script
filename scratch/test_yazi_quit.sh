#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Deploy templates
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"
cp -f templates/apps/devops/yazi_keymap.toml "$MOCK_DIR/keymap.toml"

# Create dummy test files
echo "Line 1" > "$MOCK_DIR/test_file.txt"

# Kill existing tmux session if any
tmux kill-session -t testyazi 2>/dev/null || true

# Start tmux running Yazi directly as the session command
echo "=== Starting Yazi in tmux ==="
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
sleep 2

# Check if session exists
if ! tmux has-session -t testyazi 2>/dev/null; then
    echo "ERROR: Yazi failed to start or tmux session closed unexpectedly."
    exit 1
fi
echo "Yazi started successfully (session exists)."

# Send Ctrl-q
echo "=== Sending Ctrl-q ==="
tmux send-keys -t testyazi C-q
sleep 1

# Check if session has closed (which means Yazi exited)
if tmux has-session -t testyazi 2>/dev/null; then
    echo "ERROR: Yazi did not quit (session still exists)."
    tmux kill-session -t testyazi 2>/dev/null || true
    exit 1
else
    echo "SUCCESS: Yazi exited successfully (session has closed)."
fi
