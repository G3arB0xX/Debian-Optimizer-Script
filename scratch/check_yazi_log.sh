#!/bin/bash
export YAZI_CONFIG_HOME="/tmp/mock_yazi_config"
yazi --debug > /tmp/yazi_debug_stderr.log 2>&1 || true
cat /tmp/yazi_debug_stderr.log
