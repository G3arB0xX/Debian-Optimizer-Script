use debopty_bridge::BashAction;
use debopty_core::status::StatusSnapshot;

use crate::menu::{MenuAction, MenuId, MenuScreen, build_menu};
use crate::renderer::MenuEvent;

/// External outcome emitted by `AppState` to the caller's TUI event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiOutcome {
    Exit,
    Execute(BashAction),
    RunNativeF2bSync,
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

    pub fn set_notice(&mut self, msg: impl Into<String>) {
        self.notice = Some(msg.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub fn refresh_status(&mut self, status: StatusSnapshot) {
        self.status = status;
        let item_count = self.current_screen().items().len();
        if item_count == 0 {
            self.selected_idx = 0;
        } else if self.selected_idx >= item_count {
            self.selected_idx = item_count - 1;
        }
    }

    /// Processes one `MenuEvent`, updating internal navigation state and optionally
    /// emitting a `TuiOutcome` for the host loop to execute.
    pub fn handle_event(&mut self, event: MenuEvent) -> Option<TuiOutcome> {
        if let Some(prompt) = self.pending_confirm.clone() {
            return self.handle_confirm_event(event, prompt);
        }

        let screen = self.current_screen();
        let items = screen.items();
        if items.is_empty() {
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
                self.digit_prefix = None;
                self.notice = None;
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
                None
            }
            MenuEvent::Digit(d) => self.handle_digit(&screen, d),
            MenuEvent::ConfirmYes | MenuEvent::ConfirmNo => None,
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
            MenuAction::Back => self.go_back(),
            MenuAction::Exit => Some(TuiOutcome::Exit),
        }
    }

    fn go_back(&mut self) -> Option<TuiOutcome> {
        if self.menu_stack.len() > 1 {
            self.menu_stack.pop();
            self.selected_idx = 0;
            None
        } else {
            Some(TuiOutcome::Exit)
        }
    }
}
