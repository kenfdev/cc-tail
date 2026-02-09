//! Application state for the TUI.
//!
//! The [`App`] struct owns all mutable state that drives the TUI:
//! focus tracking, sidebar visibility, quit flag, sessions list,
//! config, and the ring buffer of log entries.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::config::AppConfig;
use crate::filter::FilterState;
use crate::replay::{replay_session, DEFAULT_REPLAY_COUNT};
use crate::ring_buffer::RingBuffer;
use crate::session::{classify_new_file, Agent, NewFileKind, Session};
use crate::theme::ThemeColors;
use crate::tui::filter_overlay::{FilterOverlayState, OverlayAction};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

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
    #[allow(dead_code)]
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
// Scroll types
// ---------------------------------------------------------------------------

/// A scroll action that is either applied immediately (when `ScrollMode` is
/// already active) or stored as a pending request for the render phase to
/// snapshot the current lines and compute the initial offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingScroll {
    /// Scroll up by N lines.
    Up(usize),
    /// Scroll down by N lines.
    Down(usize),
    /// Jump to the top of the log stream.
    ToTop,
    /// Scroll up by half a page.
    HalfPageUp,
    /// Scroll down by half a page.
    HalfPageDown,
}

/// State for the scroll (freeze) mode in the log stream panel.
///
/// When active, the log stream is frozen at a snapshot of rendered lines
/// and the user can scroll through them instead of seeing live updates.
#[derive(Debug, Clone)]
pub struct ScrollMode {
    /// The snapshot of rendered lines (set by the render phase).
    pub lines: Vec<ratatui::text::Line<'static>>,
    /// Current scroll offset (0 = showing the bottom of the log).
    pub offset: usize,
    /// Total number of lines in the snapshot.
    pub total_lines: usize,
    /// Number of visible lines in the log stream area.
    pub visible_height: usize,
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
    /// Transient status message shown in the status bar.
    /// Cleared after a few ticks or on the next key press.
    pub status_message: Option<String>,
    /// The resolved project directory path used by the TUI.
    pub project_path: Option<PathBuf>,
    /// Whether the help overlay is currently visible.
    pub help_overlay_visible: bool,
    /// Human-readable project name derived from the project path
    /// (e.g. last path component: `/Users/.../cc-tail` -> `"cc-tail"`).
    /// Shown in the status bar.
    pub project_display_name: Option<String>,
    /// Active scroll (freeze) mode state, if the user has entered scroll mode.
    pub scroll_mode: Option<ScrollMode>,
    /// A pending scroll action waiting for the render phase to snapshot lines.
    pub pending_scroll: Option<PendingScroll>,
}

impl App {
    /// Create a new `App` with the given config.
    ///
    /// Starts with:
    /// - Focus on `Sidebar`
    /// - Sidebar visible
    /// - `should_quit = false`
    /// - Default-budget ring buffer
    /// - Empty sessions list
    pub fn new(config: AppConfig) -> Self {
        let theme_colors = ThemeColors::from_theme(&config.theme);
        Self {
            focus: Focus::Sidebar,
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
            status_message: None,
            project_path: None,
            help_overlay_visible: false,
            project_display_name: None,
            scroll_mode: None,
            pending_scroll: None,
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

        // Global keys (not focus-dependent).
        match key.code {
            KeyCode::Char('q') => {
                self.initiate_quit();
                return;
            }
            KeyCode::Char('?') => {
                self.help_overlay_visible = true;
                return;
            }
            KeyCode::Char('/') => {
                self.open_filter_overlay();
                return;
            }
            KeyCode::Tab => {
                self.toggle_focus();
                return;
            }
            KeyCode::Char('b') => {
                self.toggle_sidebar();
                return;
            }
            KeyCode::Enter => {
                self.confirm_session_selection();
                return;
            }
            _ => {}
        }

        // Focus-dependent keys.
        match self.focus {
            Focus::Sidebar => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.select_prev_session(),
                KeyCode::Down | KeyCode::Char('j') => self.select_next_session(),
                _ => {}
            },
            Focus::LogStream => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.enter_scroll_mode(PendingScroll::Up(1));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.is_in_scroll_mode() {
                        self.apply_scroll(PendingScroll::Down(1));
                    }
                }
                KeyCode::PageUp => {
                    self.enter_scroll_mode(PendingScroll::Up(20));
                }
                KeyCode::PageDown => {
                    if self.is_in_scroll_mode() {
                        self.apply_scroll(PendingScroll::Down(20));
                    }
                }
                KeyCode::Char('u') => {
                    self.enter_scroll_mode(PendingScroll::HalfPageUp);
                }
                KeyCode::Char('d') => {
                    if self.is_in_scroll_mode() {
                        self.apply_scroll(PendingScroll::HalfPageDown);
                    }
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    self.enter_scroll_mode(PendingScroll::ToTop);
                }
                KeyCode::Char('G') | KeyCode::End => {
                    self.exit_scroll_mode();
                }
                KeyCode::Esc => {
                    if self.is_in_scroll_mode() {
                        self.exit_scroll_mode();
                    }
                }
                _ => {}
            },
        }
    }

    /// Initiate the quit process.
    fn initiate_quit(&mut self) {
        self.should_quit = true;
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

    // -- Scroll mode ---------------------------------------------------------

    /// Returns `true` if scroll (freeze) mode is active.
    pub fn is_in_scroll_mode(&self) -> bool {
        self.scroll_mode.is_some()
    }

    /// Enter scroll mode or apply a scroll action if already in scroll mode.
    ///
    /// When scroll mode is not yet active, the action is stored as a
    /// `pending_scroll` for the render phase to snapshot the current lines
    /// and compute the initial offset. When scroll mode IS active, the
    /// action is applied immediately via `apply_scroll`.
    pub fn enter_scroll_mode(&mut self, action: PendingScroll) {
        if self.scroll_mode.is_some() {
            self.apply_scroll(action);
        } else {
            self.pending_scroll = Some(action);
        }
    }

    /// Exit scroll mode, returning to live-tailing.
    pub fn exit_scroll_mode(&mut self) {
        self.scroll_mode = None;
        self.pending_scroll = None;
    }

    /// Apply a scroll action to the active scroll mode state.
    ///
    /// - `Up(n)`: scroll up (increase offset), clamped to max.
    /// - `Down(n)`: scroll down (decrease offset); exits scroll mode if at bottom.
    /// - `ToTop`: jump to the top (max offset).
    pub fn apply_scroll(&mut self, action: PendingScroll) {
        if let Some(ref mut sm) = self.scroll_mode {
            let max_offset = sm.total_lines.saturating_sub(sm.visible_height);
            match action {
                PendingScroll::Up(n) => {
                    sm.offset = sm.offset.saturating_add(n).min(max_offset);
                }
                PendingScroll::Down(n) => {
                    if sm.offset == 0 {
                        // Already at bottom, exit scroll mode.
                        self.scroll_mode = None;
                        self.pending_scroll = None;
                        return;
                    }
                    sm.offset = sm.offset.saturating_sub(n);
                }
                PendingScroll::ToTop => {
                    sm.offset = max_offset;
                }
                PendingScroll::HalfPageUp => {
                    let half = sm.visible_height / 2;
                    sm.offset = sm.offset.saturating_add(half).min(max_offset);
                }
                PendingScroll::HalfPageDown => {
                    if sm.offset == 0 {
                        // Already at bottom, exit scroll mode.
                        self.scroll_mode = None;
                        self.pending_scroll = None;
                        return;
                    }
                    let half = sm.visible_height / 2;
                    sm.offset = sm.offset.saturating_sub(half);
                }
            }
        }
    }

    /// Handle a mouse event.
    ///
    /// ScrollUp enters/applies scroll up; ScrollDown scrolls down (only
    /// when already in scroll mode).
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        // Only respond to mouse scroll when focused on LogStream.
        if self.focus != Focus::LogStream {
            return;
        }

        // Ignore mouse events when overlays are active.
        if self.help_overlay_visible || self.filter_overlay.visible {
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.enter_scroll_mode(PendingScroll::Up(3));
            }
            MouseEventKind::ScrollDown => {
                // Only scroll down when already in scroll mode.
                if self.is_in_scroll_mode() {
                    self.apply_scroll(PendingScroll::Down(3));
                }
            }
            _ => {}
        }
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
    /// set, and replays the session's recent messages into the ring buffer.
    /// Focus is NOT changed; use Tab to switch focus explicitly.
    pub fn confirm_session_selection(&mut self) {
        if self.sessions.is_empty() {
            return;
        }

        let idx = self.selected_session_index.min(self.sessions.len() - 1);
        let session = self.sessions[idx].clone();

        // Clear the new-session highlight for this session.
        self.new_session_ids.remove(&session.id);

        self.active_session_id = Some(session.id.clone());

        // Exit scroll mode when switching sessions.
        self.exit_scroll_mode();

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

    /// Handle a newly detected JSONL file from the watcher.
    ///
    /// Classifies the file path and either creates a new session
    /// (for top-level files) or adds a subagent to an existing session.
    pub fn on_new_file_detected(&mut self, path: PathBuf) {
        // Need a project path to classify the file.
        let project_path = match &self.project_path {
            Some(p) => p.clone(),
            None => return,
        };

        // Canonicalize project_path for comparison (watcher sends canonical paths).
        let canonical_project_dir = project_path
            .canonicalize()
            .unwrap_or_else(|_| project_path.clone());

        match classify_new_file(&path, &canonical_project_dir) {
            NewFileKind::TopLevelSession { session_id } => {
                // Check for duplicate session.
                if self.sessions.iter().any(|s| s.id == session_id) {
                    return;
                }

                let was_non_empty = !self.sessions.is_empty();

                let session = Session {
                    id: session_id.clone(),
                    agents: vec![Agent {
                        agent_id: None,
                        slug: None,
                        log_path: path,
                        is_main: true,
                    }],
                    last_modified: SystemTime::now(),
                };

                self.sessions.insert(0, session);
                self.new_session_ids.insert(session_id);

                if was_non_empty {
                    self.selected_session_index += 1;
                }
            }
            NewFileKind::Subagent {
                session_id,
                agent_id,
            } => {
                // Find the parent session.
                if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    // Check for duplicate agent.
                    if session
                        .agents
                        .iter()
                        .any(|a| a.agent_id.as_deref() == Some(&agent_id))
                    {
                        return;
                    }

                    session.agents.push(Agent {
                        agent_id: Some(agent_id),
                        slug: None,
                        log_path: path,
                        is_main: false,
                    });
                    session.last_modified = SystemTime::now();
                }
            }
            NewFileKind::Unknown => {}
        }
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
                let roles: Vec<String> = new_state.enabled_roles.iter().cloned().collect();
                Some(roles.join(","))
            },
        };

        self.filter_state = new_state;

        // Exit scroll mode when filters change (content snapshot is stale).
        self.exit_scroll_mode();
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
        assert_eq!(app.focus, Focus::Sidebar);
        assert!(app.sidebar_visible);
        assert!(!app.should_quit);
        assert!(app.sessions.is_empty());
        assert_eq!(app.selected_session_index, 0);
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
        assert_eq!(app.focus, Focus::Sidebar);

        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::LogStream);

        app.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Sidebar);
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
        app.focus = Focus::Sidebar;
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];

        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 1);
    }

    #[test]
    fn test_on_key_up_arrow_selects_prev() {
        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 1;

        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 0);
    }

    #[test]
    fn test_on_key_j_selects_next() {
        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];

        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 1);
    }

    #[test]
    fn test_on_key_k_selects_prev() {
        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
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
    fn test_confirm_session_selection_keeps_focus_on_sidebar() {
        let mut app = App::new(test_config());
        app.sessions = vec![dummy_session("s1")];
        app.focus = Focus::Sidebar;

        app.confirm_session_selection();
        assert_eq!(app.focus, Focus::Sidebar);
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
        assert_eq!(app.focus, Focus::Sidebar);
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
        assert_eq!(app.active_filters.pattern, Some("error".to_string()));
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
            parse_jsonl_line(r#"{"type": "user", "message": {"role": "user", "content": "hi"}}"#)
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
            parse_jsonl_line(r#"{"type": "user", "message": {"role": "user", "content": "hi"}}"#)
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

    // -- Status message tests ------------------------------------------------

    #[test]
    fn test_new_defaults_status_fields() {
        let app = App::new(test_config());
        assert!(app.status_message.is_none());
        assert!(app.project_path.is_none());
    }

    #[test]
    fn test_status_message_cleared_on_next_key() {
        let mut app = App::new(test_config());
        app.status_message = Some("test message".to_string());

        // Any key press should clear the status message.
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(app.status_message.is_none());
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

    // -- on_new_file_detected tests -------------------------------------------

    #[test]
    fn test_on_new_file_detected_new_session() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().to_path_buf();
        let canonical_project_dir = project_dir.canonicalize().unwrap();

        let mut app = App::new(test_config());
        app.project_path = Some(project_dir);
        // Add an existing session so we can verify selected_session_index is incremented.
        app.sessions = vec![dummy_session("existing")];
        app.selected_session_index = 0;

        let new_file_path = canonical_project_dir.join("new-session-abc.jsonl");
        app.on_new_file_detected(new_file_path);

        // Session should be inserted at index 0.
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.sessions[0].id, "new-session-abc");
        // new_session_ids should contain the new session id.
        assert!(app.new_session_ids.contains("new-session-abc"));
        // selected_session_index should be incremented (was 0, now 1).
        assert_eq!(app.selected_session_index, 1);
    }

    #[test]
    fn test_on_new_file_detected_duplicate_session_ignored() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().to_path_buf();
        let canonical_project_dir = project_dir.canonicalize().unwrap();

        let mut app = App::new(test_config());
        app.project_path = Some(project_dir);
        app.sessions = vec![dummy_session("dup-session")];

        let new_file_path = canonical_project_dir.join("dup-session.jsonl");
        app.on_new_file_detected(new_file_path);

        // Sessions count should remain unchanged.
        assert_eq!(app.sessions.len(), 1);
    }

    #[test]
    fn test_on_new_file_detected_no_project_path() {
        let mut app = App::new(test_config());
        app.project_path = None;

        let path = PathBuf::from("/fake/project/.claude/new-session.jsonl");
        app.on_new_file_detected(path);

        // No-op: sessions should remain empty.
        assert!(app.sessions.is_empty());
    }

    #[test]
    fn test_on_new_file_detected_subagent_added() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().to_path_buf();
        let canonical_project_dir = project_dir.canonicalize().unwrap();

        let mut app = App::new(test_config());
        app.project_path = Some(project_dir);
        app.sessions = vec![dummy_session("parent-sess")];

        let subagent_path = canonical_project_dir
            .join("parent-sess")
            .join("subagents")
            .join("agent-sub123.jsonl");
        app.on_new_file_detected(subagent_path);

        // The parent session should now have 2 agents (1 main + 1 subagent).
        assert_eq!(app.sessions[0].agents.len(), 2);
        let sub = app.sessions[0].agents.iter().find(|a| !a.is_main).unwrap();
        assert_eq!(sub.agent_id.as_deref(), Some("sub123"));
    }

    // -- Scroll mode tests ----------------------------------------------------

    /// Helper: create an App with scroll_mode pre-set for testing.
    fn app_with_scroll_mode(offset: usize, total_lines: usize, visible_height: usize) -> App {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.scroll_mode = Some(ScrollMode {
            lines: Vec::new(),
            offset,
            total_lines,
            visible_height,
        });
        app
    }

    #[test]
    fn test_new_defaults_scroll_fields() {
        let app = App::new(test_config());
        assert!(app.scroll_mode.is_none());
        assert!(app.pending_scroll.is_none());
        assert!(!app.is_in_scroll_mode());
    }

    #[test]
    fn test_enter_scroll_mode_sets_pending_when_not_active() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.enter_scroll_mode(PendingScroll::Up(1));
        assert_eq!(app.pending_scroll, Some(PendingScroll::Up(1)));
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_enter_scroll_mode_applies_when_already_active() {
        let mut app = app_with_scroll_mode(5, 100, 20);
        app.enter_scroll_mode(PendingScroll::Up(3));
        // Should apply directly: 5 + 3 = 8
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 8);
        assert!(app.pending_scroll.is_none());
    }

    #[test]
    fn test_exit_scroll_mode_clears_state() {
        let mut app = app_with_scroll_mode(10, 100, 20);
        app.pending_scroll = Some(PendingScroll::ToTop);
        app.exit_scroll_mode();
        assert!(app.scroll_mode.is_none());
        assert!(app.pending_scroll.is_none());
        assert!(!app.is_in_scroll_mode());
    }

    #[test]
    fn test_apply_scroll_up_clamps_to_max() {
        // total=30, visible=20 => max_offset=10
        let mut app = app_with_scroll_mode(8, 30, 20);
        app.apply_scroll(PendingScroll::Up(5));
        // 8 + 5 = 13, clamped to 10
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 10);
    }

    #[test]
    fn test_apply_scroll_down_reduces_offset() {
        let mut app = app_with_scroll_mode(5, 100, 20);
        app.apply_scroll(PendingScroll::Down(3));
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 2);
    }

    #[test]
    fn test_apply_scroll_down_at_zero_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(0, 100, 20);
        app.apply_scroll(PendingScroll::Down(1));
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_apply_scroll_to_top() {
        // total=50, visible=20 => max_offset=30
        let mut app = app_with_scroll_mode(5, 50, 20);
        app.apply_scroll(PendingScroll::ToTop);
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 30);
    }

    #[test]
    fn test_apply_scroll_down_saturating_sub() {
        let mut app = app_with_scroll_mode(2, 100, 20);
        app.apply_scroll(PendingScroll::Down(5));
        // 2 - 5 saturates to 0; but offset was > 0, so it does not exit scroll mode
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 0);
    }

    #[test]
    fn test_is_in_scroll_mode_true_when_active() {
        let app = app_with_scroll_mode(0, 10, 10);
        assert!(app.is_in_scroll_mode());
    }

    // -- Focus-aware key dispatch tests ---------------------------------------

    #[test]
    fn test_up_key_on_logstream_sets_pending_scroll() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.pending_scroll, Some(PendingScroll::Up(1)));
    }

    #[test]
    fn test_down_key_on_logstream_no_scroll_mode_is_noop() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.pending_scroll.is_none());
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_down_key_on_logstream_with_scroll_mode() {
        let mut app = app_with_scroll_mode(5, 100, 20);
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 4);
    }

    #[test]
    fn test_k_key_on_logstream_sets_pending_scroll() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.pending_scroll, Some(PendingScroll::Up(1)));
    }

    #[test]
    fn test_j_key_on_logstream_no_scroll_mode_is_noop() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(app.pending_scroll.is_none());
    }

    #[test]
    fn test_pageup_on_logstream_sets_pending_scroll() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.pending_scroll, Some(PendingScroll::Up(20)));
    }

    #[test]
    fn test_pagedown_on_logstream_no_scroll_mode_is_noop() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(app.pending_scroll.is_none());
    }

    #[test]
    fn test_pagedown_on_logstream_in_scroll_mode() {
        let mut app = app_with_scroll_mode(25, 100, 20);
        app.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 5);
    }

    #[test]
    fn test_g_key_on_logstream_enters_scroll_to_top() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(app.pending_scroll, Some(PendingScroll::ToTop));
    }

    #[test]
    fn test_home_key_on_logstream_enters_scroll_to_top() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.pending_scroll, Some(PendingScroll::ToTop));
    }

    #[test]
    fn test_capital_g_on_logstream_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(10, 100, 20);
        app.on_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_end_key_on_logstream_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(10, 100, 20);
        app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_esc_on_logstream_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(10, 100, 20);
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_esc_on_logstream_no_scroll_mode_is_noop() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        let before_quit = app.should_quit;
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        // Should not quit or change state
        assert_eq!(app.should_quit, before_quit);
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_up_key_on_sidebar_navigates_sessions() {
        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 1;
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 0);
        // Should not enter scroll mode
        assert!(app.pending_scroll.is_none());
    }

    #[test]
    fn test_down_key_on_sidebar_navigates_sessions() {
        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.selected_session_index, 1);
        assert!(app.pending_scroll.is_none());
    }

    // -- Scroll reset tests ---------------------------------------------------

    #[test]
    fn test_confirm_session_selection_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(10, 100, 20);
        app.sessions = vec![dummy_session("s1")];
        app.selected_session_index = 0;
        app.confirm_session_selection();
        assert!(app.scroll_mode.is_none());
        assert!(app.pending_scroll.is_none());
    }

    #[test]
    fn test_apply_filter_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(10, 100, 20);
        // Open the filter overlay and apply an empty filter
        app.filter_overlay.visible = true;
        app.apply_filter();
        assert!(app.scroll_mode.is_none());
        assert!(app.pending_scroll.is_none());
    }

    // -- Mouse event tests ----------------------------------------------------

    #[test]
    fn test_on_mouse_scroll_up_enters_scroll_mode() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.pending_scroll, Some(PendingScroll::Up(3)));
    }

    #[test]
    fn test_on_mouse_scroll_down_noop_when_not_in_scroll_mode() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        // No pending scroll and no scroll mode
        assert!(app.pending_scroll.is_none());
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_on_mouse_scroll_down_in_scroll_mode() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        let mut app = app_with_scroll_mode(10, 100, 20);
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 7);
    }

    #[test]
    fn test_on_mouse_ignored_on_sidebar_focus() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        let mut app = App::new(test_config());
        app.focus = Focus::Sidebar;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.pending_scroll.is_none());
    }

    #[test]
    fn test_on_mouse_ignored_when_help_overlay_visible() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.help_overlay_visible = true;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.pending_scroll.is_none());
    }

    #[test]
    fn test_on_mouse_ignored_when_filter_overlay_visible() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.filter_overlay.visible = true;
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.pending_scroll.is_none());
    }

    // -- Half-page scroll (u/d) tests ----------------------------------------

    #[test]
    fn test_u_key_on_logstream_sets_pending_half_page_up() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        assert_eq!(app.pending_scroll, Some(PendingScroll::HalfPageUp));
    }

    #[test]
    fn test_d_key_on_logstream_no_scroll_mode_is_noop() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(app.pending_scroll.is_none());
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_d_key_on_logstream_in_scroll_mode() {
        // total=100, visible=20, offset=15 => half=10, new offset=5
        let mut app = app_with_scroll_mode(15, 100, 20);
        app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 5);
    }

    #[test]
    fn test_apply_scroll_half_page_up() {
        // total=100, visible=20 => max_offset=80, half=10
        let mut app = app_with_scroll_mode(5, 100, 20);
        app.apply_scroll(PendingScroll::HalfPageUp);
        // 5 + 10 = 15
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 15);
    }

    #[test]
    fn test_apply_scroll_half_page_up_clamps_to_max() {
        // total=30, visible=20 => max_offset=10, half=10
        let mut app = app_with_scroll_mode(5, 30, 20);
        app.apply_scroll(PendingScroll::HalfPageUp);
        // 5 + 10 = 15, clamped to 10
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 10);
    }

    #[test]
    fn test_apply_scroll_half_page_down() {
        // total=100, visible=20, offset=15 => half=10, new offset=5
        let mut app = app_with_scroll_mode(15, 100, 20);
        app.apply_scroll(PendingScroll::HalfPageDown);
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 5);
    }

    #[test]
    fn test_apply_scroll_half_page_down_at_zero_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(0, 100, 20);
        app.apply_scroll(PendingScroll::HalfPageDown);
        assert!(app.scroll_mode.is_none());
    }

    #[test]
    fn test_apply_scroll_half_page_down_saturating() {
        // total=100, visible=20, offset=3 => half=10, 3-10 saturates to 0
        let mut app = app_with_scroll_mode(3, 100, 20);
        app.apply_scroll(PendingScroll::HalfPageDown);
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 0);
    }

    #[test]
    fn test_u_key_in_scroll_mode_applies_half_page_up() {
        // total=100, visible=20, offset=5 => half=10, new offset=15
        let mut app = app_with_scroll_mode(5, 100, 20);
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        assert_eq!(app.scroll_mode.as_ref().unwrap().offset, 15);
    }

}
