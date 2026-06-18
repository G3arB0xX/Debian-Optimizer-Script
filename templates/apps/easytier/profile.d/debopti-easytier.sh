# debopti Easytier CLI — Bash PATH
# 路径: /etc/profile.d/debopti-easytier.sh

if [[ ":${PATH}:" != *":/opt/easytier:"* ]]; then
    export PATH="/opt/easytier:${PATH}"
fi
