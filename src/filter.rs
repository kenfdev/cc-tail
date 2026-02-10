//! Filter system for log entries.
//!
//! Provides a simple [`FilterState`] that controls two independent
//! filter dimensions:
//!
//! 1. **Tool call hiding** (`hide_tool_calls`): When true, tool call
//!    lines (`RenderedLine::ToolUse`) are hidden at the rendering level.
//! 2. **Agent filtering** (`selected_agent`): When `Some(id)`, only
//!    entries from the specified subagent are shown. When `None`, all
//!    agents (main + subagents) are shown.
//!
//! Entry-level filtering is done via `matches()` (agent filtering).
//! Line-level filtering (tool call hiding) is done in the UI renderer.

use crate::log_entry::LogEntry;

// ---------------------------------------------------------------------------
// FilterState
// ---------------------------------------------------------------------------

/// Simple filter state with two dimensions.
///
/// Stored in `App` and updated by the filter menu overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterState {
    /// When true, `RenderedLine::ToolUse` lines are hidden during rendering.
    pub hide_tool_calls: bool,
    /// When `Some(agent_id)`, only entries from that subagent are shown.
    /// When `None`, all agents (main + subagents) are shown.
    pub selected_agent: Option<String>,
}

impl FilterState {
    /// Returns `true` if any filter dimension is active.
    pub fn is_active(&self) -> bool {
        self.hide_tool_calls || self.selected_agent.is_some()
    }

    /// Test whether a log entry passes the entry-level filter (agent filtering).
    ///
    /// If no agent filter is active (`selected_agent` is `None`), all entries pass.
    /// If an agent filter is active, only subagent entries matching the selected
    /// agent_id pass. Main agent entries are hidden when an agent filter is active.
    pub fn matches(&self, entry: &LogEntry) -> bool {
        match &self.selected_agent {
            None => true,
            Some(agent_id) => {
                // When agent filter is active, only show entries from that agent.
                let is_sidechain = entry.is_sidechain == Some(true);
                if is_sidechain {
                    entry.agent_id.as_deref() == Some(agent_id.as_str())
                } else {
                    // Main agent entries are hidden when filtering by specific agent
                    false
                }
            }
        }
    }

    /// Returns `true` if tool call lines should be rendered.
    pub fn is_tool_line_visible(&self) -> bool {
        !self.hide_tool_calls
    }

    /// Format the active filters for display in the status bar.
    ///
    /// Returns `None` if no filters are active.
    /// Returns e.g. `"[filter: no tools]"`, `"[filter: agent cook]"`,
    /// or `"[filter: no tools, agent cook]"`.
    pub fn display(&self) -> Option<String> {
        if !self.is_active() {
            return None;
        }

        let mut parts: Vec<String> = Vec::new();

        if self.hide_tool_calls {
            parts.push("no tools".to_string());
        }

        if let Some(ref agent_id) = self.selected_agent {
            parts.push(format!("agent {}", agent_id));
        }

        Some(format!("[filter: {}]", parts.join(", ")))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_entry::parse_jsonl_line;

    // -- Helpers ----------------------------------------------------------

    fn user_entry(text: &str) -> LogEntry {
        let json = format!(
            r#"{{
                "type": "user",
                "sessionId": "sess-001",
                "message": {{
                    "role": "user",
                    "content": [{{"type": "text", "text": "{}"}}]
                }}
            }}"#,
            text
        );
        parse_jsonl_line(&json).unwrap()
    }

    fn assistant_entry(text: &str) -> LogEntry {
        let json = format!(
            r#"{{
                "type": "assistant",
                "sessionId": "sess-001",
                "message": {{
                    "role": "assistant",
                    "content": [{{"type": "text", "text": "{}"}}]
                }}
            }}"#,
            text
        );
        parse_jsonl_line(&json).unwrap()
    }

    fn subagent_entry(text: &str, agent_id: &str) -> LogEntry {
        let json = format!(
            r#"{{
                "type": "assistant",
                "sessionId": "sess-001",
                "isSidechain": true,
                "agentId": "{}",
                "slug": "test-agent-slug",
                "message": {{
                    "role": "assistant",
                    "content": [{{"type": "text", "text": "{}"}}]
                }}
            }}"#,
            agent_id, text
        );
        parse_jsonl_line(&json).unwrap()
    }

    fn entry_no_message() -> LogEntry {
        let json = r#"{"type": "system", "sessionId": "sess-001"}"#;
        parse_jsonl_line(json).unwrap()
    }

    // -- FilterState default tests ----------------------------------------

    #[test]
    fn test_filter_state_default_is_inactive() {
        let state = FilterState::default();
        assert!(!state.is_active());
        assert!(!state.hide_tool_calls);
        assert!(state.selected_agent.is_none());
    }

    #[test]
    fn test_filter_state_default_matches_everything() {
        let state = FilterState::default();
        assert!(state.matches(&user_entry("anything")));
        assert!(state.matches(&assistant_entry("anything")));
        assert!(state.matches(&subagent_entry("anything", "abc")));
        assert!(state.matches(&entry_no_message()));
    }

    #[test]
    fn test_filter_state_default_tool_lines_visible() {
        let state = FilterState::default();
        assert!(state.is_tool_line_visible());
    }

    #[test]
    fn test_filter_state_default_display_none() {
        let state = FilterState::default();
        assert_eq!(state.display(), None);
    }

    // -- is_active tests --------------------------------------------------

    #[test]
    fn test_is_active_hide_tool_calls() {
        let state = FilterState {
            hide_tool_calls: true,
            selected_agent: None,
        };
        assert!(state.is_active());
    }

    #[test]
    fn test_is_active_selected_agent() {
        let state = FilterState {
            hide_tool_calls: false,
            selected_agent: Some("abc".to_string()),
        };
        assert!(state.is_active());
    }

    #[test]
    fn test_is_active_both() {
        let state = FilterState {
            hide_tool_calls: true,
            selected_agent: Some("abc".to_string()),
        };
        assert!(state.is_active());
    }

    // -- matches tests (agent filtering) ----------------------------------

    #[test]
    fn test_matches_no_filter_all_pass() {
        let state = FilterState::default();
        assert!(state.matches(&user_entry("test")));
        assert!(state.matches(&assistant_entry("test")));
        assert!(state.matches(&subagent_entry("test", "abc")));
    }

    #[test]
    fn test_matches_agent_filter_hides_main() {
        let state = FilterState {
            hide_tool_calls: false,
            selected_agent: Some("abc".to_string()),
        };
        // Main agent entries should be hidden
        assert!(!state.matches(&user_entry("test")));
        assert!(!state.matches(&assistant_entry("test")));
    }

    #[test]
    fn test_matches_agent_filter_shows_matching_subagent() {
        let state = FilterState {
            hide_tool_calls: false,
            selected_agent: Some("abc".to_string()),
        };
        assert!(state.matches(&subagent_entry("test", "abc")));
    }

    #[test]
    fn test_matches_agent_filter_hides_non_matching_subagent() {
        let state = FilterState {
            hide_tool_calls: false,
            selected_agent: Some("abc".to_string()),
        };
        assert!(!state.matches(&subagent_entry("test", "xyz")));
    }

    #[test]
    fn test_matches_hide_tool_calls_does_not_affect_entry_matching() {
        // hide_tool_calls is line-level, not entry-level
        let state = FilterState {
            hide_tool_calls: true,
            selected_agent: None,
        };
        assert!(state.matches(&user_entry("test")));
        assert!(state.matches(&assistant_entry("test")));
        assert!(state.matches(&subagent_entry("test", "abc")));
    }

    #[test]
    fn test_matches_entry_no_message_with_agent_filter() {
        let state = FilterState {
            hide_tool_calls: false,
            selected_agent: Some("abc".to_string()),
        };
        // System entries with no sidechain marker are treated as main agent
        assert!(!state.matches(&entry_no_message()));
    }

    // -- is_tool_line_visible tests ---------------------------------------

    #[test]
    fn test_tool_line_visible_when_not_hidden() {
        let state = FilterState {
            hide_tool_calls: false,
            selected_agent: None,
        };
        assert!(state.is_tool_line_visible());
    }

    #[test]
    fn test_tool_line_not_visible_when_hidden() {
        let state = FilterState {
            hide_tool_calls: true,
            selected_agent: None,
        };
        assert!(!state.is_tool_line_visible());
    }

    // -- display tests ----------------------------------------------------

    #[test]
    fn test_display_no_filters() {
        let state = FilterState::default();
        assert_eq!(state.display(), None);
    }

    #[test]
    fn test_display_hide_tool_calls_only() {
        let state = FilterState {
            hide_tool_calls: true,
            selected_agent: None,
        };
        assert_eq!(state.display(), Some("[filter: no tools]".to_string()));
    }

    #[test]
    fn test_display_agent_filter_only() {
        let state = FilterState {
            hide_tool_calls: false,
            selected_agent: Some("cook".to_string()),
        };
        assert_eq!(state.display(), Some("[filter: agent cook]".to_string()));
    }

    #[test]
    fn test_display_both_filters() {
        let state = FilterState {
            hide_tool_calls: true,
            selected_agent: Some("cook".to_string()),
        };
        assert_eq!(
            state.display(),
            Some("[filter: no tools, agent cook]".to_string())
        );
    }

    // -- Combined filter tests --------------------------------------------

    #[test]
    fn test_combined_agent_filter_and_hide_tool_calls() {
        let state = FilterState {
            hide_tool_calls: true,
            selected_agent: Some("abc".to_string()),
        };
        // Entry-level: agent filtering
        assert!(!state.matches(&user_entry("test")));
        assert!(state.matches(&subagent_entry("test", "abc")));
        assert!(!state.matches(&subagent_entry("test", "xyz")));
        // Line-level: tool call hiding
        assert!(!state.is_tool_line_visible());
        // Display
        assert_eq!(
            state.display(),
            Some("[filter: no tools, agent abc]".to_string())
        );
    }
}
