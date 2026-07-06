# FreshIP IP 养护

FreshIP 通过定时向 Google 及目标地区网站发起模拟浏览流量，并配合三核区域探针，尝试改善 Google 对该 IP 的产品区域判定。效果受 IP 段类型与历史状态影响，无 guaranteed 结果。

**适用系统**：Debian 10 ~ 13（及后续版本）

---

## 1. 通过 debopti 安装（推荐）

```bash
debopti
```

主菜单 → **FreshIP IP 养护** → 按提示：

1. 选择国家/地区（从 IP-Sentinel 最新 `map.json` 同步）
2. 选择城市（多城市国家需第二步）
3. 选择 IPv4 / IPv6 / 双栈模式

安装完成后自动启用：

- `freship-core@v4.timer` / `@v6.timer`：每 20 分钟调度
- `freship-updater.timer`：每日热数据同步

---

## 2. 运行模式

| 模式 | 条件 | 行为 |
|------|------|------|
| **maintain（仅自检）** | 三核探针判定国家达标且非送中 | 仅执行轻量探针，不发起 Google/Trust 模拟流量；**每日最多自检一次**（同日后续 timer 触发静默跳过） |
| **simulate（模拟养护）** | 送中、漂移或探针失败 | 执行 Google（70%）或 Trust（30%）多步会话 |

每次 timer 触发顺序：**探针 → 判定 → simulate 或 maintain**。

状态文件（按栈实例）：

- `/etc/freship/state/v4.state`
- `/etc/freship/state/v6.state`

字段：`RUN_MODE`、`LAST_SCORE`（ok/drift/cn/fail）、`LAST_SCORE_MSG`、`LAST_PROBE_UTC`。

---

## 3. 日志查看

```bash
tail -f /opt/freship/logs/freship.log
# 或格式化查看 journal（与文件日志同一行格式）
journalctl -t freship --no-hostname -n 100 -o short-iso
```

文件日志与 TUI「查阅运行日志」统一行格式（时间 + `[FreshIP]` 仅出现一次）：

```
2026-07-04 12:08:00 [FreshIP] 🚀 | v6 | US | 启动养护任务 (活跃度: 37%)
2026-07-04 12:08:00 [FreshIP] 🔍 | v6 | US | 200 | curl_chrome116 | 关键字: example
2026-07-04 12:08:00 [FreshIP] 📰 | v6 | US | 200 | curl_chrome116 | https://news.google.com/home?hl=en&gl=US...
2026-07-04 12:08:10 [FreshIP] ✅ | v6 | US | 养护流程执行完毕
2026-07-04 14:04:25 [FreshIP] 🌙 | v4 | US | 处于目标地区深夜 (02:00)，进入休眠模式
```

journal 消息体仅存 `🚀 | v6 | US | …`，避免与 systemd 时间戳及 `[FreshIP]` 重复。

关注 `OK | 区域自检通过` / `CN | 送中` 等探针结论行判断区域是否达标。

---

## 4. 手动同步热数据

TUI：**FreshIP 养护管理** → **手动同步热数据**

或命令行：

```bash
sudo -u freship bash /opt/freship/core/freship_updater.sh
```

同步内容：

| 资产 | 频率 |
|------|------|
| `data/keywords/kw_XX.txt` | 每日 |
| `data/regions/<REGION_PATH>.json` | 每日 |
| `data/map.json` | 每日 |
| `data/user_agents.txt` | 30 天 |

数据源：[IP-Sentinel](https://github.com/hotyue/IP-Sentinel) `main` 分支。

---

## 5. 配置文件

路径：`/etc/freship/freship.conf`

主要字段：

```bash
REGION_CODE=JP          # 目标国家
REGION_PATH=JP/Default/Tokyo
TARGET_CC=JP            # 探针匹配用（UK 自动映射 GB）
KW_FILE=kw_JP.txt
WORK_MODE=dual_stack    # ipv4_only | ipv6_only | dual_stack
BIND_IPV4=...
BIND_IPV6=...
```

---

## 6. 手动启停 timer

```bash
# 启动
systemctl enable --now freship-core@v4.timer freship-updater.timer
# 双栈时 additionally:
systemctl enable --now freship-core@v6.timer

# 停止
systemctl disable --now freship-core@v4.timer freship-core@v6.timer
```

---

## 7. 卸载

TUI → **卸载模块**，或：

```bash
systemctl disable --now freship-core@v4.timer freship-core@v6.timer freship-updater.timer
rm -rf /opt/freship /etc/freship
userdel freship 2>/dev/null || true
```

---

## 8. 核心模块（`/opt/freship/core/`）

| 脚本 | 作用 |
|------|------|
| `freship_runner.sh` | 调度入口、状态机 |
| `freship_probe.sh` | 三核区域探针 |
| `freship_mod_google.sh` | Google 多步会话（Cookie、坐标抖动、Referer） |
| `freship_mod_trust.sh` | 本土白名单访问 |
| `freship_updater.sh` | 热数据 OTA |
| `freship_lib.sh` | 公共函数 |

模拟浏览使用 **curl-impersonate**（若已安装）；探针使用标准 curl + 干净 Chrome UA。

---

## 9. 与自动化脚本对齐

安装、模板、systemd 单元与 `scripts/apps/freship.sh`、`templates/apps/freship/` 一致。修改配置请优先通过 debopti TUI **修改模块设置**（热重载并重置状态）。
