#!/bin/bash
# FreshIP 入口包装（systemd ExecStart 兼容）
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CONFIG_FILE="/etc/freship/freship.conf"
if [[ -f "$CONFIG_FILE" ]]; then
    # shellcheck source=/dev/null
    source "$CONFIG_FILE"
fi
exec /bin/bash "${INSTALL_DIR:-/opt/freship}/core/freship_runner.sh" "$@"
