# 开发测试脚本

完整规范见 `instructions/VIBEINSTRUCTION.md` §2.4（脱敏推送）、§3.5（测试目录）。

所有测试脚本统一放在 `tests/` 下，按模块分子目录管理；禁止在仓库其他路径散落测试文件。

## Podman 模块容器测试

完整规范见 `instructions/VIBEINSTRUCTION.md` §3.5。

### 快速开始

```bash
# 首次：构建基础镜像（各 Debian 版本仅需一次，约 20-30 秒）
bash tests/podman-test-base/build-base.sh

# 日常开发：快速冒烟测试（默认 bookworm，约 1-2 分钟）
bash tests/podman-module-test.sh

# 完整测试：含幂等重装与卸载（约 3-5 分钟/版本）
PODMAN_TEST_MODE=full bash tests/podman-module-test.sh

# 多版本并行
PODMAN_TEST_MODE=fast PODMAN_TEST_PARALLEL=1 DEBIAN_TEST_TAGS="bookworm bullseye" bash tests/podman-module-test.sh
```

## 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PODMAN_TEST_MODE` | `fast` | `fast` 仅安装+验证；`full` 额外测重装/卸载 |
| `DEBIAN_TEST_TAGS` | `bookworm`（fast）/ `bookworm bullseye`（full） | 测试的 Debian 版本 |
| `PODMAN_TEST_PARALLEL` | `0` | `1` 时多版本并行 |
| `PODMAN_TEST_SKIP_BUILD` | `0` | `1` 跳过基础镜像构建 |
| `PODMAN_TEST_FORCE_REBUILD` | `0` | `1` 强制重建基础镜像 |
| `PODMAN_TEST_KEEP` | `0` | `1` 测试后保留容器 |

## 加速原理

1. **预构建基础镜像** `localhost/debopti-podman-test-base:<tag>`：预装 systemd/sudo/acl，跳过每次重复 apt
2. **APT 包缓存** `.cache/apt/<tag>/archives/`：二次运行复用已下载 deb
3. **fast 模式**：跳过幂等重装与卸载流程
4. **并行执行**：多版本同时跑独立容器

## 说明

嵌套 Podman 容器内 rootless 运行时可能受 `newuidmap` 限制，测试会自动降级验证安装产物与 root 级 podman，不影响真实 Debian 主机上的功能。

## Yazi 交互验证（DevOps 模块）

位于 `tests/yazi/`，用于在 tmux 会话中验证 Yazi 配置、快捷键与 Micro 编辑器联动。依赖本机已安装 `yazi`、`tmux`、`python3` 及 Pillow。

```bash
# 示例：验证 Yazi 配置与编辑器联动
bash tests/yazi/test_yazi_config.sh

# 调试 Yazi 启动日志
bash tests/yazi/check_yazi_log.sh
```

修改 `templates/apps/devops/yazi*.toml` 或相关 keymap 后，应运行对应 yazi 测试脚本确认无回归。
