//! Application state for the TUI.
//!
//! The [`App`] struct owns all mutable state that drives the TUI:
//! focus tracking, sidebar visibility, quit flag, sessions list,
//! config, and the ring buffer of log entries.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::config::AppConfig;
use crate::filter::FilterState;
use crate::replay::{replay_session, DEFAULT_REPLAY_COUNT};
use crate::ring_buffer::RingBuffer;
use crate::session::Session;
use crate::theme::ThemeColors;
use crate::tmux::TmuxManager;
use crate::tui::filter_overlay::{FilterOverlayState, OverlayAction};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ---------------------------------------------------------------------------
// ActiveFilters
// ---------------------------------------------------------------------------

/// Represents the currently active filters in the TUI.
///
/// Forward-compatible interface for Task #13 (Filter System & Overlay).
/// The status bar reads this to display active filter information.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveFilters {
    /// The current text/regex filter pattern, if any.
    pub pattern: Option<String>,
    /// The log level filter (e.g. "user", "assistant"), if any.
    pub level: Option<String>,
}

impl ActiveFilters {
    /// Returns `true` if no filters are currently active.
    pub fn is_empty(&self) -> bool {
        self.pattern.is_none() && self.level.is_none()
    }

    /// Format the active filters for display in the status bar.
    ///
    /// Returns `None` if no filters are active.
    /// Returns e.g. `"filter:foo"`, `"level:user"`, or `"filter:foo level:user"`.
    pub fn display(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(ref p) = self.pattern {
            parts.push(format!("filter:{}", p));
        }
        if let Some(ref l) = self.level {
            parts.push(format!("level:{}", l));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }

    /// Format the active filters for display, truncating to fit within
    /// `max_width` characters. Appends "..." if truncated.
    ///
    /// Returns `None` if no filters are active or `max_width` is too small
    /// to display anything meaningful (< 4 characters).
    pub fn display_truncated(&self, max_width: usize) -> Option<String> {
        let full = self.display()?;
        if full.len() <= max_width {
            Some(full)
        } else if max_width < 4 {
            // Too narrow to show even "f..."
            None
        } else {
            Some(format!("{}...", &full[..max_width - 3]))
        }
    }
}

// ---------------------------------------------------------------------------
// Focus enum
// ---------------------------------------------------------------------------

/// Which panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    LogStream,
}

// ---------------------------------------------------------------------------
// App struct
// ---------------------------------------------------------------------------

/// Root application state.
///
/// Single-owner, never shared across threads. The event loop owns the
/// `App` and passes a `&mut` reference to key handlers and the draw
/// function.
pub struct App {
    /// Which panel has keyboard focus.
    pub focus: Focus,
    /// Whether the sidebar panel is visible.
    pub sidebar_visible: bool,
    /// Set to `true` to exit the event loop.
    pub should_quit: bool,
    /// Effective application configuration.
    pub config: AppConfig,
    /// Resolved theme colors derived from `config.theme`.
    pub theme_colors: ThemeColors,
    /// Byte-budgeted ring buffer holding parsed log entries.
    pub ring_buffer: RingBuffer,
    /// Discovered sessions, sorted by last_modified descending.
    pub sessions: Vec<Session>,
    /// Index of the currently selected session in the sidebar.
    pub selected_session_index: usize,
    /// Session IDs of newly appeared sessions (highlighted in the sidebar).
    /// Cleared when the user selects a session with Enter.
    pub new_session_ids: HashSet<String>,
    /// Scroll offset for the sidebar list (number of visual rows scrolled).
    pub sidebar_scroll_offset: usize,
    /// The session ID that is currently being tailed / active in the log stream.
    /// Set when the user presses Enter on a session.
    pub active_session_id: Option<String>,
    /// Currently active filters (forward-compatible for Task #13).
    pub active_filters: ActiveFilters,
    /// The current filter state used for filtering log entries.
    pub filter_state: FilterState,
    /// State for the filter overlay modal (opened with `/`).
    pub filter_overlay: FilterOverlayState,
    /// Per-file EOF offsets from the last replay, used to hand off to the
    /// watcher so it starts tailing from where replay left off.
    pub replay_offsets: HashMap<PathBuf, u64>,
    /// Whether progress-type entries are visible in the log stream.
    /// Toggled independently of `--verbose` via the `p` key.
    pub progress_visible: bool,
    /// tmux pane lifecycle manager.
    pub tmux_manager: TmuxManager,
    /// Transient status message shown in the status bar (e.g. tmux feedback).
    /// Cleared after a few ticks or on the next key press.
    pub status_message: Option<String>,
    /// Whether a quit confirmation is pending (shown when tmux panes are active).
    pub quit_confirm_pending: bool,
    /// The resolved project directory path used by the TUI.
    /// Needed to derive the tmux session name.
    pub project_path: Option<PathBuf>,
    /// Whether the help overlay is currently visible.
    pub help_overlay_visible: bool,
    /// Human-readable project name derived from the project path
    /// (e.g. last path component: `/Users/.../cc-tail` -> `"cc-tail"`).
    /// Shown in the status bar.
    pub project_display_name: Option<String>,
}

impl App {
    /// Create a new `App` with the given config.
    ///
    /// Starts with:
    /// - Focus on `LogStream`
    /// - Sidebar visible
    /// - `should_quit = false`
    /// - Default-budget ring buffer
    /// - Empty sessions list
    pub fn new(config: AppConfig) -> Self {
        let theme_colors = ThemeColors::from_theme(&config.theme);
        let tmux_layout = config.tmux.layout.clone();
        Self {
            focus: Focus::LogStream,
            sidebar_visible: true,
            should_quit: false,
            config,
            theme_colors,
            ring_buffer: RingBuffer::with_default_budget(),
            sessions: Vec::new(),
            selected_session_index: 0,
            new_session_ids: HashSet::new(),
            sidebar_scroll_offset: 0,
            active_session_id: None,
            active_filters: ActiveFilters::default(),
            filter_state: FilterState::default(),
            filter_overlay: FilterOverlayState::default(),
            replay_offsets: HashMap::new(),
            progress_visible: false,
            tmux_manager: TmuxManager::new(tmux_layout),
            status_message: None,
            quit_confirm_pending: false,
            project_path: None,
            help_overlay_visible: false,
            project_display_name: None,
        }
    }

    // -- Key handling --------------------------------------------------------

    /// Handle a key event, dispatching to the appropriate action.
    pub fn on_key(&mut self, key: KeyEvent) {
        // Clear transient status messages on any key press.
        self.status_message = None;

        // When the help overlay is visible, ANY key dismisses it.
        if self.help_overlay_visible {
            self.help_overlay_visible = false;
            return;
        }

        // Ctrl+C always quits regardless of focus.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            // If overlay is visible, Ctrl+C cancels it instead of quitting.
            if self.filter_overlay.visible {
                self.filter_overlay.visible = false;
                return;
            }
            self.initiate_quit();
            return;
        }

        // Handle quit confirmation dialog.
        if self.quit_confirm_pending {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.tmux_manager.cleanup();
                    self.quit_confirm_pending = false;
                    self.should_quit = true;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.quit_confirm_pending = false;
                }
                _ => {} // Ignore other keys while confirmation is pending.
            }
            return;
        }

        // When the filter overlay is visible, delegate ALL key events to it.
        if self.filter_overlay.visible {
            let action = self.filter_overlay.on_key(key);
            match action {
                OverlayAction::Cancel => {
                    self.filter_overlay.visible = false;
                }
                OverlayAction::Apply => {
                    self.apply_filter();
                    self.filter_overlay.visible = false;
                }
                OverlayAction::Consumed => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.initiate_quit(),
            KeyCode::Char('?') => self.help_overlay_visible = true,
            KeyCode::Char('/') => self.open_filter_overlay(),
            KeyCode::Char('p') => self.toggle_progress_visible(),
            KeyCode::Char('t') => self.open_tmux_panes(),
            KeyCode::Tab => self.toggle_focus(),
            KeyCode::Char('b') => self.toggle_sidebar(),
            KeyCode::Up | KeyCode::Char('k') => self.select_prev_session(),
            KeyCode::Down | KeyCode::Char('j') => self.select_next_session(),
            KeyCode::Enter => self.confirm_session_selection(),
            _ => {}
        }
    }

    /// Initiate the quit process.
    ///
    /// If tmux panes are active, shows a confirmation prompt instead of
    /// quitting immediately. Otherwise, quits directly.
    fn initiate_quit(&mut self) {
        if self.tmux_manager.pane_count() > 0 {
            self.quit_confirm_pending = true;
        } else {
            self.should_quit = true;
        }
    }

    // -- tmux integration ----------------------------------------------------

    /// Handle the `t` key: spawn tmux panes for all agents in the active session.
    ///
    /// Checks whether we are inside tmux, whether a session is selected,
    /// and then delegates to `TmuxManager::spawn_session`.
    fn open_tmux_panes(&mut self) {
        use crate::tmux;

        // Must be inside tmux.
        if !tmux::is_inside_tmux() {
            self.status_message = Some("Not inside tmux (start tmux first)".to_string());
            return;
        }

        // Need a project path for session naming.
        let project_path = match &self.project_path {
            Some(p) => p.clone(),
            None => {
                self.status_message = Some("No project path detected".to_string());
                return;
            }
        };

        // Need an active session to know which agents to spawn panes for.
        let session_id = match &self.active_session_id {
            Some(id) => id.clone(),
            None => {
                self.status_message =
                    Some("Select a session first (Enter on sidebar)".to_string());
                return;
            }
        };

        // Find the active session.
        let session = match self.sessions.iter().find(|s| s.id == session_id) {
            Some(s) => s.clone(),
            None => {
                self.status_message = Some("Active session not found".to_string());
                return;
            }
        };

        // Build the list of (label, log_path) tuples for all agents.
        let agent_log_paths: Vec<(String, PathBuf)> = session
            .agents
            .iter()
            .map(|a| {
                let label = if a.is_main {
                    "main".to_string()
                } else {
                    a.slug
                        .as_deref()
                        .or(a.agent_id.as_deref())
                        .unwrap_or("agent")
                        .to_string()
                };
                (label, a.log_path.clone())
            })
            .collect();

        if agent_log_paths.is_empty() {
            self.status_message = Some("No agents to spawn panes for".to_string());
            return;
        }

        let prefix = &self.config.tmux.session_prefix;
        match self.tmux_manager.spawn_session(prefix, &project_path, &agent_log_paths) {
            Ok(count) => {
                self.status_message = Some(format!(
                    "tmux: spawned {} pane{}",
                    count,
                    if count == 1 { "" } else { "s" }
                ));
            }
            Err(e) => {
                self.status_message = Some(format!("tmux error: {}", e));
            }
        }
    }

    // -- Focus ---------------------------------------------------------------

    /// Toggle focus between Sidebar and LogStream.
    ///
    /// If the sidebar is hidden, focus stays on LogStream.
    pub fn toggle_focus(&mut self) {
        if !self.sidebar_visible {
            self.focus = Focus::LogStream;
            return;
        }

        self.focus = match self.focus {
            Focus::Sidebar => Focus::LogStream,
            Focus::LogStream => Focus::Sidebar,
        };
    }

    // -- Sidebar visibility --------------------------------------------------

    /// Toggle sidebar visibility.
    ///
    /// When the sidebar is hidden and focus was on it, focus auto-switches
    /// to LogStream.
    pub fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;

        if !self.sidebar_visible && self.focus == Focus::Sidebar {
            self.focus = Focus::LogStream;
        }
    }

    // -- Progress visibility ------------------------------------------------

    /// Toggle visibility of progress-type entries.
    ///
    /// Independent of the `--verbose` flag. When toggled, the UI re-renders
    /// to include or exclude progress entries from the log stream.
    pub fn toggle_progress_visible(&mut self) {
        self.progress_visible = !self.progress_visible;
    }

    // -- Session selection ---------------------------------------------------

    /// Move the session selection up by one.
    pub fn select_prev_session(&mut self) {
        if self.selected_session_index > 0 {
            self.selected_session_index -= 1;
        }
    }

    /// Move the session selection down by one.
    pub fn select_next_session(&mut self) {
        if !self.sessions.is_empty() && self.selected_session_index < self.sessions.len() - 1 {
            self.selected_session_index += 1;
        }
    }

    /// Confirm the currently selected session (Enter key).
    ///
    /// Sets the active session ID, removes it from the new-session highlight
    /// set, switches focus to the LogStream, and replays the session's
    /// recent messages into the ring buffer.
    pub fn confirm_session_selection(&mut self) {
        if self.sessions.is_empty() {
            return;
        }

        let idx = self.selected_session_index.min(self.sessions.len() - 1);
        let session = self.sessions[idx].clone();

        // Clear the new-session highlight for this session.
        self.new_session_ids.remove(&session.id);

        self.active_session_id = Some(session.id.clone());
        self.focus = Focus::LogStream;

        // Replay recent messages from the selected session.
        self.replay_session_entries(&session);
    }

    /// Perform session replay: read the last N visible messages from the
    /// given session's JSONL files and push them into the ring buffer.
    ///
    /// Clears the ring buffer before replaying. Stores the resulting EOF
    /// offsets in `self.replay_offsets` for watcher handoff.
    pub fn replay_session_entries(&mut self, session: &Session) {
        self.ring_buffer.clear();
        let (entries, offsets) = replay_session(
            session,
            &self.filter_state,
            DEFAULT_REPLAY_COUNT,
            self.config.verbose,
            self.progress_visible,
        );
        for entry in entries {
            self.ring_buffer.push(entry);
        }
        self.replay_offsets = offsets;
    }

    /// Push a single new log entry into the ring buffer.
    ///
    /// Called by the event loop when the watcher delivers a `NewLogEntry`.
    pub fn on_new_log_entry(&mut self, entry: crate::log_entry::LogEntry) {
        self.ring_buffer.push(entry);
    }

    /// Adjust the sidebar scroll offset so the selected session is visible.
    ///
    /// `visible_height` is the number of visual rows available in the sidebar
    /// inner area (excluding borders). Each session occupies 1 header row
    /// plus 1 row per agent child.
    pub fn adjust_sidebar_scroll(&mut self, visible_height: usize) {
        if self.sessions.is_empty() || visible_height == 0 {
            self.sidebar_scroll_offset = 0;
            return;
        }

        // Compute the visual row range for the selected session.
        let mut row = 0usize;
        let mut selected_start = 0usize;
        let mut selected_end = 0usize;

        for (i, session) in self.sessions.iter().enumerate() {
            let session_rows = 1 + session.agents.iter().filter(|a| !a.is_main).count();
            if i == self.selected_session_index {
                selected_start = row;
                selected_end = row + session_rows; // exclusive
                break;
            }
            row += session_rows;
        }

        // If the selected session starts before the scroll window, scroll up.
        if selected_start < self.sidebar_scroll_offset {
            self.sidebar_scroll_offset = selected_start;
        }

        // If the selected session ends after the scroll window, scroll down.
        if selected_end > self.sidebar_scroll_offset + visible_height {
            self.sidebar_scroll_offset = selected_end.saturating_sub(visible_height);
        }
    }

    // -- Filter overlay --------------------------------------------------

    /// Open the filter overlay, snapshotting known roles and agents
    /// from the ring buffer.
    fn open_filter_overlay(&mut self) {
        let known_roles = self.collect_known_roles();
        let known_agents = self.collect_known_agents();
        self.filter_overlay
            .open(&self.filter_state, known_roles, known_agents);
    }

    /// Apply the filter settings from the overlay to the app state.
    fn apply_filter(&mut self) {
        let new_state = self.filter_overlay.build_filter_state();

        // Update the ActiveFilters for the status bar display.
        self.active_filters = ActiveFilters {
            pattern: if new_state.pattern.is_empty() {
                None
            } else {
                Some(new_state.pattern.clone())
            },
            level: if new_state.enabled_roles.is_empty() {
                None
            } else {
                let roles: Vec<String> =
                    new_state.enabled_roles.iter().cloned().collect();
                Some(roles.join(","))
            },
        };

        self.filter_state = new_state;
    }

    /// Collect unique message roles from entries in the ring buffer.
    ///
    /// Returns a sorted list of role strings (e.g. `["assistant", "user"]`).
    pub fn collect_known_roles(&self) -> Vec<String> {
        let mut roles: HashSet<String> = HashSet::new();
        for entry in self.ring_buffer.iter() {
            if let Some(ref message) = entry.message {
                if let Some(ref role) = message.role {
                    roles.insert(role.clone());
                }
            }
        }
        let mut sorted: Vec<String> = roles.into_iter().collect();
        sorted.sort();
        sorted
    }

    /// Collect unique agent identifiers from entries in the ring buffer.
    ///
    /// Returns a list of `(agent_id, display_name)` tuples, sorted by
    /// display name. Only includes subagent entries (sidechain).
    pub fn collect_known_agents(&self) -> Vec<(String, String)> {
        let mut agents: HashSet<String> = HashSet::new();
        let mut agent_display: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for entry in self.ring_buffer.iter() {
            if entry.is_sidechain != Some(true) {
                continue;
            }
            if let Some(ref agent_id) = entry.agent_id {
                if agents.insert(agent_id.clone()) {
                    let display = entry
                        .slug
                        .as_deref()
                        .unwrap_or(agent_id.as_str())
                        .to_string();
                    agent_display.insert(agent_id.clone(), display);
                }
            }
        }

        let mut result: Vec<(String, String)> = agent_display.into_iter().collect();
        result.sort_by(|a, b| a.1.cmp(&b.1));
        result
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal AppConfig for testing.
    fn test_config() -> AppConfig {
        AppConfig::default()
    }

    /// Helper: build a dummy Session for testing.
    fn dummy_session(id: &str) -> Session {
        use crate::session::Agent;
        use std::path::PathBuf;
        use std::time::SystemTime;

        Session {
            id: id.to_string(),
            agents: vec![Agent {
                agent_id: None,
                slug: None,
                log_path: PathBuf::from(format!("/fake/{}.jsonl", id)),
                is_main: true,
            }],
            last_modified: SystemTime::now(),
        }
    }

    // -- App::new defaults ---------------------------------------------------

    #[test]
    fn test_new_defaults() {
        let app = App::new(test_config());
        assert_eq!(app.focus, Focus::LogStream);
        assert!(app.sidebar_visible);
        assert!(!app.should_quit);
        assert!(app.sessions.is_empty());
        assert_eq!(app.selected_session_index, 0);
        assert!(!app.progress_visible);
    }

    // -- toggle_focus --------------------------------------------------------

    #[test]
    fn test_toggle_focus_sidebar_to_logstream() {
        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
        app.toggle_focus();
        assert_eq!(app.focus, Focus::LogStream);
    }

    #[test]
    fn test_toggle_focus_logstream_to_sidebar() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn test_toggle_focus_with_sidebar_hidden_stays_logstream() {
        let mut app = App::new(test_config());
        app.sidebar_visible = false;
        app.focus = Focus::LogStream;
        app.toggle_focus();
        assert_eq!(app.focus, Focus::LogStream);
    }

    #[test]
    fn test_toggle_focus_with_sidebar_hidden_from_sidebar_goes_logstream() {
        let mut app = App::new(test_config());
        app.sidebar_visible = false;
        // Hypothetically focus was on sidebar (edge case)
        app.focus = Focus::Sidebar;
        app.toggle_focus();
        assert_eq!(app.focus, Focus::LogStream);
    }

    // -- toggle_sidebar ------------------------------------------------------

    #[test]
    fn test_toggle_sidebar_visible_to_hidden() {
        let mut app = App::new(test_config());
        app.sidebar_visible = true;
        app.toggle_sidebar();
        assert!(!app.sidebar_visible);
    }

    #[test]
    fn test_toggle_sidebar_hidden_to_visible() {
        let mut app = App::new(test_config());
        app.sidebar_visible = false;
        app.toggle_sidebar();
        assert!(app.sidebar_visible);
    }

    #[test]
    fn test_toggle_sidebar_hidden_focus_auto_switches() {
        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
        app.sidebar_visible = true;

        app.toggle_sidebar(); // hide sidebar

        assert!(!app.sidebar_visible);
        assert_eq!(app.focus, Focus::LogStream);
    }

    #[test]
    fn test_toggle_sidebar_hidden_focus_on_logstream_stays() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.sidebar_visible = true;

        app.toggle_sidebar(); // hide sidebar

        assert!(!app.sidebar_visible);
        assert_eq!(app.focus, Focus::LogStream);
    }

    // -- on_key: quit --------------------------------------------------------

    #[test]
    fn test_on_key_q_quits() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit);
    }

    #[test]
    fn test_on_key_ctrl_c_quits() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    // -- on_key: tab ---------------------------------------------------------

    #[test]
    fn test_on_key_tab_toggles_focus() {
        let mut app = App::new(test_config());
        assert_eq!(app.focus, Focus::LogStream);

        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Sidebar);

        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::LogStream);
    }

    // -- on_key: b -----------------------------------------------------------

    #[test]
    fn test_on_key_b_toggles_sidebar() {
        let mut app = App::new(test_config());
        assert!(app.sidebar_visible);

        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(!app.sidebar_visible);

        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(app.sidebar_visible);
    }

    // -- on_key: unknown key -------------------------------------------------

    #[test]
    fn test_on_key_unknown_key_is_noop() {
        let mut app = App::new(test_config());
        let before_focus = app.focus;
        let before_sidebar = app.sidebar_visible;
        let before_quit = app.should_quit;

        app.on_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));

        assert_eq!(app.focus, before_focus);
        assert_eq!(app.sidebar_visible, before_sidebar);
        assert_eq!(app.should_quit, before_quit);
    }

    // -- select_prev_session / select_next_session ---------------------------

    #[test]
    fn test_select_next_session() {
        let mut app = App::new(test_config());
        app.sessions = vec![
            dummy_session("s1"),
            dummy_session("s2"),
            dummy_session("s3"),
        ];
        assert_eq!(app.selected_session_index, 0);

        app.select_next_session();
        assert_eq!(app.selected_session_index, 1);

        app.select_next_session();
        assert_eq!(app.selected_session_index, 2);

        // At the end, should not go beyond
        app.select_next_session();
        assert_eq!(app.selected_session_index, 2);
    }

    #[test]
    fn test_select_prev_session() {
        let mut app = App::new(test_config());
        app.sessions = vec![
            dummy_session("s1"),
            dummy_session("s2"),
            dummy_session("s3"),
        ];
        app.selected_session_index = 2;

        app.select_prev_session();
        assert_eq!(app.selected_session_index, 1);

        app.select_prev_session();
        assert_eq!(app.selected_session_index, 0);

        // At the beginning, should not go below 0
        app.select_prev_session();
        assert_eq!(app.selected_session_index, 0);
    }

    #[test]
    fn test_select_next_session_empty_sessions() {
        let mut app = App::new(test_config());
        assert!(app.sessions.is_empty());
        app.select_next_session(); // Should not panic
        assert_eq!(app.selected_session_index, 0);
    }

    #[test]
    fn test_select_prev_session_empty_sessions() {
        let mut app = App::new(test_config());
        assert!(app.sessions.is_empty());
        app.select_prev_session(); // Should not panic
        assert_eq!(app.selected_session_index, 0);
    }

    // -- on_key: arrow keys / j/k for session navigation ---------------------

    #[test]
    fn test_on_key_down_arrow_selects_next() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];

        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 1);
    }

    #[test]
    fn test_on_key_up_arrow_selects_prev() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 1;

        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 0);
    }

    #[test]
    fn test_on_key_j_selects_next() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 1);
    }

    #[test]
    fn test_on_key_k_selects_prev() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 1;

        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 0);
    }

    // -- confirm_session_selection -------------------------------------------

    #[test]
    fn test_confirm_session_selection_sets_active_session() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 1;

        app.confirm_session_selection();
        assert_eq!(app.active_session_id, Some("s2".to_string()));
    }

    #[test]
    fn test_confirm_session_selection_switches_focus_to_logstream() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1")];
        app.focus = Focus::Sidebar;

        app.confirm_session_selection();
        assert_eq!(app.focus, Focus::LogStream);
    }

    #[test]
    fn test_confirm_session_selection_clears_new_session_highlight() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.new_session_ids.insert("s1".to_string());
        app.new_session_ids.insert("s2".to_string());
        app.selected_session_index = 0;

        app.confirm_session_selection();

        // s1 should be removed from new_session_ids
        assert!(!app.new_session_ids.contains("s1"));
        // s2 should still be there
        assert!(app.new_session_ids.contains("s2"));
    }

    #[test]
    fn test_confirm_session_selection_empty_sessions_is_noop() {
        let mut app = App::new(test_config());
        assert!(app.sessions.is_empty());

        app.confirm_session_selection(); // Should not panic
        assert_eq!(app.active_session_id, None);
    }

    #[test]
    fn test_on_key_enter_confirms_selection() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 1;
        app.focus = Focus::Sidebar;

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_session_id, Some("s2".to_string()));
        assert_eq!(app.focus, Focus::LogStream);
    }

    // -- adjust_sidebar_scroll -----------------------------------------------

    /// Helper: build a session with a specified number of subagents.
    fn session_with_agents(id: &str, num_subagents: usize) -> Session {
        use crate::session::Agent;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut agents = vec![Agent {
            agent_id: None,
            slug: None,
            log_path: PathBuf::from(format!("/fake/{}.jsonl", id)),
            is_main: true,
        }];

        for i in 0..num_subagents {
            agents.push(Agent {
                agent_id: Some(format!("agent-{}", i)),
                slug: Some(format!("slug-{}", i)),
                log_path: PathBuf::from(format!("/fake/{}/subagents/agent-{}.jsonl", id, i)),
                is_main: false,
            });
        }

        Session {
            id: id.to_string(),
            agents,
            last_modified: SystemTime::now(),
        }
    }

    #[test]
    fn test_adjust_sidebar_scroll_no_scroll_needed() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 0;

        // Visible height = 10, only 2 rows needed
        app.adjust_sidebar_scroll(10);
        assert_eq!(app.sidebar_scroll_offset, 0);
    }

    #[test]
    fn test_adjust_sidebar_scroll_selected_below_viewport() {
        let mut app = App::new(test_config());
        // 5 sessions, each with 1 row (main only) = 5 rows total
        app.sessions = vec![
            dummy_session("s1"),
            dummy_session("s2"),
            dummy_session("s3"),
            dummy_session("s4"),
            dummy_session("s5"),
        ];
        app.selected_session_index = 4; // row 4

        // Visible height = 3, so we need to scroll
        app.adjust_sidebar_scroll(3);
        // selected_end = 5, visible_height = 3, so offset = 5 - 3 = 2
        assert_eq!(app.sidebar_scroll_offset, 2);
    }

    #[test]
    fn test_adjust_sidebar_scroll_selected_above_viewport() {
        let mut app = App::new(test_config());
        app.sessions = vec![
            dummy_session("s1"),
            dummy_session("s2"),
            dummy_session("s3"),
        ];
        app.selected_session_index = 0;
        app.sidebar_scroll_offset = 2; // artificially scrolled down

        app.adjust_sidebar_scroll(3);
        // Selected session starts at row 0, which is above offset 2
        assert_eq!(app.sidebar_scroll_offset, 0);
    }

    #[test]
    fn test_adjust_sidebar_scroll_with_subagents() {
        let mut app = App::new(test_config());
        // s1: 1 header + 2 agents = 3 rows
        // s2: 1 header + 0 agents = 1 row
        // s3: 1 header + 1 agent = 2 rows
        app.sessions = vec![
            session_with_agents("s1", 2),
            dummy_session("s2"),
            session_with_agents("s3", 1),
        ];
        app.selected_session_index = 2; // s3 starts at row 4

        // Visible height = 3
        app.adjust_sidebar_scroll(3);
        // s3 starts at row 4, ends at row 6
        // offset = 6 - 3 = 3
        assert_eq!(app.sidebar_scroll_offset, 3);
    }

    #[test]
    fn test_adjust_sidebar_scroll_empty_sessions() {
        let mut app = App::new(test_config());
        app.sidebar_scroll_offset = 5; // garbage value

        app.adjust_sidebar_scroll(10);
        assert_eq!(app.sidebar_scroll_offset, 0);
    }

    #[test]
    fn test_adjust_sidebar_scroll_zero_height() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1")];
        app.sidebar_scroll_offset = 5; // garbage value

        app.adjust_sidebar_scroll(0);
        assert_eq!(app.sidebar_scroll_offset, 0);
    }

    // -- new fields defaults -------------------------------------------------

    #[test]
    fn test_new_defaults_new_fields() {
        let app = App::new(test_config());
        assert!(app.new_session_ids.is_empty());
        assert_eq!(app.sidebar_scroll_offset, 0);
        assert_eq!(app.active_session_id, None);
        assert!(app.active_filters.is_empty());
    }

    // -- ActiveFilters -------------------------------------------------------

    #[test]
    fn test_active_filters_default_is_empty() {
        let filters = ActiveFilters::default();
        assert!(filters.is_empty());
        assert_eq!(filters.display(), None);
    }

    #[test]
    fn test_active_filters_with_pattern() {
        let filters = ActiveFilters {
            pattern: Some("error".to_string()),
            level: None,
        };
        assert!(!filters.is_empty());
        assert_eq!(filters.display(), Some("filter:error".to_string()));
    }

    #[test]
    fn test_active_filters_with_level() {
        let filters = ActiveFilters {
            pattern: None,
            level: Some("user".to_string()),
        };
        assert!(!filters.is_empty());
        assert_eq!(filters.display(), Some("level:user".to_string()));
    }

    #[test]
    fn test_active_filters_with_both() {
        let filters = ActiveFilters {
            pattern: Some("foo".to_string()),
            level: Some("assistant".to_string()),
        };
        assert!(!filters.is_empty());
        assert_eq!(
            filters.display(),
            Some("filter:foo level:assistant".to_string())
        );
    }

    #[test]
    fn test_active_filters_display_truncated_fits() {
        let filters = ActiveFilters {
            pattern: Some("err".to_string()),
            level: None,
        };
        // "filter:err" is 10 chars, max_width=20 should not truncate
        assert_eq!(
            filters.display_truncated(20),
            Some("filter:err".to_string())
        );
    }

    #[test]
    fn test_active_filters_display_truncated_too_narrow() {
        let filters = ActiveFilters {
            pattern: Some("error_pattern_very_long".to_string()),
            level: None,
        };
        // "filter:error_pattern_very_long" = 30 chars, max_width=15
        let result = filters.display_truncated(15);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.ends_with("..."));
        assert_eq!(text.len(), 15);
    }

    #[test]
    fn test_active_filters_display_truncated_very_narrow() {
        let filters = ActiveFilters {
            pattern: Some("x".to_string()),
            level: None,
        };
        // max_width < 4 should return None
        assert_eq!(filters.display_truncated(3), None);
    }

    #[test]
    fn test_active_filters_display_truncated_empty() {
        let filters = ActiveFilters::default();
        assert_eq!(filters.display_truncated(50), None);
    }

    // -- Filter overlay integration tests ---------------------------------

    #[test]
    fn test_new_defaults_filter_fields() {
        let app = App::new(test_config());
        assert!(!app.filter_state.is_active());
        assert!(!app.filter_overlay.visible);
    }

    #[test]
    fn test_slash_key_opens_filter_overlay() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.filter_overlay.visible);
    }

    #[test]
    fn test_q_does_not_quit_when_overlay_is_open() {
        let mut app = App::new(test_config());
        // Open the overlay
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.filter_overlay.visible);

        // 'q' inside overlay should NOT quit; it types 'q' in the pattern input
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_quit);
        assert!(app.filter_overlay.visible);
        assert_eq!(app.filter_overlay.pattern_input, "q");
    }

    #[test]
    fn test_esc_closes_overlay_without_applying() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));

        // Type something
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.filter_overlay.pattern_input, "x");

        // Esc should close without applying
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.filter_overlay.visible);
        assert!(!app.filter_state.is_active());
    }

    #[test]
    fn test_enter_applies_filter_and_closes_overlay() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));

        // Type a pattern
        for c in "error".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }

        // Enter should apply and close
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.filter_overlay.visible);
        assert!(app.filter_state.is_active());
        assert_eq!(app.filter_state.pattern, "error");
        assert_eq!(
            app.active_filters.pattern,
            Some("error".to_string())
        );
    }

    #[test]
    fn test_ctrl_c_closes_overlay_when_visible() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.filter_overlay.visible);

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.filter_overlay.visible);
        assert!(!app.should_quit); // should NOT quit
    }

    #[test]
    fn test_collect_known_roles_from_ring_buffer() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = App::new(test_config());
        app.ring_buffer.push(
            parse_jsonl_line(
                r#"{"type": "user", "message": {"role": "user", "content": "hi"}}"#,
            )
            .unwrap(),
        );
        app.ring_buffer.push(
            parse_jsonl_line(
                r#"{"type": "assistant", "message": {"role": "assistant", "content": "hello"}}"#,
            )
            .unwrap(),
        );
        app.ring_buffer.push(
            parse_jsonl_line(
                r#"{"type": "user", "message": {"role": "user", "content": "again"}}"#,
            )
            .unwrap(),
        );

        let roles = app.collect_known_roles();
        assert_eq!(roles, vec!["assistant", "user"]);
    }

    #[test]
    fn test_collect_known_agents_from_ring_buffer() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = App::new(test_config());
        app.ring_buffer.push(
            parse_jsonl_line(
                r#"{"type": "assistant", "isSidechain": true, "agentId": "abc", "slug": "cool-agent", "message": {"role": "assistant", "content": "hi"}}"#,
            )
            .unwrap(),
        );
        app.ring_buffer.push(
            parse_jsonl_line(
                r#"{"type": "user", "message": {"role": "user", "content": "hi"}}"#,
            )
            .unwrap(),
        );

        let agents = app.collect_known_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].0, "abc");
        assert_eq!(agents[0].1, "cool-agent");
    }

    #[test]
    fn test_collect_known_agents_empty_buffer() {
        let app = App::new(test_config());
        let agents = app.collect_known_agents();
        assert!(agents.is_empty());
    }

    #[test]
    fn test_collect_known_roles_empty_buffer() {
        let app = App::new(test_config());
        let roles = app.collect_known_roles();
        assert!(roles.is_empty());
    }

    #[test]
    fn test_apply_filter_updates_active_filters_display() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));

        for c in "test".chars() {
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }

        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(!app.active_filters.is_empty());
        let display = app.active_filters.display().unwrap();
        assert!(display.contains("filter:test"));
    }

    #[test]
    fn test_empty_filter_clears_active_filters() {
        let mut app = App::new(test_config());

        // First set a filter
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.filter_state.is_active());

        // Now open and apply empty filter
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        // Delete the character
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(!app.filter_state.is_active());
        assert!(app.active_filters.is_empty());
    }

    // -- toggle_progress_visible ---------------------------------------------

    #[test]
    fn test_toggle_progress_visible() {
        let mut app = App::new(test_config());
        assert!(!app.progress_visible);

        app.toggle_progress_visible();
        assert!(app.progress_visible);

        app.toggle_progress_visible();
        assert!(!app.progress_visible);
    }

    #[test]
    fn test_on_key_p_toggles_progress_visible() {
        let mut app = App::new(test_config());
        assert!(!app.progress_visible);

        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(app.progress_visible);

        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(!app.progress_visible);
    }

    #[test]
    fn test_p_key_does_not_toggle_when_overlay_is_open() {
        let mut app = App::new(test_config());
        // Open the overlay
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.filter_overlay.visible);

        // 'p' inside overlay should type into the pattern input, not toggle progress
        app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert!(!app.progress_visible);
        assert_eq!(app.filter_overlay.pattern_input, "p");
    }

    // -- tmux integration tests -----------------------------------------------

    #[test]
    fn test_new_defaults_tmux_fields() {
        let app = App::new(test_config());
        assert!(app.status_message.is_none());
        assert!(!app.quit_confirm_pending);
        assert!(app.project_path.is_none());
        assert_eq!(app.tmux_manager.pane_count(), 0);
    }

    #[test]
    fn test_t_key_without_tmux_shows_status_message() {
        let mut app = App::new(test_config());
        // Ensure TMUX env is not set for this test.
        let original = std::env::var("TMUX").ok();
        std::env::remove_var("TMUX");

        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

        // Should show a status message about an error condition.
        // Due to test parallelism with env vars, we might hit "not inside tmux"
        // or fall through to "No project path detected" (if another test
        // concurrently restores the TMUX var). Both are valid error paths.
        assert!(app.status_message.is_some());
        let msg = app.status_message.as_ref().unwrap();
        assert!(
            msg.contains("tmux") || msg.contains("not inside") || msg.contains("project path") || msg.contains("session"),
            "expected error status message, got: {}",
            msg
        );
        assert!(!app.should_quit);

        // Restore TMUX env var.
        if let Some(val) = original {
            std::env::set_var("TMUX", val);
        }
    }

    #[test]
    fn test_t_key_does_not_work_when_overlay_is_open() {
        let mut app = App::new(test_config());
        // Open the overlay.
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.filter_overlay.visible);

        // 't' inside overlay should type into the pattern input.
        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(app.filter_overlay.pattern_input, "t");
        // Status message should not be set (the overlay consumed the key).
        // Note: status_message is cleared at the start of on_key.
    }

    #[test]
    fn test_status_message_cleared_on_next_key() {
        let mut app = App::new(test_config());
        app.status_message = Some("test message".to_string());

        // Any key press should clear the status message.
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(app.status_message.is_none());
    }

    #[test]
    fn test_q_quits_without_confirmation_when_no_tmux_panes() {
        let mut app = App::new(test_config());
        // No tmux panes active.
        assert_eq!(app.tmux_manager.pane_count(), 0);

        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit);
        assert!(!app.quit_confirm_pending);
    }

    #[test]
    fn test_quit_confirm_pending_blocks_other_keys() {
        let mut app = App::new(test_config());
        app.quit_confirm_pending = true;

        // Unknown key should be ignored.
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(app.quit_confirm_pending);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_quit_confirm_y_quits() {
        let mut app = App::new(test_config());
        app.quit_confirm_pending = true;

        app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(app.should_quit);
        assert!(!app.quit_confirm_pending);
    }

    #[test]
    fn test_quit_confirm_n_cancels() {
        let mut app = App::new(test_config());
        app.quit_confirm_pending = true;

        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(!app.should_quit);
        assert!(!app.quit_confirm_pending);
    }

    #[test]
    fn test_quit_confirm_esc_cancels() {
        let mut app = App::new(test_config());
        app.quit_confirm_pending = true;

        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.should_quit);
        assert!(!app.quit_confirm_pending);
    }

    #[test]
    fn test_quit_confirm_capital_y_quits() {
        let mut app = App::new(test_config());
        app.quit_confirm_pending = true;

        app.on_key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::NONE));
        assert!(app.should_quit);
    }

    #[test]
    fn test_ctrl_c_quits_without_confirmation_when_no_panes() {
        let mut app = App::new(test_config());
        assert_eq!(app.tmux_manager.pane_count(), 0);

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert!(!app.quit_confirm_pending);
    }

    #[test]
    fn test_t_key_without_project_path_shows_error() {
        let mut app = App::new(test_config());
        app.project_path = None;

        // Set TMUX env to simulate being inside tmux.
        let original = std::env::var("TMUX").ok();
        std::env::set_var("TMUX", "/tmp/tmux/default,1234,0");

        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

        assert!(app.status_message.is_some());
        let msg = app.status_message.as_ref().unwrap();
        assert!(
            msg.contains("project path") || msg.contains("No project"),
            "expected project path message, got: {}",
            msg
        );

        // Restore TMUX env var.
        match original {
            Some(val) => std::env::set_var("TMUX", val),
            None => std::env::remove_var("TMUX"),
        }
    }

    // -- Help overlay tests ---------------------------------------------------

    #[test]
    fn test_new_defaults_help_overlay_hidden() {
        let app = App::new(test_config());
        assert!(!app.help_overlay_visible);
    }

    #[test]
    fn test_question_mark_opens_help_overlay() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(app.help_overlay_visible);
    }

    #[test]
    fn test_any_key_dismisses_help_overlay() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(!app.help_overlay_visible);
    }

    #[test]
    fn test_keys_consumed_while_help_overlay_visible() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        // 'q' should not quit; it should just dismiss the overlay.
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.help_overlay_visible);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_ctrl_c_dismisses_help_overlay_without_quitting() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.help_overlay_visible);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_question_mark_does_not_open_help_when_filter_overlay_active() {
        let mut app = App::new(test_config());
        // Open the filter overlay first.
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.filter_overlay.visible);

        // '?' inside filter overlay should type into the pattern input, not open help.
        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(!app.help_overlay_visible);
        assert!(app.filter_overlay.visible);
    }

    #[test]
    fn test_question_mark_does_not_open_help_when_quit_confirm_active() {
        let mut app = App::new(test_config());
        app.quit_confirm_pending = true;

        // '?' while quit confirm is pending should be ignored (not open help).
        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(!app.help_overlay_visible);
        // quit_confirm_pending should still be true (unknown key is ignored).
        assert!(app.quit_confirm_pending);
    }

    #[test]
    fn test_t_key_without_active_session_shows_error() {
        let mut app = App::new(test_config());
        app.project_path = Some(PathBuf::from("/fake/project"));
        app.active_session_id = None;

        // Set TMUX env to simulate being inside tmux.
        let original = std::env::var("TMUX").ok();
        std::env::set_var("TMUX", "/tmp/tmux/default,1234,0");

        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

        assert!(app.status_message.is_some());
        let msg = app.status_message.as_ref().unwrap();
        assert!(
            msg.contains("session") || msg.contains("Select"),
            "expected session selection message, got: {}",
            msg
        );

        // Restore TMUX env var.
        match original {
            Some(val) => std::env::set_var("TMUX", val),
            None => std::env::remove_var("TMUX"),
        }
    }
}
