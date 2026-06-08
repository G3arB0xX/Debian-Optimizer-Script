# 运维工具：Fish Shell、Micro 编辑器、Yazi 文件管理器、Lego 证书管理

本文档介绍这四个运维工具的手动安装与配置步骤，完全还原自动优化脚本中的物理部署与集成逻辑。

**前提条件**：root 权限，Debian 10+

---

## 1. Fish Shell

Fish 是一个现代化的交互式 Shell，提供语法高亮、自动补全和 Git 状态显示等功能。

### 1.1 安装 Fish

```bash
apt-get install -y fish fzf fd-find curl git
```

### 1.2 安装 Fisher（插件管理器）

```bash
# 以要配置 Fish 的用户身份执行（或先 su 到该用户）
fish -c "curl -sL https://raw.githubusercontent.com/jorgebucaran/fisher/main/functions/fisher.fish | source && fisher install jorgebucaran/fisher"
```

### 1.3 安装常用插件

```bash
fish -c "fisher install PatrickF1/fzf.fish jorgebucaran/autopair.fish nickeb96/puffer-fish jorgebucaran/replay.fish"
```

插件说明：
- `fzf.fish`：在 Fish 中集成 fzf 模糊搜索（文件、历史、变量等）
- `autopair.fish`：括号和引号自动配对
- `puffer-fish`：增强的补全提示
- `replay.fish`：在 Fish 中执行 bash 命令并同步环境变量

### 1.4 安装 zoxide（智能 cd）

zoxide 会记录你访问过的目录，之后只需输入部分路径就能快速跳转：

```bash
# 优先尝试 APT
apt-get install -y zoxide 2>/dev/null || \
    curl -fsSL https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh | sh
```

在 Fish 配置中启用 zoxide：

```bash
mkdir -p ~/.config/fish/conf.d/

cat > ~/.config/fish/conf.d/zoxide.fish << 'EOF'
# zoxide 初始化
if command -q zoxide
    zoxide init fish | source
end
EOF
```

### 1.5 安装 Starship（Prompt 美化）

```bash
# 优先尝试 APT
apt-get install -y starship 2>/dev/null || \
    curl -fsSL https://starship.rs/install.sh | sh -s -- -y
```

在 Fish 配置中启用 Starship：

```bash
cat > ~/.config/fish/conf.d/starship.fish << 'EOF'
if command -q starship
    starship init fish | source
end
EOF
```

### 1.6 将 Fish 设置为默认 Shell

```bash
# 确认 fish 路径
which fish

# 设置为默认 shell（将 myuser 替换为实际用户名，root 建议保留 bash）
chsh -s "$(which fish)" myuser

# 重新登录后生效，验证
echo $SHELL
```

### 1.7 卸载 Fish

```bash
# 恢复 bash 为默认 shell
chsh -s /bin/bash myuser

# 卸载软件包
apt-get purge -y fish zoxide
rm -f /usr/local/bin/starship

# 清理配置
rm -rf ~/.config/fish ~/.config/starship.toml
```

---

## 2. Micro 编辑器

Micro 是一个高度可定制、支持鼠标且开箱即用的终端文本编辑器。它比 nano 功能更强大，比 vim 的学习曲线更平缓。本节将完全还原自动化脚本的物理部署、插件集成、真彩色环境配置以及定制的三栏式（文件树 + 编辑区 + Minimap 缩略图）界面布局。

### 2.1 安装物理依赖与程序二进制

安装 Micro 前，需要先部署语法检查、系统剪贴板以及配套插件（如文件过滤与搜索）的物理依赖包，并处理 Debian 中的软件包名冲突：

```bash
# 1. 安装系统依赖包
# - xclip: 启用 Linux 终端剪贴板共享支持
# - shellcheck: 用于 Shell 脚本的实时语法与规范检查
# - yamllint: 用于 YAML 配置文件的实时语法规范检查
# - fzf, ripgrep: 用于 MicroOmni 插件进行模糊搜索与文件过滤
# - bat: 用于语法高亮预览 (Debian 中包名为 bat)
apt-get install -y xclip shellcheck yamllint fzf ripgrep bat

# 2. 解决 Debian 环境下 bat 软件包安装为 batcat 的冲突
if command -v batcat >/dev/null 2>&1 && [[ ! -f "/usr/local/bin/bat" ]]; then
    ln -sf "$(which batcat)" /usr/local/bin/bat
fi

# 3. 部署 Micro 二进制文件（优先 APT，境内失败或源中无包时回退到官方直连安装）
if [[ ! -f "/usr/local/bin/micro" ]] && [[ ! -f "/usr/bin/micro" ]]; then
    if ! apt-get install -y micro 2>/dev/null; then
        echo "APT 源内无 micro，回退到官方托管脚本进行编译安装..."
        cd /tmp
        curl -fsSL https://getmic.ro | bash
        mv micro /usr/local/bin/
    fi
fi
```

### 2.2 部署真理源（SOT）用户基础配置文件

Micro 的配置文件保存在真理源（SOT）用户（通常为创建系统的首个普通用户，若无则为 root）的 `~/.config/micro/` 目录下。

#### 2.2.1 写入全局与文件类型覆盖配置 `settings.json`

此文件不仅配置了跨机剪贴板、自动提权、光标历史记录、真彩色支持等核心配置，还针对不同文件类型定制了最合理的缩进与空格宽度规则（例如：YAML 强制 2 空格；Makefile 与 Go 强制使用硬 Tab；Python 遵循 PEP 8 的 4 空格等）。

```bash
mkdir -p ~/.config/micro

cat > ~/.config/micro/settings.json << 'EOF'
{
    // =================================================================
    // 基础与常规设置 (Basic & General Settings)
    // =================================================================
    "colorscheme": "dracula-tc",       // 配色方案：真彩色德古拉
    "mouse": true,                     // 启用鼠标定位、滚轮滚动与文本拖拽选中
    "savecursor": true,                // 重新打开文件时自动定位到上次光标所在行
    "saveundo": true,                  // 即使关闭编辑器，依然物理保留历史撤销 (Undo) 记录
    "scrollbar": true,                 // 在编辑区右侧显示滚动进度条
    "tabsize": 4,                      // 默认 Tab 宽度为 4 个空格
    "autoindent": true,                // 换行时自动与上一行对齐
    "autosu": true,                    // 保存只读文件时自动尝试通过 sudo 提权
    "cursorline": true,                // 高亮光标所在行
    "eofnewline": true,                // 保存时自动在文件尾部追加空换行（规范）
    "fastdirty": true,                 // 优化大文件脏标记检测
    "mkparents": true,                 // 保存新文件时，若父目录不存在则自动创建
    "rmtrailingws": true,              // 保存时自动清除行尾死空格
    "softwrap": true,                  // 当内容超出屏幕宽度时，自动进行视觉折行
    "tabstospaces": true,              // 将按下 Tab 键的操作自动转换为空格
    "wordwrap": true,                  // 软换行时尽量不在单词中间折断
    "basename": true,                  // 标题栏仅显示文件名而非绝对路径
    "ignorecase": true,                // 搜索时忽略大小写
    "matchbrace": true,                // 自动高亮配对的括号
    "matchbracewait": "50ms",          // 括号匹配检测响应延迟
    "ruler": true,                     // 显示左侧行号
    "incsearch": true,                 // 增量式搜索高亮
    "smartpaste": true,                // 智能粘帖，避免多行缩进错乱
    "sucmd": "sudo",                   // 自定义提权工具为 sudo

    // =================================================================
    // 进阶与终端兼容性设置 (Advanced & Terminal Settings)
    // =================================================================
    "clipboard": "terminal",           // 利用 OSC 52 协议实现远程终端与本地剪贴板跨机同步
    "truecolor": true,                 // 强制启用 24 位真彩色
    "fileformat": "unix",              // 强制新文件使用 Unix 换行符 (LF)
    "encoding": "utf-8",               // 编码格式设为 UTF-8
    "hltrailingws": true,              // 红色背景醒目高亮行尾死空格
    "hltaberrors": true,               // 当 tabstospaces 为真时，醒目高亮残留的硬 Tab
    "colorcolumn": 100,                // 在第 100 列绘制纵向代码宽度参考虚线
    "filemanager.openonstart": true,   // 启动时默认拉出左侧目录树面板

    // =================================================================
    // 文件类型局部覆盖设置 (Filetype Overrides)
    // =================================================================
    "ft:yaml": {
        "tabsize": 2,                  // YAML 必须为 2 空格缩进
        "tabstospaces": true,
        "hltaberrors": true,
        "colorcolumn": 80              // YAML 限制 80 字符边界
    },
    "ft:makefile": {
        "tabstospaces": false,         // Makefile 规范必须使用硬 Tab
        "tabsize": 4
    },
    "ft:go": {
        "tabstospaces": false,         // Go 代码规范强制使用硬 Tab 缩进
        "tabsize": 4,
        "colorcolumn": 100
    },
    "ft:python": {
        "tabsize": 4,                  // Python PEP 8 严格规定使用 4 空格
        "tabstospaces": true,
        "colorcolumn": 79              // 限制 79 字符边界
    },
    "ft:markdown": {
        "softwrap": true,
        "wordwrap": true,
        "colorcolumn": 0,              // Markdown 关闭右侧限制虚线
        "rmtrailingws": false          // 严禁自动清除行尾空格（Markdown 的回车折行依赖行尾双空格）
    },
    "ft:text": {
        "softwrap": true,
        "wordwrap": true,
        "colorcolumn": 0
    },
    "ft:shell": {
        "tabsize": 4,
        "tabstospaces": true,
        "colorcolumn": 100
    },
    "ft:json": { "tabsize": 2, "tabstospaces": true },
    "ft:jsonc": { "tabsize": 2, "tabstospaces": true },
    "ft:html": { "tabsize": 2, "tabstospaces": true },
    "ft:javascript": { "tabsize": 2, "tabstospaces": true }
}
EOF
```

#### 2.2.2 写入快捷键映射配置 `bindings.json`

此配置文件为终端提供了类似现代 IDE 的高效率组合快捷键，覆盖了分屏、标签页切换、目录树开关以及 Omni 进阶检索功能：

```bash
cat > ~/.config/micro/bindings.json << 'EOF'
{
    "Alt-t": "command:tree",                  // Alt+t: 切换显示/隐藏左侧目录树面板
    "Alt-v": "vsplit",                        // Alt+v: 垂直分屏整个编辑区域
    "Alt-h": "hsplit",                        // Alt+h: 水平分屏整个编辑区域
    "Alt-p": "command:OmniGotoFile",          // Alt+p: 模糊文件名检索 (依赖 MicroOmni 插件)
    "Alt-f": "command:OmniSearchGlobal",      // Alt+f: 全局文本模糊检索 (依赖 MicroOmni 插件)
    "Alt-j": "command:OmniWordJump",          // Alt+j: 屏幕可见字符快速精准跳转 (EasyMotion)
    "Alt-[": "command:OmniPreviousHistory",   // Alt+[: 导航历史快速回退
    "Alt-]": "command:OmniNextHistory",       // Alt+]: 导航历史快速前进
    "Alt-m": "command:OmniMinimap",           // Alt+m: 切换显示/隐藏右侧代码缩略图 (Minimap)
    "Alt-Left": "PreviousTab",                // Alt+Left: 快速激活并跳到左侧标签页 (Tab)
    "Alt-Right": "NextTab"                    // Alt+Right: 快速激活并跳到右侧标签页 (Tab)
}
EOF
```

#### 2.2.3 编写 Lua 启动载入逻辑并实现【经典三栏布局】

通过配合 `filemanager.openonstart=true` 参数与 Lua 脚本在编辑器主程序初始化完成后的钩子行为，Micro 可以在启动时自动开启右侧的代码缩略图，从而**在没有任何前置按键输入的前提下，默认呈现 [左侧文件目录树 + 中间文本主编辑区 + 右侧 OmniMinimap 缩略图] 的现代三栏式高级 IDE 视图布局**。

```bash
cat > ~/.config/micro/init.lua << 'EOF'
local micro = import("micro")

-- postinit() 会在 micro 主程序与所有内置/第三方插件完全载入后由系统自动最后触发一次
function postinit()
    -- 强行对当前主编辑 Pane 发送激活 Minimap 指令
    -- 配合全局 settings 中已开启 of filemanager，完美拼接成三栏视图
    micro.CurPane():Command("OmniMinimap")
end
EOF
```

### 2.3 部署核心插件集

脚本配置了 5 个高效率的插件。我们采用 `git clone --depth=1` 的方式直接将其克隆存放到 Micro 的插件物理路径下：

```bash
local_plug_dir="$HOME/.config/micro/plug"
mkdir -p "$local_plug_dir"

# 克隆 NicolaiSoeborg 开发的文件管理器侧边栏插件
git clone --depth=1 https://github.com/NicolaiSoeborg/filemanager-plugin "$local_plug_dir/filemanager"

# 克隆整合了 FZF, Ripgrep 的 Omni 高级检索插件 (GotoFile / SearchGlobal)
git clone --depth=1 https://github.com/Neko-Box-Coder/MicroOmni "$local_plug_dir/MicroOmni"

# 克隆 Gutter 警报信息指示器插件
git clone --depth=1 https://github.com/usfbih8u/micro-gutter-message "$local_plug_dir/gutter_message"

# 克隆微型代码段缩写提示与快速补全插件
git clone --depth=1 https://github.com/micro-editor/snippets-plugin "$local_plug_dir/snippets"

# 克隆状态栏 Git 分支与改动标记插件
git clone --depth=1 https://github.com/Neko-Box-Coder/git-status "$local_plug_dir/gitStatus"
```

### 2.4 全局环境注入与多用户配置共享

为了让所有 SSH 用户、Shell 终端、默认系统关联程序及 Fish 用户共享这套开箱即用的配置，需要进行多用户软链同步，并注册系统默认替代项：

```bash
# 1. 自动软链接同步到系统其他用户的家目录 (排除真理源用户本身，以 sotuser 代指)
# 假定 sotuser 家目录为 /home/sotuser
# 对于其他真实用户 otheruser:
mkdir -p /home/otheruser/.config
rm -rf /home/otheruser/.config/micro
ln -sf /home/sotuser/.config/micro /home/otheruser/.config/micro
chown -h otheruser:otheruser /home/otheruser/.config/micro

# 2. 全局 Bash/Zsh 环境变量配置文件注入
cat > /etc/profile.d/micro_env.sh << 'EOF'
export EDITOR=micro
export VISUAL=micro
export MICRO_TRUECOLOR=1
EOF
chmod +x /etc/profile.d/micro_env.sh

# 3. 注入系统环境变量（作用于 cron、git commit、非交互式 shell 等）
# 在 /etc/environment 中写入：
# MICRO_TRUECOLOR=1
# EDITOR=micro
# VISUAL=micro

# 4. 注入 Fish Shell 全局变量 (若使用了 Fish)
fish -c "set -Ux MICRO_TRUECOLOR 1 && set -Ux EDITOR micro && set -Ux VISUAL micro"

# 5. 注册并强制设定为系统默认文本编辑器替代项 (Alternatives)
final_micro_bin=""
if [[ -f "/usr/local/bin/micro" ]]; then
    final_micro_bin="/usr/local/bin/micro"
elif [[ -f "/usr/bin/micro" ]]; then
    final_micro_bin="/usr/bin/micro"
fi

if [[ -n "$final_micro_bin" ]]; then
    update-alternatives --install /usr/bin/editor editor "$final_micro_bin" 100
    update-alternatives --set editor "$final_micro_bin"
fi
```

### 2.5 基础使用快捷键

| 快捷键 | 绑定映射 / 执行行为 | 核心作用说明 |
|---|---|---|
| **`Ctrl+S`** | `Save` | 保存当前文件 |
| **`Ctrl+Q`** | `Quit` | 关闭当前面板/退出编辑器 |
| **`Ctrl+Z`** | `Undo` | 撤销上一步操作 (支持关闭文件后撤销) |
| **`Ctrl+F`** | `Find` | 单文件内容快速定位与查找 |
| **`Ctrl+G`** | `GotoLine` | 跳转到指定行号 |
| **`Alt+t`** | `command:tree` | **切换显示/隐藏左侧侧边栏目录树** |
| **`Alt+v`** | `vsplit` | **左右垂直分割屏幕** |
| **`Alt+h`** | `hsplit` | **上下水平分割屏幕** |
| **`Alt+m`** | `command:OmniMinimap` | **切换显示/隐藏右侧代码缩略图** |
| **`Alt+p`** | `command:OmniGotoFile` | **类似 VS Code `Ctrl+P` 的模糊文件名检索** |
| **`Alt+f`** | `command:OmniSearchGlobal` | **类似 VS Code 的全局跨文件内容文本检索** |
| **`Alt+Left/Right`** | `PreviousTab / NextTab` | 在顶部的多标签页之间来回快速切换 |

### 2.6 卸载 Micro

若要完全擦除 Micro 及其在系统上的所有痕迹，可执行以下命令：

```bash
# 1. 物理移除二进制文件
rm -f /usr/local/bin/micro
rm -f /usr/local/bin/bat

# 2. 卸载包依赖
apt-get purge -y micro xclip

# 3. 清理所有用户的配置文件
rm -rf /root/.config/micro
rm -rf /home/*/.config/micro

# 4. 清理全局环境变量
rm -f /etc/profile.d/micro_env.sh
# 从 /etc/environment 中剔除 MICRO_TRUECOLOR, EDITOR, VISUAL 键值对

# 5. 从系统默认替代项中移除
update-alternatives --remove editor /usr/local/bin/micro 2>/dev/null || true
update-alternatives --remove editor /usr/bin/micro 2>/dev/null || true
```

---

## 3. Yazi 文件管理器

Yazi 是一个使用 Rust 开发的极速终端文件管理器，基于非阻塞异步 I/O 架构，具备极强的媒体预览与文件管理性能。本节还原脚本对 Yazi 二进制、定制快捷键（Windows/Micro 风格）、Shell 目录自适应同步（Wrapper）和官方插件集的完整手动安装设置。

### 3.1 安装物理依赖与提取二进制

Yazi 需要依赖 `unzip` 处理压缩包，`file` 辅助识别媒体类型，`jq` 处理 JSON 预览，以及 `p7zip-full` 作为内置压缩包管理器。

```bash
# 1. 安装系统物理依赖
apt-get install -y unzip file jq p7zip-full

# 2. 从 GitHub 获取最新 Release 版本号
LATEST_VERSION=$(curl -fsSL "https://api.github.com/repos/sxyazi/yazi/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)
[[ ! "$LATEST_VERSION" =~ ^v?[0-9] ]] && LATEST_VERSION="v0.4.0"  # 兜底版本

# 3. 根据系统 CPU 架构选择对应的 musl 二进制压缩包
ARCH=$(uname -m)
ASSET_NAME=""
if [[ "$ARCH" == "x86_64" ]]; then
    ASSET_NAME="yazi-x86_64-unknown-linux-musl.zip"
elif [[ "$ARCH" == "aarch64" ]]; then
    ASSET_NAME="yazi-aarch64-unknown-linux-musl.zip"
else
    echo "不支持的架构: $ARCH" && exit 1
fi

# 4. 下载并解压安装
curl -fsSL "https://github.com/sxyazi/yazi/releases/download/${LATEST_VERSION}/${ASSET_NAME}" -o /tmp/yazi.zip
mkdir -p /tmp/yazi_extracted
unzip -q -o /tmp/yazi.zip -d /tmp/yazi_extracted

# 提取主程序 yazi 与包管理器 ya 并移动到系统路径
bin_yazi=$(find /tmp/yazi_extracted -type f -name "yazi" -executable | head -n1)
bin_ya=$(find /tmp/yazi_extracted -type f -name "ya" -executable | head -n1)

if [[ -n "$bin_yazi" && -n "$bin_ya" ]]; then
    mv -f "$bin_yazi" /usr/local/bin/yazi
    mv -f "$bin_ya" /usr/local/bin/ya
    chmod +x /usr/local/bin/yazi /usr/local/bin/ya
else
    echo "未找到可执行文件，安装失败！" && exit 1
fi

rm -f /tmp/yazi.zip
rm -rf /tmp/yazi_extracted
```

### 3.2 部署真理源（SOT）配置文件

Yazi 的配置文件保存在真理源（SOT）用户的 `~/.config/yazi/` 目录下。

#### 3.2.1 优化全局设置 `yazi.toml`

```bash
mkdir -p ~/.config/yazi

cat > ~/.config/yazi/yazi.toml << 'EOF'
# =================================================================
# Yazi 全局行为优化配置 (General Settings)
# =================================================================

[manager]
# 界面比例：左侧父级目录、中间当前目录、右侧预览窗口
ratio = [1, 3, 4]

# 默认排序方式：按修改时间 (mtime) 倒序，最新修改排最上
sort_by = "mtime"
sort_sensitive = false
sort_reverse = true
sort_dir_first = true

# 默认不显示隐藏文件，可通过快捷键切换
show_hidden = false

# 显示软链指向的真实路径
show_symlink = true

[preview]
# 限制预览大小，防止大文件卡死
max_width = 800
max_height = 1000

# 缓存目录
cache_dir = "~/.cache/yazi"
EOF
```

#### 3.2.2 优化快捷键配置 `keymap.toml`（Windows / Micro 友好风格）

修改默认行为，使用符合现代 Windows 或 Micro 编辑器习惯的快捷键，大幅度降低终端文件管理的操作门槛：

```bash
cat > ~/.config/yazi/keymap.toml << 'EOF'
# =================================================================
# Yazi 快捷键配置 (Windows / Micro 风格优先)
# =================================================================

# ----------------- 文件基本操作 (Copy / Cut / Paste / Delete) -----------------
[[manager.prepend_keymap]]
on   = "<C-c>"
run  = "copy"
desc = "复制选中文件"

[[manager.prepend_keymap]]
on   = "<C-x>"
run  = "yank --cut"
desc = "剪切选中文件"

[[manager.prepend_keymap]]
on   = "<C-v>"
run  = "paste"
desc = "粘贴文件"

[[manager.prepend_keymap]]
on   = "<Delete>"
run  = "remove"
desc = "将选中文件移入回收站"

[[manager.prepend_keymap]]
on   = "<S-Delete>"
run  = "remove --permanently"
desc = "永久删除选中文件"

# ----------------- 撤销、搜索、选择与新建 -----------------
[[manager.prepend_keymap]]
on   = "<C-z>"
run  = "undo"
desc = "撤销上次文件操作"

[[manager.prepend_keymap]]
on   = "<C-a>"
run  = "select_all --state=true"
desc = "全选当前目录下文件"

[[manager.prepend_keymap]]
on   = "<Esc>"
run  = "escape --select"
desc = "取消所有选中状态"

[[manager.prepend_keymap]]
on   = "<C-n>"
run  = "create"
desc = "新建文件或文件夹 (加/结尾为文件夹)"

[[manager.prepend_keymap]]
on   = "F2"
run  = "rename --cursor=before_ext"
desc = "重命名文件 (光标定位在后缀名前)"

[[manager.prepend_keymap]]
on   = "<C-f>"
run  = "filter --smart"
desc = "开启实时搜索/过滤文件"

# ----------------- 辅助与导航 -----------------
[[manager.prepend_keymap]]
on   = "<Enter>"
run  = "open"
desc = "打开选中的文件/进入目录"

[[manager.prepend_keymap]]
on   = "<Right>"
run  = "enter"
desc = "进入目录"

[[manager.prepend_keymap]]
on   = "<Left>"
run  = "leave"
desc = "返回上一级目录"

[[manager.prepend_keymap]]
on   = "<Up>"
run  = "arrow -1"
desc = "光标上移"

[[manager.prepend_keymap]]
on   = "<Down>"
run  = "arrow 1"
desc = "光标下移"
EOF
```

### 3.3 部署 Shell 目录同步 Wrapper（核心体验）

为了使终端在使用 Yazi 退出时自动保持在 Yazi 的最后浏览目录（而非退回执行命令前的目录），需要在 shell 中注册包装函数 `y` 代替直呼 `yazi`。

#### 3.3.1 对 Bash / Zsh 用户部署包装器

```bash
cat > /etc/profile.d/yazi_wrapper.sh << 'EOF'
# =================================================================
# Yazi Shell CWD Synchronization Wrapper (Bash/Zsh)
# =================================================================
function y() {
    local tmp
    tmp="$(mktemp -t "yazi-cwd.XXXXXX")"
    yazi "$@" --cwd-file="$tmp"
    if cwd="$(command cat -- "$tmp")" && [ -n "$cwd" ] && [ "$cwd" != "$PWD" ]; then
        builtin cd -- "$cwd"
    fi
    rm -f -- "$tmp"
}
EOF
chmod +x /etc/profile.d/yazi_wrapper.sh
```

#### 3.3.2 对 Fish Shell 用户部署集成

若用户使用了 Fish，则需要在其配置目录中部署如下函数：

```bash
mkdir -p ~/.config/fish/conf.d/

cat > ~/.config/fish/conf.d/yazi.fish << 'EOF'
# =================================================================
# Yazi Shell CWD Synchronization Wrapper (Fish)
# =================================================================
function y
    set tmp (mktemp -t "yazi-cwd.XXXXXX")
    yazi $argv --cwd-file="$tmp"
    if set cwd (command cat -- "$tmp"); and [ -n "$cwd" ]; and [ "$cwd" != "$PWD" ]
        builtin cd -- "$cwd"
    end
    rm -f -- "$tmp"
end
EOF
```

### 3.4 部署官方插件集

使用 `ya` 包管理器为真理源用户安装常用的 Yazi 官方插件：

```bash
# ya pkg 需要在普通用户权限下运行（以普通用户身份执行，或 sudo -H -u username 运行）
ya pkg add yazi-rs/plugins:git            # 状态栏与侧边栏显示 Git 状态指示器
ya pkg add yazi-rs/plugins:chmod          # 支持在面板内直接修改选中文档的权限
ya pkg add yazi-rs/plugins:max-preview    # 允许一键将预览区域最大化，方便浏览长文本
```

### 3.5 多用户软链共享

类似于 Micro，将真理源的配置文件通过软链接同步给所有其他系统真实用户：

```bash
# 假定真理源普通用户为 sotuser，其家目录为 /home/sotuser
# 对其他用户 otheruser：
mkdir -p /home/otheruser/.config
rm -rf /home/otheruser/.config/yazi
ln -sf /home/sotuser/.config/yazi /home/otheruser/.config/yazi
chown -h otheruser:otheruser /home/otheruser/.config/yazi
```

### 3.6 卸载 Yazi

完全清除 Yazi 程序、Wrapper 以及所有用户的配置：

```bash
# 1. 物理移除二进制文件与全局包装器
rm -f /usr/local/bin/yazi
rm -f /usr/local/bin/ya
rm -f /etc/profile.d/yazi_wrapper.sh

# 2. 清理所有用户的配置目录
rm -rf /root/.config/yazi
rm -rf /home/*/.config/yazi
rm -f /home/*/.config/fish/conf.d/yazi.fish
```

---

## 4. Lego 自动化证书管理

[Lego](https://github.com/go-acme/lego) 是一个基于 ACME 协议的证书申请工具，支持 Let's Encrypt 和多种 DNS 提供商。相比 acme.sh，Lego 是单一二进制文件，部署更简单。脚本对其进行了自动化续期框架托管，支持多域名环境配置隔离、自动续期及 Web 服务重载（以 Ferron 推送为例）。

### 4.1 安装 Lego 二进制程序

```bash
# 1. 确定系统架构（x86_64 或 aarch64）
ARCH=$(uname -m)
[[ "$ARCH" == "x86_64" ]] && ARCH="amd64" || ARCH="arm64"

# 2. 获取最新版本并下载
LATEST=$(curl -fsSL "https://api.github.com/repos/go-acme/lego/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)
if [[ -z "$LATEST" ]]; then
    LATEST="v5.2.2"  # 稳定版兜底
fi

curl -fsSL "https://github.com/go-acme/lego/releases/download/${LATEST}/lego_${LATEST}_linux_${ARCH}.tar.gz" \
    -o /tmp/lego.tar.gz

# 3. 解压并移动到全局路径
mkdir -p /tmp/lego_extract
tar -xzf /tmp/lego.tar.gz -C /tmp/lego_extract
mv /tmp/lego_extract/lego /usr/local/bin/lego
chmod +x /usr/local/bin/lego

rm -rf /tmp/lego.tar.gz /tmp/lego_extract

# 验证
lego --version
```

### 4.2 配置目录与域名环境配置文件

托管系统采用 `/etc/lego/envs/` 目录存放各证书域名的环境变量文件。这使得定时续期任务可以自动轮询各域名环境。

首先，初始化证书及配置文件的物理存放路径：

```bash
mkdir -p /etc/lego/envs /var/lib/lego/certificates
```

编写域名的环境参数配置文件（示例：以 Cloudflare 验证申请 `example.com` 泛域名证书为例，写入 `/etc/lego/envs/example.com.env`）：

```bash
cat > /etc/lego/envs/example.com.env << 'EOF'
# DNS 提供商 API 凭证（以 Cloudflare 为例）
export CLOUDFLARE_DNS_API_TOKEN="your_cloudflare_api_token"

# Debopti 证书元数据管理
export DEBOPTI_DOMAINS="example.com,*.example.com" # 包含的所有域名 (半角逗号分隔)
export DEBOPTI_EMAIL="your@email.com"              # 注册邮箱
export DEBOPTI_PROVIDER="cloudflare"              # 验证提供商
export DEBOPTI_AUTO_RENEW="true"                  # 是否由定时任务托管自动续期
export DEBOPTI_FERRON_PUSH="true"                  # 续期成功后是否自动向 Ferron 推送重载
EOF
```

### 4.3 首次申请证书

手动导入或首次申请时，需在终端显式执行 `run` 命令（将相应变量替换为实际配置）：

```bash
CLOUDFLARE_DNS_API_TOKEN="your_cloudflare_api_token" \
/usr/local/bin/lego --email="your@email.com" \
    --dns="cloudflare" \
    --domains="example.com" \
    --domains="*.example.com" \
    --path="/var/lib/lego" \
    --accept-tos \
    run
```

证书会默认输出至：
- `/var/lib/lego/certificates/example.com.crt`  # 证书链
- `/var/lib/lego/certificates/example.com.key`  # 私钥

### 4.4 自动续期与推送重载脚本

自动框架部署了两个核心控制脚本：一个是自动续期主轮询脚本，另一个是更新后的动作推送钩子（Hook）。

#### 4.4.1 自动续期轮询脚本 `/usr/local/bin/debopti-lego-renew.sh`

此脚本扫描 `/etc/lego/envs/` 下的所有 `.env` 配置文件，并拉起 `lego` 检测续期：

```bash
cat > /usr/local/bin/debopti-lego-renew.sh << 'EOF'
#!/bin/bash
# =========================================================
# Lego 证书定时检测与自动续期主脚本 (Debopti 托管)
# =========================================================
set -euo pipefail

ENV_DIR="/etc/lego/envs"
if [[ ! -d "$ENV_DIR" ]]; then
    exit 0
fi

# 轮询所有域名的环境配置文件
for env_file in "$ENV_DIR"/*.env; do
    [[ ! -f "$env_file" ]] && continue
    
    # 局部子 Shell 执行，防止变量污染
    (
        source "$env_file"
        
        # 仅对开启自动更新的证书执行
        if [[ "${DEBOPTI_AUTO_RENEW:-}" == "true" ]]; then
            echo "🔹 [Lego] 开始检测/更新证书: $DEBOPTI_DOMAINS ..."
            
            # 解析域名列表为 lego 格式参数 (例如 "a.com,b.com" -> --domains=a.com --domains=b.com)
            domain_args=""
            IFS=',' read -ra ADDR <<< "$DEBOPTI_DOMAINS"
            for d in "${ADDR[@]}"; do
                domain_args="$domain_args --domains=$d"
            done
            
            # 首个域名作为主域名
            primary_domain="${ADDR[0]}"
            
            # 执行静默续期 (到期 30 天内才会实际触发申请)
            if /usr/local/bin/lego --email="$DEBOPTI_EMAIL" \
                             --dns="$DEBOPTI_PROVIDER" \
                             $domain_args \
                             --path="/var/lib/lego" \
                             --accept-tos \
                             renew --days 30 \
                             --renew-hook "/usr/local/bin/debopti-lego-hook.sh $primary_domain"; then
                echo "✨ [Lego] 证书检测完成: $primary_domain"
            else
                echo "❌ [Lego] 证书续期失败: $primary_domain"
            fi
        fi
    )
done
EOF
chmod +x /usr/local/bin/debopti-lego-renew.sh
```

#### 4.4.2 推送钩子脚本 `/usr/local/bin/debopti-lego-hook.sh`

当证书成功续期时，`lego` 会触发此脚本。它会自动将新证书复制给 Web 服务（如 Ferron 证书路径 `/etc/ferron/certs/`），进行权限安全加固并热重载服务：

```bash
cat > /usr/local/bin/debopti-lego-hook.sh << 'EOF'
#!/bin/bash
# =========================================================
# Lego 证书自动推送与服务重载钩子脚本 (Debopti 托管)
# =========================================================
set -euo pipefail

PRIMARY_DOMAIN="${1:-}"
if [[ -z "$PRIMARY_DOMAIN" ]]; then
    echo "❌ [Lego Hook] 缺少参数: 主域名。" >&2
    exit 1
fi

ENV_FILE="/etc/lego/envs/${PRIMARY_DOMAIN}.env"
if [[ ! -f "$ENV_FILE" ]]; then
    echo "❌ [Lego Hook] 未找到对应的配置文件: $ENV_FILE" >&2
    exit 1
fi

source "$ENV_FILE"

# 若开启了 Ferron 推送，则自动分发
if [[ "${DEBOPTI_FERRON_PUSH:-}" == "true" ]]; then
    if [[ -d "/etc/ferron" ]] || command -v ferron >/dev/null 2>&1; then
        echo "🔹 [Lego Hook] 检测到 Ferron，开始分发证书并设定高安全权限..."
        
        CRT_SRC="/var/lib/lego/certificates/${PRIMARY_DOMAIN}.crt"
        KEY_SRC="/var/lib/lego/certificates/${PRIMARY_DOMAIN}.key"
        
        if [[ ! -f "$CRT_SRC" || ! -f "$KEY_SRC" ]]; then
            echo "❌ [Lego Hook] 找不到已生成的证书文件: $CRT_SRC" >&2
            exit 1
        fi
        
        # 目标证书路径标准化
        mkdir -p /etc/ferron/certs
        cp "$CRT_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.crt"
        cp "$KEY_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.key"
        
        # 权限安全加固：私钥 600, 公钥 644, 目录 700
        chown -R ferron:ferron /etc/ferron/certs 2>/dev/null || true
        chmod 700 /etc/ferron/certs
        chmod 600 "/etc/ferron/certs/${PRIMARY_DOMAIN}.key"
        chmod 644 "/etc/ferron/certs/${PRIMARY_DOMAIN}.crt"
        
        # 重载 Web 服务
        if systemctl is-active --quiet ferron; then
            systemctl reload-or-restart ferron 2>/dev/null || systemctl restart ferron
            echo "✨ [Lego Hook] Ferron 服务已成功载入新证书并重载运行。"
        fi
    else
        echo "⚠️ [Lego Hook] 开启了 Ferron 推送但系统未安装 Ferron 服务，略过。"
    fi
fi
EOF
chmod +x /usr/local/bin/debopti-lego-hook.sh
```

### 4.5 配置 Systemd 定时续期任务

通过 Systemd Timer 定期触发（每天 03:15 和 15:15 运行，Let's Encrypt 官方建议每日运行两次以便在证书紧急吊销时做出快速自愈反应）。

#### 4.5.1 部署 Service 单元 `/etc/systemd/system/debopti-lego-renew.service`

```bash
cat > /etc/systemd/system/debopti-lego-renew.service << 'EOF'
[Unit]
Description=Lego ACME Certificate Automated Renewal Service
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/debopti-lego-renew.sh

# --- 安全沙盒限制 ---
PrivateTmp=true
ProtectSystem=full
ProtectHome=true
NoNewPrivileges=true
EOF
```

#### 4.5.2 部署 Timer 单元 `/etc/systemd/system/debopti-lego-renew.timer`

```bash
cat > /etc/systemd/system/debopti-lego-renew.timer << 'EOF'
[Unit]
Description=Twice daily check for Lego certificate renewal

[Timer]
# 每天 03:15 与 15:15 各检查执行一次
OnCalendar=*-*-* 03,15:15:00
# 随机随机延迟 0-1 小时，规避 ACME 上游并发洪峰限制
RandomizedDelaySec=1h
# 关机错过的检查开机时立即补做一次
Persistent=true

[Install]
WantedBy=timers.target
EOF
```

启用并激活定时任务：

```bash
systemctl daemon-reload
systemctl enable --now debopti-lego-renew.timer

# 查看状态
systemctl status debopti-lego-renew.timer
```

手动测试触发更新：

```bash
systemctl start debopti-lego-renew.service
# 查看日志
journalctl -u debopti-lego-renew.service -n 30
```

### 4.6 卸载 Lego

完全移除程序包、定时任务及所有相关证书配置：

```bash
# 1. 禁用并删除定时任务与服务
systemctl disable --now debopti-lego-renew.timer 2>/dev/null || true
systemctl stop debopti-lego-renew.service 2>/dev/null || true

rm -f /etc/systemd/system/debopti-lego-renew.{service,timer}
systemctl daemon-reload

# 2. 物理清除程序二进制与自动续期脚本
rm -f /usr/local/bin/lego
rm -f /usr/local/bin/debopti-lego-renew.sh
rm -f /usr/local/bin/debopti-lego-hook.sh

# 3. 彻底删除配置环境元数据及已申请的证书（警告：操作不可逆）
rm -rf /etc/lego /var/lib/lego
```
