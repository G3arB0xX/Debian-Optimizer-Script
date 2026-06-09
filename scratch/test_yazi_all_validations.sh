#!/bin/bash
# =================================================================
# Yazi Full Configurations and Shortcut Keys Integration Test
# =================================================================
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Deploy templates
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"
cp -f templates/apps/devops/yazi_keymap.toml "$MOCK_DIR/keymap.toml"

# Create dummy test files
echo "Line 1: Yazi Integration Test File" > "$MOCK_DIR/test_file.txt"

# Kill existing tmux session
tmux kill-session -t testyazi 2>/dev/null || true

# Start tmux running Yazi directly
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR/test_file.txt"
sleep 2

echo "=============================================="
echo "1. VERIFYING INITIAL YAZI VIEW"
echo "=============================================="
tmux capture-pane -t testyazi -p
echo ""

echo "=============================================="
echo "2. VERIFYING Ctrl-n (Create File Prompt)"
echo "=============================================="
tmux send-keys -t testyazi C-n
sleep 1
tmux capture-pane -t testyazi -p
# Cancel create dialog (using C-c)
tmux send-keys -t testyazi C-c
sleep 0.5
echo ""

echo "=============================================="
echo "3. VERIFYING Ctrl-f (Filter Prompt)"
echo "=============================================="
tmux send-keys -t testyazi C-f
sleep 1
tmux capture-pane -t testyazi -p
# Cancel filter dialog (using C-c)
tmux send-keys -t testyazi C-c
sleep 0.5
echo ""

echo "=============================================="
echo "4. VERIFYING F2 (Rename Prompt)"
echo "=============================================="
tmux send-keys -t testyazi F2
sleep 1
tmux capture-pane -t testyazi -p
# Cancel rename dialog (using C-c)
tmux send-keys -t testyazi C-c
sleep 0.5
echo ""

echo "=============================================="
echo "5. VERIFYING ENTER (Lauching Micro Editor)"
echo "=============================================="
tmux send-keys -t testyazi Enter
sleep 1.5
tmux capture-pane -t testyazi -p
# Exit micro editor
tmux send-keys -t testyazi C-q
sleep 1
echo ""

# Cleanup
tmux kill-session -t testyazi 2>/dev/null || true
echo "=== Integration Test Completed ==="
