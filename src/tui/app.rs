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
use crate::replay::{load_full_session, replay_session, session_file_size, DEFAULT_REPLAY_COUNT};
use crate::ring_buffer::RingBuffer;
use crate::search::SearchState;
use crate::session::{classify_new_file, Agent, NewFileKind, Session};
use crate::symbols::Symbols;
use crate::theme::ThemeColors;
use crate::tui::filter_overlay::{FilterMenuState, MenuAction};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

// NOTE: ActiveFilters struct has been removed. Filter display is now handled
// by FilterState::display() directly.

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
///
/// **Coordinate system:** `offset` and `total_visual_lines` are measured in
/// *visual* (wrapped) lines — i.e. the number of rows ratatui actually
/// renders after word-wrapping. This keeps the offset consistent with what
/// the user sees on screen.
#[derive(Debug, Clone)]
pub struct ScrollMode {
    /// The snapshot of rendered lines (set by the render phase).
    pub lines: Vec<ratatui::text::Line<'static>>,
    /// Current scroll offset in visual lines (0 = showing the bottom).
    pub offset: usize,
    /// Total number of *logical* lines in the snapshot (kept for diagnostics).
    #[allow(dead_code)]
    pub total_lines: usize,
    /// Total number of *visual* (wrapped) lines in the snapshot.
    pub total_visual_lines: usize,
    /// Number of visible lines in the log stream area.
    pub visible_height: usize,
    /// Inner width available for text (used for visual-line calculations).
    pub inner_width: u16,
}

// ---------------------------------------------------------------------------
// Visual-line helpers (wrap-aware coordinate helpers)
// ---------------------------------------------------------------------------

/// How many visual (screen) rows a single logical line occupies after
/// word-wrapping to `width` columns. Returns at least 1.
pub fn wrapped_line_height(line: &ratatui::text::Line<'_>, width: u16) -> usize {
    if width == 0 {
        return 1;
    }
    let w = line.width();
    if w == 0 {
        return 1;
    }
    w.div_ceil(width as usize)
}

/// Total visual (wrapped) lines for a slice of logical lines.
pub fn total_visual_lines(lines: &[ratatui::text::Line<'_>], width: u16) -> usize {
    lines.iter().map(|l| wrapped_line_height(l, width)).sum()
}

/// Visual-line offset of logical line `idx` (i.e. the sum of wrapped
/// heights of all lines *before* `idx`).
pub fn visual_line_position(lines: &[ratatui::text::Line<'_>], idx: usize, width: u16) -> usize {
    let end = idx.min(lines.len());
    lines[..end]
        .iter()
        .map(|l| wrapped_line_height(l, width))
        .sum()
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
    /// Symbol set (Unicode or ASCII) derived from `config.ascii`.
    pub symbols: Symbols,
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
    /// The current filter state used for filtering log entries.
    pub filter_state: FilterState,
    /// State for the filter menu overlay (opened with `f`).
    pub filter_menu: FilterMenuState,
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
    /// Search state: mode, query, matches, current match index.
    pub search_state: SearchState,
    /// Whether the full session history has been loaded (via `L` key).
    pub full_history_loaded: bool,
    /// Whether a full-load size confirmation prompt is pending.
    pub full_load_confirm_pending: bool,
    /// The file size (in MB) shown in the confirmation prompt.
    pub full_load_pending_size_mb: f64,
    /// Dirty flag: when `true`, the next tick will redraw the terminal.
    /// Set to `true` on any state mutation; cleared after `terminal.draw()`.
    pub needs_redraw: bool,
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
        let symbols = Symbols::new(config.ascii);
        Self {
            focus: Focus::Sidebar,
            sidebar_visible: true,
            should_quit: false,
            config,
            theme_colors,
            symbols,
            ring_buffer: RingBuffer::with_default_budget(),
            sessions: Vec::new(),
            selected_session_index: 0,
            new_session_ids: HashSet::new(),
            sidebar_scroll_offset: 0,
            active_session_id: None,
            filter_state: FilterState::default(),
            filter_menu: FilterMenuState::default(),
            replay_offsets: HashMap::new(),
            status_message: None,
            project_path: None,
            help_overlay_visible: false,
            project_display_name: None,
            scroll_mode: None,
            pending_scroll: None,
            search_state: SearchState::default(),
            full_history_loaded: false,
            full_load_confirm_pending: false,
            full_load_pending_size_mb: 0.0,
            needs_redraw: true,
        }
    }

    /// Mark the app as needing a redraw on the next tick.
    #[inline]
    #[allow(dead_code)]
    pub fn mark_dirty(&mut self) {
        self.needs_redraw = true;
    }

    // -- Key handling --------------------------------------------------------

    /// Handle a key event, dispatching to the appropriate action.
    pub fn on_key(&mut self, key: KeyEvent) {
        self.needs_redraw = true;
        // Clear transient status messages on any key press.
        self.status_message = None;

        // When the help overlay is visible, only `?` (toggle) and `Escape` close it.
        // All other keys are consumed without action.
        if self.help_overlay_visible {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => {
                    self.help_overlay_visible = false;
                }
                _ => {} // consume the key
            }
            return;
        }

        // Full-load confirmation prompt: intercept y/n/Esc before anything else.
        if self.full_load_confirm_pending {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.full_load_confirm_pending = false;
                    self.perform_full_history_load();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.full_load_confirm_pending = false;
                    self.status_message = Some("Full history load cancelled".to_string());
                }
                _ => {} // consume other keys
            }
            return;
        }

        // Ctrl+C always quits regardless of focus.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            // If filter menu is visible, Ctrl+C cancels it instead of quitting.
            if self.filter_menu.visible {
                self.filter_menu.visible = false;
                return;
            }
            // If search input is active, cancel it instead of quitting.
            if self.search_state.is_input() {
                self.search_state.cancel();
                return;
            }
            self.initiate_quit();
            return;
        }

        // When search is in Input mode, delegate all key events to the search input handler.
        if self.search_state.is_input() {
            match key.code {
                KeyCode::Esc => {
                    self.search_state.cancel();
                }
                KeyCode::Enter => {
                    self.search_state.confirm();
                    // If search became active, force scroll mode so highlights are stable.
                    if self.search_state.is_active() {
                        self.force_scroll_mode_for_search();
                    }
                }
                KeyCode::Backspace => {
                    self.search_state.on_backspace();
                }
                KeyCode::Char(ch) => {
                    self.search_state.on_char(ch);
                }
                _ => {} // consume other keys
            }
            return;
        }

        // When the filter menu is visible, delegate ALL key events to it.
        if self.filter_menu.visible {
            let action = self.filter_menu.on_key(key);
            match action {
                MenuAction::Close => {
                    self.filter_menu.visible = false;
                }
                MenuAction::Selected => {
                    self.apply_filter_from_menu();
                }
                MenuAction::Consumed => {}
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
            KeyCode::Char('f') => {
                self.open_filter_menu();
                return;
            }
            KeyCode::Char('/') => {
                self.search_state.start_input();
                return;
            }
            KeyCode::Char('L') => {
                self.handle_full_history_load();
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

        // Search active mode keys: n/N navigate matches, Esc cancels search.
        if self.search_state.is_active() {
            match key.code {
                KeyCode::Char('n') => {
                    self.search_state.next_match();
                    self.invalidate_scroll_snapshot();
                    return;
                }
                KeyCode::Char('N') => {
                    self.search_state.prev_match();
                    self.invalidate_scroll_snapshot();
                    return;
                }
                KeyCode::Esc => {
                    self.search_state.cancel();
                    self.invalidate_scroll_snapshot();
                    return;
                }
                _ => {}
            }
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
            let max_offset = sm.total_visual_lines.saturating_sub(sm.visible_height);
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
        self.needs_redraw = true;

        // Only respond to mouse scroll when focused on LogStream.
        if self.focus != Focus::LogStream {
            return;
        }

        // Ignore mouse events when overlays are active.
        if self.help_overlay_visible || self.filter_menu.visible {
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

        // Cancel search when switching sessions (matches would be stale).
        self.cancel_search();

        // Reset full history loaded flag when switching sessions.
        self.full_history_loaded = false;
        self.full_load_confirm_pending = false;

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
        self.needs_redraw = true;
        self.ring_buffer.push(entry);
    }

    /// Handle a newly detected JSONL file from the watcher.
    ///
    /// Classifies the file path and either creates a new session
    /// (for top-level files) or adds a subagent to an existing session.
    pub fn on_new_file_detected(&mut self, path: PathBuf) {
        self.needs_redraw = true;

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

    // -- Full history load ------------------------------------------------

    /// Size threshold (bytes) above which a confirmation prompt is shown.
    const FULL_LOAD_SIZE_THRESHOLD: u64 = 50 * 1024 * 1024; // 50 MB

    /// Handle the `L` key press for full history loading.
    ///
    /// If already loaded, shows a status message. Otherwise, checks file
    /// size and either loads directly or shows a confirmation prompt.
    fn handle_full_history_load(&mut self) {
        if self.full_history_loaded {
            self.status_message = Some("Full history already loaded".to_string());
            return;
        }

        // Need an active session to load.
        let session = match self.get_active_session() {
            Some(s) => s,
            None => {
                self.status_message = Some("No active session to load".to_string());
                return;
            }
        };

        let total_size = session_file_size(&session);
        if total_size > Self::FULL_LOAD_SIZE_THRESHOLD {
            let size_mb = total_size as f64 / (1024.0 * 1024.0);
            self.full_load_pending_size_mb = size_mb;
            self.full_load_confirm_pending = true;
            self.status_message = Some(format!(
                "Session is {:.1} MB. Load full history? (y/n)",
                size_mb
            ));
        } else {
            self.perform_full_history_load();
        }
    }

    /// Perform the actual full history load.
    ///
    /// Loads all visible entries from the active session, replaces the
    /// ring buffer contents, restores scroll position (distance from bottom),
    /// cancels search, and sets the `full_history_loaded` flag.
    fn perform_full_history_load(&mut self) {
        let session = match self.get_active_session() {
            Some(s) => s,
            None => return,
        };

        // Save the distance from bottom (so we can restore position after load).
        let distance_from_bottom = self.scroll_mode.as_ref().map(|sm| sm.offset).unwrap_or(0);

        let (entries, offsets) =
            load_full_session(&session, &self.filter_state, self.config.verbose);
        let entry_count = entries.len();

        // Replace ring buffer contents.
        self.ring_buffer.clear();
        for entry in entries {
            self.ring_buffer.push(entry);
        }
        self.replay_offsets = offsets;

        // Cancel search (matches would be stale).
        self.cancel_search();

        // Restore scroll position: if we were in scroll mode, re-enter it
        // at the same distance from bottom.
        if distance_from_bottom > 0 {
            self.pending_scroll = Some(PendingScroll::Up(distance_from_bottom));
        } else {
            // Exit scroll mode to show the latest entries.
            self.exit_scroll_mode();
        }

        self.full_history_loaded = true;
        self.status_message = Some(format!("Loaded full history ({} entries)", entry_count));
    }

    /// Get the currently active session, if any.
    fn get_active_session(&self) -> Option<Session> {
        let active_id = self.active_session_id.as_ref()?;
        self.sessions.iter().find(|s| &s.id == active_id).cloned()
    }

    // -- Filter menu -----------------------------------------------------

    /// Open the filter menu, populating it with the current filter state
    /// and known agents from the ring buffer.
    fn open_filter_menu(&mut self) {
        let known_agents = self.collect_known_agents();
        self.filter_menu.open(
            self.filter_state.hide_tool_calls,
            self.filter_state.selected_agent.clone(),
            known_agents,
        );
    }

    /// Apply the current filter menu selections to the app filter state.
    ///
    /// Called immediately on each menu selection (MenuAction::Selected).
    fn apply_filter_from_menu(&mut self) {
        self.filter_state.hide_tool_calls = self.filter_menu.hide_tool_calls;
        self.filter_state.selected_agent = self.filter_menu.selected_agent.clone();

        // Exit scroll mode when filters change (content snapshot is stale).
        self.exit_scroll_mode();

        // Cancel search when filters change (matches would be stale).
        self.cancel_search();
    }

    // -- Search ------------------------------------------------------------

    /// Cancel any active search, resetting to Inactive state.
    pub fn cancel_search(&mut self) {
        self.search_state.cancel();
    }

    /// Force scroll mode when search is confirmed.
    ///
    /// This ensures highlights are stable and navigable. If scroll mode
    /// is not already active, sets a pending scroll to the bottom.
    fn force_scroll_mode_for_search(&mut self) {
        if self.is_in_scroll_mode() {
            // Already in scroll mode — invalidate the snapshot so the next render
            // rebuilds lines and recomputes search highlights.
            self.invalidate_scroll_snapshot();
        } else {
            // Set a pending scroll so the render phase creates a snapshot.
            // We use Up(0) which will create the snapshot at the current bottom position.
            self.pending_scroll = Some(PendingScroll::Up(0));
        }
    }

    /// Invalidate the current scroll snapshot, forcing a rebuild on the next render.
    ///
    /// Preserves the current scroll offset by converting the snapshot back to a
    /// pending scroll. This is used when search highlights need to be recomputed
    /// (e.g., after navigating to next/prev match or canceling search).
    fn invalidate_scroll_snapshot(&mut self) {
        if let Some(scroll) = self.scroll_mode.take() {
            self.pending_scroll = Some(PendingScroll::Up(scroll.offset));
        }
    }

    /// Scroll the view so the current search match is visible.
    ///
    /// Converts the target logical line index to a visual (wrapped) line
    /// position and adjusts the scroll offset to centre it in the viewport.
    pub fn scroll_to_current_search_match(&mut self) {
        let target_line = match self.search_state.current_match_line() {
            Some(line) => line,
            None => return,
        };

        if let Some(ref mut sm) = self.scroll_mode {
            let max_offset = sm.total_visual_lines.saturating_sub(sm.visible_height);
            let half_visible = sm.visible_height / 2;

            // Convert logical line index → visual line position.
            let target_visual = visual_line_position(&sm.lines, target_line, sm.inner_width);

            // Centre the target visual line in the viewport.
            let desired_ratatui_top = target_visual.saturating_sub(half_visible);
            // Convert to our offset system (0 = bottom):
            //   ratatui_scroll = max_offset - offset
            let new_offset = max_offset.saturating_sub(desired_ratatui_top);
            sm.offset = new_offset.min(max_offset);
        }
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
        assert!(!app.filter_state.is_active());
    }

    // -- Filter menu integration tests ------------------------------------

    #[test]
    fn test_new_defaults_filter_fields() {
        let app = App::new(test_config());
        assert!(!app.filter_state.is_active());
        assert!(!app.filter_menu.visible);
    }

    #[test]
    fn test_f_key_opens_filter_menu() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);
    }

    #[test]
    fn test_q_does_not_quit_when_filter_menu_is_open() {
        let mut app = App::new(test_config());
        // Open the filter menu
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);

        // 'q' inside filter menu should NOT quit; unknown keys are consumed
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_quit);
        assert!(app.filter_menu.visible);
    }

    #[test]
    fn test_esc_closes_filter_menu() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);

        // Esc should close
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.filter_menu.visible);
    }

    #[test]
    fn test_f_toggles_filter_menu_closed() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);

        // Pressing 'f' again inside menu should close it
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(!app.filter_menu.visible);
    }

    #[test]
    fn test_enter_toggles_tool_calls_in_filter_menu() {
        let mut app = App::new(test_config());
        assert!(!app.filter_state.hide_tool_calls);

        // Open filter menu (first item is ToolCallToggle, selected by default)
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);

        // Enter should toggle tool calls and apply immediately
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.filter_state.hide_tool_calls);
        assert!(app.filter_menu.visible); // menu stays open on selection

        // Toggle back
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.filter_state.hide_tool_calls);
    }

    #[test]
    fn test_ctrl_c_closes_filter_menu_when_visible() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.filter_menu.visible);
        assert!(!app.should_quit); // should NOT quit
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
    fn test_apply_filter_from_menu_updates_filter_state() {
        let mut app = App::new(test_config());
        // Open filter menu
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));

        // Toggle tool calls (Enter on first item)
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.filter_state.hide_tool_calls);
        assert!(app.filter_state.is_active());
        let display = app.filter_state.display().unwrap();
        assert!(display.contains("no tools"));
    }

    #[test]
    fn test_filter_state_display_reflects_tool_call_toggle() {
        let mut app = App::new(test_config());
        // Open filter menu
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));

        // Toggle tool calls on
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.filter_state.display(),
            Some("[filter: no tools]".to_string())
        );

        // Toggle back off
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.filter_state.display(), None);
    }

    #[test]
    fn test_slash_key_does_not_open_filter() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        // '/' opens search, not the filter menu
        assert!(!app.filter_menu.visible);
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
    fn test_escape_dismisses_help_overlay() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.help_overlay_visible);
    }

    #[test]
    fn test_question_mark_toggles_help_overlay_off() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(!app.help_overlay_visible);
    }

    #[test]
    fn test_random_key_does_not_dismiss_help_overlay() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        // Help should still be visible; random keys are consumed but don't close.
        assert!(app.help_overlay_visible);
    }

    #[test]
    fn test_keys_consumed_while_help_overlay_visible() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        // 'q' should not quit; it is consumed by the overlay.
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.help_overlay_visible);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_ctrl_c_consumed_while_help_overlay_visible() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        // Ctrl+C is consumed by the overlay (does not quit).
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.help_overlay_visible);
        assert!(!app.should_quit);
    }

    #[test]
    fn test_question_mark_does_not_open_help_when_filter_menu_active() {
        let mut app = App::new(test_config());
        // Open the filter menu first.
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);

        // '?' inside filter menu should be consumed, not open help.
        app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert!(!app.help_overlay_visible);
        assert!(app.filter_menu.visible);
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
    ///
    /// Uses `total_lines` as `total_visual_lines` (assumes no wrapping)
    /// and a default inner_width of 80.
    fn app_with_scroll_mode(offset: usize, total_lines: usize, visible_height: usize) -> App {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.scroll_mode = Some(ScrollMode {
            lines: Vec::new(),
            offset,
            total_lines,
            total_visual_lines: total_lines,
            visible_height,
            inner_width: 80,
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
    fn test_apply_filter_from_menu_exits_scroll_mode() {
        let mut app = app_with_scroll_mode(10, 100, 20);
        // Simulate applying a filter from the menu
        app.filter_menu.visible = true;
        app.filter_menu.hide_tool_calls = true;
        app.apply_filter_from_menu();
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
    fn test_on_mouse_ignored_when_filter_menu_visible() {
        use crossterm::event::{MouseEvent, MouseEventKind};

        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.filter_menu.visible = true;
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

    // -- Search tests --------------------------------------------------------

    #[test]
    fn test_new_defaults_search_state() {
        let app = App::new(test_config());
        assert!(!app.search_state.is_active());
        assert!(!app.search_state.is_input());
    }

    #[test]
    fn test_slash_key_starts_search_input() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.search_state.is_input());
    }

    #[test]
    fn test_search_input_typing_and_confirm() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.search_state.is_input());

        // Type "test"
        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(app.search_state.input_buffer, "test");

        // Confirm
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.search_state.is_active());
        assert_eq!(app.search_state.query, "test");
    }

    #[test]
    fn test_search_input_escape_cancels() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.search_state.is_input());
        assert!(!app.search_state.is_active());
    }

    #[test]
    fn test_search_input_backspace() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.search_state.input_buffer, "a");
    }

    #[test]
    fn test_search_input_ctrl_c_cancels_instead_of_quit() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(app.search_state.is_input());

        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.search_state.is_input());
        assert!(!app.should_quit);
    }

    #[test]
    fn test_search_q_does_not_quit_in_input_mode() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        // 'q' should be typed into the search buffer, not quit
        assert!(!app.should_quit);
        assert_eq!(app.search_state.input_buffer, "q");
    }

    #[test]
    fn test_search_active_n_navigates_next() {
        use crate::search::SearchMatch;

        let mut app = App::new(test_config());
        // Manually set up active search with matches
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.query = "test".to_string();
        app.search_state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 4,
            },
            SearchMatch {
                line_index: 5,
                byte_start: 0,
                byte_len: 4,
            },
        ];
        app.search_state.current_match_index = Some(0);

        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(app.search_state.current_match_index, Some(1));
    }

    #[test]
    fn test_search_active_shift_n_navigates_prev() {
        use crate::search::SearchMatch;

        let mut app = App::new(test_config());
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.query = "test".to_string();
        app.search_state.matches = vec![
            SearchMatch {
                line_index: 0,
                byte_start: 0,
                byte_len: 4,
            },
            SearchMatch {
                line_index: 5,
                byte_start: 0,
                byte_len: 4,
            },
        ];
        app.search_state.current_match_index = Some(1);

        app.on_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE));
        assert_eq!(app.search_state.current_match_index, Some(0));
    }

    #[test]
    fn test_search_active_escape_cancels() {
        let mut app = App::new(test_config());
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.query = "test".to_string();

        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.search_state.is_active());
        assert!(app.search_state.query.is_empty());
    }

    #[test]
    fn test_search_cancelled_on_filter_change() {
        let mut app = App::new(test_config());
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.query = "test".to_string();

        // Simulate applying a filter from the menu
        app.filter_menu.visible = true;
        app.filter_menu.hide_tool_calls = true;
        app.apply_filter_from_menu();

        assert!(!app.search_state.is_active());
    }

    #[test]
    fn test_search_cancelled_on_session_switch() {
        let mut app = App::new(test_config());
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.query = "test".to_string();
        app.sessions = vec![dummy_session("s1")];
        app.selected_session_index = 0;

        app.confirm_session_selection();

        assert!(!app.search_state.is_active());
    }

    #[test]
    fn test_search_confirm_forces_scroll_mode() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.search_state.is_active());
        // Should have a pending scroll to enter scroll mode
        assert!(app.pending_scroll.is_some());
    }

    #[test]
    fn test_slash_does_not_open_search_when_filter_menu_visible() {
        let mut app = App::new(test_config());
        app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(app.filter_menu.visible);

        // '/' inside filter menu should be consumed, not start search
        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(!app.search_state.is_input());
    }

    #[test]
    fn test_slash_does_not_open_search_when_help_visible() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        app.on_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(!app.search_state.is_input());
    }

    #[test]
    fn test_n_is_noop_when_search_not_active() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;

        // 'n' when search is not active should be a no-op
        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(!app.search_state.is_active());
    }

    #[test]
    fn test_scroll_to_search_match() {
        use ratatui::text::Line;

        // Create 100 short lines that don't wrap at width 80.
        let lines: Vec<Line<'static>> = (0..100)
            .map(|i| Line::from(format!("line {}", i)))
            .collect();

        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.scroll_mode = Some(ScrollMode {
            total_visual_lines: 100,
            total_lines: 100,
            visible_height: 20,
            inner_width: 80,
            offset: 0,
            lines,
        });
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.matches = vec![crate::search::SearchMatch {
            line_index: 50,
            byte_start: 0,
            byte_len: 4,
        }];
        app.search_state.current_match_index = Some(0);

        app.scroll_to_current_search_match();

        // The offset should be adjusted so line 50 is visible
        let offset = app.scroll_mode.as_ref().unwrap().offset;
        assert!(offset > 0, "offset should have changed from 0");
    }

    #[test]
    fn test_visual_line_helpers() {
        use ratatui::text::{Line, Span};

        // Short lines (width < 80) → 1 visual line each.
        let lines: Vec<Line> = vec![Line::from("short"), Line::from("also short")];
        assert_eq!(wrapped_line_height(&lines[0], 80), 1);
        assert_eq!(total_visual_lines(&lines, 80), 2);
        assert_eq!(visual_line_position(&lines, 0, 80), 0);
        assert_eq!(visual_line_position(&lines, 1, 80), 1);

        // A 160-char line at width 80 → wraps to 2 visual lines.
        let long = "x".repeat(160);
        let lines_with_wrap: Vec<Line> = vec![Line::from(Span::raw(long)), Line::from("after")];
        assert_eq!(wrapped_line_height(&lines_with_wrap[0], 80), 2);
        assert_eq!(total_visual_lines(&lines_with_wrap, 80), 3);
        // "after" starts at visual line 2.
        assert_eq!(visual_line_position(&lines_with_wrap, 1, 80), 2);
    }

    #[test]
    fn test_scroll_to_search_match_with_wrapping() {
        use ratatui::text::{Line, Span};

        // 10 lines: first 5 are 160-chars wide (wrap to 2 visual lines each at width 80).
        // Lines 5..9 are short (1 visual line each).
        // Total visual lines = 5*2 + 5*1 = 15.
        let mut lines: Vec<Line<'static>> = Vec::new();
        for _ in 0..5 {
            lines.push(Line::from(Span::raw("x".repeat(160))));
        }
        for i in 5..10 {
            lines.push(Line::from(format!("short line {}", i)));
        }

        let vis_total = total_visual_lines(&lines, 80);
        assert_eq!(vis_total, 15);

        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.scroll_mode = Some(ScrollMode {
            total_visual_lines: vis_total,
            total_lines: 10,
            visible_height: 10,
            inner_width: 80,
            offset: 0,
            lines,
        });

        // Match on logical line 7 → visual line position = 5*2 + 2*1 = 12.
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.matches = vec![crate::search::SearchMatch {
            line_index: 7,
            byte_start: 0,
            byte_len: 4,
        }];
        app.search_state.current_match_index = Some(0);

        app.scroll_to_current_search_match();

        let sm = app.scroll_mode.as_ref().unwrap();
        let max_visual = sm.total_visual_lines.saturating_sub(sm.visible_height); // 15 - 10 = 5
        let ratatui_scroll = max_visual.saturating_sub(sm.offset);
        // Target visual line is 12, viewport height 10.
        // The target should be within [ratatui_scroll, ratatui_scroll + 10).
        assert!(
            ratatui_scroll <= 12 && 12 < ratatui_scroll + 10,
            "target visual line 12 not in viewport [{}, {})",
            ratatui_scroll,
            ratatui_scroll + 10
        );
    }

    // -- Full history load tests -------------------------------------------

    #[test]
    fn test_new_defaults_full_history_fields() {
        let app = App::new(test_config());
        assert!(!app.full_history_loaded);
        assert!(!app.full_load_confirm_pending);
        assert_eq!(app.full_load_pending_size_mb, 0.0);
    }

    #[test]
    fn test_l_key_no_active_session_shows_status() {
        let mut app = App::new(test_config());
        // No active session, no sessions at all
        app.on_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::NONE));
        assert!(!app.full_history_loaded);
        assert!(app.status_message.is_some());
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .contains("No active session"));
    }

    #[test]
    fn test_l_key_triggers_load_with_active_session() {
        use std::fs;
        use std::io::Write;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("test-sess.jsonl");

        // Write a small JSONL file (well under 50 MB)
        let mut file = fs::File::create(&log_path).unwrap();
        writeln!(
            file,
            r#"{{"type":"user","timestamp":"2025-01-15T10:00:00Z","message":{{"role":"user","content":[{{"type":"text","text":"hello"}}]}}}}"#
        )
        .unwrap();

        let mut app = App::new(test_config());
        app.sessions = vec![Session {
            id: "test-sess".to_string(),
            agents: vec![crate::session::Agent {
                agent_id: None,
                slug: None,
                log_path,
                is_main: true,
            }],
            last_modified: std::time::SystemTime::now(),
        }];
        app.active_session_id = Some("test-sess".to_string());

        app.on_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::NONE));

        assert!(app.full_history_loaded);
        assert!(!app.full_load_confirm_pending);
        assert!(app.status_message.is_some());
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .contains("Loaded full history"));
    }

    #[test]
    fn test_full_history_loaded_flag_prevents_reload() {
        let mut app = App::new(test_config());
        app.full_history_loaded = true;
        app.sessions = vec![dummy_session("s1")];
        app.active_session_id = Some("s1".to_string());

        app.on_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::NONE));

        // Should show "already loaded" message, not reload
        assert!(app.status_message.is_some());
        assert!(app
            .status_message
            .as_ref()
            .unwrap()
            .contains("already loaded"));
    }

    #[test]
    fn test_confirmation_flow_y_accepts() {
        let mut app = App::new(test_config());
        app.full_load_confirm_pending = true;
        app.full_load_pending_size_mb = 75.0;
        // Need an active session for perform_full_history_load
        app.sessions = vec![dummy_session("s1")];
        app.active_session_id = Some("s1".to_string());

        app.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert!(!app.full_load_confirm_pending);
        // full_history_loaded will be true (even if file doesn't exist, the flag is set)
        assert!(app.full_history_loaded);
    }

    #[test]
    fn test_confirmation_flow_n_cancels() {
        let mut app = App::new(test_config());
        app.full_load_confirm_pending = true;
        app.full_load_pending_size_mb = 75.0;

        app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

        assert!(!app.full_load_confirm_pending);
        assert!(!app.full_history_loaded);
        assert!(app.status_message.is_some());
        assert!(app.status_message.as_ref().unwrap().contains("cancelled"));
    }

    #[test]
    fn test_confirmation_flow_esc_cancels() {
        let mut app = App::new(test_config());
        app.full_load_confirm_pending = true;
        app.full_load_pending_size_mb = 75.0;

        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!app.full_load_confirm_pending);
        assert!(!app.full_history_loaded);
    }

    #[test]
    fn test_confirmation_pending_consumes_other_keys() {
        let mut app = App::new(test_config());
        app.full_load_confirm_pending = true;

        // 'q' should be consumed, not quit
        app.on_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.should_quit);
        assert!(app.full_load_confirm_pending);
    }

    #[test]
    fn test_session_switch_resets_full_history_loaded() {
        let mut app = App::new(test_config());
        app.full_history_loaded = true;
        app.full_load_confirm_pending = true;
        app.sessions = vec![dummy_session("s1"), dummy_session("s2")];
        app.selected_session_index = 1;

        app.confirm_session_selection();

        assert!(!app.full_history_loaded);
        assert!(!app.full_load_confirm_pending);
    }

    #[test]
    fn test_l_key_does_not_open_in_help_overlay() {
        let mut app = App::new(test_config());
        app.help_overlay_visible = true;

        app.on_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::NONE));
        // 'L' should be consumed by help overlay, not trigger load
        assert!(!app.full_history_loaded);
        assert!(app.help_overlay_visible); // help still visible
    }

    // -- Symbols / ASCII mode tests ----------------------------------------

    #[test]
    fn test_app_new_default_uses_unicode_symbols() {
        let app = App::new(test_config());
        assert_eq!(app.symbols.active_marker, "\u{25cf}");
    }

    #[test]
    fn test_app_new_ascii_mode_uses_ascii_symbols() {
        let mut config = test_config();
        config.ascii = true;
        let app = App::new(config);
        assert_eq!(app.symbols.active_marker, "*");
        assert_eq!(app.symbols.tree_connector, "`-");
        assert_eq!(app.symbols.progress_indicator, ">");
        assert_eq!(app.symbols.search_cursor, "_");
    }

    // -- needs_redraw dirty-flag tests ----------------------------------------

    #[test]
    fn test_new_defaults_needs_redraw_true() {
        let app = App::new(test_config());
        assert!(
            app.needs_redraw,
            "App::new() should set needs_redraw to true"
        );
    }

    #[test]
    fn test_on_key_sets_needs_redraw() {
        let mut app = App::new(test_config());
        app.needs_redraw = false;

        // Any key should set the dirty flag.
        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(app.needs_redraw, "on_key() should set needs_redraw to true");
    }

    #[test]
    fn test_on_mouse_sets_needs_redraw() {
        let mut app = App::new(test_config());
        app.focus = Focus::LogStream;
        app.needs_redraw = false;

        let mouse = MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        app.on_mouse(mouse);
        assert!(
            app.needs_redraw,
            "on_mouse() should set needs_redraw to true"
        );
    }

    #[test]
    fn test_on_new_log_entry_sets_needs_redraw() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = App::new(test_config());
        app.needs_redraw = false;

        let entry = parse_jsonl_line(
            r#"{"type": "user", "message": {"role": "user", "content": "hello"}}"#,
        )
        .unwrap();
        app.on_new_log_entry(entry);
        assert!(
            app.needs_redraw,
            "on_new_log_entry() should set needs_redraw to true"
        );
    }

    #[test]
    fn test_on_new_file_detected_sets_needs_redraw() {
        let mut app = App::new(test_config());
        app.needs_redraw = false;

        // Even though this path won't classify to anything meaningful,
        // the method should still set the dirty flag.
        app.on_new_file_detected(std::path::PathBuf::from("/fake/new-file.jsonl"));
        assert!(
            app.needs_redraw,
            "on_new_file_detected() should set needs_redraw to true"
        );
    }

    #[test]
    fn test_needs_redraw_false_after_manual_clear() {
        let mut app = App::new(test_config());
        assert!(app.needs_redraw);

        app.needs_redraw = false;
        assert!(
            !app.needs_redraw,
            "needs_redraw should be false after manual clear"
        );
    }

    #[test]
    fn test_mark_dirty_sets_needs_redraw() {
        let mut app = App::new(test_config());
        app.needs_redraw = false;

        app.mark_dirty();
        assert!(
            app.needs_redraw,
            "mark_dirty() should set needs_redraw to true"
        );
    }
}
