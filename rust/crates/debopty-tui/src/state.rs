use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use debopty_bridge::{AppId, BashAction};
use debopty_core::apps::devops::lego::{LegoDomainSpec, LegoDomainStatus, LegoFlag};
use debopty_core::apps::freship::{FreshipMap, FreshipSelection, FreshipWorkMode};
use debopty_core::status::StatusSnapshot;
use debopty_core::telemetry::{
    SystemSpecs, TelemetryCollector, TelemetryHistory, probe_system_specs,
};

use crate::menu::{
    MenuAction, MenuId, MenuItem, MenuScreen, PaletteEntry, WorkspaceTab, build_menu,
    build_palette_entries, build_workspace_menu,
};
use crate::motion::{
    MotionPolicy, MotionTransition, TOAST_FADE_DURATION, TOAST_REVEAL_DURATION, TOAST_TTL_DURATION,
    TransitionKind, WakeKind, WakeSchedule, ease_out_quart,
};
use crate::renderer::MenuEvent;

/// Legacy compatibility constant: default toast lifetime in ~30fps ticks (~3.5s).
pub const DEFAULT_TOAST_TTL_TICKS: u16 = 105;
/// Legacy compatibility constant: canonical transition ticks.
pub const TRANSITION_TOTAL_TICKS: u8 = 8;
/// Maximum retained log lines in `TaskRunState` ring buffer (U39).
pub const TASK_LOG_MAX_LINES: usize = 2000;
/// Lookahead scroll-off margin (rows kept visible above/below selection when scrolling lists).
pub const SCROLL_MARGIN: usize = 2;

/// Computes the next top-of-viewport row offset while preserving up to `margin` lookahead rows
/// above and below `selected` without thrashing on narrow/short viewports.
pub fn compute_scroll_offset(
    selected: usize,
    len: usize,
    visible_rows: usize,
    current_offset: usize,
    margin: usize,
) -> usize {
    if visible_rows == 0 || len <= visible_rows {
        return 0;
    }
    let max_offset = len - visible_rows;
    let selected = selected.min(len - 1);
    let eff_margin = margin.min(visible_rows.saturating_sub(1) / 2);
    let mut offset = current_offset.min(max_offset);

    if selected < offset.saturating_add(eff_margin) {
        offset = selected.saturating_sub(eff_margin);
    } else if selected.saturating_add(eff_margin) >= offset.saturating_add(visible_rows) {
        offset = selected
            .saturating_add(eff_margin + 1)
            .saturating_sub(visible_rows);
    }
    offset.min(max_offset)
}

const BRAILLE_SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Sanitizes a single live execution output line before storing or rendering in the TUI (U39).
///
/// Strips ANSI CSI (`\x1b[...`), OSC (`\x1b]...BEL/ST`), DCS/APC/PM/SOS, and 2-byte ESC
/// control sequences, removes `\r` and C0/C1 control characters (such as `\x07`), and
/// safely expands `\t` to 4 spaces so child commands can never corrupt the alternate screen.
pub fn sanitize_live_log_line(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    // CSI: ESC [ <param bytes 0x30..=0x3F>* <intermediate 0x20..=0x2F>* <final 0x40..=0x7E>
                    chars.next();
                    for next_ch in chars.by_ref() {
                        if ('@'..='~').contains(&next_ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: ESC ] ... (BEL | ESC \)
                    chars.next();
                    while let Some(next_ch) = chars.next() {
                        if next_ch == '\x07' {
                            break;
                        }
                        if next_ch == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('P' | 'X' | '^' | '_') => {
                    // DCS / SOS / PM / APC: terminated by ST (ESC \)
                    chars.next();
                    while let Some(next_ch) = chars.next() {
                        if next_ch == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
                    // Character set designation: ESC ( <final>
                    chars.next();
                    chars.next();
                }
                Some(_) => {
                    // Other 2-byte ESC sequence (e.g., ESC c, ESC 7, ESC 8, ESC M)
                    chars.next();
                }
                None => {}
            }
        } else if ch == '\t' {
            out.push_str("    ");
        } else if ch == '\r' || ch == '\n' || ch.is_control() {
            continue;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Lifecycle state of an in-TUI live task execution (U39).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRunStatus {
    Running,
    Succeeded,
    Failed,
}

/// Live in-TUI task execution log buffer, spinner clock, and scrollback state (U39).
#[derive(Debug, Clone)]
pub struct TaskRunState {
    pub label: String,
    pub status: TaskRunStatus,
    pub started_at: Instant,
    pub finished_at: Option<Instant>,
    pub summary: Option<String>,
    pub lines: VecDeque<String>,
    pub scroll_offset: usize,
    pub auto_scroll: bool,
    pub preview_elapsed: Option<Duration>,
}

impl TaskRunState {
    pub fn new_running(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: TaskRunStatus::Running,
            started_at: Instant::now(),
            finished_at: None,
            summary: None,
            lines: VecDeque::with_capacity(64),
            scroll_offset: 0,
            auto_scroll: true,
            preview_elapsed: None,
        }
    }

    /// Creates a deterministic snapshot for `ui-preview --screen task-run` and tests.
    pub fn preview(
        label: impl Into<String>,
        status: TaskRunStatus,
        elapsed: Duration,
        summary: Option<String>,
        lines: impl IntoIterator<Item = String>,
    ) -> Self {
        let now = Instant::now();
        let mut ring = VecDeque::with_capacity(TASK_LOG_MAX_LINES.min(128));
        for raw in lines {
            if ring.len() >= TASK_LOG_MAX_LINES {
                ring.pop_front();
            }
            ring.push_back(sanitize_live_log_line(&raw));
        }
        let scroll_offset = ring.len().saturating_sub(1);
        Self {
            label: label.into(),
            status,
            started_at: now,
            finished_at: (status != TaskRunStatus::Running).then_some(now),
            summary,
            lines: ring,
            scroll_offset,
            auto_scroll: true,
            preview_elapsed: Some(elapsed),
        }
    }

    pub fn is_running(&self) -> bool {
        self.status == TaskRunStatus::Running
    }

    pub fn is_preview(&self) -> bool {
        self.preview_elapsed.is_some()
    }

    pub fn elapsed_at(&self, now: Instant) -> Duration {
        if let Some(d) = self.preview_elapsed {
            return d;
        }
        self.finished_at
            .unwrap_or(now)
            .saturating_duration_since(self.started_at)
    }

    pub fn spinner_char_at(&self, now: Instant) -> &'static str {
        match self.status {
            TaskRunStatus::Running => {
                let idx = ((self.elapsed_at(now).as_millis() / 80) as usize)
                    % BRAILLE_SPINNER_FRAMES.len();
                BRAILLE_SPINNER_FRAMES[idx]
            }
            TaskRunStatus::Succeeded => "✓",
            TaskRunStatus::Failed => "✖",
        }
    }

    pub fn push_line(&mut self, raw: &str) {
        let clean = sanitize_live_log_line(raw);
        if self.lines.len() >= TASK_LOG_MAX_LINES {
            self.lines.pop_front();
            if !self.auto_scroll {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
        }
        self.lines.push_back(clean);
        if self.auto_scroll {
            self.scroll_offset = self.lines.len().saturating_sub(1);
        }
    }

    pub fn finish(&mut self, succeeded: bool, summary: impl Into<String>) {
        self.status = if succeeded {
            TaskRunStatus::Succeeded
        } else {
            TaskRunStatus::Failed
        };
        self.finished_at = Some(Instant::now());
        self.summary = Some(summary.into());
        if self.auto_scroll {
            self.scroll_offset = self.lines.len().saturating_sub(1);
        }
    }

    pub fn scroll_by(&mut self, delta: i16, viewport_h: usize) {
        if self.lines.is_empty() || delta == 0 {
            return;
        }
        let max_top = if viewport_h > 0 {
            self.lines.len().saturating_sub(viewport_h)
        } else {
            self.lines.len().saturating_sub(1)
        };
        if delta < 0 {
            let base = if self.auto_scroll {
                max_top
            } else {
                self.scroll_offset.min(max_top)
            };
            self.scroll_offset = base.saturating_sub(delta.unsigned_abs() as usize);
            self.auto_scroll = false;
        } else {
            let next = self.scroll_offset.saturating_add(delta as usize);
            if next >= max_top {
                self.scroll_offset = max_top;
                self.auto_scroll = true;
            } else {
                self.scroll_offset = next;
                self.auto_scroll = false;
            }
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = false;
    }

    pub fn scroll_to_bottom(&mut self, viewport_h: usize) {
        let max_top = if viewport_h > 0 {
            self.lines.len().saturating_sub(viewport_h)
        } else {
            self.lines.len().saturating_sub(1)
        };
        self.scroll_offset = max_top;
        self.auto_scroll = true;
    }
}

/// Boolean toggle item inside an in-TUI option modal (U41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleOption {
    pub key: &'static str,
    pub label: String,
    pub description: String,
    pub checked: bool,
}

/// Semantic kind of option / confirmation-with-toggles modal (U41).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionPromptKind {
    YaziInstall,
    PodmanUninstall,
    ScriptUninstall,
    ReconcileApply { drift_count: usize },
}

/// In-TUI option modal state (`cursor`: `0..options.len()` toggles, `options.len()` confirm, `options.len()+1` cancel).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionPromptState {
    pub kind: OptionPromptKind,
    pub title: String,
    pub message: String,
    pub destructive: bool,
    pub options: Vec<ToggleOption>,
    pub cursor: usize,
}

impl OptionPromptState {
    pub fn yazi_install() -> Self {
        Self {
            kind: OptionPromptKind::YaziInstall,
            title: "Yazi 安装选项".to_string(),
            message: "请选择是否一并安装多媒体与文档预览依赖：".to_string(),
            destructive: false,
            options: vec![ToggleOption {
                key: "media_preview",
                label: "安装多媒体预览组件".to_string(),
                description: "ffmpeg / 7z / jq / poppler / fd / rg / fzf".to_string(),
                checked: false,
            }],
            cursor: 0,
        }
    }

    pub fn podman_uninstall() -> Self {
        Self {
            kind: OptionPromptKind::PodmanUninstall,
            title: "卸载 Podman 环境".to_string(),
            message: "将停止并移除 Podman 服务，请确认可选清理项：".to_string(),
            destructive: true,
            options: vec![
                ToggleOption {
                    key: "delete_data",
                    label: "同时删除容器数据目录".to_string(),
                    description: "/home/apps/srv 业务数据（不可恢复）".to_string(),
                    checked: false,
                },
                ToggleOption {
                    key: "delete_user",
                    label: "同时删除 apps 系统用户".to_string(),
                    description: "移除无根容器运行账户 apps".to_string(),
                    checked: false,
                },
            ],
            cursor: 3, // defaults to Cancel button
        }
    }

    pub fn script_uninstall() -> Self {
        Self {
            kind: OptionPromptKind::ScriptUninstall,
            title: "卸载 debopty".to_string(),
            message: "将移除 /opt/debopty 程序目录与全局软链接：".to_string(),
            destructive: true,
            options: vec![ToggleOption {
                key: "delete_config",
                label: "同时删除 /etc/debopti 配置".to_string(),
                description: "默认保留配置文件以便后续重装复用".to_string(),
                checked: false,
            }],
            cursor: 2, // defaults to Cancel button
        }
    }

    pub fn reconcile_apply(drift_count: usize) -> Self {
        Self {
            kind: OptionPromptKind::ReconcileApply { drift_count },
            title: "应用稳态收敛".to_string(),
            message: format!("检测到 {drift_count} 项配置漂移，是否自动备份并收敛至稳态？"),
            destructive: false,
            options: Vec::new(),
            cursor: 1, // defaults to Cancel button
        }
    }

    pub fn confirm_idx(&self) -> usize {
        self.options.len()
    }

    pub fn cancel_idx(&self) -> usize {
        self.options.len() + 1
    }

    pub fn total_stops(&self) -> usize {
        self.options.len() + 2
    }

    pub fn toggle_checked(&self, key: &str) -> bool {
        self.options
            .iter()
            .find(|o| o.key == key)
            .is_some_and(|o| o.checked)
    }

    pub fn into_submission(&self) -> PromptSubmission {
        match self.kind {
            OptionPromptKind::YaziInstall => PromptSubmission::YaziInstall {
                media_preview: self.toggle_checked("media_preview"),
            },
            OptionPromptKind::PodmanUninstall => PromptSubmission::PodmanUninstall {
                delete_data: self.toggle_checked("delete_data"),
                delete_user: self.toggle_checked("delete_user"),
            },
            OptionPromptKind::ScriptUninstall => PromptSubmission::ScriptUninstall {
                delete_config: self.toggle_checked("delete_config"),
            },
            OptionPromptKind::ReconcileApply { .. } => PromptSubmission::ReconcileApply,
        }
    }
}

/// Single selectable row in `PickerState` (U41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub value: String,
    pub label: String,
    pub detail: String,
}

/// Semantic kind of in-TUI single-selection picker modal (U41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerKind {
    LegoToggleFlagDomain,
    LegoToggleFlagChoice { domain: String },
    LegoRunOnceDomain,
    LegoRunOnceMode { domain: String },
    LegoTroubleshootDomain,
    LegoRemoveDomain,
}

/// Generic in-TUI list picker modal state (U41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    pub kind: PickerKind,
    pub title: String,
    pub subtitle: String,
    pub items: Vec<PickerItem>,
    pub selected: usize,
}

impl PickerState {
    pub fn lego_domains(kind: PickerKind, domains: &[LegoDomainStatus]) -> Self {
        let (title, subtitle) = match &kind {
            PickerKind::LegoToggleFlagDomain => ("切换 Lego 域名开关", "步骤 1/2 · 选择目标主域名"),
            PickerKind::LegoRunOnceDomain => ("立即签发 / 续期证书", "步骤 1/2 · 选择目标主域名"),
            PickerKind::LegoTroubleshootDomain => ("生成 Lego 排障命令", "选择要排查的主域名"),
            PickerKind::LegoRemoveDomain => ("移除域名托管", "选择要移除托管的主域名（保留证书）"),
            _ => ("选择域名", ""),
        };
        let items = domains
            .iter()
            .map(|d| {
                let cert_tag = if d.cert_exists {
                    d.days_left
                        .map(|days| format!("剩余 {days}天"))
                        .unwrap_or_else(|| "已签发".to_string())
                } else {
                    "未签发".to_string()
                };
                PickerItem {
                    value: d.primary_domain.clone(),
                    label: d.primary_domain.clone(),
                    detail: format!("{} · {}", d.provider, cert_tag),
                }
            })
            .collect();
        Self {
            kind,
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            items,
            selected: 0,
        }
    }

    pub fn lego_flag_choice(domain: String) -> Self {
        Self {
            kind: PickerKind::LegoToggleFlagChoice {
                domain: domain.clone(),
            },
            title: "切换 Lego 域名开关".to_string(),
            subtitle: format!("步骤 2/2 · {domain} · 选择开关"),
            items: vec![
                PickerItem {
                    value: "auto-renew".to_string(),
                    label: "自动续期".to_string(),
                    detail: "DEBOPTI_AUTO_RENEW".to_string(),
                },
                PickerItem {
                    value: "dns-skip-propagation".to_string(),
                    label: "跳过 DNS 传播等待".to_string(),
                    detail: "DEBOPTI_DNS_SKIP_PROPAGATION".to_string(),
                },
                PickerItem {
                    value: "ferron-push".to_string(),
                    label: "自动推送至 Ferron".to_string(),
                    detail: "DEBOPTI_FERRON_PUSH".to_string(),
                },
            ],
            selected: 0,
        }
    }

    pub fn lego_run_mode(domain: String) -> Self {
        Self {
            kind: PickerKind::LegoRunOnceMode {
                domain: domain.clone(),
            },
            title: "立即签发 / 续期证书".to_string(),
            subtitle: format!("步骤 2/2 · {domain} · 选择执行模式"),
            items: vec![
                PickerItem {
                    value: "renew".to_string(),
                    label: "续期证书 (renew)".to_string(),
                    detail: "推荐 · 续期已有证书".to_string(),
                },
                PickerItem {
                    value: "issue".to_string(),
                    label: "首次签发 (issue)".to_string(),
                    detail: "新域名首次申请证书".to_string(),
                },
                PickerItem {
                    value: "auto".to_string(),
                    label: "自动判断 (auto)".to_string(),
                    detail: "无证书签发，已有则续期".to_string(),
                },
            ],
            selected: 0,
        }
    }
}

/// Active step in the 5-step FreshIP cascading region & stack selector (U41).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshipStep {
    Continent,
    Country,
    State,
    City,
    WorkMode,
}

/// 5-step cascading selector modal for FreshIP installation & reconfiguration (U41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshipPickerState {
    pub map: FreshipMap,
    pub is_reconfigure: bool,
    pub step: FreshipStep,
    pub continent_idx: usize,
    pub country_idx: usize,
    pub state_idx: usize,
    pub city_idx: usize,
    pub selected: usize,
}

impl FreshipPickerState {
    pub fn new(map: FreshipMap, is_reconfigure: bool) -> Self {
        Self {
            map,
            is_reconfigure,
            step: FreshipStep::Continent,
            continent_idx: 0,
            country_idx: 0,
            state_idx: 0,
            city_idx: 0,
            selected: 0,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.is_reconfigure {
            "重配 FreshIP 地区与网络栈"
        } else {
            "安装 FreshIP · 选择目标地区"
        }
    }

    pub fn subtitle(&self) -> String {
        match self.step {
            FreshipStep::Continent => "步骤 1/5 · 选择大洲".to_string(),
            FreshipStep::Country => {
                let c = self
                    .map
                    .continents
                    .get(self.continent_idx)
                    .map(|x| x.name.as_str())
                    .unwrap_or("-");
                format!("步骤 2/5 · 选择国家/地区 ({c})")
            }
            FreshipStep::State => {
                let co = self
                    .current_country()
                    .map(|x| x.name.as_str())
                    .unwrap_or("-");
                format!("步骤 3/5 · 选择州/省 ({co})")
            }
            FreshipStep::City => {
                let st = self
                    .current_state()
                    .map(|x| {
                        if x.name.is_empty() {
                            x.id.as_str()
                        } else {
                            x.name.as_str()
                        }
                    })
                    .unwrap_or("-");
                format!("步骤 4/5 · 选择目标城市 ({st})")
            }
            FreshipStep::WorkMode => {
                let city = self.current_city().map(|x| x.name.as_str()).unwrap_or("-");
                format!("步骤 5/5 · 选择网络栈模式 ({city})")
            }
        }
    }

    fn current_country(&self) -> Option<&debopty_core::apps::freship::FreshipCountry> {
        self.map
            .continents
            .get(self.continent_idx)?
            .countries
            .get(self.country_idx)
    }

    fn current_state(&self) -> Option<&debopty_core::apps::freship::FreshipState> {
        self.current_country()?.states.get(self.state_idx)
    }

    fn current_city(&self) -> Option<&debopty_core::apps::freship::FreshipCity> {
        self.current_state()?.cities.get(self.city_idx)
    }

    pub fn current_items(&self) -> Vec<PickerItem> {
        match self.step {
            FreshipStep::Continent => self
                .map
                .continents
                .iter()
                .map(|c| PickerItem {
                    value: c.id.clone(),
                    label: c.name.clone(),
                    detail: c.id.clone(),
                })
                .collect(),
            FreshipStep::Country => self
                .map
                .continents
                .get(self.continent_idx)
                .map(|c| {
                    c.countries
                        .iter()
                        .map(|co| PickerItem {
                            value: co.id.clone(),
                            label: co.name.clone(),
                            detail: co.id.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            FreshipStep::State => self
                .current_country()
                .map(|co| {
                    co.states
                        .iter()
                        .map(|s| PickerItem {
                            value: s.id.clone(),
                            label: if s.name.is_empty() {
                                s.id.clone()
                            } else {
                                s.name.clone()
                            },
                            detail: s.id.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            FreshipStep::City => self
                .current_state()
                .map(|s| {
                    s.cities
                        .iter()
                        .map(|ci| PickerItem {
                            value: ci.id.clone(),
                            label: ci.name.clone(),
                            detail: ci.id.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            FreshipStep::WorkMode => vec![
                PickerItem {
                    value: "auto".to_string(),
                    label: "自动探测本机公网栈".to_string(),
                    detail: "推荐".to_string(),
                },
                PickerItem {
                    value: "ipv4_only".to_string(),
                    label: "仅 IPv4".to_string(),
                    detail: "ipv4_only".to_string(),
                },
                PickerItem {
                    value: "ipv6_only".to_string(),
                    label: "仅 IPv6".to_string(),
                    detail: "ipv6_only".to_string(),
                },
                PickerItem {
                    value: "dual_stack".to_string(),
                    label: "双栈并行".to_string(),
                    detail: "dual_stack".to_string(),
                },
            ],
        }
    }

    /// Advances one step; returns `Some(FreshipSelection)` when step 5 completes.
    pub fn advance(&mut self) -> Option<FreshipSelection> {
        let len = self.current_items().len();
        if len == 0 {
            return None;
        }
        let picked = self.selected.min(len - 1);
        match self.step {
            FreshipStep::Continent => {
                self.continent_idx = picked;
                self.country_idx = 0;
                self.state_idx = 0;
                self.city_idx = 0;
                self.selected = 0;
                self.step = FreshipStep::Country;
                None
            }
            FreshipStep::Country => {
                self.country_idx = picked;
                self.state_idx = 0;
                self.city_idx = 0;
                self.selected = 0;
                let states_len = self.current_country().map_or(0, |c| c.states.len());
                if states_len <= 1 {
                    let cities_len = self.current_state().map_or(0, |s| s.cities.len());
                    if cities_len <= 1 {
                        self.step = FreshipStep::WorkMode;
                    } else {
                        self.step = FreshipStep::City;
                    }
                } else {
                    self.step = FreshipStep::State;
                }
                None
            }
            FreshipStep::State => {
                self.state_idx = picked;
                self.city_idx = 0;
                self.selected = 0;
                let cities_len = self.current_state().map_or(0, |s| s.cities.len());
                if cities_len <= 1 {
                    self.step = FreshipStep::WorkMode;
                } else {
                    self.step = FreshipStep::City;
                }
                None
            }
            FreshipStep::City => {
                self.city_idx = picked;
                self.selected = 0;
                self.step = FreshipStep::WorkMode;
                None
            }
            FreshipStep::WorkMode => {
                let country = self.current_country()?;
                let state = self.current_state()?;
                let city = self.current_city()?;
                let work_mode = match picked {
                    1 => Some(FreshipWorkMode::Ipv4Only),
                    2 => Some(FreshipWorkMode::Ipv6Only),
                    3 => Some(FreshipWorkMode::DualStack),
                    _ => None,
                };
                Some(FreshipSelection {
                    country_id: country.id.clone(),
                    state_id: state.id.clone(),
                    city_id: city.id.clone(),
                    city_name: city.name.clone(),
                    kw_filename: country.keyword_file.clone(),
                    work_mode,
                    bind_ipv4: None,
                    bind_ipv6: None,
                })
            }
        }
    }

    /// Steps backward to the previous non-skipped step; returns `false` if already at step 1 (cancel).
    pub fn step_back(&mut self) -> bool {
        match self.step {
            FreshipStep::Continent => false,
            FreshipStep::Country => {
                self.step = FreshipStep::Continent;
                self.selected = self.continent_idx;
                true
            }
            FreshipStep::State => {
                self.step = FreshipStep::Country;
                self.selected = self.country_idx;
                true
            }
            FreshipStep::City => {
                let states_len = self.current_country().map_or(0, |c| c.states.len());
                if states_len <= 1 {
                    self.step = FreshipStep::Country;
                    self.selected = self.country_idx;
                } else {
                    self.step = FreshipStep::State;
                    self.selected = self.state_idx;
                }
                true
            }
            FreshipStep::WorkMode => {
                let cities_len = self.current_state().map_or(0, |s| s.cities.len());
                if cities_len > 1 {
                    self.step = FreshipStep::City;
                    self.selected = self.city_idx;
                } else {
                    let states_len = self.current_country().map_or(0, |c| c.states.len());
                    if states_len > 1 {
                        self.step = FreshipStep::State;
                        self.selected = self.state_idx;
                    } else {
                        self.step = FreshipStep::Country;
                        self.selected = self.country_idx;
                    }
                }
                true
            }
        }
    }
}

/// Single editable or toggleable field inside `FormState` (U41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub value: String,
    pub masked: bool,
    pub required: bool,
    pub is_bool: bool,
    pub bool_value: bool,
}

/// Multi-field in-TUI form modal state (e.g. Lego Add-Domain with masked API token) (U41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormState {
    pub title: String,
    pub subtitle: String,
    pub fields: Vec<FormField>,
    pub focused_field: usize,
    pub error_msg: Option<String>,
}

impl FormState {
    pub fn lego_add_domain() -> Self {
        Self {
            title: "添加 Lego 域名托管".to_string(),
            subtitle: "支持 SAN / 多 DNS 提供商 · API Token 自动脱敏隐藏".to_string(),
            fields: vec![
                FormField {
                    key: "primary_domain",
                    label: "主域名",
                    placeholder: "example.com",
                    value: String::new(),
                    masked: false,
                    required: true,
                    is_bool: false,
                    bool_value: false,
                },
                FormField {
                    key: "san_domains",
                    label: "额外 SAN 域名",
                    placeholder: "*.example.com (逗号分隔，可选)",
                    value: String::new(),
                    masked: false,
                    required: false,
                    is_bool: false,
                    bool_value: false,
                },
                FormField {
                    key: "email",
                    label: "ACME 通知邮箱",
                    placeholder: "admin@example.com",
                    value: String::new(),
                    masked: false,
                    required: true,
                    is_bool: false,
                    bool_value: false,
                },
                FormField {
                    key: "provider",
                    label: "DNS 提供商",
                    placeholder: "cloudflare",
                    value: "cloudflare".to_string(),
                    masked: false,
                    required: true,
                    is_bool: false,
                    bool_value: false,
                },
                FormField {
                    key: "cf_token",
                    label: "DNS API Token",
                    placeholder: "输入 API Token (不回显明文)",
                    value: String::new(),
                    masked: true,
                    required: true,
                    is_bool: false,
                    bool_value: false,
                },
                FormField {
                    key: "issue_immediately",
                    label: "添加后立即签发证书",
                    placeholder: "按 Space 切换",
                    value: String::new(),
                    masked: false,
                    required: false,
                    is_bool: true,
                    bool_value: false,
                },
            ],
            focused_field: 0,
            error_msg: None,
        }
    }

    pub fn field_value(&self, key: &str) -> &str {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.as_str())
            .unwrap_or("")
    }

    pub fn field_bool(&self, key: &str) -> bool {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .is_some_and(|f| f.bool_value)
    }

    pub fn validate_and_build_lego_spec(&mut self) -> Result<LegoDomainSpec, String> {
        for (idx, field) in self.fields.iter().enumerate() {
            if field.required && !field.is_bool && field.value.trim().is_empty() {
                self.focused_field = idx;
                let msg = format!("请填写必填项：{}", field.label);
                self.error_msg = Some(msg.clone());
                return Err(msg);
            }
        }
        let primary_domain = self.field_value("primary_domain").trim().to_string();
        let san_domains: Vec<String> = self
            .field_value("san_domains")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        let email = self.field_value("email").trim().to_string();
        let provider_raw = self.field_value("provider").trim();
        let provider = if provider_raw.is_empty() {
            "cloudflare".to_string()
        } else {
            provider_raw.to_string()
        };
        let cf_token = self.field_value("cf_token").trim().to_string();
        let issue_immediately = self.field_bool("issue_immediately");
        self.error_msg = None;
        Ok(LegoDomainSpec {
            primary_domain,
            san_domains,
            email,
            cf_token,
            provider,
            issue_immediately,
        })
    }
}

/// Active in-TUI modal prompt state (U41: zero console stdin dropouts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InTuiPrompt {
    Options(OptionPromptState),
    Picker(PickerState),
    Freship(FreshipPickerState),
    Form(FormState),
}

/// Structured payload submitted from an `InTuiPrompt` modal (U41).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PromptSubmission {
    YaziInstall {
        media_preview: bool,
    },
    PodmanUninstall {
        delete_data: bool,
        delete_user: bool,
    },
    ScriptUninstall {
        delete_config: bool,
    },
    ReconcileApply,
    Freship {
        selection: FreshipSelection,
        is_reconfigure: bool,
    },
    LegoAddDomain(LegoDomainSpec),
    LegoToggleFlag {
        domain: String,
        flag: LegoFlag,
    },
    LegoRunOnce {
        domain: String,
        mode: String,
    },
    LegoTroubleshoot {
        domain: String,
    },
    LegoRemoveDomain {
        domain: String,
    },
}

/// Active focus pane in split-pane dashboards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePane {
    #[default]
    Menu,
    Telemetry,
}

/// Semantic severity for floating toast notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeKind {
    #[default]
    Info,
    Success,
    Warning,
    Danger,
}

impl NoticeKind {
    pub fn infer(msg: &str) -> Self {
        if msg.contains("失败") || msg.contains("错误") || msg.contains("无效") {
            Self::Danger
        } else if msg.contains("取消") || msg.contains("警告") || msg.contains("跳过") {
            Self::Warning
        } else if msg.contains("完成")
            || msg.contains("已开启")
            || msg.contains("已关闭")
            || msg.contains("已部署")
            || msg.contains("已刷新")
            || msg.contains("已生成")
            || msg.contains("已卸载")
            || msg.contains("稳态")
            || msg.contains("已原子升级")
        {
            Self::Success
        } else {
            Self::Info
        }
    }
}

/// External outcome emitted by `AppState` to the caller's TUI event loop.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TuiOutcome {
    Exit,
    Refresh,
    Execute(BashAction),
    RunNativeF2bSync,
    RunNativeReconcile,
    RunMenuAction(MenuAction),
    PromptSubmitted(PromptSubmission),
    ElevatedApproved(Box<TuiOutcome>),
}

/// Pending destructive-action or sudo-elevation confirmation prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPrompt {
    pub message: String,
    pub action: BashAction,
    pub menu_action: Option<MenuAction>,
    pub selected_yes: bool,
    pub elevation_outcome: Option<Box<TuiOutcome>>,
}

impl ConfirmPrompt {
    /// Returns `true` when this prompt guards a destructive/irreversible action.
    pub fn is_destructive(&self) -> bool {
        if self.elevation_outcome.is_some() {
            return false;
        }
        matches!(
            self.action,
            BashAction::UninstallApp(_) | BashAction::ScriptUninstall
        ) || matches!(self.menu_action, Some(MenuAction::LegoUninstall))
    }
}

/// Pure navigation, wall-clock motion, telemetry, and selection state machine for the debopty TUI.
#[derive(Debug, Clone)]
pub struct AppState {
    pub status: StatusSnapshot,
    menu_stack: Vec<MenuId>,
    workspace_tab: WorkspaceTab,
    selected_idx: usize,
    menu_viewport_offset: usize,
    digit_prefix: Option<u8>,
    pending_confirm: Option<ConfirmPrompt>,
    active_prompt: Option<InTuiPrompt>,
    native_prompts: bool,
    task_run: Option<TaskRunState>,
    task_log_viewport_h: usize,
    notice: Option<String>,
    notice_kind: NoticeKind,
    notice_deadline: Option<Instant>,
    toast_started_at: Option<Instant>,
    active_pane: ActivePane,
    split_pane_available: bool,
    telemetry_scroll: u16,
    filter_active: bool,
    filter_query: String,
    palette_active: bool,
    palette_query: String,
    palette_selected: usize,
    help_active: bool,
    motion_policy: MotionPolicy,
    effective_frame_interval: Duration,
    active_transition: Option<MotionTransition>,
    telemetry_specs: SystemSpecs,
    telemetry_history: TelemetryHistory,
    telemetry_collector: Option<TelemetryCollector>,
    next_telemetry_at: Option<Instant>,
    dirty: bool,
    tick_count: u64,
    exit_requested: bool,
}

impl AppState {
    pub fn new(status: StatusSnapshot) -> Self {
        let motion_policy = MotionPolicy::static_disabled();
        Self {
            status,
            menu_stack: vec![MenuId::Main],
            workspace_tab: WorkspaceTab::Overview,
            selected_idx: 0,
            menu_viewport_offset: 0,
            digit_prefix: None,
            pending_confirm: None,
            active_prompt: None,
            native_prompts: false,
            task_run: None,
            task_log_viewport_h: 0,
            notice: None,
            notice_kind: NoticeKind::Info,
            notice_deadline: None,
            toast_started_at: None,
            active_pane: ActivePane::Menu,
            split_pane_available: true,
            telemetry_scroll: 0,
            filter_active: false,
            filter_query: String::new(),
            palette_active: false,
            palette_query: String::new(),
            palette_selected: 0,
            help_active: false,
            effective_frame_interval: motion_policy.frame_interval,
            motion_policy,
            active_transition: None,
            telemetry_specs: SystemSpecs::default(),
            telemetry_history: TelemetryHistory::default(),
            telemetry_collector: None,
            next_telemetry_at: None,
            dirty: true,
            tick_count: 0,
            exit_requested: false,
        }
    }

    pub fn current_menu(&self) -> MenuId {
        self.menu_stack.last().copied().unwrap_or(MenuId::Main)
    }

    pub fn workspace_tab(&self) -> WorkspaceTab {
        self.workspace_tab
    }

    pub fn set_workspace_tab(&mut self, tab: WorkspaceTab) {
        let old_idx = WorkspaceTab::ALL
            .iter()
            .position(|&t| t == self.workspace_tab)
            .unwrap_or(0);
        let new_idx = WorkspaceTab::ALL
            .iter()
            .position(|&t| t == tab)
            .unwrap_or(0);
        let dir = if new_idx >= old_idx { 1 } else { -1 };

        self.workspace_tab = tab;
        self.menu_stack = vec![MenuId::Main];
        self.selected_idx = 0;
        self.menu_viewport_offset = 0;
        self.digit_prefix = None;
        self.filter_active = false;
        self.filter_query.clear();
        self.pending_confirm = None;
        self.start_transition(TransitionKind::TabSwitch { dir });
        self.dirty = true;
    }

    pub fn current_screen(&self) -> MenuScreen {
        let menu = self.current_menu();
        if menu == MenuId::Main && self.workspace_tab != WorkspaceTab::Overview {
            build_workspace_menu(self.workspace_tab, &self.status)
        } else {
            build_menu(menu, &self.status)
        }
    }

    pub fn selected_idx(&self) -> usize {
        self.selected_idx
    }

    pub fn menu_viewport_offset(&self) -> usize {
        self.menu_viewport_offset
    }

    pub fn set_menu_viewport_offset(&mut self, offset: usize) {
        self.menu_viewport_offset = offset;
    }

    /// Recomputes `menu_viewport_offset` using `SCROLL_MARGIN` lookahead for `visible_rows`.
    pub fn sync_menu_viewport(&mut self, visible_rows: usize) {
        let len = self.filtered_items().len();
        self.menu_viewport_offset = compute_scroll_offset(
            self.selected_idx,
            len,
            visible_rows,
            self.menu_viewport_offset,
            SCROLL_MARGIN,
        );
    }

    pub fn set_selected_idx(&mut self, idx: usize) {
        let count = self.filtered_items().len();
        let clamped = if count == 0 { 0 } else { idx.min(count - 1) };
        if self.selected_idx != clamped {
            self.selected_idx = clamped;
            self.dirty = true;
        }
    }

    pub fn digit_prefix(&self) -> Option<u8> {
        self.digit_prefix
    }

    pub fn pending_confirm(&self) -> Option<&ConfirmPrompt> {
        self.pending_confirm.as_ref()
    }

    /// Enables or disables automatic in-TUI option/form modals for actions that otherwise prompt via console stdin (U41).
    pub fn set_native_prompts(&mut self, enabled: bool) {
        self.native_prompts = enabled;
    }

    pub fn native_prompts_enabled(&self) -> bool {
        self.native_prompts
    }

    pub fn active_prompt(&self) -> Option<&InTuiPrompt> {
        self.active_prompt.as_ref()
    }

    pub fn open_prompt(&mut self, prompt: InTuiPrompt) {
        self.active_prompt = Some(prompt);
        self.pending_confirm = None;
        self.help_active = false;
        self.palette_active = false;
        self.filter_active = false;
        self.digit_prefix = None;
        self.start_transition(TransitionKind::ModalOpen);
        self.dirty = true;
    }

    pub fn dismiss_prompt(&mut self) {
        if self.active_prompt.take().is_some() {
            self.dirty = true;
        }
    }

    pub fn open_yazi_install_prompt(&mut self) {
        self.open_prompt(InTuiPrompt::Options(OptionPromptState::yazi_install()));
    }

    pub fn open_podman_uninstall_prompt(&mut self) {
        self.open_prompt(InTuiPrompt::Options(OptionPromptState::podman_uninstall()));
    }

    pub fn open_script_uninstall_prompt(&mut self) {
        self.open_prompt(InTuiPrompt::Options(OptionPromptState::script_uninstall()));
    }

    pub fn open_reconcile_apply_confirm(&mut self, drift_count: usize) {
        self.open_prompt(InTuiPrompt::Options(OptionPromptState::reconcile_apply(
            drift_count,
        )));
    }

    pub fn open_freship_picker(&mut self, map: FreshipMap, is_reconfigure: bool) {
        if map.continents.is_empty() {
            self.set_notice_with_kind(NoticeKind::Danger, "FreshIP 地区列表为空，无法选择节点");
            return;
        }
        self.open_prompt(InTuiPrompt::Freship(FreshipPickerState::new(
            map,
            is_reconfigure,
        )));
    }

    pub fn open_lego_add_domain_form(&mut self) {
        self.open_prompt(InTuiPrompt::Form(FormState::lego_add_domain()));
    }

    pub fn open_lego_domain_picker(&mut self, kind: PickerKind, domains: &[LegoDomainStatus]) {
        if domains.is_empty() {
            self.set_notice_with_kind(NoticeKind::Warning, "暂无已托管的 Lego 域名，请先添加域名");
            return;
        }
        self.open_prompt(InTuiPrompt::Picker(PickerState::lego_domains(
            kind, domains,
        )));
    }

    /// Starts a live in-TUI task execution stream in the right pane (U39).
    pub fn start_task_run(&mut self, label: impl Into<String>) {
        self.task_run = Some(TaskRunState::new_running(label));
        self.pending_confirm = None;
        self.active_prompt = None;
        self.dirty = true;
    }

    /// Appends one sanitized output line to the active `TaskRunState` (U39).
    pub fn push_task_log_line(&mut self, line: &str) {
        if let Some(task) = self.task_run.as_mut() {
            task.push_line(line);
            self.dirty = true;
        }
    }

    /// Marks the active `TaskRunState` as succeeded or failed with a concise summary (U39).
    pub fn finish_task_run(&mut self, succeeded: bool, summary: impl Into<String>) {
        if let Some(task) = self.task_run.as_mut() {
            task.finish(succeeded, summary);
            self.dirty = true;
        }
    }

    /// Dismisses the right-pane task execution log panel (U39).
    pub fn dismiss_task_run(&mut self) {
        if self.task_run.take().is_some() {
            self.dirty = true;
        }
    }

    /// Scrolls the live task execution log by signed line delta (U39).
    pub fn scroll_task_log(&mut self, delta: i16) {
        let vh = self.task_log_viewport_h;
        if let Some(task) = self.task_run.as_mut() {
            task.scroll_by(delta, vh);
            self.dirty = true;
        }
    }

    pub fn task_run(&self) -> Option<&TaskRunState> {
        self.task_run.as_ref()
    }

    pub fn is_task_running(&self) -> bool {
        self.task_run.as_ref().is_some_and(TaskRunState::is_running)
    }

    /// Installs a deterministic `TaskRunState` preview for `ui-preview --screen task-run` (U39).
    pub fn open_task_run_preview(&mut self, task: TaskRunState) {
        self.task_run = Some(task);
        self.dirty = true;
    }

    /// Updates cached visible row count of the live task log viewport from the renderer.
    pub fn set_task_log_viewport_height(&mut self, height: usize) {
        self.task_log_viewport_h = height;
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn notice_kind(&self) -> NoticeKind {
        self.notice_kind
    }

    pub fn notice_ttl(&self) -> u16 {
        let Some(deadline) = self.notice_deadline else {
            return 0;
        };
        let remaining_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis();
        remaining_ms.div_ceil(33).min(u128::from(u16::MAX)) as u16
    }

    pub fn active_pane(&self) -> ActivePane {
        if !self.split_pane_available {
            ActivePane::Menu
        } else {
            self.active_pane
        }
    }

    pub fn set_active_pane(&mut self, pane: ActivePane) {
        let next = if self.split_pane_available {
            pane
        } else {
            ActivePane::Menu
        };
        if self.active_pane != next {
            self.active_pane = next;
            self.dirty = true;
        }
    }

    pub fn set_split_pane_available(&mut self, available: bool) {
        if self.split_pane_available != available {
            self.split_pane_available = available;
            self.dirty = true;
        }
        if !available && self.active_pane != ActivePane::Menu {
            self.active_pane = ActivePane::Menu;
            self.dirty = true;
        }
    }

    pub fn split_pane_available(&self) -> bool {
        self.split_pane_available
    }

    pub fn telemetry_scroll(&self) -> u16 {
        self.telemetry_scroll
    }

    pub fn set_telemetry_scroll(&mut self, scroll: u16) {
        let clamped = scroll.min(64);
        if self.telemetry_scroll != clamped {
            self.telemetry_scroll = clamped;
            self.dirty = true;
        }
    }

    pub fn telemetry_specs(&self) -> &SystemSpecs {
        &self.telemetry_specs
    }

    pub fn telemetry_history(&self) -> &TelemetryHistory {
        &self.telemetry_history
    }

    pub fn with_telemetry(mut self, specs: SystemSpecs, history: TelemetryHistory) -> Self {
        self.telemetry_specs = specs;
        self.telemetry_history = history;
        self.dirty = true;
        self
    }

    pub fn set_telemetry(&mut self, specs: SystemSpecs, history: TelemetryHistory) {
        self.telemetry_specs = specs;
        self.telemetry_history = history;
        self.dirty = true;
    }

    pub fn refresh_telemetry_specs(&mut self, root_prefix: &Path) {
        self.telemetry_specs = probe_system_specs(root_prefix);
        self.dirty = true;
    }

    pub fn attach_live_collector(&mut self, mut collector: TelemetryCollector) {
        let sample = collector.sample_now();
        self.telemetry_history.push_sample(sample);
        self.telemetry_collector = Some(collector);
        self.next_telemetry_at = self
            .motion_policy
            .telemetry_interval
            .map(|interval| Instant::now() + interval);
        self.dirty = true;
    }

    pub fn filter_active(&self) -> bool {
        self.filter_active
    }

    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    pub fn palette_active(&self) -> bool {
        self.palette_active
    }

    pub fn palette_query(&self) -> &str {
        &self.palette_query
    }

    pub fn palette_selected(&self) -> usize {
        self.palette_selected
    }

    pub fn help_active(&self) -> bool {
        self.help_active
    }

    /// Opens the `?` / `F1` Help overlay directly (used by `ui-preview` and tests).
    pub fn open_help_preview(&mut self) {
        self.help_active = true;
        self.palette_active = false;
        self.filter_active = false;
        self.pending_confirm = None;
        self.dirty = true;
    }

    /// Returns `true` whenever a text-entry mode (inline `/` filter, `Ctrl+K` palette, or in-TUI form) is active.
    pub fn is_text_input_active(&self) -> bool {
        self.filter_active
            || self.palette_active
            || matches!(self.active_prompt, Some(InTuiPrompt::Form(_)))
    }

    /// Opens the global fuzzy Command Palette with a deterministic query (used by `ui-preview`).
    pub fn open_palette_preview(&mut self, query: impl Into<String>) {
        self.palette_active = true;
        self.palette_query = query.into();
        self.palette_selected = 0;
        self.filter_active = false;
        self.pending_confirm = None;
        self.dirty = true;
    }

    /// Returns global Command Palette entries matching `palette_query` (multi-token fuzzy substring).
    pub fn filtered_palette_entries(&self) -> Vec<PaletteEntry> {
        let all = build_palette_entries(&self.status);
        let q = self.palette_query.trim().to_lowercase();
        if q.is_empty() {
            return all;
        }
        let tokens: Vec<&str> = q.split_whitespace().collect();
        all.into_iter()
            .filter(|entry| {
                let haystack = format!(
                    "{} {} {} {}",
                    entry.domain.to_lowercase(),
                    entry.label.to_lowercase(),
                    entry.keywords.to_lowercase(),
                    entry.action.cli_hint().to_lowercase()
                );
                tokens.iter().all(|tok| haystack.contains(tok))
            })
            .collect()
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub fn set_tick_count(&mut self, tick: u64) {
        self.tick_count = tick;
    }

    pub fn motion_policy(&self) -> MotionPolicy {
        self.motion_policy
    }

    pub fn set_motion_policy(&mut self, policy: MotionPolicy) {
        self.motion_policy = policy;
        self.effective_frame_interval = policy.frame_interval;
        if !policy.animations_enabled {
            self.active_transition = None;
        }
        if self.telemetry_collector.is_some() {
            self.next_telemetry_at = policy
                .telemetry_interval
                .map(|interval| Instant::now() + interval);
        }
        self.dirty = true;
    }

    /// Returns the active animation frame cadence (possibly adapted by `FramePacer` under SSH load).
    pub fn effective_frame_interval(&self) -> Duration {
        self.effective_frame_interval
    }

    /// Updates the active animation frame cadence from `FramePacer`.
    pub fn set_effective_frame_interval(&mut self, interval: Duration) {
        self.effective_frame_interval = interval.max(Duration::from_millis(1));
    }

    /// Returns smooth toast visibility progress in `[0.0, 1.0]` at `now`:
    /// ramps `0.0 -> 1.0` over `TOAST_REVEAL_DURATION` (~140ms) on entry, holds `1.0`,
    /// and fades `1.0 -> 0.0` over `TOAST_FADE_DURATION` (~200ms) before expiry.
    /// Always returns `1.0` when animations are disabled.
    pub fn toast_progress_at(&self, now: Instant) -> f64 {
        if self.notice.is_none() {
            return 0.0;
        }
        if !self.motion_policy.animations_enabled {
            return 1.0;
        }
        if let Some(start) = self.toast_started_at {
            let elapsed = now.saturating_duration_since(start);
            if elapsed < TOAST_REVEAL_DURATION {
                let raw = elapsed.as_secs_f64() / TOAST_REVEAL_DURATION.as_secs_f64();
                return ease_out_quart(raw);
            }
        }
        if let Some(deadline) = self.notice_deadline {
            let remaining = deadline.saturating_duration_since(now);
            if remaining < TOAST_FADE_DURATION {
                let raw = remaining.as_secs_f64() / TOAST_FADE_DURATION.as_secs_f64();
                return ease_out_quart(raw);
            }
        }
        1.0
    }

    pub fn animations_enabled(&self) -> bool {
        self.motion_policy.animations_enabled
    }

    pub fn set_animations_enabled(&mut self, enabled: bool) {
        self.motion_policy.animations_enabled = enabled;
        if !enabled {
            self.active_transition = None;
        }
        self.dirty = true;
    }

    pub fn active_transition(&self) -> Option<MotionTransition> {
        self.active_transition
    }

    pub fn transition_eased_at(&self, now: Instant) -> Option<f64> {
        self.active_transition.map(|t| t.eased_progress_at(now))
    }

    pub fn set_transition_preview(&mut self, kind: TransitionKind, raw_progress: f64) {
        self.active_transition = Some(MotionTransition::preview(kind, raw_progress));
        self.dirty = true;
    }

    pub fn clear_transition(&mut self) {
        if self.active_transition.take().is_some() {
            self.dirty = true;
        }
    }

    pub fn transition_ticks(&self) -> u8 {
        let Some(tr) = self.active_transition else {
            return 0;
        };
        let remaining_frac = (1.0 - tr.raw_progress_at(Instant::now())).clamp(0.0, 1.0);
        (remaining_frac * f64::from(TRANSITION_TOTAL_TICKS)).ceil() as u8
    }

    pub fn set_transition_ticks(&mut self, ticks: u8) {
        if ticks == 0 {
            self.active_transition = None;
        } else {
            let clamped = ticks.min(TRANSITION_TOTAL_TICKS);
            let raw = 1.0 - f64::from(clamped) / f64::from(TRANSITION_TOTAL_TICKS);
            self.active_transition = Some(MotionTransition::preview(TransitionKind::MenuPush, raw));
        }
        self.dirty = true;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    /// Computes the earliest scheduled wake timeout for the event loop, or `None` when completely
    /// idle (guaranteeing zero idle CPU wakeups and zero idle redraws).
    pub fn next_wake_at(&self, now: Instant, split_mode: bool) -> Option<WakeSchedule> {
        let mut best: Option<WakeSchedule> = None;
        let frame_step = self.effective_frame_interval;

        if let Some(task) = self.task_run.as_ref()
            && task.is_running()
            && !task.is_preview()
        {
            best = Some(WakeSchedule {
                timeout: frame_step,
                kind: WakeKind::AnimFrame,
            });
        }

        if let Some(tr) = self.active_transition
            && tr.preview_progress.is_none()
        {
            let timeout = if tr.is_finished_at(now) {
                Duration::ZERO
            } else {
                frame_step.min(tr.remaining_at(now))
            };
            let cand = WakeSchedule {
                timeout,
                kind: WakeKind::AnimFrame,
            };
            if best.is_none_or(|b| cand.timeout < b.timeout) {
                best = Some(cand);
            }
        }

        if self.notice.is_some()
            && let Some(deadline) = self.notice_deadline
        {
            let remaining = deadline.saturating_duration_since(now);
            let timeout = if self.motion_policy.animations_enabled {
                if let Some(start) = self.toast_started_at
                    && let elapsed = now.saturating_duration_since(start)
                    && elapsed < TOAST_REVEAL_DURATION
                {
                    frame_step.min(TOAST_REVEAL_DURATION.saturating_sub(elapsed))
                } else if remaining <= TOAST_FADE_DURATION {
                    frame_step.min(remaining)
                } else {
                    remaining.saturating_sub(TOAST_FADE_DURATION)
                }
            } else {
                remaining
            };
            let cand = WakeSchedule {
                timeout,
                kind: WakeKind::AnimFrame,
            };
            if best.is_none_or(|b| cand.timeout < b.timeout) {
                best = Some(cand);
            }
        }

        if split_mode
            && self.telemetry_collector.is_some()
            && self.motion_policy.telemetry_interval.is_some()
        {
            let next_at = self.next_telemetry_at.unwrap_or(now);
            let timeout = next_at.saturating_duration_since(now);
            let cand = WakeSchedule {
                timeout,
                kind: WakeKind::TelemetrySample,
            };
            if best.is_none_or(|b| cand.timeout < b.timeout) {
                best = Some(cand);
            }
        }

        best
    }

    /// Advances wall-clock animations, live task spinner, and toast reveal/fade/expiry at `now`.
    /// Returns `true` and marks `dirty = true` ONLY when visible state actually changed.
    pub fn advance_clock_at(&mut self, now: Instant) -> bool {
        let mut changed = false;

        if let Some(task) = self.task_run.as_ref()
            && task.is_running()
            && !task.is_preview()
        {
            changed = true;
        }

        if let Some(tr) = self.active_transition
            && tr.preview_progress.is_none()
        {
            if tr.is_finished_at(now) {
                self.active_transition = None;
            }
            changed = true;
        }

        if self.notice.is_some()
            && let Some(deadline) = self.notice_deadline
        {
            if now >= deadline {
                self.notice = None;
                self.notice_deadline = None;
                self.toast_started_at = None;
                changed = true;
            } else if self.motion_policy.animations_enabled {
                let in_reveal = self.toast_started_at.is_some_and(|start| {
                    now.saturating_duration_since(start) <= TOAST_REVEAL_DURATION
                });
                let in_fade = deadline.saturating_duration_since(now) <= TOAST_FADE_DURATION;
                if in_reveal || in_fade {
                    changed = true;
                }
            }
        }

        if changed {
            self.tick_count = self.tick_count.wrapping_add(1);
            self.dirty = true;
        }
        changed
    }

    pub fn set_notice(&mut self, msg: impl Into<String>) {
        let text = msg.into();
        let kind = NoticeKind::infer(&text);
        self.set_notice_with_kind(kind, text);
    }

    pub fn set_notice_with_kind(&mut self, kind: NoticeKind, msg: impl Into<String>) {
        let now = Instant::now();
        self.notice = Some(msg.into());
        self.notice_kind = kind;
        self.toast_started_at = Some(now);
        self.notice_deadline = Some(now + TOAST_TTL_DURATION);
        self.dirty = true;
    }

    pub fn clear_notice(&mut self) {
        if self.notice.take().is_some() {
            self.dirty = true;
        }
        self.notice_deadline = None;
        self.toast_started_at = None;
    }

    pub fn request_exit(&mut self) {
        self.exit_requested = true;
    }

    pub fn should_exit(&self) -> bool {
        self.exit_requested
    }

    /// Directly sets the active menu stack (used by deterministic showcase preview rendering).
    pub fn navigate_to(&mut self, menu: MenuId) {
        self.menu_stack = if menu == MenuId::Main {
            vec![MenuId::Main]
        } else {
            vec![MenuId::Main, menu]
        };
        self.selected_idx = 0;
        self.menu_viewport_offset = 0;
        self.digit_prefix = None;
        self.filter_active = false;
        self.filter_query.clear();
        self.pending_confirm = None;
        self.dirty = true;
    }

    /// Activates inline `/` filter with a deterministic query (used by `ui-preview`).
    pub fn open_filter_preview(&mut self, query: impl Into<String>) {
        self.filter_active = true;
        self.filter_query = query.into();
        self.selected_idx = 0;
        self.menu_viewport_offset = 0;
        self.dirty = true;
    }

    /// Opens a confirmation modal directly (used by deterministic showcase preview rendering).
    pub fn open_confirm_preview(&mut self, message: impl Into<String>, action: BashAction) {
        self.pending_confirm = Some(ConfirmPrompt {
            message: message.into(),
            action,
            menu_action: None,
            selected_yes: false,
            elevation_outcome: None,
        });
        self.dirty = true;
    }

    /// Opens an in-TUI sudo elevation confirmation modal for a privileged action (defaulting to Yes).
    pub fn open_elevation_confirm(&mut self, outcome: TuiOutcome, label: &str) {
        self.pending_confirm = Some(ConfirmPrompt {
            message: format!("操作「{label}」需要管理员权限，是否通过 sudo 提权执行？"),
            action: BashAction::LegacyFullEntry,
            menu_action: None,
            selected_yes: true,
            elevation_outcome: Some(Box::new(outcome)),
        });
        self.start_transition(TransitionKind::ModalOpen);
        self.dirty = true;
    }

    /// Returns items matching the active inline filter query (case-insensitive substring match).
    pub fn filtered_items(&self) -> Vec<MenuItem> {
        let screen = self.current_screen();
        if self.filter_query.is_empty() {
            return screen.items().into_iter().cloned().collect();
        }
        let needle = self.filter_query.to_lowercase();
        screen
            .items()
            .into_iter()
            .filter(|item| {
                item.label.to_lowercase().contains(&needle) || item.id.to_string().contains(&needle)
            })
            .cloned()
            .collect()
    }

    pub fn refresh_status(&mut self, status: StatusSnapshot) {
        self.status = status;
        let item_count = self.filtered_items().len();
        if item_count == 0 {
            self.selected_idx = 0;
        } else if self.selected_idx >= item_count {
            self.selected_idx = item_count - 1;
        }
        self.dirty = true;
    }

    fn start_transition(&mut self, kind: TransitionKind) {
        if self.motion_policy.animations_enabled {
            self.active_transition = Some(MotionTransition::start(
                kind,
                self.motion_policy.transition_duration,
                Instant::now(),
            ));
        } else {
            self.active_transition = None;
        }
    }

    fn apply_telemetry_scroll_delta(&mut self, delta: i16) {
        let next = if delta < 0 {
            self.telemetry_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.telemetry_scroll.saturating_add(delta as u16).min(64)
        };
        if self.telemetry_scroll != next {
            self.telemetry_scroll = next;
            self.dirty = true;
        }
    }

    /// Processes one `MenuEvent`, updating internal navigation state and optionally
    /// emitting a `TuiOutcome` for the host loop to execute.
    pub fn handle_event(&mut self, event: MenuEvent) -> Option<TuiOutcome> {
        match event {
            MenuEvent::Tick => {
                self.advance_clock_at(Instant::now());
                return None;
            }
            MenuEvent::TelemetryTick => {
                if let Some(collector) = self.telemetry_collector.as_mut() {
                    let before = self.telemetry_history.visual_fingerprint();
                    let sample = collector.sample_now();
                    self.telemetry_history.push_sample(sample);
                    let after = self.telemetry_history.visual_fingerprint();
                    if let Some(interval) = self.motion_policy.telemetry_interval {
                        self.next_telemetry_at = Some(Instant::now() + interval);
                    }
                    if after != before {
                        self.dirty = true;
                    }
                }
                return None;
            }
            MenuEvent::Resize => {
                self.dirty = true;
                return None;
            }
            _ => {}
        }

        if self.active_prompt.is_some() {
            return self.handle_prompt_event(event);
        }

        if let Some(prompt) = self.pending_confirm.clone() {
            return self.handle_confirm_event(event, prompt);
        }

        if self.help_active {
            return match event {
                MenuEvent::HelpToggle
                | MenuEvent::Back
                | MenuEvent::Select
                | MenuEvent::FilterClear => {
                    self.help_active = false;
                    self.dirty = true;
                    None
                }
                MenuEvent::Quit => {
                    self.help_active = false;
                    Some(TuiOutcome::Exit)
                }
                _ => None,
            };
        }

        if self.palette_active {
            return self.handle_palette_event(event);
        }

        if self.filter_active {
            return self.handle_filter_event(event);
        }

        // Handle direct task-log and mouse actions regardless of currently focused pane.
        match event {
            MenuEvent::ScrollTaskLog(delta) => {
                self.scroll_task_log(delta);
                return None;
            }
            MenuEvent::DismissTaskLog => {
                if self.task_run.as_ref().is_some_and(|t| !t.is_running()) {
                    self.dismiss_task_run();
                    if self.active_pane == ActivePane::Telemetry {
                        self.active_pane = ActivePane::Menu;
                    }
                }
                return None;
            }
            MenuEvent::ClickTab(tab) => {
                self.active_pane = ActivePane::Menu;
                self.set_workspace_tab(tab);
                return None;
            }
            MenuEvent::ClickMenuItem(idx) => {
                let items = self.filtered_items();
                if idx < items.len() {
                    self.digit_prefix = None;
                    if self.active_pane() == ActivePane::Menu && self.selected_idx == idx {
                        let action = items[idx].action.clone();
                        return self.apply_action(action);
                    }
                    self.active_pane = ActivePane::Menu;
                    self.selected_idx = idx;
                    self.dirty = true;
                }
                return None;
            }
            MenuEvent::FocusPane(pane) => {
                self.set_active_pane(pane);
                return None;
            }
            MenuEvent::ScrollTelemetry(delta) => {
                if self.task_run.is_some() {
                    self.scroll_task_log(delta);
                } else {
                    self.apply_telemetry_scroll_delta(delta);
                }
                return None;
            }
            _ => {}
        }

        // Narrow mode (< SPLIT_PANE_MIN_WIDTH) displays active TaskRunState in body_rect.
        if !self.split_pane_available && self.task_run.is_some() {
            let vh = self.task_log_viewport_h;
            return match event {
                MenuEvent::Up => {
                    self.scroll_task_log(-1);
                    None
                }
                MenuEvent::Down => {
                    self.scroll_task_log(1);
                    None
                }
                MenuEvent::PageUp => {
                    self.scroll_task_log(-8);
                    None
                }
                MenuEvent::PageDown => {
                    self.scroll_task_log(8);
                    None
                }
                MenuEvent::Home => {
                    if let Some(task) = self.task_run.as_mut() {
                        task.scroll_to_top();
                        self.dirty = true;
                    }
                    None
                }
                MenuEvent::End => {
                    if let Some(task) = self.task_run.as_mut() {
                        task.scroll_to_bottom(vh);
                        self.dirty = true;
                    }
                    None
                }
                MenuEvent::Back => {
                    if self.task_run.as_ref().is_some_and(|t| !t.is_running()) {
                        self.dismiss_task_run();
                    }
                    None
                }
                MenuEvent::HelpToggle => {
                    self.help_active = true;
                    self.start_transition(TransitionKind::ModalOpen);
                    self.dirty = true;
                    None
                }
                MenuEvent::Quit => Some(TuiOutcome::Exit),
                _ => None,
            };
        }

        // Dedicated interactive right-pane scroll mode (TaskRunState or Telemetry) when focused.
        if self.active_pane() == ActivePane::Telemetry {
            let has_task = self.task_run.is_some();
            let vh = self.task_log_viewport_h;
            match event {
                MenuEvent::Up => {
                    if has_task {
                        self.scroll_task_log(-1);
                    } else {
                        self.apply_telemetry_scroll_delta(-1);
                    }
                    return None;
                }
                MenuEvent::Down => {
                    if has_task {
                        self.scroll_task_log(1);
                    } else {
                        self.apply_telemetry_scroll_delta(1);
                    }
                    return None;
                }
                MenuEvent::PageUp => {
                    if has_task {
                        self.scroll_task_log(-8);
                    } else {
                        self.apply_telemetry_scroll_delta(-5);
                    }
                    return None;
                }
                MenuEvent::PageDown => {
                    if has_task {
                        self.scroll_task_log(8);
                    } else {
                        self.apply_telemetry_scroll_delta(5);
                    }
                    return None;
                }
                MenuEvent::Home => {
                    if let Some(task) = self.task_run.as_mut() {
                        task.scroll_to_top();
                        self.dirty = true;
                    } else {
                        self.set_telemetry_scroll(0);
                    }
                    return None;
                }
                MenuEvent::End => {
                    if let Some(task) = self.task_run.as_mut() {
                        task.scroll_to_bottom(vh);
                        self.dirty = true;
                    } else {
                        self.set_telemetry_scroll(64);
                    }
                    return None;
                }
                MenuEvent::TogglePane => {
                    self.active_pane = ActivePane::Menu;
                    self.dirty = true;
                    return None;
                }
                MenuEvent::Back => {
                    if self.task_run.as_ref().is_some_and(|t| !t.is_running()) {
                        self.dismiss_task_run();
                    }
                    self.active_pane = ActivePane::Menu;
                    self.dirty = true;
                    return None;
                }
                MenuEvent::NextTab => {
                    self.set_workspace_tab(self.workspace_tab.next());
                    self.active_pane = ActivePane::Menu;
                    return None;
                }
                MenuEvent::PrevTab => {
                    self.set_workspace_tab(self.workspace_tab.prev());
                    self.active_pane = ActivePane::Menu;
                    return None;
                }
                MenuEvent::PaletteOpen => {
                    self.palette_active = true;
                    self.palette_query.clear();
                    self.palette_selected = 0;
                    self.start_transition(TransitionKind::ModalOpen);
                    self.dirty = true;
                    return None;
                }
                MenuEvent::HelpToggle => {
                    self.help_active = true;
                    self.start_transition(TransitionKind::ModalOpen);
                    self.dirty = true;
                    return None;
                }
                MenuEvent::Refresh => {
                    self.set_notice_with_kind(NoticeKind::Success, "状态已刷新");
                    return Some(TuiOutcome::Refresh);
                }
                MenuEvent::Quit => return Some(TuiOutcome::Exit),
                MenuEvent::FilterStart => {
                    self.active_pane = ActivePane::Menu;
                    self.filter_active = true;
                    self.selected_idx = 0;
                    self.dirty = true;
                    return None;
                }
                _ => return None,
            }
        }

        let screen = self.current_screen();
        let items = self.filtered_items();
        if items.is_empty() {
            return matches!(event, MenuEvent::Quit | MenuEvent::Back).then_some(TuiOutcome::Exit);
        }

        match event {
            MenuEvent::Up => {
                self.digit_prefix = None;
                self.selected_idx = if self.selected_idx == 0 {
                    items.len() - 1
                } else {
                    self.selected_idx - 1
                };
                self.dirty = true;
                None
            }
            MenuEvent::Down => {
                self.digit_prefix = None;
                self.selected_idx = (self.selected_idx + 1) % items.len();
                self.dirty = true;
                None
            }
            MenuEvent::PageUp => {
                self.digit_prefix = None;
                self.selected_idx = self.selected_idx.saturating_sub(5);
                self.dirty = true;
                None
            }
            MenuEvent::PageDown => {
                self.digit_prefix = None;
                self.selected_idx = (self.selected_idx + 5).min(items.len() - 1);
                self.dirty = true;
                None
            }
            MenuEvent::Home => {
                self.digit_prefix = None;
                self.selected_idx = 0;
                self.dirty = true;
                None
            }
            MenuEvent::End => {
                self.digit_prefix = None;
                self.selected_idx = items.len() - 1;
                self.dirty = true;
                None
            }
            MenuEvent::Select => {
                if let Some(prefix) = self.digit_prefix.take() {
                    self.dirty = true;
                    return self.activate_by_id(&screen, prefix);
                }
                let action = items[self.selected_idx.min(items.len() - 1)].action.clone();
                self.apply_action(action)
            }
            MenuEvent::Back => {
                if self.digit_prefix.take().is_some() {
                    self.dirty = true;
                    return None;
                }
                if self.current_menu() == MenuId::Main
                    && self.task_run.as_ref().is_some_and(|t| !t.is_running())
                {
                    self.dismiss_task_run();
                    return None;
                }
                self.go_back()
            }
            MenuEvent::Quit => {
                self.digit_prefix = None;
                self.pending_confirm = None;
                Some(TuiOutcome::Exit)
            }
            MenuEvent::Refresh => {
                self.digit_prefix = None;
                self.set_notice_with_kind(NoticeKind::Success, "状态已刷新");
                Some(TuiOutcome::Refresh)
            }
            MenuEvent::FilterStart => {
                self.digit_prefix = None;
                self.filter_active = true;
                self.selected_idx = 0;
                self.menu_viewport_offset = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterClear => {
                self.filter_active = false;
                self.filter_query.clear();
                self.selected_idx = 0;
                self.menu_viewport_offset = 0;
                self.dirty = true;
                None
            }
            MenuEvent::TogglePane => {
                if self.split_pane_available {
                    self.active_pane = match self.active_pane {
                        ActivePane::Menu => ActivePane::Telemetry,
                        ActivePane::Telemetry => ActivePane::Menu,
                    };
                    self.dirty = true;
                }
                None
            }
            MenuEvent::NextTab => {
                if self.current_menu() == MenuId::Main {
                    self.set_workspace_tab(self.workspace_tab.next());
                    None
                } else {
                    if let Some(prefix) = self.digit_prefix.take() {
                        self.dirty = true;
                        return self.activate_by_id(&screen, prefix);
                    }
                    let action = items[self.selected_idx.min(items.len() - 1)].action.clone();
                    self.apply_action(action)
                }
            }
            MenuEvent::PrevTab => {
                if self.current_menu() == MenuId::Main {
                    self.set_workspace_tab(self.workspace_tab.prev());
                    None
                } else {
                    self.go_back()
                }
            }
            MenuEvent::PaletteOpen => {
                self.digit_prefix = None;
                self.palette_active = true;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.start_transition(TransitionKind::ModalOpen);
                self.dirty = true;
                None
            }
            MenuEvent::HelpToggle => {
                self.digit_prefix = None;
                self.help_active = true;
                self.start_transition(TransitionKind::ModalOpen);
                self.dirty = true;
                None
            }
            MenuEvent::Resize
            | MenuEvent::ConfirmYes
            | MenuEvent::ConfirmNo
            | MenuEvent::FilterChar(_)
            | MenuEvent::FilterBackspace
            | MenuEvent::FilterClearLine
            | MenuEvent::FilterDeleteWord
            | MenuEvent::Tick
            | MenuEvent::TelemetryTick
            | MenuEvent::ClickTab(_)
            | MenuEvent::ClickMenuItem(_)
            | MenuEvent::ClickPaletteItem(_)
            | MenuEvent::FocusPane(_)
            | MenuEvent::ScrollTelemetry(_)
            | MenuEvent::ScrollTaskLog(_)
            | MenuEvent::DismissTaskLog
            | MenuEvent::FormToggle
            | MenuEvent::ClickModalRow(_) => None,
            MenuEvent::Digit(d) => self.handle_digit(&screen, d),
        }
    }

    fn handle_palette_event(&mut self, event: MenuEvent) -> Option<TuiOutcome> {
        match event {
            MenuEvent::PaletteOpen | MenuEvent::FilterClear | MenuEvent::Back => {
                self.palette_active = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterChar(ch) => {
                self.palette_query.push(ch);
                self.palette_selected = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterBackspace => {
                if self.palette_query.pop().is_none() {
                    self.palette_active = false;
                }
                self.palette_selected = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterClearLine => {
                self.palette_query.clear();
                self.palette_selected = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterDeleteWord => {
                while self
                    .palette_query
                    .chars()
                    .last()
                    .is_some_and(char::is_whitespace)
                {
                    self.palette_query.pop();
                }
                while self
                    .palette_query
                    .chars()
                    .last()
                    .is_some_and(|c| !c.is_whitespace())
                {
                    self.palette_query.pop();
                }
                self.palette_selected = 0;
                self.dirty = true;
                None
            }
            MenuEvent::Up => {
                let count = self.filtered_palette_entries().len();
                if count > 0 {
                    self.palette_selected = if self.palette_selected == 0 {
                        count - 1
                    } else {
                        self.palette_selected - 1
                    };
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Down => {
                let count = self.filtered_palette_entries().len();
                if count > 0 {
                    self.palette_selected = (self.palette_selected + 1) % count;
                    self.dirty = true;
                }
                None
            }
            MenuEvent::PageUp => {
                self.palette_selected = self.palette_selected.saturating_sub(5);
                self.dirty = true;
                None
            }
            MenuEvent::PageDown => {
                let count = self.filtered_palette_entries().len();
                if count > 0 {
                    self.palette_selected = (self.palette_selected + 5).min(count - 1);
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Home => {
                self.palette_selected = 0;
                self.dirty = true;
                None
            }
            MenuEvent::End => {
                let count = self.filtered_palette_entries().len();
                if count > 0 {
                    self.palette_selected = count - 1;
                    self.dirty = true;
                }
                None
            }
            MenuEvent::ClickPaletteItem(idx) => {
                let entries = self.filtered_palette_entries();
                if idx < entries.len() {
                    if self.palette_selected == idx {
                        let action = entries[idx].action.clone();
                        self.palette_active = false;
                        self.palette_query.clear();
                        self.palette_selected = 0;
                        self.dirty = true;
                        return self.apply_action(action);
                    }
                    self.palette_selected = idx;
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Select => {
                let entries = self.filtered_palette_entries();
                self.palette_active = false;
                let picked = entries
                    .get(self.palette_selected.min(entries.len().saturating_sub(1)))
                    .map(|e| e.action.clone());
                self.palette_query.clear();
                self.palette_selected = 0;
                self.dirty = true;
                if let Some(action) = picked {
                    return self.apply_action(action);
                }
                None
            }
            MenuEvent::Quit => Some(TuiOutcome::Exit),
            _ => None,
        }
    }

    fn handle_filter_event(&mut self, event: MenuEvent) -> Option<TuiOutcome> {
        match event {
            MenuEvent::FilterChar(ch) => {
                self.filter_query.push(ch);
                self.selected_idx = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterBackspace => {
                if self.filter_query.pop().is_none() {
                    self.filter_active = false;
                }
                self.selected_idx = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterClearLine => {
                self.filter_query.clear();
                self.selected_idx = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterDeleteWord => {
                while self
                    .filter_query
                    .chars()
                    .last()
                    .is_some_and(char::is_whitespace)
                {
                    self.filter_query.pop();
                }
                while self
                    .filter_query
                    .chars()
                    .last()
                    .is_some_and(|c| !c.is_whitespace())
                {
                    self.filter_query.pop();
                }
                self.selected_idx = 0;
                self.dirty = true;
                None
            }
            MenuEvent::FilterClear | MenuEvent::Back => {
                self.filter_active = false;
                self.filter_query.clear();
                self.selected_idx = 0;
                self.dirty = true;
                None
            }
            MenuEvent::ClickMenuItem(idx) => {
                let items = self.filtered_items();
                if idx < items.len() {
                    if self.selected_idx == idx {
                        let action = items[idx].action.clone();
                        self.filter_active = false;
                        self.filter_query.clear();
                        self.dirty = true;
                        return self.apply_action(action);
                    }
                    self.selected_idx = idx;
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Select => {
                let items = self.filtered_items();
                self.filter_active = false;
                self.dirty = true;
                if let Some(item) = items.get(self.selected_idx.min(items.len().saturating_sub(1)))
                {
                    let action = item.action.clone();
                    self.filter_query.clear();
                    return self.apply_action(action);
                }
                self.filter_query.clear();
                None
            }
            MenuEvent::Up => {
                let count = self.filtered_items().len();
                if count > 0 {
                    self.selected_idx = if self.selected_idx == 0 {
                        count - 1
                    } else {
                        self.selected_idx - 1
                    };
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Down => {
                let count = self.filtered_items().len();
                if count > 0 {
                    self.selected_idx = (self.selected_idx + 1) % count;
                    self.dirty = true;
                }
                None
            }
            MenuEvent::PageUp => {
                self.selected_idx = self.selected_idx.saturating_sub(5);
                self.dirty = true;
                None
            }
            MenuEvent::PageDown => {
                let count = self.filtered_items().len();
                if count > 0 {
                    self.selected_idx = (self.selected_idx + 5).min(count - 1);
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Home => {
                self.selected_idx = 0;
                self.dirty = true;
                None
            }
            MenuEvent::End => {
                let count = self.filtered_items().len();
                if count > 0 {
                    self.selected_idx = count - 1;
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Quit => Some(TuiOutcome::Exit),
            _ => None,
        }
    }

    fn handle_prompt_event(&mut self, event: MenuEvent) -> Option<TuiOutcome> {
        let prompt = self.active_prompt.take()?;
        match prompt {
            InTuiPrompt::Options(opts) => self.handle_option_prompt_event(event, opts),
            InTuiPrompt::Picker(picker) => self.handle_picker_prompt_event(event, picker),
            InTuiPrompt::Freship(freship) => self.handle_freship_picker_event(event, freship),
            InTuiPrompt::Form(form) => self.handle_form_prompt_event(event, form),
        }
    }

    fn handle_option_prompt_event(
        &mut self,
        event: MenuEvent,
        mut opts: OptionPromptState,
    ) -> Option<TuiOutcome> {
        let total = opts.total_stops();
        let confirm_idx = opts.confirm_idx();
        let cancel_idx = opts.cancel_idx();
        match event {
            MenuEvent::Quit => Some(TuiOutcome::Exit),
            MenuEvent::ConfirmNo | MenuEvent::Back | MenuEvent::FilterClear => {
                self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                None
            }
            MenuEvent::ConfirmYes => {
                self.clear_notice();
                self.dirty = true;
                Some(TuiOutcome::PromptSubmitted(opts.into_submission()))
            }
            MenuEvent::Up => {
                opts.cursor = if opts.cursor == 0 {
                    total - 1
                } else {
                    opts.cursor - 1
                };
                self.active_prompt = Some(InTuiPrompt::Options(opts));
                self.dirty = true;
                None
            }
            MenuEvent::Down | MenuEvent::TogglePane => {
                opts.cursor = (opts.cursor + 1) % total;
                self.active_prompt = Some(InTuiPrompt::Options(opts));
                self.dirty = true;
                None
            }
            MenuEvent::PrevTab | MenuEvent::NextTab => {
                if opts.cursor < opts.options.len() {
                    opts.options[opts.cursor].checked = !opts.options[opts.cursor].checked;
                } else {
                    opts.cursor = if opts.cursor == confirm_idx {
                        cancel_idx
                    } else {
                        confirm_idx
                    };
                }
                self.active_prompt = Some(InTuiPrompt::Options(opts));
                self.dirty = true;
                None
            }
            MenuEvent::FormToggle => {
                if opts.cursor < opts.options.len() {
                    opts.options[opts.cursor].checked = !opts.options[opts.cursor].checked;
                }
                self.active_prompt = Some(InTuiPrompt::Options(opts));
                self.dirty = true;
                None
            }
            MenuEvent::ClickModalRow(idx) => {
                if idx < opts.options.len() {
                    opts.cursor = idx;
                    opts.options[idx].checked = !opts.options[idx].checked;
                    self.active_prompt = Some(InTuiPrompt::Options(opts));
                    self.dirty = true;
                    None
                } else if idx == confirm_idx {
                    self.clear_notice();
                    self.dirty = true;
                    Some(TuiOutcome::PromptSubmitted(opts.into_submission()))
                } else {
                    self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                    None
                }
            }
            MenuEvent::Select => {
                if opts.cursor < opts.options.len() {
                    opts.options[opts.cursor].checked = !opts.options[opts.cursor].checked;
                    self.active_prompt = Some(InTuiPrompt::Options(opts));
                    self.dirty = true;
                    None
                } else if opts.cursor == confirm_idx {
                    self.clear_notice();
                    self.dirty = true;
                    Some(TuiOutcome::PromptSubmitted(opts.into_submission()))
                } else {
                    self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                    None
                }
            }
            _ => {
                self.active_prompt = Some(InTuiPrompt::Options(opts));
                None
            }
        }
    }

    fn handle_picker_prompt_event(
        &mut self,
        event: MenuEvent,
        mut picker: PickerState,
    ) -> Option<TuiOutcome> {
        let count = picker.items.len();
        match event {
            MenuEvent::Quit => Some(TuiOutcome::Exit),
            MenuEvent::Back | MenuEvent::ConfirmNo | MenuEvent::FilterClear => {
                self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                None
            }
            MenuEvent::Up => {
                if count > 0 {
                    picker.selected = if picker.selected == 0 {
                        count - 1
                    } else {
                        picker.selected - 1
                    };
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Picker(picker));
                None
            }
            MenuEvent::Down => {
                if count > 0 {
                    picker.selected = (picker.selected + 1) % count;
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Picker(picker));
                None
            }
            MenuEvent::PageUp => {
                picker.selected = picker.selected.saturating_sub(5);
                self.active_prompt = Some(InTuiPrompt::Picker(picker));
                self.dirty = true;
                None
            }
            MenuEvent::PageDown => {
                if count > 0 {
                    picker.selected = (picker.selected + 5).min(count - 1);
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Picker(picker));
                None
            }
            MenuEvent::Home => {
                picker.selected = 0;
                self.active_prompt = Some(InTuiPrompt::Picker(picker));
                self.dirty = true;
                None
            }
            MenuEvent::End => {
                if count > 0 {
                    picker.selected = count - 1;
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Picker(picker));
                None
            }
            MenuEvent::Digit(d) if d >= 1 && (d as usize) <= count => {
                picker.selected = (d as usize) - 1;
                self.submit_picker_selection(picker)
            }
            MenuEvent::ClickModalRow(idx) if idx < count => {
                if picker.selected == idx {
                    self.submit_picker_selection(picker)
                } else {
                    picker.selected = idx;
                    self.active_prompt = Some(InTuiPrompt::Picker(picker));
                    self.dirty = true;
                    None
                }
            }
            MenuEvent::Select => self.submit_picker_selection(picker),
            _ => {
                self.active_prompt = Some(InTuiPrompt::Picker(picker));
                None
            }
        }
    }

    fn submit_picker_selection(&mut self, picker: PickerState) -> Option<TuiOutcome> {
        let picked = picker
            .items
            .get(picker.selected.min(picker.items.len().saturating_sub(1)))?
            .value
            .clone();
        match picker.kind {
            PickerKind::LegoToggleFlagDomain => {
                self.active_prompt =
                    Some(InTuiPrompt::Picker(PickerState::lego_flag_choice(picked)));
                self.dirty = true;
                None
            }
            PickerKind::LegoToggleFlagChoice { domain } => {
                let flag = match picked.as_str() {
                    "dns-skip-propagation" => LegoFlag::DnsSkipPropagation,
                    "ferron-push" => LegoFlag::FerronPush,
                    _ => LegoFlag::AutoRenew,
                };
                self.clear_notice();
                self.dirty = true;
                Some(TuiOutcome::PromptSubmitted(
                    PromptSubmission::LegoToggleFlag { domain, flag },
                ))
            }
            PickerKind::LegoRunOnceDomain => {
                self.active_prompt = Some(InTuiPrompt::Picker(PickerState::lego_run_mode(picked)));
                self.dirty = true;
                None
            }
            PickerKind::LegoRunOnceMode { domain } => {
                self.clear_notice();
                self.dirty = true;
                Some(TuiOutcome::PromptSubmitted(PromptSubmission::LegoRunOnce {
                    domain,
                    mode: picked,
                }))
            }
            PickerKind::LegoTroubleshootDomain => {
                self.clear_notice();
                self.dirty = true;
                Some(TuiOutcome::PromptSubmitted(
                    PromptSubmission::LegoTroubleshoot { domain: picked },
                ))
            }
            PickerKind::LegoRemoveDomain => {
                self.clear_notice();
                self.dirty = true;
                Some(TuiOutcome::PromptSubmitted(
                    PromptSubmission::LegoRemoveDomain { domain: picked },
                ))
            }
        }
    }

    fn handle_freship_picker_event(
        &mut self,
        event: MenuEvent,
        mut freship: FreshipPickerState,
    ) -> Option<TuiOutcome> {
        let count = freship.current_items().len();
        match event {
            MenuEvent::Quit => Some(TuiOutcome::Exit),
            MenuEvent::Back | MenuEvent::ConfirmNo | MenuEvent::FilterClear => {
                if freship.step_back() {
                    self.active_prompt = Some(InTuiPrompt::Freship(freship));
                    self.dirty = true;
                } else {
                    self.set_notice_with_kind(NoticeKind::Warning, "已取消 FreshIP 地区选择");
                }
                None
            }
            MenuEvent::Up => {
                if count > 0 {
                    freship.selected = if freship.selected == 0 {
                        count - 1
                    } else {
                        freship.selected - 1
                    };
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Freship(freship));
                None
            }
            MenuEvent::Down => {
                if count > 0 {
                    freship.selected = (freship.selected + 1) % count;
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Freship(freship));
                None
            }
            MenuEvent::PageUp => {
                freship.selected = freship.selected.saturating_sub(5);
                self.active_prompt = Some(InTuiPrompt::Freship(freship));
                self.dirty = true;
                None
            }
            MenuEvent::PageDown => {
                if count > 0 {
                    freship.selected = (freship.selected + 5).min(count - 1);
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Freship(freship));
                None
            }
            MenuEvent::Home => {
                freship.selected = 0;
                self.active_prompt = Some(InTuiPrompt::Freship(freship));
                self.dirty = true;
                None
            }
            MenuEvent::End => {
                if count > 0 {
                    freship.selected = count - 1;
                    self.dirty = true;
                }
                self.active_prompt = Some(InTuiPrompt::Freship(freship));
                None
            }
            MenuEvent::Digit(d) if d >= 1 && (d as usize) <= count => {
                freship.selected = (d as usize) - 1;
                if let Some(selection) = freship.advance() {
                    let is_reconfigure = freship.is_reconfigure;
                    self.clear_notice();
                    self.dirty = true;
                    Some(TuiOutcome::PromptSubmitted(PromptSubmission::Freship {
                        selection,
                        is_reconfigure,
                    }))
                } else {
                    self.active_prompt = Some(InTuiPrompt::Freship(freship));
                    self.dirty = true;
                    None
                }
            }
            MenuEvent::ClickModalRow(idx) if idx < count => {
                if freship.selected == idx {
                    if let Some(selection) = freship.advance() {
                        let is_reconfigure = freship.is_reconfigure;
                        self.clear_notice();
                        self.dirty = true;
                        Some(TuiOutcome::PromptSubmitted(PromptSubmission::Freship {
                            selection,
                            is_reconfigure,
                        }))
                    } else {
                        self.active_prompt = Some(InTuiPrompt::Freship(freship));
                        self.dirty = true;
                        None
                    }
                } else {
                    freship.selected = idx;
                    self.active_prompt = Some(InTuiPrompt::Freship(freship));
                    self.dirty = true;
                    None
                }
            }
            MenuEvent::Select => {
                if let Some(selection) = freship.advance() {
                    let is_reconfigure = freship.is_reconfigure;
                    self.clear_notice();
                    self.dirty = true;
                    Some(TuiOutcome::PromptSubmitted(PromptSubmission::Freship {
                        selection,
                        is_reconfigure,
                    }))
                } else {
                    self.active_prompt = Some(InTuiPrompt::Freship(freship));
                    self.dirty = true;
                    None
                }
            }
            _ => {
                self.active_prompt = Some(InTuiPrompt::Freship(freship));
                None
            }
        }
    }

    fn handle_form_prompt_event(
        &mut self,
        event: MenuEvent,
        mut form: FormState,
    ) -> Option<TuiOutcome> {
        let count = form.fields.len();
        if count == 0 {
            return None;
        }
        let idx = form.focused_field.min(count - 1);
        match event {
            MenuEvent::Quit => Some(TuiOutcome::Exit),
            MenuEvent::FilterClear | MenuEvent::Back => {
                self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                None
            }
            MenuEvent::Up | MenuEvent::PrevTab => {
                form.focused_field = if idx == 0 { count - 1 } else { idx - 1 };
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::Down | MenuEvent::TogglePane | MenuEvent::NextTab => {
                form.focused_field = (idx + 1) % count;
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::ClickModalRow(row) if row < count => {
                form.focused_field = row;
                if form.fields[row].is_bool {
                    form.fields[row].bool_value = !form.fields[row].bool_value;
                }
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::FormToggle => {
                if form.fields[idx].is_bool {
                    form.fields[idx].bool_value = !form.fields[idx].bool_value;
                } else {
                    form.fields[idx].value.push(' ');
                }
                form.error_msg = None;
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::FilterChar(ch) => {
                if form.fields[idx].is_bool {
                    match ch {
                        ' ' => form.fields[idx].bool_value = !form.fields[idx].bool_value,
                        'y' | 'Y' => form.fields[idx].bool_value = true,
                        'n' | 'N' => form.fields[idx].bool_value = false,
                        _ => {}
                    }
                } else {
                    form.fields[idx].value.push(ch);
                }
                form.error_msg = None;
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::FilterBackspace => {
                if !form.fields[idx].is_bool {
                    form.fields[idx].value.pop();
                }
                form.error_msg = None;
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::FilterClearLine => {
                if !form.fields[idx].is_bool {
                    form.fields[idx].value.clear();
                }
                form.error_msg = None;
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::FilterDeleteWord => {
                if !form.fields[idx].is_bool {
                    while form.fields[idx]
                        .value
                        .chars()
                        .last()
                        .is_some_and(char::is_whitespace)
                    {
                        form.fields[idx].value.pop();
                    }
                    while form.fields[idx]
                        .value
                        .chars()
                        .last()
                        .is_some_and(|c| !c.is_whitespace())
                    {
                        form.fields[idx].value.pop();
                    }
                }
                form.error_msg = None;
                self.active_prompt = Some(InTuiPrompt::Form(form));
                self.dirty = true;
                None
            }
            MenuEvent::Select => {
                if idx + 1 < count {
                    form.focused_field = idx + 1;
                    self.active_prompt = Some(InTuiPrompt::Form(form));
                    self.dirty = true;
                    None
                } else {
                    match form.validate_and_build_lego_spec() {
                        Ok(spec) => {
                            self.clear_notice();
                            self.dirty = true;
                            Some(TuiOutcome::PromptSubmitted(
                                PromptSubmission::LegoAddDomain(spec),
                            ))
                        }
                        Err(_) => {
                            self.active_prompt = Some(InTuiPrompt::Form(form));
                            self.dirty = true;
                            None
                        }
                    }
                }
            }
            _ => {
                self.active_prompt = Some(InTuiPrompt::Form(form));
                None
            }
        }
    }

    fn handle_confirm_event(
        &mut self,
        event: MenuEvent,
        prompt: ConfirmPrompt,
    ) -> Option<TuiOutcome> {
        let emit_confirmed = |p: ConfirmPrompt| -> TuiOutcome {
            if let Some(elevated) = p.elevation_outcome {
                TuiOutcome::ElevatedApproved(elevated)
            } else if let Some(menu_act) = p.menu_action {
                TuiOutcome::RunMenuAction(menu_act)
            } else {
                TuiOutcome::Execute(p.action)
            }
        };
        match event {
            MenuEvent::ConfirmYes => {
                self.pending_confirm = None;
                self.clear_notice();
                self.dirty = true;
                Some(emit_confirmed(prompt))
            }
            MenuEvent::ConfirmNo | MenuEvent::Back => {
                self.pending_confirm = None;
                self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                None
            }
            MenuEvent::PrevTab | MenuEvent::NextTab | MenuEvent::TogglePane => {
                if let Some(ref mut current) = self.pending_confirm {
                    current.selected_yes = !current.selected_yes;
                    self.dirty = true;
                }
                None
            }
            MenuEvent::Select => {
                if prompt.selected_yes {
                    self.pending_confirm = None;
                    self.clear_notice();
                    self.dirty = true;
                    Some(emit_confirmed(prompt))
                } else {
                    self.pending_confirm = None;
                    self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                    None
                }
            }
            MenuEvent::Quit => {
                self.pending_confirm = None;
                Some(TuiOutcome::Exit)
            }
            _ => None,
        }
    }

    fn handle_digit(&mut self, screen: &MenuScreen, digit: u8) -> Option<TuiOutcome> {
        if let Some(prefix) = self.digit_prefix.take() {
            let combined = prefix.saturating_mul(10).saturating_add(digit);
            self.dirty = true;
            return self.activate_by_id(screen, combined);
        }

        let max_id = screen.max_item_id();
        // If `digit` can be the tens digit of a valid 2-digit menu ID (e.g. `1` for 10/11/12),
        // buffer it and highlight the single-digit match until Enter or the second digit arrives.
        if digit > 0 && digit.saturating_mul(10) <= max_id {
            if let Some(pos) = screen.items().iter().position(|item| item.id == digit) {
                self.selected_idx = pos;
            }
            self.digit_prefix = Some(digit);
            self.dirty = true;
            return None;
        }

        self.activate_by_id(screen, digit)
    }

    fn activate_by_id(&mut self, screen: &MenuScreen, target_id: u8) -> Option<TuiOutcome> {
        let items = screen.items();
        if let Some((pos, item)) = items
            .iter()
            .enumerate()
            .find(|(_, item)| item.id == target_id)
        {
            self.selected_idx = pos;
            self.dirty = true;
            self.apply_action(item.action.clone())
        } else {
            self.set_notice_with_kind(NoticeKind::Danger, format!("无效编号: {target_id}"));
            None
        }
    }

    fn apply_action(&mut self, action: MenuAction) -> Option<TuiOutcome> {
        if self.is_task_running()
            && !matches!(
                action,
                MenuAction::Navigate(_) | MenuAction::Back | MenuAction::Exit
            )
        {
            self.set_notice_with_kind(NoticeKind::Warning, "当前有任务正在执行，请等待完成");
            return None;
        }

        match action {
            MenuAction::Navigate(next) => {
                self.menu_stack.push(next);
                self.selected_idx = 0;
                self.menu_viewport_offset = 0;
                self.digit_prefix = None;
                self.filter_active = false;
                self.filter_query.clear();
                self.start_transition(TransitionKind::MenuPush);
                self.dirty = true;
                None
            }
            MenuAction::Execute(BashAction::InstallApp(AppId::Yazi)) if self.native_prompts => {
                self.open_yazi_install_prompt();
                None
            }
            MenuAction::Execute(bash_action) => Some(TuiOutcome::Execute(bash_action)),
            MenuAction::ExecuteWithConfirm {
                action: BashAction::UninstallApp(AppId::Podman),
                ..
            } if self.native_prompts => {
                self.open_podman_uninstall_prompt();
                None
            }
            MenuAction::ExecuteWithConfirm {
                action: BashAction::ScriptUninstall,
                ..
            } if self.native_prompts => {
                self.open_script_uninstall_prompt();
                None
            }
            MenuAction::ExecuteWithConfirm { action, prompt } => {
                self.pending_confirm = Some(ConfirmPrompt {
                    message: prompt,
                    action,
                    menu_action: None,
                    selected_yes: false,
                    elevation_outcome: None,
                });
                self.start_transition(TransitionKind::ModalOpen);
                self.dirty = true;
                None
            }
            MenuAction::NativeF2bSync => Some(TuiOutcome::RunNativeF2bSync),
            MenuAction::NativeReconcile => Some(TuiOutcome::RunNativeReconcile),
            MenuAction::LegoAddDomain if self.native_prompts => {
                self.open_lego_add_domain_form();
                None
            }
            MenuAction::LegoUninstall => {
                self.pending_confirm = Some(ConfirmPrompt {
                    message: "移除 Lego 客户端及自动续期定时任务？".to_string(),
                    action: BashAction::ManageLego,
                    menu_action: Some(MenuAction::LegoUninstall),
                    selected_yes: false,
                    elevation_outcome: None,
                });
                self.start_transition(TransitionKind::ModalOpen);
                self.dirty = true;
                None
            }
            other @ (MenuAction::XrayToggleRuleset
            | MenuAction::XrayToggleCron
            | MenuAction::FreshipReconfigure
            | MenuAction::FreshipToggleTimers
            | MenuAction::FreshipSync
            | MenuAction::FreshipLogs
            | MenuAction::LegoInstall
            | MenuAction::LegoAddDomain
            | MenuAction::LegoToggleFlag
            | MenuAction::LegoRunOnce
            | MenuAction::LegoTroubleshoot
            | MenuAction::LegoRemoveDomain) => Some(TuiOutcome::RunMenuAction(other)),
            MenuAction::Back => self.go_back(),
            MenuAction::Exit => Some(TuiOutcome::Exit),
        }
    }

    fn go_back(&mut self) -> Option<TuiOutcome> {
        if self.menu_stack.len() > 1 {
            self.menu_stack.pop();
            self.selected_idx = 0;
            self.menu_viewport_offset = 0;
            self.filter_active = false;
            self.filter_query.clear();
            self.start_transition(TransitionKind::MenuPop);
            self.dirty = true;
            None
        } else {
            Some(TuiOutcome::Exit)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use debopty_bridge::{AppId, BashAction};
    use debopty_core::status::{StatusSnapshot, TRACKED_TOOLS};

    use super::*;

    fn test_status() -> StatusSnapshot {
        let mut tools = BTreeMap::new();
        for &tool in TRACKED_TOOLS {
            tools.insert(tool.to_string(), false);
        }
        StatusSnapshot {
            version_display: "2.0.0".to_string(),
            is_cn_region: false,
            base_optimized: true,
            ip_forward: false,
            podman_gc_enabled: false,
            tools,
        }
    }

    #[test]
    fn context_aware_prev_next_tab_switches_tabs_on_main_and_navigates_in_submenu() {
        let mut state = AppState::new(test_status());
        assert_eq!(state.workspace_tab(), WorkspaceTab::Overview);

        // On Main: NextTab / PrevTab cycles WorkspaceTab.
        assert_eq!(state.handle_event(MenuEvent::NextTab), None);
        assert_eq!(state.workspace_tab(), WorkspaceTab::SysSec);
        assert_eq!(state.handle_event(MenuEvent::PrevTab), None);
        assert_eq!(state.workspace_tab(), WorkspaceTab::Overview);

        // Enter Devops submenu (item 3).
        assert_eq!(state.handle_event(MenuEvent::Digit(3)), None);
        assert_eq!(state.current_menu(), MenuId::Devops);

        // Inside submenu: NextTab selects first item (Fish -> AppCrud(Fish)).
        assert_eq!(state.handle_event(MenuEvent::NextTab), None);
        assert_eq!(state.current_menu(), MenuId::AppCrud(AppId::Fish));

        // PrevTab pops back to Devops, then back to Main.
        assert_eq!(state.handle_event(MenuEvent::PrevTab), None);
        assert_eq!(state.current_menu(), MenuId::Devops);
        assert_eq!(state.handle_event(MenuEvent::PrevTab), None);
        assert_eq!(state.current_menu(), MenuId::Main);
    }

    #[test]
    fn mouse_events_handle_tabs_menu_click_activate_palette_focus_and_scroll() {
        let mut state = AppState::new(test_status());
        assert!(state.take_dirty());

        // Focus Telemetry and scroll it.
        assert_eq!(
            state.handle_event(MenuEvent::FocusPane(ActivePane::Telemetry)),
            None
        );
        assert_eq!(state.active_pane(), ActivePane::Telemetry);
        assert_eq!(state.handle_event(MenuEvent::ScrollTelemetry(3)), None);
        assert_eq!(state.telemetry_scroll(), 3);
        assert_eq!(state.handle_event(MenuEvent::ScrollTelemetry(-2)), None);
        assert_eq!(state.telemetry_scroll(), 1);

        // ClickMenuItem while unfocused on Menu focuses Menu & selects row without activating.
        assert_eq!(state.handle_event(MenuEvent::ClickMenuItem(1)), None);
        assert_eq!(state.active_pane(), ActivePane::Menu);
        assert_eq!(state.selected_idx(), 1);

        // Second click on already-selected focused row activates ToggleIpForwarding.
        assert_eq!(
            state.handle_event(MenuEvent::ClickMenuItem(1)),
            Some(TuiOutcome::Execute(BashAction::ToggleIpForwarding))
        );

        // ClickTab switches workspace tab.
        assert_eq!(
            state.handle_event(MenuEvent::ClickTab(WorkspaceTab::DevOps)),
            None
        );
        assert_eq!(state.workspace_tab(), WorkspaceTab::DevOps);

        // Palette click selects on first click and activates on second click.
        assert_eq!(state.handle_event(MenuEvent::PaletteOpen), None);
        assert!(state.palette_active());
        assert_eq!(state.handle_event(MenuEvent::ClickPaletteItem(1)), None);
        assert_eq!(state.palette_selected(), 1);
        assert_eq!(
            state.handle_event(MenuEvent::ClickPaletteItem(1)),
            Some(TuiOutcome::Execute(BashAction::ToggleIpForwarding))
        );
        assert!(!state.palette_active());
    }

    #[test]
    fn menu_action_vanity_purged_and_detail_keys_mapped() {
        assert_eq!(
            MenuAction::Execute(BashAction::RunBaseOptimization).kind_label(),
            "执行"
        );
        assert_eq!(MenuAction::NativeReconcile.kind_label(), "维护");
        assert_eq!(MenuAction::NativeF2bSync.kind_label(), "维护");
        let legacy = MenuAction::Execute(BashAction::LegacyFullEntry).inspector_meta();
        assert_eq!(legacy.unit, "debopty");
        assert_eq!(legacy.cli, "debopty");

        assert_eq!(
            MenuAction::Execute(BashAction::RunBaseOptimization).detail_key(),
            "base"
        );
        assert_eq!(
            MenuAction::Execute(BashAction::ToggleIpForwarding).detail_key(),
            "ip_forward"
        );
        assert_eq!(MenuAction::NativeF2bSync.detail_key(), "f2b_sync");
        assert_eq!(MenuAction::NativeReconcile.detail_key(), "reconcile");
        assert_eq!(
            MenuAction::Navigate(MenuId::AppCrud(AppId::Ferron)).detail_key(),
            "ferron"
        );
    }

    #[test]
    fn help_overlay_toggle_preview_and_dismiss() {
        let mut state = AppState::new(test_status());
        assert!(!state.help_active());

        assert_eq!(state.handle_event(MenuEvent::HelpToggle), None);
        assert!(state.help_active());
        // Navigation keys are absorbed while help overlay is active.
        assert_eq!(state.handle_event(MenuEvent::Down), None);
        assert_eq!(state.selected_idx(), 0);
        assert!(state.help_active());

        // Dismiss via Back (Esc / outside-click).
        assert_eq!(state.handle_event(MenuEvent::Back), None);
        assert!(!state.help_active());

        // Open via preview helper and toggle closed via HelpToggle.
        state.open_help_preview();
        assert!(state.help_active());
        assert_eq!(state.handle_event(MenuEvent::HelpToggle), None);
        assert!(!state.help_active());
    }

    #[test]
    fn u37_confirm_defaults_to_cancel_and_toggles_with_arrows_or_tab() {
        let mut state = AppState::new(test_status());
        state.open_confirm_preview(
            "移除 Podman 环境？",
            BashAction::UninstallApp(AppId::Podman),
        );
        assert!(
            !state.pending_confirm().expect("confirm open").selected_yes,
            "default confirm selection must be Cancel"
        );

        // Enter on default Cancel cancels without executing.
        assert_eq!(state.handle_event(MenuEvent::Select), None);
        assert!(state.pending_confirm().is_none());
        assert_eq!(state.notice(), Some("已取消"));

        // Toggle via PrevTab / TogglePane / NextTab, then Enter executes.
        state.open_confirm_preview(
            "移除 Podman 环境？",
            BashAction::UninstallApp(AppId::Podman),
        );
        assert_eq!(state.handle_event(MenuEvent::PrevTab), None);
        assert!(state.pending_confirm().expect("confirm open").selected_yes);
        assert_eq!(state.handle_event(MenuEvent::TogglePane), None);
        assert!(!state.pending_confirm().expect("confirm open").selected_yes);
        assert_eq!(state.handle_event(MenuEvent::NextTab), None);
        assert!(state.pending_confirm().expect("confirm open").selected_yes);
        assert_eq!(
            state.handle_event(MenuEvent::Select),
            Some(TuiOutcome::Execute(BashAction::UninstallApp(AppId::Podman)))
        );
        assert!(state.pending_confirm().is_none());

        // Direct y confirms, n/Esc cancels.
        state.open_confirm_preview(
            "移除 Podman 环境？",
            BashAction::UninstallApp(AppId::Podman),
        );
        assert_eq!(
            state.handle_event(MenuEvent::ConfirmYes),
            Some(TuiOutcome::Execute(BashAction::UninstallApp(AppId::Podman)))
        );
        state.open_confirm_preview(
            "移除 Podman 环境？",
            BashAction::UninstallApp(AppId::Podman),
        );
        assert_eq!(state.handle_event(MenuEvent::Back), None);
        assert!(state.pending_confirm().is_none());
    }

    #[test]
    fn elevation_confirm_defaults_to_yes_non_destructive_and_emits_approved() {
        let mut state = AppState::new(test_status());
        let inner = TuiOutcome::Execute(BashAction::RunBaseOptimization);
        state.open_elevation_confirm(inner.clone(), "基础系统优化");

        let prompt = state.pending_confirm().expect("elevation confirm open");
        assert!(prompt.selected_yes, "elevation confirm must default to Yes");
        assert!(
            !prompt.is_destructive(),
            "elevation confirm must not be styled as destructive"
        );
        assert!(prompt.message.contains("基础系统优化"));
        assert!(prompt.message.contains("sudo"));

        // Pressing Enter immediately confirms and emits ElevatedApproved(inner).
        assert_eq!(
            state.handle_event(MenuEvent::Select),
            Some(TuiOutcome::ElevatedApproved(Box::new(inner.clone())))
        );
        assert!(state.pending_confirm().is_none());

        // Cancelling via Back returns None and clears pending_confirm.
        state.open_elevation_confirm(inner.clone(), "基础系统优化");
        assert_eq!(state.handle_event(MenuEvent::Back), None);
        assert!(state.pending_confirm().is_none());

        // Direct ConfirmYes emits ElevatedApproved(inner).
        state.open_elevation_confirm(inner.clone(), "基础系统优化");
        assert_eq!(
            state.handle_event(MenuEvent::ConfirmYes),
            Some(TuiOutcome::ElevatedApproved(Box::new(inner)))
        );
        assert!(state.pending_confirm().is_none());
    }
}
