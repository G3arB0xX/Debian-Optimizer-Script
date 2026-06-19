# 运维工具：Fish Shell、Micro 编辑器、Yazi 文件管理器、Lego 证书管理

本文档介绍这四个运维工具的手动安装与配置步骤，与 `debopti` 脚本自动化步骤完全对齐。

**前提条件**：root 权限，Debian 10+

**架构概要**：
- Fish / Micro / Yazi 采用 SOT 只读软链接模式：配置物理落在 SOT 用户家目录，`debopti` 启动时 `sync_devops_sot_links` 为新建用户自动同步
- 长配置文件完整内容统一存放在 `templates/apps/devops/` 与 `templates/apps/lego/`
- Fish / Yazi / Micro 的**使用教程**见 [09-terminal-toolchain.md](09-terminal-toolchain.md)
- **共享 CLI 依赖**：Fish / Micro / Yazi 重合的工具（`fzf`、`fd`、`ripgrep`、`zoxide`、剪贴板、`bat`、0xProto Nerd Font）由 `scripts/apps/devops.sh` 内 `_install_devops_shared_cli_deps()` **统一安装**；各模块仅调用此入口，避免重复逻辑与路径/版本冲突。策略为 **apt 优先 → 官方 Release/脚本回退**，已安装且版本满足要求则跳过。

### 共享 CLI 依赖一览

| 工具 | 主要用途 | Debian 包名 | apt 不可用时的官方回退 |
| --- | --- | --- | --- |
| `fzf`（≥ 0.53） | Fish / Micro / Yazi 模糊搜索 | `fzf` | [junegunn/fzf](https://github.com/junegunn/fzf/releases) 预编译包 → `/usr/local/bin/fzf` |
| `fd` | Yazi / Fish 文件搜索 | `fd-find`（`fdfind`） | 软链 `/usr/local/bin/fd`；仍无则 [sharkdp/fd](https://github.com/sharkdp/fd/releases) |
| `rg` | Micro / Yazi 内容搜索 | `ripgrep` | [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep/releases) `.deb` |
| `zoxide` | Fish / Yazi 历史目录 | `zoxide` | [官方 install.sh](https://github.com/ajeetdsouza/zoxide) |
| 剪贴板 | Micro / Yazi | `xclip` / `wl-clipboard` / `xsel` | 按序尝试 apt 安装三者 |
| `bat` | MicroOmni 高亮预览 | `bat`（命令常为 `batcat`） | 软链 `/usr/local/bin/bat` → `batcat` |
| 0xProto Nerd Font | Yazi 图标（NF + Mono + Propo） | `fonts-0xproto-nerd-font*`（源中有则装） | [Nerd Fonts 0xProto.tar.xz](https://github.com/ryanoasis/nerd-fonts/releases) → `/usr/local/share/fonts/0xProto-Nerd-Font/` |

Yazi 另有多媒体预览整组（`ffmpeg`、`poppler-utils`、`imagemagick`、`resvg`），其中 `resvg` 在 bookworm 常无 apt 包，x86_64 回退 [linebender/resvg](https://github.com/linebender/resvg/releases) 预编译包。

---

## 1. Fish Shell

Fish 是一个现代化的交互式 Shell，提供语法高亮、自动补全和 Git 状态显示等功能。

### 1.1 安装 Fish

Fish 本体与共享 CLI 依赖分开安装（与脚本 `install_fish` 一致）：

```bash
apt-get install -y fish curl git
# 共享依赖（fzf / fd / ripgrep / zoxide / 剪贴板 / bat / 0xProto Nerd Font）
# 脚本内由 _install_devops_shared_cli_deps 统一处理；手动对齐见上文「共享 CLI 依赖一览」
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

安装 puffer-fish 后，脚本会覆盖 SOT 用户的 `functions/_puffer_fish_expand_dot.fish`（模板 `templates/apps/devops/puffer_fish_expand_dot.fish`）：Debian 10~12 的 apt 自带 Fish 3.x 不支持 `commandline --search-field`，不覆盖则在输入 `.` 时会报错；Fish 4.x 仍保留上游在 pager 搜索框中的展开行为。

手动对齐时，在 fisher 安装插件后执行：

```bash
# 将 SOT_FISH 替换为真理源用户的 ~/.config/fish
cp templates/apps/devops/puffer_fish_expand_dot.fish "$SOT_FISH/functions/_puffer_fish_expand_dot.fish"
```

插件说明：
- `fzf.fish`：在 Fish 中集成 fzf 模糊搜索（文件、历史、变量等）
- `autopair.fish`：括号和引号自动配对
- `puffer-fish`：增强的补全提示
- `replay.fish`：在 Fish 中执行 bash 命令并同步环境变量

### 1.4 安装 zoxide（智能 cd）

zoxide 由共享依赖层 `_ensure_devops_zoxide` 安装（Fish / Yazi 共用，已装则跳过）。手动安装：

```bash
apt-get install -y zoxide 2>/dev/null || \
    curl -fsSL https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh | sh -s -- -y
```

在 Fish 配置中启用 zoxide：

```bash
mkdir -p ~/.config/fish/conf.d/
```

写入 `~/.config/fish/conf.d/zoxide.fish`（完整内容参见 `templates/apps/devops/zoxide.fish`）：

```fish
# zoxide 初始化：智能目录跳转
if command -v zoxide >/dev/null 2>&1
    zoxide init fish | source
else if test -f /usr/local/bin/zoxide
    /usr/local/bin/zoxide init fish | source
end
```

### 1.5 安装 Starship（Prompt 美化）

```bash
# 优先尝试 APT
apt-get install -y starship 2>/dev/null || \
    curl -fsSL https://starship.rs/install.sh | sh -s -- -y
```

在 Fish 配置中启用 Starship：

写入 `~/.config/fish/conf.d/starship.fish`（完整内容参见 `templates/apps/devops/starship.fish`）：

```fish
# Starship Prompt 初始化
if command -v starship >/dev/null 2>&1
    starship init fish | source
else if test -f /usr/local/bin/starship
    /usr/local/bin/starship init fish | source
end
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

> Fish 交互用法见 [09-terminal-toolchain.md](09-terminal-toolchain.md#1-fish-shell-命令行交互)。

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

Micro 专属依赖与共享 CLI 依赖分开安装（与脚本 `install_micro` 一致）：

```bash
# 1. Micro 专属 apt 包
apt-get install -y shellcheck yamllint

# 2. 共享 CLI 依赖（fzf / ripgrep / xclip / bat 等）——见「共享 CLI 依赖一览」

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
```

写入 `~/.config/micro/settings.json`（完整内容参见 `templates/apps/devops/micro_settings.json`）：

```json
{
    // =================================================================
    // 基础与常规设置 (Basic & General Settings)
    // =================================================================
    
    // 配色方案：simple (经典白底黑字) / railscast 等
    "colorscheme": "railscast",
    
    // 鼠标支持：允许鼠标点击定位、滚轮滚动与文本选中
    "mouse": true,
    
    // 记录光标位置：重新打开文件时，自动定位到上次光标所在行
    "savecursor": true,
    
    // 记录撤销历史：关闭文件后，依然保留历史撤销 (Undo) 记录
    "saveundo": true,
    
    // 滚动条：在编辑器右侧（中间编辑区右侧）显示静态滚动进度条
    "scrollbar": true,
    
    // 缩进大小：默认 Tab 宽度为 4 个空格
    "tabsize": 4,
    
    // 自动缩进：换行时自动与上一行对齐
    "autoindent": true,
    
    // 自动提权：当修改只读/系统文件时，保存时自动尝试 sudo 提权
    "autosu": true,
    
    // 光标行高亮：高亮显示当前光标所在行
    "cursorline": true,
    
    // 文件尾空行：保存时自动在文件末尾追加一个换行符（符合 POSIX 标准）
    "eofnewline": true,
    
    // 快速脏标记：优化大文件修改状态的检测性能
    "fastdirty": true,
    
    // 自动创建父目录：保存不存在的路径时，自动创建缺失的父文件夹
    "mkparents": true,
    
    // 去除行尾死空格：保存时自动清除所有行尾多余的空格与制表符
    "rmtrailingws": true,
    
    // 软换行：当单行内容超出窗口宽度时，自动进行视觉软换行
    "softwrap": true,
    
    // Tab 转空格：按下 Tab 键时自动转换为对应数量的空格
    "tabstospaces": true,
    
    // 单词换行：软换行时尽量不在单词中间折行，保证阅读连贯性
    "wordwrap": true,
    
    // 仅显示文件名：在标题栏/状态栏中仅显示当前文件名，不显示绝对路径
    "basename": true,
    
    // 搜索忽略大小写：查找文本时默认忽略大小写
    "ignorecase": true,
    
    // 括号匹配：光标位于括号上时，自动高亮对应的配对括号
    "matchbrace": true,
    
    // 括号高亮延迟：括号匹配检测的响应时间
    "matchbracewait": "50ms",
    
    // 行号显示：在编辑器左侧显示行号
    "ruler": true,
    
    // 增量搜索：在输入查找关键字时，实时高亮匹配结果
    "incsearch": true,
    
    // 智能粘贴：在粘贴代码时自动临时关闭自动缩进，防止缩进混乱
    "smartpaste": true,
    
    // 提权命令：指定 autosu 所使用的提权命令，默认为 sudo
    "sucmd": "sudo",
    
    // =================================================================
    // 进阶与终端兼容性设置 (Advanced & Terminal Settings)
    // =================================================================
    
    // 跨机剪贴板：使用 terminal (OSC 52 协议)，支持远程 SSH 内容直接复制到本地系统剪贴板
    "clipboard": "terminal",
    
    // 真彩色支持：自动检测或启用 24 位真彩色，确保在 SSH/tmux 等复杂环境下主题配色不降级
    "truecolor": "auto",
    
    // 换行符格式：强制新文件使用 Unix 格式换行符 (LF)，防止 Windows 换行符 (CRLF) 导致脚本失效
    "fileformat": "unix",
    
    // 字符编码：强制新文件使用 UTF-8 编码
    "encoding": "utf-8",
    
    // 行尾空格高亮：用醒目的背景色实时高亮行尾的死空格
    "hltrailingws": true,
    
    // 制表符错误高亮：在 tabstospaces 为 true 时，若文件中混入硬 Tab，则醒目高亮显示
    "hltaberrors": true,
    
    // 视觉边界线：在第 100 列绘制一条垂直虚线，辅助规范单行代码长度 (0 为禁用)
    "colorcolumn": 0,
    
    // =================================================================
    // 插件启用与配置 (Plugin Activation & Configuration)
    // =================================================================
    
    // 1. 显式启用内置插件 (Enable Built-in Plugins)
    "autoclose": true,
    "comment": true,
    "ftoptions": true,
    "linter": true,
    "literate": true,
    "status": true,
    "diff": true,
    
    // 2. 显式启用自定义插件 (Enable Custom Plugins)
    "MicroOmni": true,
    "gutter_message": true,
    "snippets": true,
    "gitStatus": true,

    // 3. 内置插件最佳实践配置 (Built-in Plugin Settings)
    "diffgutter": true,                // 开启左侧 Git 差异指示线
    "statusline": true,                // 开启底部状态栏

    // 5. MicroOmni 插件详细配置
    "MicroOmni.FzfCmd": "fzf",         // 绑定 FZF 命令
    "MicroOmni.NewFileMethod": "smart_newtab", // 模糊打开新文件的窗口方式
    "MicroOmni.AutoSaveEnabled": true, // 开启会话自动保存
    "MicroOmni.AutoSaveInterval": 60,  // 每 60 秒自动保存会话
    
    // =================================================================
    // 文件类型局部覆盖设置 (Filetype Overrides)
    // =================================================================
    
    // YAML 配置：严禁使用硬 Tab，缩进固定为 2 空格
    "ft:yaml": {
        "tabsize": 2,
        "tabstospaces": true,
        "hltaberrors": true,
        "colorcolumn": 80
    },
    
    // Makefile：语法规定必须使用原生硬 Tab，禁用空格转换
    "ft:makefile": {
        "tabstospaces": false,
        "tabsize": 4
    },
    
    // Go 语言：遵循 gofmt 标准，强制使用硬 Tab 缩进
    "ft:go": {
        "tabstospaces": false,
        "tabsize": 4,
        "colorcolumn": 0
    },
    
    // Python：严格遵循 PEP 8 规范，使用 4 空格缩进，边界线设为 79
    "ft:python": {
        "tabsize": 4,
        "tabstospaces": true,
        "colorcolumn": 79
    },
    
    // Markdown 与纯文本：关闭纵向参考线，允许视觉软折行，且禁用保存时自动删除行尾空格（Markdown换行需要行尾双空格）
    "ft:markdown": {
        "softwrap": true,
        "wordwrap": true,
        "colorcolumn": 0,
        "rmtrailingws": false
    },
    "ft:text": {
        "softwrap": true,
        "wordwrap": true,
        "colorcolumn": 0
    },
    
    // Shell 脚本 (Bash / Sh)
    "ft:shell": {
        "tabsize": 4,
        "tabstospaces": true,
        "colorcolumn": 0
    },
    
    // 常见 Web 配置文件 (JSON, JSONC, HTML, JS 等)，采用 2 空格缩进规范
    "ft:json": { "tabsize": 2, "tabstospaces": true },
    "ft:jsonc": { "tabsize": 2, "tabstospaces": true },
    "ft:html": { "tabsize": 2, "tabstospaces": true },
    "ft:javascript": { "tabsize": 2, "tabstospaces": true }
}
```

#### 2.2.2 写入快捷键映射配置 `bindings.json`

此配置文件为终端提供了类似现代 IDE 的高效率组合快捷键，覆盖了分屏、标签页切换、目录树开关以及 Omni 进阶检索功能：

写入 `~/.config/micro/bindings.json`（完整内容参见 `templates/apps/devops/micro_bindings.json`）：

```json
{
    // =================================================================
    // 快捷键映射配置 (Bindings Settings)
    // =================================================================

    // Alt+v: 垂直分屏 (Vertical Split)
    "Alt-v": "VSplit",

    // Alt+h: 水平分屏 (Horizontal Split)
    "Alt-h": "HSplit",

    // Alt+p: 模糊文件名检索 (类似 VS Code 的 Ctrl+P，需要 MicroOmni 插件)
    "Alt-p": "command:OmniGotoFile",

    // Alt+f: 全局文本检索 (类似 VS Code 的 Ctrl+Shift+F，需要 MicroOmni 插件)
    "Alt-f": "command:OmniSearchGlobal",

    // Alt+j: 屏幕内字符快速定位/跳转 (EasyMotion，需要 MicroOmni 插件)
    "Alt-j": "command:OmniWordJump",

    // Alt+[: 导航历史回退 (跳转到定义后快速返回，需要 MicroOmni 插件)
    "Alt-[": "command:OmniPreviousHistory",

    // Alt+]: 导航历史前进 (需要 MicroOmni 插件)
    "Alt-]": "command:OmniNextHistory",

    // Alt+Left: 快速切换到左侧标签页 (Tab)
    "Alt-Left": "PreviousTab",

    // Alt-Right: 快速切换到右侧标签页 (Tab)
    "Alt-Right": "NextTab"
}
```

#### 2.2.3 编写 Lua 启动载入逻辑并实现【纯净单栏布局】

通过配合 Lua 脚本的就绪钩子事件，我们可以对 Micro 进行启动后初始化定制。默认情况下，我们保持纯净的单栏编辑器布局，以换取最大的代码视野与极简外观：

写入 `~/.config/micro/init.lua`（完整内容参见 `templates/apps/devops/micro_init.lua`）：

```lua
local micro = import("micro")

-- postinit() 会在 micro 主程序以及所有插件加载完毕、窗口完全就绪后自动运行一次
function postinit()
    -- 默认呈现纯净的单栏编辑器窗口
end
```

> Micro 与 Yazi 的协同使用流程见 [09-terminal-toolchain.md](09-terminal-toolchain.md#4-工具链协同工作流与场景示例)。

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

# MicroOmni 兼容补丁：upstream Session.lua 误用 os.MkdirAll，自动保存会话时会报错
# 必须使用精确规则；禁止 s/os\.ModePerm/goos.ModePerm/ 全局替换（会把 goos 变成 gogoos）
sed -i -f templates/apps/devops/micro_omni_session.sed "$local_plug_dir/MicroOmni/Session.lua"
mkdir -p "$local_plug_dir/MicroOmni/sessions"
```

MicroOmni 在开启 `MicroOmni.AutoSaveEnabled` 时会定时调用 `SaveSession`。上游 commit `ff28759e` 将 `goos.MkdirAll` 误写为 `os.MkdirAll`，导致约每 60 秒弹出 Lua 错误。脚本在克隆插件后应用 `templates/apps/devops/micro_omni_session.sed` 修正该行，并预建 `sessions/` 目录。

**已用错误命令修过文件时**（`s/os\.ModePerm/goos.ModePerm/` 会把 `goos.ModePerm` 变成 `gogoos.ModePerm`）：

```bash
sed -i 's/gogogoos/goos/g; s/gogoos/goos/g' ~/.config/micro/plug/MicroOmni/Session.lua
sed -i -f templates/apps/devops/micro_omni_session.sed ~/.config/micro/plug/MicroOmni/Session.lua
mkdir -p ~/.config/micro/plug/MicroOmni/sessions
```

已安装环境可执行上述命令，或重新运行 Micro 安装。

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
```

写入 `/etc/profile.d/micro_env.sh`（完整内容参见 `templates/apps/devops/micro_env.sh`）：

```bash
# Micro 编辑器全局环境变量（真彩色由 settings.json 的 "truecolor": "auto" 控制，见官方 options.md）
export EDITOR=micro
export VISUAL=micro
```

```bash
chmod +x /etc/profile.d/micro_env.sh

# 3. 注入系统环境变量（作用于 cron、git commit、非交互式 shell 等）
# 在 /etc/environment 中写入：
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

> Micro 快捷键与插件用法见 [09-terminal-toolchain.md](09-terminal-toolchain.md#3-micro-终端文本编辑器)。

### 2.5 卸载 Micro

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
# 从 /etc/environment 中剔除 EDITOR, VISUAL（及旧版遗留的 MICRO_TRUECOLOR）键值对

# 5. 从系统默认替代项中移除
update-alternatives --remove editor /usr/local/bin/micro 2>/dev/null || true
update-alternatives --remove editor /usr/bin/micro 2>/dev/null || true
```

---

## 3. Yazi 文件管理器

Yazi 是一个使用 Rust 开发的极速终端文件管理器，基于非阻塞异步 I/O 架构，具备极强的媒体预览与文件管理性能。本节还原脚本对 Yazi 二进制、定制快捷键（Windows/Micro 风格）、Shell 目录自适应同步（Wrapper）和官方插件集的完整手动安装设置。

### 3.1 安装物理依赖与提取二进制

脚本安装/更新 Yazi 时会同步安装 [Yazi 官方推荐的可选 CLI 依赖](https://yazi-rs.github.io/docs/installation/)。远程 VPS 上通常用不上的**多媒体预览**部分（`ffmpeg`、`poppler-utils`、`resvg`、`imagemagick`）在首次安装时以 **y/N** 整组选装，选择写入 `/etc/debopti/debopti.conf` 的 `YAZI_MEDIA_PREVIEW`（`true` / `false`）；之后重装/更新将按该值自动补齐，无需重复询问。卸载 Yazi **不会**移除这些 apt 包（可能与 Fish、Micro 等模块共用）。

| 官方工具 | Debian/Ubuntu 包名 | 脚本策略 | 用途 |
| --- | --- | --- | --- |
| nerd-fonts（推荐） | `fonts-0xproto-nerd-font*` 或 Nerd Fonts Release | **共享层**默认装 0xProto（NF + Mono + Propo） | 终端图标；SSH 场景客户端字体仍建议用 Nerd Font |
| 7-Zip | `p7zip-full` | Yazi 专属 apt | 压缩包解压与预览 |
| `file` | `file` | Yazi 专属 apt | 识别媒体 MIME 类型 |
| `jq` | `jq` | Yazi 专属 apt | JSON 预览 |
| `fd` | `fd-find` 或 GitHub Release | **共享层** `_ensure_devops_fd` | 文件搜索 |
| `rg` | `ripgrep` 或 GitHub `.deb` | **共享层** `_ensure_devops_ripgrep` | 文件内容搜索 |
| `fzf` | `fzf`（≥ 0.53）或 GitHub Release | **共享层** `_ensure_devops_fzf` | 快速子树导航 |
| `zoxide` | `zoxide` 或官方 install.sh | **共享层** `_ensure_devops_zoxide` | 历史目录导航（依赖 fzf） |
| 剪贴板 | `xclip` / `wl-clipboard` / `xsel` | **共享层** `_ensure_devops_clipboard` | Linux 剪贴板 |
| `ffmpeg` | `ffmpeg` | **多媒体整组** | 视频缩略图 |
| `poppler` | `poppler-utils` | **多媒体整组** | PDF 预览 |
| `resvg` | `resvg` 或 GitHub 预编译（x86_64） | **多媒体整组** + `_ensure_devops_resvg` | SVG 预览 |
| ImageMagick | `imagemagick`（≥ 7.1.1 功能更完整） | **多媒体整组** | 字体、HEIC、JPEG XL 等预览 |

手动安装时，Yazi 专属 apt 包与共享层分开（与脚本 `_install_yazi_dependencies` 一致）：

```bash
# 1. Yazi 专属 apt
apt-get install -y unzip file jq p7zip-full

# 2. 共享 CLI 依赖——见上文「共享 CLI 依赖一览」各工具的手动命令

# 3. 多媒体预览整组（远程 VPS 可跳过，对应 YAZI_MEDIA_PREVIEW=false）
# apt-get install -y ffmpeg poppler-utils imagemagick
# resvg：apt 无包时 x86_64 可从 linebender/resvg Releases 解压 resvg 至 /usr/local/bin/

# 4. 从 GitHub 获取最新 Release 版本号
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
```

写入 `~/.config/yazi/yazi.toml`（完整内容参见 `templates/apps/devops/yazi.toml`）：

```toml
# =================================================================
# Yazi 全局行为优化配置 (General Settings)
# =================================================================

[mgr]
# 界面比例：左侧父级目录、中间当前目录、右侧预览窗口（预览约占 56% 宽度）
ratio = [1, 3, 5]

# 列表行右侧默认显示最后修改时间（运行时可用 m,m / m,p / m,o 等切换）
linemode = "mtime"

# 默认排序方式：文件夹置顶，其余按文件名自然数升序
sort_by = "natural"
sort_sensitive = false
sort_reverse = false
sort_dir_first = true

# 默认显示隐藏文件（以 . 开头的文件/文件夹），可在 Yazi 中按 . 键切换
show_hidden = true

# 显示软链指向的真实路径
show_symlink = true

[preview]
# 长行自动换行，便于阅读代码与日志
wrap = "yes"

# 图片预览上限（设大以随终端自适应；修改后需 yazi --clear-cache）
max_width = 10000
max_height = 10000

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
```

#### 3.2.2 自定义列表元信息 `init.lua`（可选）

默认 `linemode = "mtime"` 使用 Yazi 内置能力，无需 `init.lua`。若要在列表行**同时**显示权限与所有者，可将 `linemode` 改为 `perm_owner` 并配合下方 `init.lua`（内置 linemode 仅支持 `permissions` / `owner` 二选一，无法同时显示）。

写入 `~/.config/yazi/init.lua`（完整内容参见 `templates/apps/devops/yazi_init.lua`）：

```lua
function Linemode:perm_owner()
	if ya.target_family() ~= "unix" then
		return ""
	end

	local perm = self._file.cha:perm() or ""
	local user = ya.user_name and ya.user_name(self._file.cha.uid) or self._file.cha.uid
	local group = ya.group_name and ya.group_name(self._file.cha.gid) or self._file.cha.gid
	return string.format("%s %s:%s", perm, user, group)
end
```

#### 3.2.3 优化快捷键配置 `keymap.toml`（Windows / Micro 友好风格）

修改默认行为，使用符合现代 Windows 或 Micro 编辑器习惯的快捷键，大幅度降低终端文件管理的操作门槛：

写入 `~/.config/yazi/keymap.toml`（完整内容参见 `templates/apps/devops/yazi_keymap.toml`）：

```toml
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

# ----------------- 在当前目录打开 Shell -----------------
[[mgr.prepend_keymap]]
on   = [ "t", "e" ]
run  = "shell fish --block"
desc = "在当前目录打开 Fish 终端"

[[mgr.prepend_keymap]]
on   = "!"
run  = "shell fish --block"
desc = "在当前目录打开 Fish 终端（需 CSI u）"

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
```

### 3.3 部署 Shell 目录同步 Wrapper（核心体验）

为了使终端在使用 Yazi 退出时自动保持在 Yazi 的最后浏览目录（而非退回执行命令前的目录），需要在 shell 中注册包装函数 `y` 代替直呼 `yazi`。

#### 3.3.1 对 Bash / Zsh 用户部署包装器

写入 `/etc/profile.d/yazi_wrapper.sh`（完整内容参见 `templates/apps/devops/yazi_wrapper.sh`）：

```bash
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
```

```bash
chmod +x /etc/profile.d/yazi_wrapper.sh
```

#### 3.3.2 对 Fish Shell 用户部署集成

脚本通过 `deploy_fish_yazi_wrapper` 与 `sync_devops_sot_links` 自动部署（`install_yazi` / `install_fish` 结束及 `debopti` 启动时调用），覆盖：

- `/etc/fish/conf.d/yazi.fish`（系统级兜底）
- SOT 用户 `~/.config/fish/functions/y.fish`（非 SOT 用户经只读软链共享 `functions/`）
- 非 SOT 用户的 `fish_variables` 保留本地可写，不链入 SOT

手动部署时写入 `~/.config/fish/functions/y.fish`（完整内容参见 `templates/apps/devops/y.fish`）：

```fish
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
```

### 3.4 部署官方插件集

使用 `ya` 包管理器为真理源用户安装常用的 Yazi 官方插件：

```bash
# ya pkg 需要在普通用户权限下运行（以普通用户身份执行，或 sudo -H -u username 运行）
ya pkg add yazi-rs/plugins:git            # 状态栏与侧边栏显示 Git 状态指示器
ya pkg add yazi-rs/plugins:chmod          # 支持在面板内直接修改选中文档的权限
ya pkg add yazi-rs/plugins:max-preview    # 允许一键将预览区域最大化，方便浏览长文本
```

### 3.5 多用户只读软链共享

`sync_devops_sot_links` 将真理源配置以只读软链接同步给 `get_all_real_users()` 范围内的其他用户（含 `root`；排除规则不变）。手动示例：

```bash
# 假定真理源普通用户为 sotuser，其家目录为 /home/sotuser
# 对其他用户 otheruser：
mkdir -p /home/otheruser/.config
rm -rf /home/otheruser/.config/yazi
ln -sf /home/sotuser/.config/yazi /home/otheruser/.config/yazi
chown -h otheruser:otheruser /home/otheruser/.config/yazi
```

> Yazi 快捷键、文件操作与 Shell 命令联动见 [09-terminal-toolchain.md](09-terminal-toolchain.md#2-yazi-终端文件管理器)。

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
rm -f /etc/fish/conf.d/yazi.fish
rm -f /etc/debopti/sot_known_users
# 非 SOT 用户仅删除软链，勿 rm -rf 跟随软链指向的 SOT 物理目录
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

将占位符替换为实际值，写入 `/etc/lego/envs/example.com.env`（完整内容参见 `templates/apps/lego/lego.env`）：

```bash
# DNS 提供商 API 凭证（以 Cloudflare 为例）
export CLOUDFLARE_DNS_API_TOKEN="your_cloudflare_api_token"

# Debopti 证书元数据
export DEBOPTI_DOMAINS="example.com,*.example.com"  # 包含的所有域名（半角逗号分隔）
export DEBOPTI_EMAIL="your@email.com"               # 注册邮箱
export DEBOPTI_PROVIDER="cloudflare"                # DNS 验证提供商
export DEBOPTI_AUTO_RENEW="true"                    # 是否由定时任务托管自动续期
export DEBOPTI_FERRON_PUSH="true"                   # 续期成功后是否自动向 Ferron 推送重载
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

写入 `/usr/local/bin/debopti-lego-renew.sh`（完整内容参见 `templates/apps/lego/debopti-lego-renew.sh`）：

```bash
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
    
    # 局部载入环境变量，防止变量污染
    (
        # shellcheck disable=SC1090
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
            # shellcheck disable=SC2086
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
```

```bash
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

# 载入配置
# shellcheck disable=SC1090
source "$ENV_FILE"

# 若开启了 Ferron 推送
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
        
        # 优雅重载/重启 Web 服务
        if systemctl is-active --quiet ferron; then
            systemctl reload-or-restart ferron 2>/dev/null || systemctl restart ferron
            echo "✨ [Lego Hook] Ferron 服务已成功载入新证书并重载运行。"
        fi
    else
        echo "⚠️ [Lego Hook] 开启了 Ferron 推送但系统未安装 Ferron 服务，略过。"
    fi
fi
```

#### 4.4.2 推送钩子脚本 `/usr/local/bin/debopti-lego-hook.sh`

当证书成功续期时，`lego` 会触发此脚本。它会自动将新证书复制给 Web 服务（如 Ferron 证书路径 `/etc/ferron/certs/`），进行权限安全加固并热重载服务：

写入 `/usr/local/bin/debopti-lego-hook.sh`（完整内容参见 `templates/apps/lego/debopti-lego-hook.sh`）：

```bash
#!/bin/bash
set -euo pipefail
# 续期成功后推送证书至 Ferron 并重载服务
```

```bash
chmod +x /usr/local/bin/debopti-lego-hook.sh
```

### 4.5 配置 Systemd 定时续期任务

通过 Systemd Timer 定期触发（每天 03:15 和 15:15 运行，Let's Encrypt 官方建议每日运行两次以便在证书紧急吊销时做出快速自愈反应）。

#### 4.5.1 部署 Service 单元 `/etc/systemd/system/debopti-lego-renew.service`

写入 `/etc/systemd/system/debopti-lego-renew.service`（完整内容参见 `templates/apps/lego/debopti-lego-renew.service`）：

```ini
[Unit]
Description=Lego ACME Certificate Automated Renewal Service
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/debopti-lego-renew.sh

# 安全沙盒限制
PrivateTmp=true
ProtectSystem=full
ProtectHome=true
NoNewPrivileges=true
```

#### 4.5.2 部署 Timer 单元 `/etc/systemd/system/debopti-lego-renew.timer`

写入 `/etc/systemd/system/debopti-lego-renew.timer`（完整内容参见 `templates/apps/lego/debopti-lego-renew.timer`）：

```ini
[Unit]
Description=Twice daily check for Lego certificate renewal

[Timer]
# 每天 03:15 与 15:15 各检查一次
OnCalendar=*-*-* 03,15:15:00
# 随机延迟 0-1 小时，规避 ACME 并发洪峰
RandomizedDelaySec=1h
# 关机错过的检查开机时补做
Persistent=true

[Install]
WantedBy=timers.target
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
