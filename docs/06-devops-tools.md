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

Micro 是一个高度可定制、支持鼠标且开箱即用的终端文本编辑器。它比 nano 功能更强大，比 vim 的学习曲线更平缓。本节将完全还原自动化脚本的物理部署、插件集成、真彩色环境配置以及定制的纯净高效单栏编辑界面。

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
    "colorscheme": "railscast",          // 配色方案：Railscast
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
    "truecolor": "auto",                 // 自动检测或启用 24 位真彩色
    "fileformat": "unix",              // 强制新文件使用 Unix 换行符 (LF)
    "encoding": "utf-8",               // 编码格式设为 UTF-8
    "hltrailingws": true,              // 红色背景醒目高亮行尾死空格
    "hltaberrors": true,               // 当 tabstospaces 为真时，醒目高亮残留的硬 Tab
    "colorcolumn": 0,                  // 关闭右侧纵向代码宽度参考虚线 (0 为禁用)
    // =================================================================
    // 插件启用与配置 (Plugin Activation & Configuration)
    // =================================================================
    "autoclose": true,                 // 启用括号引号自动配对
    "comment": true,                   // 启用自动注释
    "ftoptions": true,                 // 启用文件类型专用选项
    "linter": true,                    // 启用实时语法检查
    "literate": true,                  // 启用 Literate 语法高亮
    "status": true,                    // 启用状态栏扩展
    "diff": true,                      // 启用 Git 修改差异对比
    
    "MicroOmni": true,                 // 启用高级检索插件
    "gutter_message": true,            // 启用 Gutter 报错气泡
    "snippets": true,                  // 启用代码片段展开
    "gitStatus": true,                 // 启用状态栏 Git 标志

    "diffgutter": true,                // 启用左侧 Git 差异指示线
    "statusline": true,                // 启用底部状态栏

    "MicroOmni.FzfCmd": "fzf",         // 模糊检索命令
    "MicroOmni.NewFileMethod": "smart_newtab", // 选中文件后的打开方式
    "MicroOmni.AutoSaveEnabled": true, // 开启会话自动保存
    "MicroOmni.AutoSaveInterval": 60,  // 每 60 秒自动保存会话

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
        "colorcolumn": 0
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
        "colorcolumn": 0
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
    "Alt-v": "VSplit",                        // Alt+v: 垂直分屏整个编辑区域
    "Alt-h": "HSplit",                        // Alt+h: 水平分屏整个编辑区域
    "Alt-p": "command:OmniGotoFile",          // Alt+p: 模糊文件名检索 (依赖 MicroOmni 插件)
    "Alt-f": "command:OmniSearchGlobal",      // Alt+f: 全局文本模糊检索 (依赖 MicroOmni 插件)
    "Alt-j": "command:OmniWordJump",          // Alt+j: 屏幕可见字符快速精准跳转 (EasyMotion)
    "Alt-[": "command:OmniPreviousHistory",   // Alt+[: 导航历史快速回退
    "Alt-]": "command:OmniNextHistory",       // Alt+]: 导航历史快速前进
    "Alt-Left": "PreviousTab",                // Alt+Left: 快速激活并跳到左侧标签页 (Tab)
    "Alt-Right": "NextTab"                    // Alt+Right: 快速激活并跳到右侧标签页 (Tab)
}
EOF
```

#### 2.2.3 编写 Lua 启动载入逻辑并实现【纯净单栏布局】

通过配合 Lua 脚本的就绪钩子事件，我们可以对 Micro 进行启动后初始化定制。默认情况下，我们保持纯净的单栏编辑器布局，以换取最大的代码视野与极简外观：

```bash
cat > ~/.config/micro/init.lua << 'EOF'
local micro = import("micro")

-- postinit() 会在 micro 主程序以及所有插件加载完毕、窗口完全就绪后自动运行一次
function postinit()
    -- 默认呈现纯净的单栏编辑器窗口
end
EOF
```

#### 2.2.4 项目与文件管理推荐流 (配合 Yazi 文件管理器)

由于传统 Micro 侧边栏文件树插件 (`filemanager`) 长期缺乏维护，且在 v2.0+ 下对于鼠标点击事件有严重的 Lua API 兼容报错，本项目**不再默认部署 filemanager 侧栏插件**，转而采用更加符合 Unix 哲学且极其高效的 **Yazi 联动流** 进行项目文件管理：
1. **唤起与导航**：在终端通过别名命令 `y` 瞬间唤起 Yazi，在纯 Rust 驱动的极速界面中进行目录导航、过滤和查找。
2. **编辑联动**：在 Yazi 中选中目标文件后直接回车，即可通过系统默认关联的 Micro 编辑器打开文件进行编辑。
3. **流畅返回**：编辑完毕后，在 Micro 中通过 `Ctrl+Q` 退出，即可瞬间无缝退回 Yazi 继续浏览，形成极速流畅的项目文件操控体验。

### 2.3 部署核心插件集

脚本配置了 4 个高效率的插件。我们采用 `git clone --depth=1` 的方式直接将其克隆存放到 Micro 的插件物理路径下：

```bash
local_plug_dir="$HOME/.config/micro/plug"
mkdir -p "$local_plug_dir"

# 克隆整合了 FZF, Ripgrep 的 Omni 高级检索插件 (GotoFile / SearchGlobal)
git clone --depth=1 https://github.com/Neko-Box-Coder/MicroOmni "$local_plug_dir/MicroOmni"

# 克隆 Gutter 警报信息指示器插件
git clone --depth=1 https://github.com/usfbih8u/micro-gutter-message "$local_plug_dir/gutter_message"

# 克隆官方 updated-plugins 仓库并提取 snippets 插件
git clone --depth=1 https://github.com/micro-editor/updated-plugins.git "/tmp/micro_updated_plugins"
mkdir -p "$local_plug_dir/snippets"
cp -rf "/tmp/micro_updated_plugins/micro-snippets-plugin/"* "$local_plug_dir/snippets/"
rm -rf "/tmp/micro_updated_plugins"

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
fish -c "set -Ux EDITOR micro && set -Ux VISUAL micro"

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
| **`Alt+v`** | `vsplit` | **左右垂直分割屏幕** |
| **`Alt+h`** | `hsplit` | **上下水平分割屏幕** |
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

[mgr]
# 界面比例：左侧父级目录、中间当前目录、右侧预览窗口
ratio = [1, 3, 4]

# 默认排序方式：文件夹置顶，其余按文件名字母升序
sort_by = "alphabetical"
sort_sensitive = false
sort_reverse = false
sort_dir_first = true

# 默认显示隐藏文件（以 . 开头的文件/文件夹），可在 Yazi 中按 . 键切换
show_hidden = true

# 显示软链指向的真实路径
show_symlink = true

[preview]
# 限制预览大小，防止大文件卡死
max_width = 800
max_height = 1000

# 缓存目录
cache_dir = "~/.cache/yazi"

[opener]
edit = [
    { run = 'micro "$@"', block = true, desc = "Edit", for = "unix" }
]

[open]
prepend_rules = [
    { mime = "text/*", use = "edit" },
    { mime = "application/json", use = "edit" },
    { mime = "application/javascript", use = "edit" },
    { mime = "", use = "edit" }
]
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
[[mgr.prepend_keymap]]
on   = [ "<C-c>" ]
run  = "yank"
desc = "复制选中文件 (Yank)"

[[mgr.prepend_keymap]]
on   = [ "<C-x>" ]
run  = "yank --cut"
desc = "剪切选中文件"

[[mgr.prepend_keymap]]
on   = [ "<C-v>" ]
run  = "paste"
desc = "粘贴文件"

[[mgr.prepend_keymap]]
on   = [ "<Delete>" ]
run  = "remove"
desc = "将选中文件移入回收站"

[[mgr.prepend_keymap]]
on   = [ "<S-Delete>" ]
run  = "remove --permanently"
desc = "永久删除选中文件"

# ----------------- 搜索、选择、新建与退出 -----------------
[[mgr.prepend_keymap]]
on   = [ "<C-a>" ]
run  = "select_all --state=true"
desc = "全选当前目录下文件"

[[mgr.prepend_keymap]]
on   = [ "<Esc>" ]
run  = "escape --select"
desc = "取消所有选中状态"

[[mgr.prepend_keymap]]
on   = [ "<C-n>" ]
run  = "create"
desc = "新建文件或文件夹 (加/结尾为文件夹)"

[[mgr.prepend_keymap]]
on   = [ "<F2>" ]
run  = "rename --cursor=before_ext"
desc = "重命名文件 (光标定位在后缀名前)"

[[mgr.prepend_keymap]]
on   = [ "<C-f>" ]
run  = "filter --smart"
desc = "开启实时搜索/过滤文件"

[[mgr.prepend_keymap]]
on   = [ "<C-p>" ]
run  = "search --via=fd"
desc = "全局模糊搜索文件名 (fd)"

[[mgr.prepend_keymap]]
on   = [ "<C-S-f>" ]
run  = "search --via=rg"
desc = "全局模糊搜索文件内容 (ripgrep)"

[[mgr.prepend_keymap]]
on   = [ "<C-q>" ]
run  = "quit"
desc = "退出 Yazi"

[[mgr.prepend_keymap]]
on   = [ "<C-Space>" ]
run  = "toggle"
desc = "选中/取消选中当前文件"

# ----------------- 辅助与导航 -----------------
[[mgr.prepend_keymap]]
on   = [ "<Enter>" ]
run  = "open"
desc = "打开选中的文件/进入目录"

[[mgr.prepend_keymap]]
on   = [ "<Right>" ]
run  = "enter"
desc = "进入目录"

[[mgr.prepend_keymap]]
on   = [ "<Left>" ]
run  = "leave"
desc = "返回上一级目录"

[[mgr.prepend_keymap]]
on   = [ "<Backspace>" ]
run  = "leave"
desc = "返回上一级目录"

[[mgr.prepend_keymap]]
on   = [ "<A-Up>" ]
run  = "leave"
desc = "返回上一级目录"

[[mgr.prepend_keymap]]
on   = [ "<A-Left>" ]
run  = "back"
desc = "回退历史目录"

[[mgr.prepend_keymap]]
on   = [ "<A-Right>" ]
run  = "forward"
desc = "前进历史目录"

[[mgr.prepend_keymap]]
on   = [ "<Up>" ]
run  = "arrow -1"
desc = "光标上移"

[[mgr.prepend_keymap]]
on   = [ "<Down>" ]
run  = "arrow 1"
desc = "光标下移"

[[mgr.prepend_keymap]]
on   = [ "<Home>" ]
run  = "arrow top"
desc = "跳转到首个文件"

[[mgr.prepend_keymap]]
on   = [ "<End>" ]
run  = "arrow bot"
desc = "跳转到末尾文件"

[[mgr.prepend_keymap]]
on   = [ "<PageUp>" ]
run  = "arrow -100%"
desc = "向上翻页"

[[mgr.prepend_keymap]]
on   = [ "<PageDown>" ]
run  = "arrow 100%"
desc = "向下翻页"
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

### 3.7 Yazi 进阶使用与日常操作教程 (Windows / Micro 快捷键风格)

本系统所部署的 Yazi 经过了深度的个性化快捷键重构，全面摆脱了传统 Vim 风格操作的学习门槛，改为了符合现代 Windows 资源管理器和 Micro 编辑器习惯的快捷键组合，极大地提升了终端文件管理的效率。

---

#### 3.7.1 界面结构与基础导航
Yazi 启动后，终端界面在视觉上被水平分割为三栏：
1. **左侧栏 (Parent)**：当前目录的父目录，用于展示所处目录层级。
2. **中间栏 (Current)**：当前活跃目录的文件列表，所有的操作光标和焦点都在此栏。
3. **右侧栏 (Preview)**：当前 hovered（光标停留）的文件的实时高亮预览（支持代码高亮、文本、JSON、媒体元数据等）。

##### 导航按键对照表：
| 操作目标 | 物理按键 (Windows/Micro 风格) | 说明 |
| :--- | :--- | :--- |
| **光标上/下移动** | `↑` / `↓` 方向键 | 在文件列表中逐个文件移动光标 |
| **向上翻页** | `PageUp` | 向上滚动一整个屏幕 |
| **向下翻页** | `PageDown` | 向下滚动一整个屏幕 |
| **跳至首个文件** | `Home` | 光标瞬间移动到当前目录的第一个文件 |
| **跳至最后一个文件** | `End` | 光标瞬间移动到当前目录的最后一个文件 |
| **进入目录 / 打开文件** | `Enter` (回车) 或 `→` 方向键 | 进入文件夹，或者根据关联规则打开选中的文件 |
| **返回上级目录** | `Backspace` (退格键)、`Alt + ↑` 或 `←` 方向键 | 返回当前目录的上一层父目录 |
| **目录历史：后退** | `Alt + ←` | 回退到上一个浏览过的目录历史 (类似浏览器后退) |
| **目录历史：前进** | `Alt + →` | 前进到下一个浏览过的目录历史 (类似浏览器前进) |
| **退出文件管理器** | `Ctrl + Q` | 瞬间优雅地关闭 Yazi 并清理终端界面 |

---

#### 3.7.2 多文件选择与选择模式
当需要对多个文件执行批量复制、移动或删除操作时，可以使用以下快捷键进行选择：
- **选中/取消选中当前文件**：按下 `Space` (空格键) 或 `Ctrl + Space`，当前行的文件名颜色会发生改变，表示被选中，同时光标自动下移一行。
- **视觉连续选择模式 (Visual Mode)**：按一下键盘上的小写字母 `v` 键，Yazi 会进入“选中状态选择模式”，此时您只需直接按下 `↑` 或 `↓` 方向键移动，所经过的所有行文件都会被自动连续选中。再次按 `v` 键可退出选择模式。
- **全选当前目录下所有文件**：按下 `Ctrl + A`。
- **取消全选/清除所有选中状态**：按下 `Esc` 键。

---

#### 3.7.3 日常文件操作与管理
所有文件管理快捷键已完美对齐 Windows 体验：
1. **新建文件或文件夹**：
   - 按下 **`Ctrl + N`**，界面底部会弹出 `Create:` 提示框。
   - 输入您要创建的名称。
   - **重要技巧**：如果名称以斜杠 `/` 结尾（例如 `my_new_folder/`），Yazi 将会创建为一个**文件夹**；若不带斜杠（例如 `config.json`），则会创建为一个**空白文件**。
2. **重命名文件**：
   - 将光标移动到目标文件上，按下 **`F2`** 键。
   - 界面底部会弹出 `Rename:` 修改框，并且光标已自动帮您**定位在文件后缀名（.ext）之前**，极大地方便了非破坏性重命名。
3. **复制 (Copy)**：
   - 选中一个或多个文件，按下 **`Ctrl + C`**（控制台下方会闪烁提示已 Yank 复制的文件数）。
4. **剪切 (Cut)**：
   - 选中一个或多个文件，按下 **`Ctrl + X`**（控制台下方提示已 Yank 剪切的文件）。
5. **粘贴 (Paste)**：
   - 导航进入您希望存放文件的目标目录中，按下 **`Ctrl + V`**，刚才复制或剪切的文件会瞬间出现在当前目录下。
6. **移入回收站 (Trash)**：
   - 选中文件后按下 **`Delete`** 键，文件会被安全地移至系统回收站（支持通过 CLI 恢复）。
7. **永久删除 (Delete Permanently)**：
   - 选中文件后按下 **`Shift + Delete`**，文件将被彻底物理删除，不经过回收站。
8. **过滤/实时搜索**：
   - 按下 **`Ctrl + F`**，在底部弹出 `Filter:` 输入框，随着您的拼写，当前目录下不匹配的文件会实时被过滤隐藏，方便在大目录中瞬间锁定特定目标。按 `Esc` 或 `Ctrl+C` 可以清空过滤恢复完整显示。

---

#### 3.7.4 使用不同软件打开文件 & 关联选择 (Openers)
Yazi 拥有极强的文件类型 MIME 检测和打开方式关联机制：

1. **默认关联编辑器 (Micro)**：
   本系统已在全局 `yazi.toml` 中为您做好了默认关联。对于所有的文本文件（`text/*`）、JSON 配置文件（`application/json`）、JavaScript 代码（`application/javascript`）以及无后缀或特殊系统配置文件，按下 **`Enter` (回车)** 后，Yazi 会在当前终端中**自动、挂起阻塞地拉起 `micro` 编辑器**。编辑完成后在 Micro 中按下 `Ctrl+Q` 退出，即可瞬间退回 Yazi，过程极为流畅。
2. **调出“打开方式...”关联菜单**：
   如果您需要用其他编辑器（例如系统自带的 `vi`、`nano`，或者是安装的开发包等）打开当前文件：
   - 移动光标至目标文件，按下小写字母 **`o`** 键。
   - 界面上会弹出一个悬浮的 `Open with` 菜单，列出当前系统为该类型文件配置的所有备选打开程序。
   - 输入菜单前对应的数字（或使用方向键选中），即可强制使用非默认的其他软件来编辑或查看该文件。

---

#### 3.7.5 运行自定义脚本与执行终端命令
Yazi 支持用户在无需退出界面的情况下，直接对选中的一个或多个文件运行任意 Linux 终端命令或脚本。

##### 1. 命令输入方式
- **交互式非阻塞命令 (按分号键 `;`)**：
  在底部调出命令窗口。在此输入的 Shell 命令将在后台异步、静默地运行，不会打断您的文件管理界面，适合运行耗时备份、下载等无交互任务。
- **阻塞挂起命令 (按冒号键 `:`)**：
  在底部调出命令窗口。在此输入的命令会**接管整个终端屏幕并挂起 Yazi 的界面**。您可以实时查看命令的执行进度、输出日志，甚至是进行控制台交互（如运行需要用户输入 `[y/N]` 确认的脚本）。执行完成后按任意键便可退回 Yazi。

##### 2. 占位符变量扩展（重点）
在按下 `;` 或 `:` 弹出的命令输入框中，Yazi 提供了强大的动态占位符，允许您将刚才在界面中**勾选/选中的文件作为参数**传递给 Shell 命令：

| 占位符 | 传递行为与含义 | 实际操作示例 |
| :--- | :--- | :--- |
| **`%s`** | 代表**当前所有选中文件**的绝对路径（以空格分隔） | 输入 `tar -czvf backup.tar.gz %s` 瞬间打包选中的多个文件 |
| **`%s1`** | 代表选中的第一个文件的绝对路径 | 输入 `diff %s1 %s2` 快速比对两个选中的配置文件 |
| **`%d`** | 代表选中文件所在的**当前目录绝对路径** | 输入 `echo %d` 可以查看或复制当前所处的物理目录 |

##### 3. 运行自定义脚本示例
假设您在项目目录中编写了一个名为 `process.sh` 的 Shell 脚本，需要将该脚本批量应用到当前目录下选中的若干个日志文件上：
1. 在 Yazi 列表中，通过 **`Space`** 键逐个勾选您要处理的日志文件（如 `log1.txt`, `log2.txt`）。
2. 按下冒号键 **`:`** 打开阻塞运行命令行。
3. 在弹出的 `Shell:` 输入框中，直接输入您的脚本命令，并配合 `%s` 占位符：
   ```bash
   chmod +x ./process.sh && ./process.sh %s
   ```
4. 按下回车，终端会清屏并实时打印出 `process.sh` 依次处理这些文件时的每一行控制台输出日志。
5. 脚本运行结束后，根据提示按下回车，您就再次回到了 Yazi 的精美两栏管理界面中，数据处理流程完美闭环。

---

### 3.8 多用户软链共享
类似于 Micro，将真理源的配置文件通过软链接同步给所有其他系统真实用户：

```bash
# 假定真理源普通用户为 sotuser，其家目录为 /home/sotuser
# 对其他用户 otheruser：
mkdir -p /home/otheruser/.config
rm -rf /home/otheruser/.config/yazi
ln -sf /home/sotuser/.config/yazi /home/otheruser/.config/yazi
chown -h otheruser:otheruser /home/otheruser/.config/yazi
```

### 3.9 卸载 Yazi

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
