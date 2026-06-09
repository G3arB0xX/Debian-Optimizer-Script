#!/bin/bash
set -euo pipefail

MOCK_DIR="/tmp/mock_yazi_config"

rm -rf "$MOCK_DIR"
mkdir -p "$MOCK_DIR"

# Write keymap.toml using [[mgr.prepend_keymap]] syntax
cat > "$MOCK_DIR/keymap.toml" << 'EOF'
[[mgr.prepend_keymap]]
on   = [ "n" ]
run  = "create"
desc = "Create file/directory"
EOF

# Copy yazi.toml
cp -f templates/apps/devops/yazi.toml "$MOCK_DIR/yazi.toml"

# Create dummy test files
echo "dummy file" > "$MOCK_DIR/test_file.txt"

# Start tmux
tmux kill-session -t testyazi 2>/dev/null || true
tmux new-session -d -s testyazi -x 100 -y 30 "YAZI_CONFIG_HOME=$MOCK_DIR yazi $MOCK_DIR"
sleep 2

# Send key 'n'
echo "=== Sending key 'n' ==="
tmux send-keys -t testyazi n
sleep 1

# Capture TUI text directly
echo "=== Captured TUI Screen ==="
tmux capture-pane -t testyazi -p
tmux kill-session -t testyazi 2>/dev/null || true
