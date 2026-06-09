#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"
SCRATCH_DIR="/home/user/文档/CodeDev/deboptiscript/scratch"

# Test Ctrl-n
tmux kill-session -t testyazi 2>/dev/null || true
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
sleep 2
tmux send-keys -t testyazi C-n
sleep 1
echo "=== Screen Capture for Ctrl-n ==="
tmux capture-pane -t testyazi -p
tmux kill-session -t testyazi 2>/dev/null || true

# Test F2
tmux kill-session -t testyazi 2>/dev/null || true
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
sleep 2
tmux send-keys -t testyazi F2
sleep 1
echo "=== Screen Capture for F2 ==="
tmux capture-pane -t testyazi -p
tmux kill-session -t testyazi 2>/dev/null || true
