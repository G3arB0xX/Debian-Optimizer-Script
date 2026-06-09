# Fish + Yazi + Micro 终端工具链协同使用教程

本项目在 Debian 系统中部署了 Fish Shell、Yazi 文件管理器和 Micro 编辑器，并对其快捷键进行了对齐与配置共享。本教程介绍这三个工具的独立使用方法、快捷键以及它们在日常运维中的协同工作流。

---

## 快速索引

按使用场景直接跳转：

| 我想… | 去看 |
| :--- | :--- |
| 查 Fish 插件快捷键 | [1.5 Fish 插件与生态工具](#15-fish-插件与生态工具) |
| 查 Yazi 文件排序 | [2.2 文件排序方式](#22-文件排序方式) |
| 查 Yazi 全部快捷键 | [2.1 快捷键指南](#21-快捷键指南) |
| 查 Micro 插件快捷键 | [3.4 Micro 插件](#34-micro-插件) |
| 查 Micro 基础快捷键 | [3.1 常用快捷键](#31-常用快捷键) |
| 查三工具协同操作 | [第 4 章 协同工作流](#4-工具链协同工作流与场景示例) |

**插件快捷键速查（脚本默认配置）**

| 工具 | 快捷键 | 功能 |
| :--- | :--- | :--- |
| Fish / fzf.fish | `Ctrl+Alt+F` | 模糊搜索文件/目录 |
| Fish / fzf.fish | `Ctrl+R` | 模糊搜索命令历史 |
| Fish / fzf.fish | `Ctrl+Alt+S` | 模糊搜索 Git 状态 |
| Fish / zoxide | `z <关键字>` | 智能目录跳转 |
| Yazi | `,a` | 临时切换为字母升序 |
| Yazi | `,A` | 临时切换为字母降序 |
| Micro / MicroOmni | `Alt+P` | 模糊搜索并打开文件 |
| Micro / MicroOmni | `Alt+F` | 全局文本搜索 |
| Micro / MicroOmni | `Alt+J` | 屏幕内字词跳转 |
| Micro / snippets | `Alt+S` | 插入代码片段 |

---

## 1. Fish Shell 命令行交互

Fish 是一个开箱即用的交互式 Shell，具备智能自动建议、语法高亮和简易的脚本语法。

### 1.1 自动建议

Fish 会根据历史输入记录，在键入命令时以灰色虚影实时显示建议：

| 操作 | 快捷键 |
| :--- | :--- |
| 接受完整建议 | `→` 或 `End` |
| 逐词接受建议 | `Alt+→` 或 `Ctrl+F` |
| 不接受建议 | 继续输入，或按退格修改 |

### 1.2 历史记录检索

| 方式 | 快捷键 | 说明 |
| :--- | :--- | :--- |
| 前缀匹配 | `↑` / `↓` | 先输入命令前缀（如 `systemctl`），再按方向键，仅匹配同前缀历史 |
| 模糊搜索 | `Ctrl+R` | 由 **fzf.fish** 提供，打开交互式历史搜索（见 [1.5](#15-fish-插件与生态工具)） |

### 1.3 目录快捷跳转与缩写

**快速向上返回（abbrs.fish 预配置）**

| 输入 | 等价命令 |
| :--- | :--- |
| `..` + 空格 | `cd ..` |
| `...` + 空格 | `cd ../..` |
| `....` + 空格 | `cd ../../..` |

**运维缩写（输入缩写后按空格自动展开）**

| 缩写 | 展开为 |
| :--- | :--- |
| `l` | `ls -lah` |
| `gs` | `git status` |
| `gd` | `git diff` |
| `gaa` | `git add .` |
| `gc` | `git commit -m` |
| `gp` | `git push` |
| `debopti` | `/usr/local/bin/debopti` |

自定义缩写：`abbr -a 缩写 完整命令`（例如 `abbr -a g git`）。

### 1.4 路径同步包装器 `y`

直接运行 `yazi` 退出后，Shell 会保持在启动前的目录。脚本在 `~/.config/fish/conf.d/yazi.fish` 注册了包装器函数 `y`，退出后自动 `cd` 到 Yazi 最后浏览的目录。

```fish
function y
    set tmp (mktemp -t "yazi-cwd.XXXXXX")
    yazi $argv --cwd-file="$tmp"
    if set cwd (cat $tmp); and [ -n "$cwd" ]; and [ "$cwd" != "$PWD" ]
        cd "$cwd"
    end
    rm -f $tmp
end
```

**用法**：终端输入 `y` 代替 `yazi` 启动文件管理器。

### 1.5 Fish 插件与生态工具

脚本通过 Fisher 自动安装以下插件，并配置 zoxide、Starship 与缩写文件。所有用户的 Fish 配置通过 SOT 软链接共享，路径为 `~/.config/fish/`（实际指向 `/etc/fish/shared_sot`）。

#### 1.5.1 Fisher 插件一览

| 插件 | 功能摘要 | 是否需要快捷键 |
| :--- | :--- | :--- |
| [fzf.fish](https://github.com/PatrickF1/fzf.fish) | 模糊搜索目录、历史、Git、进程、环境变量 | 是 |
| [autopair.fish](https://github.com/jorgebucaran/autopair.fish) | 自动配对括号与引号 | 否（输入时自动生效） |
| [puffer-fish](https://github.com/nickeb96/puffer-fish) | 命令行文本展开 | 否（输入特定符号时自动展开） |
| [replay.fish](https://github.com/jorgebucaran/replay.fish) | 在 Fish 中执行 Bash 命令并继承环境变更 | 否（命令行调用） |

插件管理：`fisher list` 查看已安装；`fisher update` 更新全部。

#### 1.5.2 fzf.fish — 模糊搜索

在命令行中按快捷键弹出 fzf 窗口，支持 Tab 多选；若光标位于某个词上，该词会自动作为初始搜索词。

| 快捷键 | 搜索对象 | 选中后 |
| :--- | :--- | :--- |
| `Ctrl+Alt+F` | 当前目录下的文件/目录（递归，遵循 gitignore） | 将路径插入命令行；目录带尾部 `/`，单选目录后可直接回车 `cd` |
| `Ctrl+R` | 命令历史 | 将整条历史命令插入命令行 |
| `Ctrl+Alt+L` | Git 提交日志 | 插入 commit hash 等信息 |
| `Ctrl+Alt+S` | Git 工作区状态（已修改/新增文件） | 插入文件路径 |
| `Ctrl+Alt+P` | 系统进程列表 | 插入 PID 等信息 |
| `Ctrl+V` | 环境变量名与值 | 插入变量相关内容 |

**自定义快捷键**：在 `~/.config/fish/conf.d/` 新建配置文件，写入：

```fish
fzf_configure_bindings --directory=ctrl-f --history=
```

上例将目录搜索改为 `Ctrl+F`，并禁用历史搜索（改由 Fish 原生 `Ctrl+R` 处理）。完整选项运行 `fzf_configure_bindings --help` 查看。

#### 1.5.3 autopair.fish — 自动配对

输入左符号时自动补全右符号，无需记忆快捷键：

| 输入 | 效果 |
| :--- | :--- |
| `(` `[` `{` `"` `'` | 自动插入配对右符号，光标停在中间 |
| `Backspace`（光标在配对中间） | 成对删除 |
| 在已闭合的右符号上再输入同符号 | 跳过右符号（不重复插入） |

#### 1.5.4 puffer-fish — 文本展开

在命令行输入以下符号后按空格（或继续输入），自动展开：

| 输入 | 展开为 | 典型场景 |
| :--- | :--- | :--- |
| `..` 后再输入 `.` | `../` → `../../` → `../../../` … | `cd ....` 快速进入上级目录 |
| `!!` | 上一条完整命令 | `sudo !!` 提权重跑 |
| `!$` | 上一条命令的最后一个参数 | `pacman -Ss pkg` 后 `paru -Ss !$` |
| `!*` | 上一条命令的全部参数 | `mkdir a b c` 后 `chmod 700 !*` |

#### 1.5.5 replay.fish — Bash 命令回放

在 Fish 会话中执行 Bash 命令，并将环境变更（导出变量、别名、`cd` 等）同步回 Fish，无需 `exec bash` 退出当前会话。

| 命令示例 | 效果 |
| :--- | :--- |
| `replay export PYTHON=python2` | 在 Fish 中设置环境变量 |
| `replay cd ~` | 在 Fish 中切换目录 |
| `replay alias g=git` | 在 Fish 中注册别名 |
| `replay "source ~/.nvm/nvm.sh && nvm use latest"` | 执行多行 Bash 脚本并继承变更 |

不支持交互式工具（如 `ssh-add`）。

#### 1.5.6 zoxide — 智能目录跳转

脚本通过 `~/.config/fish/conf.d/zoxide.fish` 初始化，记录访问过的目录并模糊匹配。

| 命令 | 说明 |
| :--- | :--- |
| `z <关键字>` | 跳转到最匹配的曾访问目录 |
| `zi` | 交互式选择目录（调用 fzf） |

#### 1.5.7 Starship — 提示符

脚本通过 `~/.config/fish/conf.d/starship.fish` 加载，在提示符中显示路径、Git 分支与状态等信息。无需额外操作，登录 Fish 后自动生效。

---

## 2. Yazi 终端文件管理器

Yazi 是基于 Rust 开发的非阻塞异步文件管理器。界面分为左（父目录）、中（当前目录）、右（文件或目录预览）三栏。

### 2.1 快捷键指南

#### 2.1.1 目录导航与浏览

| 快捷键 | 对应命令 | 说明 |
| :--- | :--- | :--- |
| `↑` / `↓` | `arrow -1` / `arrow 1` | 光标逐行移动 |
| `PageUp` / `PageDown` | `arrow -100%` / `arrow 100%` | 向上/向下滚动一整屏 |
| `Home` / `End` | `arrow top` / `arrow bot` | 跳转到当前目录的第一个/最后一个项目 |
| `Enter` 或 `→` | `open` / `enter` | 进入文件夹，或以关联程序打开文件 |
| `Backspace` / `Alt+Up` / `←` | `leave` | 返回上一级父目录 |
| `Alt+←` / `Alt+→` | `back` / `forward` | 回退/前进历史目录 |
| `Ctrl+Q` | `quit` | 退出 Yazi |

#### 2.1.2 项目选择与批量操作

| 快捷键 | 对应命令 | 说明 |
| :--- | :--- | :--- |
| `Space` 或 `Ctrl+Space` | `toggle` | 选中/取消当前文件，光标自动下移 |
| `v` | `visual_mode` | 连续选择模式：按 `v` 开启，方向键连续选中，再按 `v` 退出 |
| `Ctrl+A` | `select_all --state=true` | 全选当前目录下所有文件 |
| `Esc` | `escape --select` | 清空所有已选状态 |

#### 2.1.3 文件基本管理

| 快捷键 | 对应命令 | 说明 |
| :--- | :--- | :--- |
| `Ctrl+C` | `yank` | 复制选中项目，支持跨目录粘贴 |
| `Ctrl+X` | `yank --cut` | 剪切选中项目 |
| `Ctrl+V` | `paste` | 粘贴 |
| `Delete` | `remove` | 移入系统回收站 |
| `Shift+Delete` | `remove --permanently` | 永久删除 |
| `Ctrl+N` | `create` | 新建；名称以 `/` 结尾则创建文件夹 |
| `F2` | `rename` | 重命名，光标定位在后缀名之前 |
| `Ctrl+F` | `filter --smart` | 当前目录实时过滤（非全局搜索） |

### 2.2 文件排序方式

#### 脚本默认配置

配置文件：`~/.config/yazi/yazi.toml`，`[mgr]` 段：

```toml
sort_by = "alphabetical"   # 按文件名字母排序
sort_reverse = false       # 升序（A → Z）
sort_dir_first = true      # 文件夹始终排在文件之前
```

效果：当前目录中**文件夹置顶**，文件夹之间和文件之间均按**字母升序**排列。

#### 配置项说明

| 配置项 | 可选值 | 含义 |
| :--- | :--- | :--- |
| `sort_by` | `alphabetical` / `natural` / `mtime` / `btime` / `extension` / `size` / `random` / `none` | 排序依据 |
| `sort_reverse` | `true` / `false` | `true` 为降序，`false` 为升序 |
| `sort_dir_first` | `true` / `false` | `true` 时文件夹排在文件前面 |
| `sort_sensitive` | `true` / `false` | 是否区分大小写 |

`alphabetical` 与 `natural` 的区别：`1.md` < `10.md` < `2.md`（字母序）vs `1.md` < `2.md` < `10.md`（自然序）。

#### 永久修改排序

1. 编辑 `~/.config/yazi/yazi.toml` 中 `[mgr]` 段的 `sort_by`、`sort_reverse`、`sort_dir_first`。
2. 保存后退出 Yazi 并重新启动（或重新执行 DevOps 模块的 Yazi 安装以由脚本重新渲染模板）。

示例——改为按修改时间降序、文件夹仍置顶：

```toml
sort_by = "mtime"
sort_reverse = true
sort_dir_first = true
```

#### 运行时临时切换

Yazi 内置排序快捷键，本项目 `keymap.toml` 未覆盖，直接可用。操作方式：先按 `,`（逗号），松开后按排序键（两步输入，非组合键）。

| 按键 | 排序方式 | 顺序 |
| :--- | :--- | :--- |
| `,a` | 字母 | 升序 |
| `,A` | 字母 | 降序 |
| `,n` | 自然数 | 升序 |
| `,N` | 自然数 | 降序 |
| `,m` | 修改时间 | 升序 |
| `,M` | 修改时间 | 降序 |
| `,b` | 创建时间 | 升序 |
| `,B` | 创建时间 | 降序 |
| `,e` | 扩展名 | 升序 |
| `,E` | 扩展名 | 降序 |
| `,s` | 文件大小 | 升序 |
| `,S` | 文件大小 | 降序 |
| `,r` | 随机 | — |

临时切换仅影响当前 Yazi 会话；退出后不保留。要设为默认，请修改 `yazi.toml`。

### 2.3 全局搜索模式

| 快捷键 | 工具 | 说明 |
| :--- | :--- | :--- |
| `Ctrl+P` | `fd` | 递归模糊搜索文件名；回车进入目标目录，`Esc` 退出 |
| `Ctrl+Shift+F` | `ripgrep` | 递归搜索文件内容；结果列表显示文件、行号、匹配行；回车用 Micro 打开并定位 |

### 2.4 打开方式关联与选择

Yazi 在 `~/.config/yazi/yazi.toml` 中通过 MIME 规则关联打开程序。

| 文件类型 | 默认程序 | 行为 |
| :--- | :--- | :--- |
| 文本、`json`、`javascript`、无后缀文件 | `micro` | `block = true`，编辑器前台独占，退出后返回 Yazi |

手动选择打开方式：选中文件后按 `Shift+O`，在弹出菜单中选择备用程序（如 `vi`、`nano`）。

---

## 3. Micro 终端文本编辑器

Micro 原生支持鼠标操作，快捷键行为与现代图形编辑器一致。插件配置位于 `~/.config/micro/`（SOT 软链接共享）。

### 3.1 常用快捷键

| 分类 | 快捷键 | 功能 |
| :--- | :--- | :--- |
| 文件 | `Ctrl+S` | 保存 |
| 文件 | `Ctrl+Q` | 退出 |
| 编辑 | `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | 复制 / 剪切 / 粘贴 |
| 编辑 | `Ctrl+Z` / `Ctrl+Y` | 撤销 / 重做 |
| 编辑 | `Ctrl+A` | 全选 |
| 搜索 | `Ctrl+F` | 查找；`Enter` 或 `Ctrl+N` 下一个，`Ctrl+P` 上一个 |
| 搜索 | `Ctrl+R` | 替换 |
| 导航 | `Ctrl+G` | 跳转到指定行 |
| 命令 | `Ctrl+E` | 打开命令栏（如 `set tabsize 4`、`goto 150`） |

### 3.2 多标签与分屏管理

**多标签**

| 快捷键 | 功能 |
| :--- | :--- |
| `Ctrl+T` | 新建标签页 |
| `Ctrl+W` | 关闭当前标签页 |
| `Alt+,` / `Alt+.` | 向前/向后切换标签页 |
| `Alt+Left` / `Alt+Right` | 向前/向后切换标签页（脚本自定义绑定） |

**分屏**

| 快捷键 | 功能 |
| :--- | :--- |
| `Alt+V` | 垂直分屏（左右，脚本自定义绑定） |
| `Alt+H` | 水平分屏（上下，脚本自定义绑定） |
| `Ctrl+E` → `split vertical` | 垂直分屏（命令栏方式） |
| `Ctrl+E` → `split horizontal` | 水平分屏（命令栏方式） |
| `Ctrl+W` | 切换分屏焦点 |

### 3.3 鼠标交互说明

| 操作 | 说明 |
| :--- | :--- |
| 单击 | 移动光标 |
| 拖动 | 选择文本 |
| 双击 / 三击 | 选中单词 / 整行 |
| 滚轮 | 上下滚动 |

### 3.4 Micro 插件

脚本自动安装并启用以下插件，配置见 `settings.json` 与 `bindings.json`。

#### 3.4.1 第三方插件一览

| 插件 | 功能摘要 | 依赖 |
| :--- | :--- | :--- |
| **MicroOmni** | 模糊找文件、全局搜索、EasyMotion 跳转、光标历史、会话自动保存 | `fzf`、`ripgrep`、`bat` |
| **gutter_message** | 在报错行弹出 linter 详情 Tooltip | 需配合 `linter` 插件 |
| **snippets** | 代码片段展开与占位符跳转 | 无 |
| **gitStatus** | 状态栏显示 Git 分支与变更统计 | 需在 Git 仓库内 |

#### 3.4.2 MicroOmni

脚本在 `bindings.json` 中预绑定了常用功能：

| 快捷键 | 命令 | 功能 |
| :--- | :--- | :--- |
| `Alt+P` | `OmniGotoFile` | 模糊搜索项目内文件并打开（类似 VS Code `Ctrl+P`） |
| `Alt+F` | `OmniSearchGlobal` | 全局文本搜索（类似 VS Code `Ctrl+Shift+F`） |
| `Alt+J` | `OmniWordJump` | 屏幕可见区域内字词快速跳转（EasyMotion） |
| `Alt+[` | `OmniPreviousHistory` | 光标位置历史后退 |
| `Alt+]` | `OmniNextHistory` | 光标位置历史前进 |

**fzf 搜索窗口内通用操作**（适用于 `Alt+P`、`Alt+F` 弹出的窗口）：

| 按键 | 功能 |
| :--- | :--- |
| `Enter` | 选中并跳转 |
| `Alt+Enter` | 将当前过滤结果输出到新缓冲区 |
| `Alt+Q` / `Esc` | 退出搜索 |
| `PageUp` / `PageDown` | 滚动预览区 |

**命令栏调用**（`Ctrl+E` 输入后回车）：

| 命令 | 功能 |
| :--- | :--- |
| `OmniMinimap` | 开关右侧代码缩略图 |
| `OmniDiff` | 对比当前缓冲区与另一文件/标签 |
| `OmniSaveSession <名称>` | 保存当前标签与分屏布局 |
| `OmniLoadSession <名称>` | 恢复已保存的会话 |

会话每 60 秒自动保存（`MicroOmni.AutoSaveEnabled`）。

#### 3.4.3 gutter_message

配合内置 `linter` 插件使用。保存文件时 `linter` 调用 `shellcheck`（Shell）或 `yamllint`（YAML）检查语法，在左侧 gutter 显示报错标记；`gutter_message` 将详细错误信息显示为 Tooltip。

| 触发方式 | 说明 |
| :--- | :--- |
| 自动 | 光标移动到 gutter 报错标记所在行时弹出 Tooltip |
| 手动 | `Ctrl+E` 输入 `gutter_message next` / `prev` / `display` 跳转或显示 |

#### 3.4.4 snippets

输入片段触发词后使用快捷键展开代码块，并用 `Alt+W` 在占位符间跳转。

| 快捷键 | 功能 |
| :--- | :--- |
| `Alt+S` | 插入片段（无参数时取光标前的词作为片段名） |
| `Alt+W` | 跳转到下一个占位符 |
| `Alt+A` | 完成当前片段编辑 |
| `Alt+D` | 取消并移除当前片段 |

自定义片段文件路径：`~/.config/micro/plug/snippets/<文件类型>.snippets`。

#### 3.4.5 gitStatus

自动运行，无需快捷键。在底部状态栏右侧显示：

- 当前 Git 分支名
- 变更标记：`+` 新增行、`-` 删除行、`~` 修改行
- 与远程的差异：`↑` 领先、`↓` 落后

#### 3.4.6 脚本启用的内置插件

以下插件在 `settings.json` 中已启用，无需单独安装：

| 插件 | 功能 | 使用方式 |
| :--- | :--- | :--- |
| `linter` | 保存时语法检查 | 保存文件后自动运行；左侧 gutter 显示错误标记 |
| `diff` + `diffgutter` | Git 差异指示 | 编辑 Git 跟踪文件时，gutter 显示 `+`/`-` 变更线 |
| `autoclose` | 自动闭合括号/引号 | 输入时自动生效 |
| `comment` | 切换注释 | `Ctrl+E` → `comment`（或安装后默认绑定） |
| `status` | 底部状态栏 | 显示文件类型、编码、光标位置等 |

---

## 4. 工具链协同工作流与场景示例

本节介绍 Fish、Yazi、Micro 三者如何协同配合完成日常运维任务。

### 4.1 场景 A：修改配置并测试服务

**目标**：修改 `/etc/nginx/nginx.conf` 并重启服务。

1. Fish 中输入 `y` 启动 Yazi。
2. 导航至 `/etc/nginx/`。
3. 选中 `nginx.conf`，按 `Enter` → Micro 打开。
4. 修改后 `Ctrl+S` 保存，`Ctrl+Q` 退出。
5. Yazi 中 `Ctrl+Q` 退出。
6. Fish 当前目录已自动切换到 `/etc/nginx/`。
7. 执行 `nginx -t && systemctl restart nginx`。

### 4.2 场景 B：全局内容检索与精准定位修改

**目标**：在源码目录中查找 `install_yazi` 并修改。

1. Fish 中输入 `y` 启动 Yazi。
2. `Ctrl+Shift+F`，输入 `install_yazi` 回车。
3. 选中目标行（如 `scripts/apps/devops.sh:785`），按 `Enter`。
4. Micro 打开并定位到对应行。
5. 修改后 `Ctrl+S`、`Ctrl+Q` 保存退出。
6. Yazi 搜索结果界面按 `Esc` 退出搜索。

### 4.3 场景 C：批量文件操作与 Shell 命令传递

Yazi 支持将选中文件列表传给 Shell 命令：

| 占位符 | 含义 |
| :--- | :--- |
| `%s` | 所有已选中文件的绝对路径 |
| `%d` | 当前工作目录的绝对路径 |

**前台阻塞（按 `:`）**：勾选文件 → `:` → 输入 `tar -czvf logs.tar.gz %s` → 回车执行，完成后按键返回 Yazi。

**后台非阻塞（按 `;`）**：选中项目 → `;` → 输入 `cp -r %s /backup/%d/` → 回车，后台复制，不阻塞界面。

### 4.4 场景 D：配置共享与多用户继承

所有系统用户（含 `root`）通过 SOT 软链接共享同一套配置：

| 工具 | 配置路径 |
| :--- | :--- |
| Yazi | `~/.config/yazi/yazi.toml`、`keymap.toml` |
| Micro | `~/.config/micro/settings.json`、`bindings.json` |
| Fish | `~/.config/fish/conf.d/yazi.fish` 等（整体链接至 `/etc/fish/shared_sot`） |

任意用户终端输入 `y` 即可使用一致的快捷键与插件配置。
