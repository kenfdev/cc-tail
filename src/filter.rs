//! Filter system for log entries.
//!
//! Provides a [`MessageFilter`] trait and concrete implementations for
//! filtering log entries by text content (regex), message role, and
//! agent identity. Filters can be composed with [`AndFilter`] for
//! AND semantics.
//!
//! The [`FilterState`] struct bundles user-configured filter settings
//! and builds the composed filter on demand.

use std::collections::HashSet;

use regex::Regex;

use crate::content_render::render_content_blocks;
use crate::log_entry::LogEntry;

// ---------------------------------------------------------------------------
// MessageFilter trait
// ---------------------------------------------------------------------------

/// Trait for filtering log entries.
///
/// Implementors decide whether a given `LogEntry` should be shown
/// (returns `true`) or hidden (returns `false`).
pub trait MessageFilter {
    /// Returns `true` if the entry should be displayed.
    fn matches(&self, entry: &LogEntry) -> bool;

    /// Human-readable description of this filter (for debugging / display).
    #[allow(dead_code)]
    fn description(&self) -> String;
}

// ---------------------------------------------------------------------------
// RegexFilter
// ---------------------------------------------------------------------------

/// Matches entries whose text content matches a compiled regex.
///
/// Extracts text from the entry's message content blocks using
/// [`render_content_blocks`] and checks whether any line matches
/// the pattern.
pub struct RegexFilter {
    regex: Regex,
}

impl RegexFilter {
    /// Create a new `RegexFilter` from a compiled `Regex`.
    pub fn new(regex: Regex) -> Self {
        Self { regex }
    }
}

impl MessageFilter for RegexFilter {
    fn matches(&self, entry: &LogEntry) -> bool {
        let Some(ref message) = entry.message else {
            return false;
        };

        let rendered = render_content_blocks(&message.content);
        for line in &rendered {
            let text = match line {
                crate::content_render::RenderedLine::Text(t) => t.as_str(),
                crate::content_render::RenderedLine::ToolUse(t) => t.as_str(),
                crate::content_render::RenderedLine::Unknown(t) => t.as_str(),
            };
            if self.regex.is_match(text) {
                return true;
            }
        }
        false
    }

    fn description(&self) -> String {
        format!("regex:{}", self.regex.as_str())
    }
}

// ---------------------------------------------------------------------------
// RoleFilter
// ---------------------------------------------------------------------------

/// Matches entries whose message role is in the allowed set.
///
/// For example, allowing only `{"user", "assistant"}` will hide
/// entries with role `"system"` or entries without a role.
pub struct RoleFilter {
    allowed_roles: HashSet<String>,
}

impl RoleFilter {
    /// Create a new `RoleFilter` from a set of allowed roles.
    pub fn new(allowed_roles: HashSet<String>) -> Self {
        Self { allowed_roles }
    }
}

impl MessageFilter for RoleFilter {
    fn matches(&self, entry: &LogEntry) -> bool {
        let role = entry
            .message
            .as_ref()
            .and_then(|m| m.role.as_deref())
            .unwrap_or("unknown");
        self.allowed_roles.contains(role)
    }

    fn description(&self) -> String {
        let roles: Vec<&str> = self.allowed_roles.iter().map(|s| s.as_str()).collect();
        format!("roles:{}", roles.join(","))
    }
}

// ---------------------------------------------------------------------------
// AgentFilter
// ---------------------------------------------------------------------------

/// Controls which agent entries are shown.
///
/// - `include_main`: if `true`, entries from the main agent (no agent_id)
///   are shown.
/// - `allowed_agents`: the set of agent IDs whose entries should be shown.
///   If empty and `include_main` is `true`, only main-agent entries pass.
pub struct AgentFilter {
    include_main: bool,
    allowed_agents: HashSet<String>,
}

impl AgentFilter {
    /// Create a new `AgentFilter`.
    pub fn new(include_main: bool, allowed_agents: HashSet<String>) -> Self {
        Self {
            include_main,
            allowed_agents,
        }
    }
}

impl MessageFilter for AgentFilter {
    fn matches(&self, entry: &LogEntry) -> bool {
        let is_main = entry.is_sidechain != Some(true);

        if is_main {
            return self.include_main;
        }

        // Subagent entry: check if the agent_id is in the allowed set.
        match &entry.agent_id {
            Some(agent_id) => self.allowed_agents.contains(agent_id.as_str()),
            None => {
                // Subagent with no agent_id -- show if we have no restrictions.
                self.allowed_agents.is_empty() && self.include_main
            }
        }
    }

    fn description(&self) -> String {
        let mut parts = Vec::new();
        if self.include_main {
            parts.push("main".to_string());
        }
        for agent in &self.allowed_agents {
            parts.push(agent.clone());
        }
        format!("agents:{}", parts.join(","))
    }
}

// ---------------------------------------------------------------------------
// AndFilter
// ---------------------------------------------------------------------------

/// Combines multiple filters with AND semantics.
///
/// An entry passes only if it passes **all** contained filters.
/// An empty `AndFilter` matches everything.
#[allow(dead_code)]
pub struct AndFilter {
    filters: Vec<Box<dyn MessageFilter>>,
}

impl AndFilter {
    /// Create a new `AndFilter` from a list of boxed filters.
    #[allow(dead_code)]
    pub fn new(filters: Vec<Box<dyn MessageFilter>>) -> Self {
        Self { filters }
    }
}

impl MessageFilter for AndFilter {
    fn matches(&self, entry: &LogEntry) -> bool {
        self.filters.iter().all(|f| f.matches(entry))
    }

    fn description(&self) -> String {
        let descs: Vec<String> = self.filters.iter().map(|f| f.description()).collect();
        descs.join(" AND ")
    }
}

// ---------------------------------------------------------------------------
// FilterState
// ---------------------------------------------------------------------------

/// Bundles the user-configured filter settings.
///
/// This struct is stored in `App` and updated by the filter overlay.
/// It provides a `matches()` method that efficiently tests a log entry
/// against the current filter configuration.
#[derive(Debug, Clone)]
pub struct FilterState {
    /// The regex pattern string entered by the user.
    pub pattern: String,
    /// Compiled regex (None if pattern is empty or invalid).
    compiled_regex: Option<Regex>,
    /// Whether the current pattern compiles to a valid regex.
    pub pattern_valid: bool,
    /// Enabled message roles. If empty, all roles pass.
    pub enabled_roles: HashSet<String>,
    /// Enabled agent IDs. If empty, all agents pass.
    pub enabled_agents: HashSet<String>,
    /// Whether main-agent entries are included.
    pub include_main: bool,
}

impl Default for FilterState {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            compiled_regex: None,
            pattern_valid: true,
            enabled_roles: HashSet::new(),
            enabled_agents: HashSet::new(),
            include_main: true,
        }
    }
}

impl FilterState {
    /// Returns `true` if no filters are active.
    ///
    /// When inactive, the log stream should render unfiltered.
    pub fn is_active(&self) -> bool {
        !self.pattern.is_empty()
            || !self.enabled_roles.is_empty()
            || !self.enabled_agents.is_empty()
    }

    /// Set the regex pattern, recompiling immediately.
    pub fn set_pattern(&mut self, pattern: &str) {
        self.pattern = pattern.to_string();
        if pattern.is_empty() {
            self.compiled_regex = None;
            self.pattern_valid = true;
        } else {
            match Regex::new(pattern) {
                Ok(re) => {
                    self.compiled_regex = Some(re);
                    self.pattern_valid = true;
                }
                Err(_) => {
                    self.compiled_regex = None;
                    self.pattern_valid = false;
                }
            }
        }
    }

    /// Build a composed filter from the current state.
    ///
    /// Returns `None` if no filters are active.
    #[allow(dead_code)]
    pub fn build_filter(&self) -> Option<Box<dyn MessageFilter>> {
        if !self.is_active() {
            return None;
        }

        let mut filters: Vec<Box<dyn MessageFilter>> = Vec::new();

        // Regex filter
        if let Some(ref re) = self.compiled_regex {
            filters.push(Box::new(RegexFilter::new(re.clone())));
        }

        // Role filter
        if !self.enabled_roles.is_empty() {
            filters.push(Box::new(RoleFilter::new(self.enabled_roles.clone())));
        }

        // Agent filter
        if !self.enabled_agents.is_empty() {
            filters.push(Box::new(AgentFilter::new(
                self.include_main,
                self.enabled_agents.clone(),
            )));
        }

        if filters.is_empty() {
            None
        } else if filters.len() == 1 {
            Some(filters.remove(0))
        } else {
            Some(Box::new(AndFilter::new(filters)))
        }
    }

    /// Test whether a log entry matches the current filter state.
    ///
    /// If no filters are active, returns `true` (show everything).
    pub fn matches(&self, entry: &LogEntry) -> bool {
        // Short-circuit: if no filters are active, everything matches.
        if !self.is_active() {
            return true;
        }

        // Regex filter
        if let Some(ref re) = self.compiled_regex {
            let regex_filter = RegexFilter::new(re.clone());
            if !regex_filter.matches(entry) {
                return false;
            }
        }

        // Role filter
        if !self.enabled_roles.is_empty() {
            let role_filter = RoleFilter::new(self.enabled_roles.clone());
            if !role_filter.matches(entry) {
                return false;
            }
        }

        // Agent filter
        if !self.enabled_agents.is_empty() {
            let agent_filter = AgentFilter::new(self.include_main, self.enabled_agents.clone());
            if !agent_filter.matches(entry) {
                return false;
            }
        }

        true
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

    // -- RegexFilter tests ------------------------------------------------

    #[test]
    fn test_regex_filter_matches_text() {
        let re = Regex::new("hello").unwrap();
        let filter = RegexFilter::new(re);

        assert!(filter.matches(&user_entry("hello world")));
        assert!(!filter.matches(&user_entry("goodbye world")));
    }

    #[test]
    fn test_regex_filter_case_sensitive() {
        let re = Regex::new("Hello").unwrap();
        let filter = RegexFilter::new(re);

        assert!(filter.matches(&user_entry("Hello world")));
        assert!(!filter.matches(&user_entry("hello world")));
    }

    #[test]
    fn test_regex_filter_case_insensitive() {
        let re = Regex::new("(?i)hello").unwrap();
        let filter = RegexFilter::new(re);

        assert!(filter.matches(&user_entry("Hello world")));
        assert!(filter.matches(&user_entry("HELLO world")));
    }

    #[test]
    fn test_regex_filter_no_message_returns_false() {
        let re = Regex::new("anything").unwrap();
        let filter = RegexFilter::new(re);

        assert!(!filter.matches(&entry_no_message()));
    }

    #[test]
    fn test_regex_filter_description() {
        let re = Regex::new("foo.*bar").unwrap();
        let filter = RegexFilter::new(re);
        assert_eq!(filter.description(), "regex:foo.*bar");
    }

    // -- RoleFilter tests -------------------------------------------------

    #[test]
    fn test_role_filter_matches_allowed_role() {
        let mut roles = HashSet::new();
        roles.insert("user".to_string());
        let filter = RoleFilter::new(roles);

        assert!(filter.matches(&user_entry("test")));
        assert!(!filter.matches(&assistant_entry("test")));
    }

    #[test]
    fn test_role_filter_multiple_roles() {
        let mut roles = HashSet::new();
        roles.insert("user".to_string());
        roles.insert("assistant".to_string());
        let filter = RoleFilter::new(roles);

        assert!(filter.matches(&user_entry("test")));
        assert!(filter.matches(&assistant_entry("test")));
    }

    #[test]
    fn test_role_filter_no_message_uses_unknown() {
        let mut roles = HashSet::new();
        roles.insert("unknown".to_string());
        let filter = RoleFilter::new(roles);

        assert!(filter.matches(&entry_no_message()));
    }

    #[test]
    fn test_role_filter_description() {
        let mut roles = HashSet::new();
        roles.insert("user".to_string());
        let filter = RoleFilter::new(roles);
        assert!(filter.description().starts_with("roles:"));
    }

    // -- AgentFilter tests ------------------------------------------------

    #[test]
    fn test_agent_filter_main_only() {
        let filter = AgentFilter::new(true, HashSet::new());

        // Main agent entries pass
        assert!(filter.matches(&user_entry("test")));
        // Subagent entries fail
        assert!(!filter.matches(&subagent_entry("test", "abc")));
    }

    #[test]
    fn test_agent_filter_specific_agent() {
        let mut agents = HashSet::new();
        agents.insert("abc".to_string());
        let filter = AgentFilter::new(false, agents);

        // Main agent fails
        assert!(!filter.matches(&user_entry("test")));
        // Matching subagent passes
        assert!(filter.matches(&subagent_entry("test", "abc")));
        // Non-matching subagent fails
        assert!(!filter.matches(&subagent_entry("test", "xyz")));
    }

    #[test]
    fn test_agent_filter_main_and_specific_agent() {
        let mut agents = HashSet::new();
        agents.insert("abc".to_string());
        let filter = AgentFilter::new(true, agents);

        assert!(filter.matches(&user_entry("test")));
        assert!(filter.matches(&subagent_entry("test", "abc")));
        assert!(!filter.matches(&subagent_entry("test", "xyz")));
    }

    #[test]
    fn test_agent_filter_description() {
        let mut agents = HashSet::new();
        agents.insert("abc".to_string());
        let filter = AgentFilter::new(true, agents);
        let desc = filter.description();
        assert!(desc.starts_with("agents:"));
        assert!(desc.contains("main"));
        assert!(desc.contains("abc"));
    }

    // -- AndFilter tests --------------------------------------------------

    #[test]
    fn test_and_filter_all_pass() {
        let re = Regex::new("hello").unwrap();
        let mut roles = HashSet::new();
        roles.insert("user".to_string());

        let filters: Vec<Box<dyn MessageFilter>> = vec![
            Box::new(RegexFilter::new(re)),
            Box::new(RoleFilter::new(roles)),
        ];
        let and_filter = AndFilter::new(filters);

        assert!(and_filter.matches(&user_entry("hello world")));
    }

    #[test]
    fn test_and_filter_one_fails() {
        let re = Regex::new("hello").unwrap();
        let mut roles = HashSet::new();
        roles.insert("assistant".to_string());

        let filters: Vec<Box<dyn MessageFilter>> = vec![
            Box::new(RegexFilter::new(re)),
            Box::new(RoleFilter::new(roles)),
        ];
        let and_filter = AndFilter::new(filters);

        // Text matches but role doesn't
        assert!(!and_filter.matches(&user_entry("hello world")));
    }

    #[test]
    fn test_and_filter_empty_matches_everything() {
        let and_filter = AndFilter::new(Vec::new());
        assert!(and_filter.matches(&user_entry("anything")));
    }

    #[test]
    fn test_and_filter_description() {
        let re = Regex::new("foo").unwrap();
        let mut roles = HashSet::new();
        roles.insert("user".to_string());

        let filters: Vec<Box<dyn MessageFilter>> = vec![
            Box::new(RegexFilter::new(re)),
            Box::new(RoleFilter::new(roles)),
        ];
        let and_filter = AndFilter::new(filters);
        let desc = and_filter.description();
        assert!(desc.contains("AND"));
    }

    // -- FilterState tests ------------------------------------------------

    #[test]
    fn test_filter_state_default_is_inactive() {
        let state = FilterState::default();
        assert!(!state.is_active());
        assert!(state.pattern_valid);
    }

    #[test]
    fn test_filter_state_set_pattern_valid() {
        let mut state = FilterState::default();
        state.set_pattern("hello");
        assert!(state.is_active());
        assert!(state.pattern_valid);
        assert!(state.compiled_regex.is_some());
    }

    #[test]
    fn test_filter_state_set_pattern_invalid() {
        let mut state = FilterState::default();
        state.set_pattern("[invalid");
        assert!(state.is_active()); // pattern is non-empty
        assert!(!state.pattern_valid);
        assert!(state.compiled_regex.is_none());
    }

    #[test]
    fn test_filter_state_set_pattern_empty() {
        let mut state = FilterState::default();
        state.set_pattern("hello");
        state.set_pattern("");
        assert!(!state.is_active());
        assert!(state.pattern_valid);
        assert!(state.compiled_regex.is_none());
    }

    #[test]
    fn test_filter_state_matches_no_filter() {
        let state = FilterState::default();
        assert!(state.matches(&user_entry("anything")));
    }

    #[test]
    fn test_filter_state_matches_with_regex() {
        let mut state = FilterState::default();
        state.set_pattern("error");

        assert!(state.matches(&user_entry("there was an error")));
        assert!(!state.matches(&user_entry("all good")));
    }

    #[test]
    fn test_filter_state_matches_with_role() {
        let mut state = FilterState::default();
        state.enabled_roles.insert("user".to_string());

        assert!(state.matches(&user_entry("test")));
        assert!(!state.matches(&assistant_entry("test")));
    }

    #[test]
    fn test_filter_state_matches_combined() {
        let mut state = FilterState::default();
        state.set_pattern("hello");
        state.enabled_roles.insert("user".to_string());

        // Both match
        assert!(state.matches(&user_entry("hello world")));
        // Regex matches but role doesn't
        assert!(!state.matches(&assistant_entry("hello world")));
        // Role matches but regex doesn't
        assert!(!state.matches(&user_entry("goodbye")));
    }

    #[test]
    fn test_filter_state_invalid_regex_matches_everything() {
        let mut state = FilterState::default();
        state.set_pattern("[invalid");
        // Invalid regex -> compiled_regex is None, so regex check is skipped
        // but pattern is non-empty so is_active() is true
        // Since compiled_regex is None, the regex check passes (no filter),
        // so all entries match
        assert!(state.matches(&user_entry("anything")));
    }

    #[test]
    fn test_filter_state_build_filter_none_when_inactive() {
        let state = FilterState::default();
        assert!(state.build_filter().is_none());
    }

    #[test]
    fn test_filter_state_build_filter_regex_only() {
        let mut state = FilterState::default();
        state.set_pattern("hello");

        let filter = state.build_filter();
        assert!(filter.is_some());

        let f = filter.unwrap();
        assert!(f.matches(&user_entry("hello world")));
        assert!(!f.matches(&user_entry("goodbye")));
    }

    #[test]
    fn test_filter_state_build_filter_combined() {
        let mut state = FilterState::default();
        state.set_pattern("hello");
        state.enabled_roles.insert("user".to_string());

        let filter = state.build_filter();
        assert!(filter.is_some());

        let f = filter.unwrap();
        assert!(f.matches(&user_entry("hello world")));
        assert!(!f.matches(&assistant_entry("hello world")));
        assert!(!f.matches(&user_entry("goodbye")));
    }

    #[test]
    fn test_filter_state_matches_with_agents() {
        let mut state = FilterState::default();
        state.enabled_agents.insert("abc".to_string());
        state.include_main = true;

        assert!(state.matches(&user_entry("test")));
        assert!(state.matches(&subagent_entry("test", "abc")));
        assert!(!state.matches(&subagent_entry("test", "xyz")));
    }

    #[test]
    fn test_filter_state_matches_agents_no_main() {
        let mut state = FilterState::default();
        state.enabled_agents.insert("abc".to_string());
        state.include_main = false;

        assert!(!state.matches(&user_entry("test")));
        assert!(state.matches(&subagent_entry("test", "abc")));
    }
}
