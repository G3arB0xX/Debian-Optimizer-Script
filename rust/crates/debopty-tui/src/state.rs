use std::path::Path;
use std::time::{Duration, Instant};

use debopty_bridge::BashAction;
use debopty_core::status::StatusSnapshot;
use debopty_core::telemetry::{
    SystemSpecs, TelemetryCollector, TelemetryHistory, probe_system_specs,
};

use crate::menu::{
    MenuAction, MenuId, MenuItem, MenuScreen, PaletteEntry, WorkspaceTab, build_menu,
    build_palette_entries, build_workspace_menu,
};
use crate::motion::{
    MotionPolicy, MotionTransition, TOAST_TTL_DURATION, TransitionKind, WakeKind, WakeSchedule,
};
use crate::renderer::MenuEvent;

/// Legacy compatibility constant: default toast lifetime in ~30fps ticks (~3.5s).
pub const DEFAULT_TOAST_TTL_TICKS: u16 = 105;
/// Legacy compatibility constant: canonical transition ticks.
pub const TRANSITION_TOTAL_TICKS: u8 = 8;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiOutcome {
    Exit,
    Refresh,
    Execute(BashAction),
    RunNativeF2bSync,
    RunNativeReconcile,
}

/// Pending destructive-action confirmation prompt (`[y/N]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPrompt {
    pub message: String,
    pub action: BashAction,
}

impl ConfirmPrompt {
    /// Returns `true` when this prompt guards a destructive/irreversible action.
    pub fn is_destructive(&self) -> bool {
        matches!(
            self.action,
            BashAction::UninstallApp(_) | BashAction::ScriptUninstall
        )
    }
}

/// Pure navigation, wall-clock motion, telemetry, and selection state machine for the debopty TUI.
#[derive(Debug, Clone)]
pub struct AppState {
    pub status: StatusSnapshot,
    menu_stack: Vec<MenuId>,
    workspace_tab: WorkspaceTab,
    selected_idx: usize,
    digit_prefix: Option<u8>,
    pending_confirm: Option<ConfirmPrompt>,
    notice: Option<String>,
    notice_kind: NoticeKind,
    notice_deadline: Option<Instant>,
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
        Self {
            status,
            menu_stack: vec![MenuId::Main],
            workspace_tab: WorkspaceTab::Overview,
            selected_idx: 0,
            digit_prefix: None,
            pending_confirm: None,
            notice: None,
            notice_kind: NoticeKind::Info,
            notice_deadline: None,
            active_pane: ActivePane::Menu,
            split_pane_available: true,
            telemetry_scroll: 0,
            filter_active: false,
            filter_query: String::new(),
            palette_active: false,
            palette_query: String::new(),
            palette_selected: 0,
            help_active: false,
            motion_policy: MotionPolicy::static_disabled(),
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

    /// Returns `true` whenever a text-entry mode (inline `/` filter or `Ctrl+K` palette) is active.
    pub fn is_text_input_active(&self) -> bool {
        self.filter_active || self.palette_active
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

        if let Some(tr) = self.active_transition
            && tr.preview_progress.is_none()
        {
            let timeout = if tr.is_finished_at(now) {
                Duration::ZERO
            } else {
                self.motion_policy.frame_interval.min(tr.remaining_at(now))
            };
            best = Some(WakeSchedule {
                timeout,
                kind: WakeKind::AnimFrame,
            });
        }

        if self.notice.is_some()
            && let Some(deadline) = self.notice_deadline
        {
            let timeout = deadline.saturating_duration_since(now);
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

    /// Advances wall-clock animations and toast expiry at `now`.
    /// Returns `true` and marks `dirty = true` ONLY when visible state actually changed.
    pub fn advance_clock_at(&mut self, now: Instant) -> bool {
        let mut changed = false;

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
            && now >= deadline
        {
            self.notice = None;
            self.notice_deadline = None;
            changed = true;
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
        self.notice = Some(msg.into());
        self.notice_kind = kind;
        self.notice_deadline = Some(Instant::now() + TOAST_TTL_DURATION);
        self.dirty = true;
    }

    pub fn clear_notice(&mut self) {
        if self.notice.take().is_some() {
            self.dirty = true;
        }
        self.notice_deadline = None;
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
        self.dirty = true;
    }

    /// Opens a confirmation modal directly (used by deterministic showcase preview rendering).
    pub fn open_confirm_preview(&mut self, message: impl Into<String>, action: BashAction) {
        self.pending_confirm = Some(ConfirmPrompt {
            message: message.into(),
            action,
        });
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

        // Handle direct mouse actions regardless of currently focused pane.
        match event {
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
                self.apply_telemetry_scroll_delta(delta);
                return None;
            }
            _ => {}
        }

        // Dedicated interactive Telemetry scroll mode when focused on Telemetry pane.
        if self.active_pane() == ActivePane::Telemetry {
            match event {
                MenuEvent::Up => {
                    self.apply_telemetry_scroll_delta(-1);
                    return None;
                }
                MenuEvent::Down => {
                    self.apply_telemetry_scroll_delta(1);
                    return None;
                }
                MenuEvent::PageUp => {
                    self.apply_telemetry_scroll_delta(-5);
                    return None;
                }
                MenuEvent::PageDown => {
                    self.apply_telemetry_scroll_delta(5);
                    return None;
                }
                MenuEvent::Home => {
                    self.set_telemetry_scroll(0);
                    return None;
                }
                MenuEvent::End => {
                    self.set_telemetry_scroll(64);
                    return None;
                }
                MenuEvent::TogglePane | MenuEvent::Back => {
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
                self.dirty = true;
                None
            }
            MenuEvent::FilterClear => {
                self.filter_active = false;
                self.filter_query.clear();
                self.selected_idx = 0;
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
            | MenuEvent::ScrollTelemetry(_) => None,
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

    fn handle_confirm_event(
        &mut self,
        event: MenuEvent,
        prompt: ConfirmPrompt,
    ) -> Option<TuiOutcome> {
        match event {
            MenuEvent::ConfirmYes => {
                self.pending_confirm = None;
                self.clear_notice();
                self.dirty = true;
                Some(TuiOutcome::Execute(prompt.action))
            }
            MenuEvent::ConfirmNo | MenuEvent::Back | MenuEvent::Select => {
                self.pending_confirm = None;
                self.set_notice_with_kind(NoticeKind::Warning, "已取消");
                None
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
        match action {
            MenuAction::Navigate(next) => {
                self.menu_stack.push(next);
                self.selected_idx = 0;
                self.digit_prefix = None;
                self.filter_active = false;
                self.filter_query.clear();
                self.start_transition(TransitionKind::MenuPush);
                self.dirty = true;
                None
            }
            MenuAction::Execute(bash_action) => Some(TuiOutcome::Execute(bash_action)),
            MenuAction::ExecuteWithConfirm { action, prompt } => {
                self.pending_confirm = Some(ConfirmPrompt {
                    message: prompt,
                    action,
                });
                self.start_transition(TransitionKind::ModalOpen);
                self.dirty = true;
                None
            }
            MenuAction::NativeF2bSync => Some(TuiOutcome::RunNativeF2bSync),
            MenuAction::NativeReconcile => Some(TuiOutcome::RunNativeReconcile),
            MenuAction::Back => self.go_back(),
            MenuAction::Exit => Some(TuiOutcome::Exit),
        }
    }

    fn go_back(&mut self) -> Option<TuiOutcome> {
        if self.menu_stack.len() > 1 {
            self.menu_stack.pop();
            self.selected_idx = 0;
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
}
