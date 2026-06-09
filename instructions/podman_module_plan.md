安装 Podman 核心组件
sudo apt update
sudo apt install -y podman podman-docker systemd-container

创建一个叫 apps 的无sudo权限用户，专门用来非root运行podman。锁定这个用户的密码。开启 Linger 让其服务驻留后台 `sudo loginctl enable-linger apps`。在 apps/srv 目录下统一管理容器映射数据。

在~/.config/containers/containers.conf做系统调优如下

```
[containers]
# 将容器日志驱动切为 journald，停止产生无节制的磁盘 json 文本日志
log_driver = "journald"
# 强力卡死单容器内存日志缓冲区上限（50MB）
log_size_max = 52428800

[engine]
# 使用 systemd 管理 cgroup 行为
cgroup_manager = "systemd"
# 事件记录同样收拢到 journald 
events_logger = "journald"
# 切换为更轻量的 sqlite 作为本地元数据后端（Podman 5.x+ 生产标准）
database_backend = "sqlite"
# 关闭端口常驻守护进程，节省由于暴露端口引发的内存浪费
enable_port_reservation = false
```

将sudoer主用户加入 apps 组方便后续维护。

如安装了fish，则进行如下操作：

```
1. 配置 Fish 的环境变量

mkdir -p ~/.config/fish/
nano ~/.config/fish/config.fish
在 config.fish 文件末尾添加以下内容：

# 智能化判断：只有当用户级的 Podman Socket 存在时才挂载变量
if test -S "$XDG_RUNTIME_DIR/podman/podman.sock"
    set -gx DOCKER_HOST "unix://$XDG_RUNTIME_DIR/podman/podman.sock"
end

2. 在 Fish 中配置 docker 命令别名（Alias）
Debian 13 的 podman-docker 已经帮你做好了 /usr/bin/docker 的软链接，但为了确保在 Fish 终端里万无一失，直接用 Fish 的自带命令保存一个永久别名：

Code snippet
alias docker=podman
funcsave docker

3. 使用 Fish 的缩写功能（Abbreviation）
自托管社区的 Fish 用户更喜欢用 abbr（缩写）。当你输入 docker ps 并按下空格时，Fish 会极其酷炫地自动在屏幕上把 docker 展开替换为 podman，让你时刻明确自己在用 Podman。

abbr -a docker podman
```

给sudoer主用户赋予管理apps用户文件和目录的权限

```
# 赋予当前已有文件和目录的权限 
sudo setfacl -R -m u:myuser:rwx /home/apps/srv /home/apps/.config 
# 设置“默认 ACL”，确保未来 apps 或容器新建的文件，myuser 也自动拥有 rwx 权限 
sudo setfacl -R -d -m u:myuser:rwx /home/apps/srv /home/apps/.config
# 放开权限以支持 shell 路径自动补齐功能
sudo setfacl -m u:myuser:rx /home/apps
```
给sudoer主用户fish配置快捷别名

封装一个名为 `appctl` 的动态函数，自动寻找 `apps` 用户的系统运行时运行时目录

```
function appctl
    # 动态获取 apps 用户的 UID (Debian 默认通常是 1002 左右)
    set -l apps_uid (id -u apps)
    # 带着正确的环境变量和身份去执行 systemctl
    sudo -u apps XDG_RUNTIME_DIR=/run/user/$apps_uid systemctl --user $argv
end

# 将这个函数永久保存
funcsave appctl
```

使用 Bash 的场景，对应的 Bash 别名写法为：`alias appctl='sudo -u apps XDG_RUNTIME_DIR=/run/user/$(id -u apps) systemctl --user'`

允许sudoer主用户免密码运行 `sudo machinectl shell apps@ `

主用户的 `appod` 对应 apps 用户的 `podman`，`appctl` 对应  `sudo machinectl shell apps@ `

```
function apppod
    # 1. 记下你当前所在的时空（当前工作目录）
    set -l old_pwd $PWD
    
    # 2. 瞬间移动到全系统所有人都能进的公用物理目录
    builtin cd /tmp
    
    # 3. 此时大家都在公共区域，爽快地以 apps 身份执行 podman，没有任何 chdir 冲突
    sudo -u apps podman $argv
    
    # 4. 执行完毕后，悄悄把你带回原本的工作目录，不留一丝痕迹
    builtin cd $old_pwd
end

# 永久覆盖并保存该函数
funcsave apppod
```

允许sudoer主用户免密码运行 apps 用户的各种命令

```
myuser ALL=(apps) NOPASSWD: SETENV: /usr/bin/systemctl --user *, /usr/bin/journalctl *, /usr/bin/podman *
```