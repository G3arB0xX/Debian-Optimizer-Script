#!/bin/bash
# 容器内验证 Micro 剪贴板：内部 Ctrl+C/V + OSC 52 同步到主机
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export CI=true
unset DISPLAY WAYLAND_DISPLAY || true

for _i in $(seq 1 30); do
    if ! fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

_fail() { echo "ASSERT_FAIL: $*"; exit 1; }

echo "=== SETUP MICRO CLIPBOARD (skip shared deps / plugins) ==="
apt-get update -yq >/dev/null 2>&1
apt-get install -yq --no-install-recommends micro python3 ca-certificates passwd >/dev/null

if ! id testuser >/dev/null 2>&1; then
    useradd -m -s /bin/bash testuser
fi

mkdir -p /etc/debopti
cat > /etc/debopti/debopti.conf << 'EOF'
IS_CN_REGION="false"
BASE_OPTIMIZED="false"
INSTALLED="false"
SSH_HARDENED="false"
EDITOR_CMD="micro"
PODMAN_GC_ENABLED="false"
YAZI_MEDIA_PREVIEW="false"
EOF

export SCRIPT_DIR=/opt/debopti
# shellcheck disable=SC1091
source /opt/debopti/scripts/common.sh
load_project_config
# shellcheck disable=SC1091
source /opt/debopti/scripts/network.sh
# shellcheck disable=SC1091
source /opt/debopti/scripts/apps/devops.sh

bash -n /opt/debopti/scripts/apps/devops.sh

sot_user=$(get_sot_user)
sot_home=$(eval echo "~$sot_user")
mkdir -p "$sot_home/.config/micro"
render_template "templates/apps/devops/micro_settings.json" "$sot_home/.config/micro/settings.json"
render_template "templates/apps/devops/micro_bindings.json" "$sot_home/.config/micro/bindings.json"
render_template "templates/apps/devops/micro_init.lua" "$sot_home/.config/micro/init.lua"
render_template "templates/apps/devops/debopti-osc52" "/usr/local/bin/debopti-osc52"
chmod 0755 /usr/local/bin/debopti-osc52
ln -sfn /usr/local/bin/debopti-osc52 /usr/local/bin/micro-clip
chown -R "$sot_user:$sot_user" "$sot_home/.config/micro" 2>/dev/null || true

bash -n /usr/local/bin/debopti-osc52

echo "=== ASSERT CONFIG ==="
command -v micro >/dev/null || _fail "micro missing"
command -v debopti-osc52 >/dev/null || _fail "debopti-osc52 missing"
[[ -x /usr/local/bin/debopti-osc52 ]] || _fail "debopti-osc52 not executable"
[[ -L /usr/local/bin/micro-clip ]] || _fail "micro-clip symlink missing"

sot_user=$(get_sot_user)
sot_home=$(eval echo "~$sot_user")
settings="$sot_home/.config/micro/settings.json"
grep -q '"clipboard": "internal"' "$settings" || _fail "clipboard is not internal in $settings"
grep -q 'terminal' "$settings" && grep -q '"clipboard": "terminal"' "$settings" && _fail "clipboard still terminal"
grep -q 'Copy,command:deboptiOsc52' "$sot_home/.config/micro/bindings.json" || _fail "Ctrl-c not chained to Copy+deboptiOsc52"
grep -q 'function deboptiOsc52' "$sot_home/.config/micro/init.lua" || _fail "deboptiOsc52 missing in init.lua"

echo "=== ASSERT OSC52 HELPER ==="

python3 - << 'PY'
import os, pty, select, time, base64, re, subprocess, sys

def run_on_pty(cmd, timeout=3):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ.pop("DISPLAY", None)
        os.environ.pop("WAYLAND_DISPLAY", None)
        os.execvp("bash", ["bash", "-lc", cmd])
    data = b""
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.2)
        if fd in r:
            try:
                chunk = os.read(fd, 8192)
            except OSError:
                break
            if not chunk:
                break
            data += chunk
        else:
            try:
                waited = os.waitpid(pid, os.WNOHANG)
            except ChildProcessError:
                break
            if waited[0] == pid:
                # drain
                while True:
                    r, _, _ = select.select([fd], [], [], 0.05)
                    if fd not in r:
                        break
                    try:
                        chunk = os.read(fd, 8192)
                    except OSError:
                        break
                    if not chunk:
                        break
                    data += chunk
                break
    try:
        os.waitpid(pid, os.WNOHANG)
    except ChildProcessError:
        pass
    try:
        os.close(fd)
    except OSError:
        pass
    return data

marker = "HELLO-DEBOPTI-OSC52"
out = run_on_pty(f"printf %s '{marker}' | debopti-osc52 -i clipboard")
if b"]52;c;" not in out:
    sys.stderr.write("OSC52 sequence missing in helper output:\n" + repr(out[:500]) + "\n")
    sys.exit(2)
m = re.search(rb"\]52;c;([A-Za-z0-9+/=]+)", out)
if not m:
    sys.stderr.write("OSC52 payload not parsed\n")
    sys.exit(3)
decoded = base64.b64decode(m.group(1))
if decoded != marker.encode():
    sys.stderr.write(f"OSC52 payload mismatch: {decoded!r}\n")
    sys.exit(4)
paste = subprocess.check_output(["debopti-osc52", "-o", "clipboard"], env={**os.environ, "DISPLAY": "", "WAYLAND_DISPLAY": ""})
if paste != marker.encode():
    sys.stderr.write(f"internal clip file mismatch: {paste!r}\n")
    sys.exit(5)
print("OSC52_HELPER_OK")
PY

echo "=== ASSERT MICRO COPY/PASTE VIA PTY ==="
workdir=$(eval echo "~$sot_user")
src="$workdir/clip-src.txt"
dst_marker="HELLO-MICRO-CLIP-TEST"
printf '%s\n' "$dst_marker" > "$src"
chown "$sot_user:$sot_user" "$src"

# 以 SOT 用户运行，确保读取其 ~/.config/micro
python3 - "$sot_user" "$src" "$dst_marker" << 'PY'
import os, pty, select, time, sys, pwd, fcntl, termios, struct, re, base64

user, src, marker = sys.argv[1], sys.argv[2], sys.argv[3]
pw = pwd.getpwnam(user)

def set_winsize(fd, rows=24, cols=100):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

pid, fd = pty.fork()
if pid == 0:
    os.environ.pop("DISPLAY", None)
    os.environ.pop("WAYLAND_DISPLAY", None)
    os.environ["HOME"] = pw.pw_dir
    os.environ["USER"] = user
    os.environ["LOGNAME"] = user
    os.environ["TERM"] = "xterm-256color"
    os.chdir(pw.pw_dir)
    os.setgid(pw.pw_gid)
    os.setuid(pw.pw_uid)
    os.execvp("micro", ["micro", src])

set_winsize(fd)
time.sleep(0.8)

def send(data):
    if isinstance(data, str):
        data = data.encode()
    os.write(fd, data)

def read_for(seconds):
    data = b""
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.15)
        if fd in r:
            try:
                chunk = os.read(fd, 8192)
            except OSError:
                break
            if not chunk:
                break
            data += chunk
    return data

startup = read_for(1.0)
# Shift-End 选中当前行，Ctrl-C：内部复制 + OSC 52
send(b"\x1b[1;2F")
time.sleep(0.2)
send(b"\x03")
copied = read_for(1.5)
if b"]52;c;" not in copied:
    copied += read_for(1.0)
if b"]52;c;" not in copied:
    sys.stderr.write("micro Ctrl-C did not emit OSC 52\nstartup=" + repr(startup[:400]) + "\ncopied=" + repr(copied[:800]) + "\n")
    send(b"\x11")
    time.sleep(0.2)
    sys.exit(10)

m = re.search(rb"\]52;c;([A-Za-z0-9+/=]+)", copied)
decoded = base64.b64decode(m.group(1)) if m else b""
if marker.encode() not in decoded:
    sys.stderr.write(f"OSC52 from micro mismatch: {decoded!r}\n")
    send(b"\x11")
    sys.exit(11)

# 行尾换行后 Ctrl-V，验证编辑器内粘贴，然后保存退出
send(b"\x1b[F")  # End
time.sleep(0.15)
send(b"\r")
time.sleep(0.15)
send(b"\x16")  # Ctrl-V
time.sleep(0.3)
send(b"\x13")  # Ctrl-S
time.sleep(0.5)
send(b"\x11")  # Ctrl-Q
time.sleep(0.6)
read_for(0.5)
try:
    os.waitpid(pid, os.WNOHANG)
except ChildProcessError:
    pass

with open(src, "rb") as f:
    content = f.read()
if content.count(marker.encode()) < 2:
    sys.stderr.write(f"internal paste failed, file content={content!r}\n")
    sys.exit(12)
print("MICRO_COPY_PASTE_OK")
PY

echo "ALL_TESTS_PASSED"
