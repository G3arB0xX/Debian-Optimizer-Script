#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Write keymap.toml with F2 mapped to <F2>
cat > "$MOCK_DIR/keymap.toml" << 'EOF'
[[mgr.prepend_keymap]]
on   = [ "<F2>" ]
run  = "rename --cursor=before_ext"
desc = "Rename"
EOF

# Copy yazi.toml
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"

# Create dummy test files
echo "dummy file" > "$MOCK_DIR/test_file.txt"

# Start tmux
tmux kill-session -t testyazi 2>/dev/null || true
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR/test_file.txt"
sleep 2

# Send key 'F2'
echo "=== Sending key 'F2' ==="
tmux send-keys -t testyazi F2
sleep 1

# Capture TUI text directly
echo "=== Captured TUI Screen ==="
tmux capture-pane -t testyazi -p
tmux kill-session -t testyazi 2>/dev/null || true
