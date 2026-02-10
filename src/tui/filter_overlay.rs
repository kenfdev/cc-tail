//! Filter menu overlay for the TUI.
//!
//! Provides a simple menu-style overlay activated by `f` that lets the
//! user toggle tool call visibility and select an agent filter.
//! Changes are applied immediately on selection.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ---------------------------------------------------------------------------
// FilterMenuItem
// ---------------------------------------------------------------------------

/// An item in the filter menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterMenuItem {
    /// Toggle hide/show tool calls.
    ToolCallToggle,
    /// Show all agents (clear agent filter).
    AgentAll,
    /// Filter to a specific agent (agent_id, display_name).
    Agent(String, String),
}

// ---------------------------------------------------------------------------
// MenuAction
// ---------------------------------------------------------------------------

/// Result of handling a key event in the filter menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// The menu consumed the event; no further handling needed.
    Consumed,
    /// Close the menu without changes.
    Close,
    /// A selection was made; the caller should read the updated state.
    Selected,
}

// ---------------------------------------------------------------------------
// FilterMenuState
// ---------------------------------------------------------------------------

/// State for the filter menu overlay.
///
/// Opened when the user presses `f`. Contains a list of menu items
/// with the current selection index, plus the filter state that can
/// be read by the caller after a selection.
#[derive(Debug, Clone, Default)]
pub struct FilterMenuState {
    /// Whether the menu is currently visible.
    pub visible: bool,
    /// The list of menu items.
    pub items: Vec<FilterMenuItem>,
    /// Currently highlighted item index.
    pub selected: usize,
    /// Current tool call hide state (toggled in-place).
    pub hide_tool_calls: bool,
    /// Current selected agent filter (None = all agents).
    pub selected_agent: Option<String>,
}

// Default is derived (all fields default to false/0/None/empty).


impl FilterMenuState {
    /// Open the filter menu with the given known agents.
    ///
    /// Restores the current filter state into the menu fields.
    /// The menu always has at least one item (ToolCallToggle).
    /// Agent items are only shown when there are known subagents.
    pub fn open(
        &mut self,
        hide_tool_calls: bool,
        selected_agent: Option<String>,
        known_agents: Vec<(String, String)>, // (agent_id, display_name)
    ) {
        self.visible = true;
        self.hide_tool_calls = hide_tool_calls;
        self.selected_agent = selected_agent;
        self.selected = 0;

        // Build menu items
        self.items = vec![FilterMenuItem::ToolCallToggle];

        if !known_agents.is_empty() {
            self.items.push(FilterMenuItem::AgentAll);
            for (agent_id, display_name) in known_agents {
                self.items
                    .push(FilterMenuItem::Agent(agent_id, display_name));
            }
        }
    }

    /// Handle a key event while the menu is visible.
    pub fn on_key(&mut self, key: KeyEvent) -> MenuAction {
        // Ctrl+C always closes
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return MenuAction::Close;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('f') => MenuAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                MenuAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                MenuAction::Consumed
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate_selected();
                MenuAction::Selected
            }
            _ => MenuAction::Consumed,
        }
    }

    /// Move selection up.
    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    fn move_down(&mut self) {
        if !self.items.is_empty() && self.selected < self.items.len() - 1 {
            self.selected += 1;
        }
    }

    /// Activate the currently selected item.
    fn activate_selected(&mut self) {
        if self.items.is_empty() {
            return;
        }

        match &self.items[self.selected] {
            FilterMenuItem::ToolCallToggle => {
                self.hide_tool_calls = !self.hide_tool_calls;
            }
            FilterMenuItem::AgentAll => {
                self.selected_agent = None;
            }
            FilterMenuItem::Agent(agent_id, _) => {
                self.selected_agent = Some(agent_id.clone());
            }
        }
    }

    /// Get the display label for a menu item at the given index.
    pub fn item_label(&self, index: usize) -> String {
        match &self.items[index] {
            FilterMenuItem::ToolCallToggle => {
                let checkbox = if self.hide_tool_calls {
                    "[x]"
                } else {
                    "[ ]"
                };
                format!("{} Hide Tool Calls", checkbox)
            }
            FilterMenuItem::AgentAll => {
                let radio = if self.selected_agent.is_none() {
                    "(*)"
                } else {
                    "( )"
                };
                format!("{} All Agents", radio)
            }
            FilterMenuItem::Agent(agent_id, display_name) => {
                let radio =
                    if self.selected_agent.as_deref() == Some(agent_id.as_str()) {
                        "(*)"
                    } else {
                        "( )"
                    };
                format!("{} {}", radio, display_name)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helpers ----------------------------------------------------------

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn sample_agents() -> Vec<(String, String)> {
        vec![
            ("abc".to_string(), "cook".to_string()),
            ("xyz".to_string(), "baker".to_string()),
        ]
    }

    // -- Default state tests ----------------------------------------------

    #[test]
    fn test_default_state_not_visible() {
        let menu = FilterMenuState::default();
        assert!(!menu.visible);
        assert!(menu.items.is_empty());
        assert_eq!(menu.selected, 0);
        assert!(!menu.hide_tool_calls);
        assert!(menu.selected_agent.is_none());
    }

    // -- open tests -------------------------------------------------------

    #[test]
    fn test_open_sets_visible() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        assert!(menu.visible);
    }

    #[test]
    fn test_open_with_no_agents_has_one_item() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        // Only ToolCallToggle
        assert_eq!(menu.items.len(), 1);
        assert_eq!(menu.items[0], FilterMenuItem::ToolCallToggle);
    }

    #[test]
    fn test_open_with_agents_has_all_items() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());
        // ToolCallToggle + AgentAll + 2 agents = 4
        assert_eq!(menu.items.len(), 4);
        assert_eq!(menu.items[0], FilterMenuItem::ToolCallToggle);
        assert_eq!(menu.items[1], FilterMenuItem::AgentAll);
        assert_eq!(
            menu.items[2],
            FilterMenuItem::Agent("abc".to_string(), "cook".to_string())
        );
        assert_eq!(
            menu.items[3],
            FilterMenuItem::Agent("xyz".to_string(), "baker".to_string())
        );
    }

    #[test]
    fn test_open_restores_filter_state() {
        let mut menu = FilterMenuState::default();
        menu.open(true, Some("abc".to_string()), sample_agents());
        assert!(menu.hide_tool_calls);
        assert_eq!(menu.selected_agent, Some("abc".to_string()));
    }

    #[test]
    fn test_open_resets_selection_to_zero() {
        let mut menu = FilterMenuState::default();
        menu.selected = 5;
        menu.open(false, None, sample_agents());
        assert_eq!(menu.selected, 0);
    }

    // -- Navigation tests -------------------------------------------------

    #[test]
    fn test_move_down() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());
        assert_eq!(menu.selected, 0);

        assert_eq!(menu.on_key(key(KeyCode::Down)), MenuAction::Consumed);
        assert_eq!(menu.selected, 1);

        assert_eq!(menu.on_key(key(KeyCode::Down)), MenuAction::Consumed);
        assert_eq!(menu.selected, 2);

        assert_eq!(menu.on_key(key(KeyCode::Down)), MenuAction::Consumed);
        assert_eq!(menu.selected, 3);

        // At end, stays
        assert_eq!(menu.on_key(key(KeyCode::Down)), MenuAction::Consumed);
        assert_eq!(menu.selected, 3);
    }

    #[test]
    fn test_move_up() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());
        menu.selected = 3;

        assert_eq!(menu.on_key(key(KeyCode::Up)), MenuAction::Consumed);
        assert_eq!(menu.selected, 2);

        assert_eq!(menu.on_key(key(KeyCode::Up)), MenuAction::Consumed);
        assert_eq!(menu.selected, 1);

        assert_eq!(menu.on_key(key(KeyCode::Up)), MenuAction::Consumed);
        assert_eq!(menu.selected, 0);

        // At start, stays
        assert_eq!(menu.on_key(key(KeyCode::Up)), MenuAction::Consumed);
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn test_j_moves_down() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());
        assert_eq!(menu.on_key(char_key('j')), MenuAction::Consumed);
        assert_eq!(menu.selected, 1);
    }

    #[test]
    fn test_k_moves_up() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());
        menu.selected = 2;
        assert_eq!(menu.on_key(char_key('k')), MenuAction::Consumed);
        assert_eq!(menu.selected, 1);
    }

    // -- Close tests ------------------------------------------------------

    #[test]
    fn test_esc_closes() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        assert_eq!(menu.on_key(key(KeyCode::Esc)), MenuAction::Close);
    }

    #[test]
    fn test_f_closes() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        assert_eq!(menu.on_key(char_key('f')), MenuAction::Close);
    }

    #[test]
    fn test_ctrl_c_closes() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        assert_eq!(menu.on_key(ctrl_key('c')), MenuAction::Close);
    }

    // -- Selection / activation tests -------------------------------------

    #[test]
    fn test_enter_toggles_tool_calls() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        assert!(!menu.hide_tool_calls);

        // selected=0 is ToolCallToggle
        assert_eq!(menu.on_key(key(KeyCode::Enter)), MenuAction::Selected);
        assert!(menu.hide_tool_calls);

        // Toggle back
        assert_eq!(menu.on_key(key(KeyCode::Enter)), MenuAction::Selected);
        assert!(!menu.hide_tool_calls);
    }

    #[test]
    fn test_space_toggles_tool_calls() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);

        assert_eq!(menu.on_key(char_key(' ')), MenuAction::Selected);
        assert!(menu.hide_tool_calls);
    }

    #[test]
    fn test_enter_selects_agent_all() {
        let mut menu = FilterMenuState::default();
        menu.open(false, Some("abc".to_string()), sample_agents());
        menu.selected = 1; // AgentAll

        assert_eq!(menu.on_key(key(KeyCode::Enter)), MenuAction::Selected);
        assert!(menu.selected_agent.is_none());
    }

    #[test]
    fn test_enter_selects_specific_agent() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());
        menu.selected = 2; // Agent("abc", "cook")

        assert_eq!(menu.on_key(key(KeyCode::Enter)), MenuAction::Selected);
        assert_eq!(menu.selected_agent, Some("abc".to_string()));
    }

    #[test]
    fn test_agent_selection_is_mutually_exclusive() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());

        // Select agent "abc"
        menu.selected = 2;
        menu.on_key(key(KeyCode::Enter));
        assert_eq!(menu.selected_agent, Some("abc".to_string()));

        // Select agent "xyz"
        menu.selected = 3;
        menu.on_key(key(KeyCode::Enter));
        assert_eq!(menu.selected_agent, Some("xyz".to_string()));

        // Select "All Agents"
        menu.selected = 1;
        menu.on_key(key(KeyCode::Enter));
        assert!(menu.selected_agent.is_none());
    }

    // -- item_label tests -------------------------------------------------

    #[test]
    fn test_item_label_tool_call_toggle_off() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        assert_eq!(menu.item_label(0), "[ ] Hide Tool Calls");
    }

    #[test]
    fn test_item_label_tool_call_toggle_on() {
        let mut menu = FilterMenuState::default();
        menu.open(true, None, vec![]);
        assert_eq!(menu.item_label(0), "[x] Hide Tool Calls");
    }

    #[test]
    fn test_item_label_agent_all_selected() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, sample_agents());
        assert_eq!(menu.item_label(1), "(*) All Agents");
    }

    #[test]
    fn test_item_label_agent_all_not_selected() {
        let mut menu = FilterMenuState::default();
        menu.open(false, Some("abc".to_string()), sample_agents());
        assert_eq!(menu.item_label(1), "( ) All Agents");
    }

    #[test]
    fn test_item_label_agent_selected() {
        let mut menu = FilterMenuState::default();
        menu.open(false, Some("abc".to_string()), sample_agents());
        assert_eq!(menu.item_label(2), "(*) cook");
    }

    #[test]
    fn test_item_label_agent_not_selected() {
        let mut menu = FilterMenuState::default();
        menu.open(false, Some("abc".to_string()), sample_agents());
        assert_eq!(menu.item_label(3), "( ) baker");
    }

    // -- Unknown key is consumed ------------------------------------------

    #[test]
    fn test_unknown_key_consumed() {
        let mut menu = FilterMenuState::default();
        menu.open(false, None, vec![]);
        assert_eq!(menu.on_key(char_key('z')), MenuAction::Consumed);
    }

    // -- Empty items edge case --------------------------------------------

    #[test]
    fn test_move_down_empty_items_noop() {
        let mut menu = FilterMenuState::default();
        // items is empty
        menu.move_down();
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn test_activate_empty_items_noop() {
        let mut menu = FilterMenuState::default();
        // items is empty - should not panic
        menu.activate_selected();
    }
}
