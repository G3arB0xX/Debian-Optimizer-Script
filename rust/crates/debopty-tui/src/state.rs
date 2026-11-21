use debopty_bridge::BashAction;
use debopty_core::status::StatusSnapshot;

use crate::menu::{MenuAction, MenuId, MenuItem, MenuScreen, build_menu};
use crate::renderer::MenuEvent;

/// Active focus pane in split-pane dashboards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePane {
    #[default]
    Menu,
    Telemetry,
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

/// Pure navigation and selection state machine for the debopty TUI.
#[derive(Debug, Clone)]
pub struct AppState {
    pub status: StatusSnapshot,
    menu_stack: Vec<MenuId>,
    selected_idx: usize,
    digit_prefix: Option<u8>,
    pending_confirm: Option<ConfirmPrompt>,
    notice: Option<String>,
    active_pane: ActivePane,
    filter_active: bool,
    filter_query: String,
    tick_count: u64,
    transition_ticks: u8,
    animations_enabled: bool,
}

impl AppState {
    pub fn new(status: StatusSnapshot) -> Self {
        Self {
            status,
            menu_stack: vec![MenuId::Main],
            selected_idx: 0,
            digit_prefix: None,
            pending_confirm: None,
            notice: None,
            active_pane: ActivePane::Menu,
            filter_active: false,
            filter_query: String::new(),
            tick_count: 0,
            transition_ticks: 0,
            animations_enabled: false,
        }
    }

    pub fn current_menu(&self) -> MenuId {
        self.menu_stack.last().copied().unwrap_or(MenuId::Main)
    }

    pub fn current_screen(&self) -> MenuScreen {
        build_menu(self.current_menu(), &self.status)
    }

    pub fn selected_idx(&self) -> usize {
        self.selected_idx
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

    pub fn active_pane(&self) -> ActivePane {
        self.active_pane
    }

    pub fn filter_active(&self) -> bool {
        self.filter_active
    }

    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    pub fn transition_ticks(&self) -> u8 {
        self.transition_ticks
    }

    pub fn animations_enabled(&self) -> bool {
        self.animations_enabled
    }

    pub fn set_animations_enabled(&mut self, enabled: bool) {
        self.animations_enabled = enabled;
        if !enabled {
            self.transition_ticks = 0;
        }
    }

    pub fn set_notice(&mut self, msg: impl Into<String>) {
        self.notice = Some(msg.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
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
    }

    /// Opens a confirmation modal directly (used by deterministic showcase preview rendering).
    pub fn open_confirm_preview(&mut self, message: impl Into<String>, action: BashAction) {
        self.pending_confirm = Some(ConfirmPrompt {
            message: message.into(),
            action,
        });
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
    }

    /// Processes one `MenuEvent`, updating internal navigation state and optionally
    /// emitting a `TuiOutcome` for the host loop to execute.
    pub fn handle_event(&mut self, event: MenuEvent) -> Option<TuiOutcome> {
        if let MenuEvent::Tick = event {
            if self.animations_enabled {
                self.tick_count = self.tick_count.wrapping_add(1);
                if self.transition_ticks > 0 {
                    self.transition_ticks -= 1;
                }
            }
            return None;
        }

        if let Some(prompt) = self.pending_confirm.clone() {
            return self.handle_confirm_event(event, prompt);
        }

        if self.filter_active {
            match event {
                MenuEvent::FilterChar(ch) => {
                    self.filter_query.push(ch);
                    self.selected_idx = 0;
                    return None;
                }
                MenuEvent::FilterBackspace => {
                    if self.filter_query.pop().is_none() {
                        self.filter_active = false;
                    }
                    self.selected_idx = 0;
                    return None;
                }
                MenuEvent::FilterClear | MenuEvent::Back => {
                    self.filter_active = false;
                    self.filter_query.clear();
                    self.selected_idx = 0;
                    return None;
                }
                MenuEvent::Select => {
                    let items = self.filtered_items();
                    self.filter_active = false;
                    if let Some(item) =
                        items.get(self.selected_idx.min(items.len().saturating_sub(1)))
                    {
                        let action = item.action.clone();
                        self.filter_query.clear();
                        return self.apply_action(action);
                    }
                    self.filter_query.clear();
                    return None;
                }
                MenuEvent::Up => {
                    let count = self.filtered_items().len();
                    if count > 0 {
                        self.selected_idx = if self.selected_idx == 0 {
                            count - 1
                        } else {
                            self.selected_idx - 1
                        };
                    }
                    return None;
                }
                MenuEvent::Down => {
                    let count = self.filtered_items().len();
                    if count > 0 {
                        self.selected_idx = (self.selected_idx + 1) % count;
                    }
                    return None;
                }
                MenuEvent::Quit => return Some(TuiOutcome::Exit),
                _ => return None,
            }
        }

        let screen = self.current_screen();
        let items = self.filtered_items();
        if items.is_empty() {
            if matches!(event, MenuEvent::Back | MenuEvent::FilterClear)
                && !self.filter_query.is_empty()
            {
                self.filter_query.clear();
                self.selected_idx = 0;
                return None;
            }
            return matches!(event, MenuEvent::Quit | MenuEvent::Back).then_some(TuiOutcome::Exit);
        }

        match event {
            MenuEvent::Up => {
                self.digit_prefix = None;
                self.notice = None;
                self.selected_idx = if self.selected_idx == 0 {
                    items.len() - 1
                } else {
                    self.selected_idx - 1
                };
                None
            }
            MenuEvent::Down => {
                self.digit_prefix = None;
                self.notice = None;
                self.selected_idx = (self.selected_idx + 1) % items.len();
                None
            }
            MenuEvent::Select => {
                self.notice = None;
                if let Some(prefix) = self.digit_prefix.take() {
                    return self.activate_by_id(&screen, prefix);
                }
                let action = items[self.selected_idx.min(items.len() - 1)].action.clone();
                self.apply_action(action)
            }
            MenuEvent::Back => {
                self.notice = None;
                if self.digit_prefix.take().is_some() {
                    return None;
                }
                if !self.filter_query.is_empty() {
                    self.filter_query.clear();
                    self.selected_idx = 0;
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
                self.notice = Some("已刷新状态显示。".to_string());
                Some(TuiOutcome::Refresh)
            }
            MenuEvent::FilterStart => {
                self.digit_prefix = None;
                self.notice = None;
                self.filter_active = true;
                self.selected_idx = 0;
                None
            }
            MenuEvent::FilterClear => {
                self.filter_active = false;
                self.filter_query.clear();
                self.selected_idx = 0;
                None
            }
            MenuEvent::TogglePane => {
                self.active_pane = match self.active_pane {
                    ActivePane::Menu => ActivePane::Telemetry,
                    ActivePane::Telemetry => ActivePane::Menu,
                };
                None
            }
            MenuEvent::Resize
            | MenuEvent::ConfirmYes
            | MenuEvent::ConfirmNo
            | MenuEvent::FilterChar(_)
            | MenuEvent::FilterBackspace
            | MenuEvent::Tick => None,
            MenuEvent::Digit(d) => self.handle_digit(&screen, d),
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
                self.notice = None;
                Some(TuiOutcome::Execute(prompt.action))
            }
            MenuEvent::ConfirmNo | MenuEvent::Back | MenuEvent::Select => {
                self.pending_confirm = None;
                self.notice = Some("已取消卸载操作。".to_string());
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
        self.notice = None;
        if let Some(prefix) = self.digit_prefix.take() {
            let combined = prefix.saturating_mul(10).saturating_add(digit);
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
            self.apply_action(item.action.clone())
        } else {
            self.notice = Some(format!("无效指令: {target_id}"));
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
                if self.animations_enabled {
                    self.transition_ticks = 4;
                }
                None
            }
            MenuAction::Execute(bash_action) => Some(TuiOutcome::Execute(bash_action)),
            MenuAction::ExecuteWithConfirm { action, prompt } => {
                self.pending_confirm = Some(ConfirmPrompt {
                    message: prompt,
                    action,
                });
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
            if self.animations_enabled {
                self.transition_ticks = 3;
            }
            None
        } else {
            Some(TuiOutcome::Exit)
        }
    }
}
