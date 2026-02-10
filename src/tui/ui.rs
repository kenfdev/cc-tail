//! Layout and rendering for the TUI.
//!
//! Implements the three-panel layout:
//! - **Sidebar** (left, width 30): session list
//! - **Log stream** (right, fills remaining width): log entries
//! - **Status bar** (bottom, height 1): key hints and status info

use std::time::SystemTime;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::content_render::{has_renderable_content, render_content_blocks, RenderedLine};
use crate::log_entry::{EntryType, LogEntry};
use crate::search::{find_matches, SearchMatch};
use crate::session::SessionStatus;
use crate::session_stats::compute_session_stats;
use crate::theme::ThemeColors;
use crate::tui::app::{App, Focus, ScrollMode};


// ---------------------------------------------------------------------------
// Main draw function
// ---------------------------------------------------------------------------

/// Draw the entire TUI frame.
///
/// Splits the terminal into:
/// 1. A vertical split: main area (fills) + status bar (1 row)
/// 2. Within main area, a horizontal split: sidebar (30 cols) + log stream (rest)
///
/// When the sidebar is hidden, the log stream takes the full width.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    // Vertical split: main area + status bar
    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(size);

    let main_area = vertical_chunks[0];
    let status_area = vertical_chunks[1];

    if app.sidebar_visible {
        // Horizontal split: sidebar + log stream
        let horizontal_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(30), Constraint::Min(20)])
            .split(main_area);

        let sidebar_area = horizontal_chunks[0];
        let logstream_area = horizontal_chunks[1];

        draw_sidebar(frame, app, sidebar_area);
        draw_logstream(frame, app, logstream_area);
    } else {
        // No sidebar; log stream takes all width
        draw_logstream(frame, app, main_area);
    }

    // Show search input bar when in search input mode, otherwise status bar.
    if app.search_state.is_input() {
        draw_search_input_bar(frame, app, status_area);
    } else {
        draw_status_bar(frame, app, status_area);
    }

    // Draw filter menu on top of everything when visible.
    if app.filter_menu.visible {
        draw_filter_menu(frame, app, size);
    }

    // Draw help overlay on top of everything when visible.
    if app.help_overlay_visible {
        draw_help_overlay(frame, app, size);
    }
}

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

/// Draw the sidebar panel with the session list as a tree layout.
///
/// Each session renders as:
///   `● abc123  5m`     (header row: active marker + 6-char ID prefix + relative time)
///   `  └ slug-name`    (one row per non-main agent, indented)
///
/// Navigation operates at session level; j/k skip over agent child rows.
/// New sessions (in `app.new_session_ids`) are highlighted in bold yellow.
fn draw_sidebar(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Sidebar;
    let border_style = if focused {
        Style::default().fg(app.theme_colors.border_focused)
    } else {
        Style::default().fg(app.theme_colors.border_unfocused)
    };

    let block = Block::default()
        .title(" Sessions ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if app.sessions.is_empty() {
        let placeholder = Paragraph::new("No sessions found")
            .style(Style::default().fg(app.theme_colors.sidebar_placeholder))
            .block(block);
        frame.render_widget(placeholder, area);
        return;
    }

    // Compute the inner area height for scroll adjustment.
    let inner = block.inner(area);
    let visible_height = inner.height as usize;

    // Adjust scroll offset so the selected session is visible.
    // This must happen before we borrow theme immutably.
    app.adjust_sidebar_scroll(visible_height);

    // Now take an immutable reference to the theme for the remainder.
    let theme = &app.theme_colors;

    // Build the full list of visual rows (session headers + agent children).
    let mut all_rows: Vec<ListItem> = Vec::new();
    // Max width available inside the block (inner width).
    let max_width = inner.width as usize;

    for (i, session) in app.sessions.iter().enumerate() {
        let is_selected = i == app.selected_session_index;
        let is_new = app.new_session_ids.contains(&session.id);
        let is_active_target = app.active_session_id.as_ref() == Some(&session.id);

        // -- Session header row --

        // Active marker: "● " (or "* " in ASCII mode) for active sessions, "  " for inactive.
        let marker = match session.status() {
            SessionStatus::Active => Span::styled(
                format!("{} ", app.symbols.active_marker),
                Style::default()
                    .fg(theme.sidebar_active_marker)
                    .add_modifier(Modifier::BOLD),
            ),
            SessionStatus::Inactive => {
                Span::styled("  ", Style::default().fg(theme.sidebar_inactive_marker))
            }
        };

        // 6-char ID prefix.
        let id_prefix: String = session.id.chars().take(6).collect();

        // Relative timestamp.
        let rel_time = format_relative_time(session.last_modified);

        // Build the display text: "abc123  5m"
        let header_text = format!("{}  {}", id_prefix, rel_time);

        // Determine style based on selection and new-session status.
        let header_style = if is_selected {
            Style::default()
                .fg(theme.sidebar_selected_fg)
                .bg(theme.sidebar_selected_bg)
                .add_modifier(Modifier::BOLD)
        } else if is_new {
            Style::default()
                .fg(theme.sidebar_new_session)
                .add_modifier(Modifier::BOLD)
        } else if is_active_target {
            Style::default().fg(theme.sidebar_active_target)
        } else {
            Style::default().fg(theme.sidebar_default_session)
        };

        let header_line = Line::from(vec![marker, Span::styled(header_text, header_style)]);
        all_rows.push(ListItem::new(header_line));

        // -- Agent child rows (non-main agents) --
        let sub_agents: Vec<_> = session.agents.iter().filter(|a| !a.is_main).collect();
        for agent in &sub_agents {
            let slug_display = agent
                .slug
                .as_deref()
                .or(agent.agent_id.as_deref())
                .unwrap_or("unknown");

            // Indent: "  └ slug-name" (2 spaces + corner + space + slug)
            let prefix = format!("  {} ", app.symbols.tree_connector);
            let available = max_width.saturating_sub(prefix.len());
            let truncated_slug = if slug_display.len() > available {
                format!("{}...", &slug_display[..available.saturating_sub(3)])
            } else {
                slug_display.to_string()
            };

            let child_style = if is_selected {
                Style::default()
                    .fg(theme.sidebar_selected_child_fg)
                    .bg(theme.sidebar_selected_child_bg)
            } else {
                Style::default().fg(theme.sidebar_unselected_child)
            };

            let child_line = Line::from(vec![
                Span::styled(prefix, Style::default().fg(theme.sidebar_child_prefix)),
                Span::styled(truncated_slug, child_style),
            ]);
            all_rows.push(ListItem::new(child_line));
        }
    }

    // Apply scroll offset: skip rows before the offset, take visible_height rows.
    let scrolled_rows: Vec<ListItem> = all_rows
        .into_iter()
        .skip(app.sidebar_scroll_offset)
        .take(visible_height)
        .collect();

    let list = List::new(scrolled_rows).block(block);
    frame.render_widget(list, area);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a `SystemTime` as a human-readable relative duration from now.
///
/// Examples: "now", "30s", "5m", "2h", "3d", "5w"
fn format_relative_time(time: SystemTime) -> String {
    let elapsed = match SystemTime::now().duration_since(time) {
        Ok(d) => d,
        Err(_) => return "now".to_string(),
    };

    let secs = elapsed.as_secs();
    if secs < 5 {
        "now".to_string()
    } else if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else if secs < 604800 {
        format!("{}d", secs / 86400)
    } else {
        format!("{}w", secs / 604800)
    }
}

// ---------------------------------------------------------------------------
// Log stream
// ---------------------------------------------------------------------------

/// Draw the log stream panel.
///
/// Filters entries by `active_session_id` (if set), renders each entry
/// as styled lines with timestamps, role indicators, optional agent
/// prefixes, and content text. Uses ratatui word-wrap and auto-scrolls
/// to the bottom in normal mode.
///
/// Three rendering branches:
/// - **Branch A**: `scroll_mode` is active -- render from frozen snapshot.
/// - **Branch B**: `pending_scroll` is set -- build lines, create snapshot,
///   apply pending action, render from new snapshot.
/// - **Branch C**: normal -- existing auto-scroll behavior.
fn draw_logstream(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme_colors;
    let focused = app.focus == Focus::LogStream;
    let border_style = if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border_unfocused)
    };

    // Dynamic title: show scroll indicator when in scroll mode or pending scroll.
    let title = if app.scroll_mode.is_some() || app.pending_scroll.is_some() {
        " Log Stream [SCROLL mode - Esc:exit] "
    } else {
        " Log Stream "
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    let inner_height = inner.height as usize;
    let inner_width = inner.width;

    // -- Branch A: scroll_mode already active -- render from frozen snapshot.
    if let Some(ref scroll) = app.scroll_mode {
        // scroll.offset is "visual lines from the bottom": 0 = bottom, max = top.
        // Convert to ratatui scroll (visual lines from the top).
        let max_visual = scroll.total_visual_lines.saturating_sub(scroll.visible_height);
        let ratatui_scroll = max_visual.saturating_sub(scroll.offset);
        let paragraph = Paragraph::new(scroll.lines.clone())
            .style(Style::default().fg(theme.logstream_text))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((ratatui_scroll as u16, 0));
        frame.render_widget(paragraph, area);
        return;
    }

    // -- Build lines from the ring buffer (used by both Branch B and C). --
    let filter_state = &app.filter_state;

    // Entry-type visibility predicate: User, Assistant, System are always
    // visible; Progress, FileHistorySnapshot and other types are always hidden.
    let is_type_visible = |e: &LogEntry| -> bool {
        match e.entry_type {
            EntryType::User => {
                // Skip user entries that only contain tool_result blocks
                // (they produce no visible output and would show as empty lines).
                e.message
                    .as_ref()
                    .is_none_or(|msg| has_renderable_content(&msg.content))
            }
            EntryType::Assistant | EntryType::System => true,
            _ => false,
        }
    };

    let entries: Vec<&LogEntry> = if let Some(ref session_id) = app.active_session_id {
        app.ring_buffer
            .iter_filtered(|e| {
                e.session_id.as_deref() == Some(session_id.as_str())
                    && is_type_visible(e)
                    && filter_state.matches(e)
            })
            .collect()
    } else {
        app.ring_buffer
            .iter_filtered(|e| is_type_visible(e) && filter_state.matches(e))
            .collect()
    };

    if entries.is_empty() {
        // Clear pending scroll if there are no entries to snapshot.
        app.pending_scroll = None;
        let paragraph = Paragraph::new("Waiting for log entries...")
            .style(Style::default().fg(theme.logstream_placeholder))
            .block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    // Build styled lines for all entries.
    let mut lines: Vec<Line<'static>> = Vec::new();

    for entry in &entries {
        let ts = format_timestamp(&entry.timestamp);
        let ts_span = Span::styled(
            ts,
            Style::default()
                .fg(theme.logstream_timestamp)
                .add_modifier(Modifier::DIM),
        );

        // Progress entries have a special rendering path.
        if entry.entry_type == EntryType::Progress {
            let description = extract_progress_description(entry);
            let spans = vec![
                ts_span,
                Span::raw(" "),
                Span::styled(
                    format!("{} {}", app.symbols.progress_indicator, description),
                    Style::default().fg(theme.logstream_progress),
                ),
            ];
            lines.push(Line::from(spans));
            continue;
        }

        // Determine the entry-level role from the message.
        let entry_role = entry
            .message
            .as_ref()
            .and_then(|m| m.role.as_deref())
            .unwrap_or("unknown");

        let prefix = agent_prefix(entry);
        let prefix_span = prefix.map(|p| {
            Span::styled(
                format!(" {}", p),
                Style::default().fg(agent_color(entry, theme)),
            )
        });

        // Render content blocks from the message.
        let rendered = entry
            .message
            .as_ref()
            .map(|m| render_content_blocks(&m.content))
            .unwrap_or_default();

        if rendered.is_empty() {
            // Even with no content, show the timestamp + role indicator line.
            let (indicator, color) = role_indicator(entry_role, theme);
            let mut spans = vec![
                ts_span.clone(),
                Span::raw(" "),
                Span::styled(String::from(indicator), Style::default().fg(color)),
            ];
            if let Some(ref ps) = prefix_span {
                spans.push(ps.clone());
            }
            lines.push(Line::from(spans));
        } else {
            // Track which rendered-line index produced the first visible line
            // so we know when to attach the agent prefix.
            let mut first_visible = true;
            for rendered_line in rendered.iter() {
                // Skip tool call lines when tool call hiding is active.
                if !filter_state.is_tool_line_visible()
                    && matches!(rendered_line, RenderedLine::ToolUse(_))
                {
                    continue;
                }

                let (indicator, color, text) = match rendered_line {
                    RenderedLine::Text(t) => {
                        let (ind, col) = role_indicator(entry_role, theme);
                        (ind, col, t.as_str())
                    }
                    RenderedLine::ToolUse(t) => ('~', theme.role_tool_use, t.as_str()),
                    RenderedLine::Unknown(t) => ('?', theme.role_unknown, t.as_str()),
                };

                let mut spans = vec![
                    ts_span.clone(),
                    Span::raw(" "),
                    Span::styled(String::from(indicator), Style::default().fg(color)),
                ];

                // Only show agent prefix on the first visible line of each entry.
                if first_visible {
                    if let Some(ref ps) = prefix_span {
                        spans.push(ps.clone());
                    }
                    first_visible = false;
                }

                spans.push(Span::raw(" "));
                spans.push(Span::styled(text.to_string(), Style::default().fg(color)));

                lines.push(Line::from(spans));
            }
        }
    }

    // -- Search highlights: compute matches and apply highlights to lines. --
    if app.search_state.is_active() && !app.search_state.query.is_empty() {
        let query = &app.search_state.query;
        let mut all_matches: Vec<SearchMatch> = Vec::new();

        // Compute matches for each line.
        for (line_idx, line) in lines.iter().enumerate() {
            let line_text = line_to_text(line);
            let matches = find_matches(&line_text, query);
            for (byte_start, byte_len) in matches {
                all_matches.push(SearchMatch {
                    line_index: line_idx,
                    byte_start,
                    byte_len,
                });
            }
        }

        // Update the search state with computed matches.
        app.search_state.matches = all_matches;
        // If current_match_index is out of bounds, reset it.
        if let Some(idx) = app.search_state.current_match_index {
            if idx >= app.search_state.matches.len() {
                app.search_state.current_match_index = if app.search_state.matches.is_empty() {
                    None
                } else {
                    Some(0)
                };
            }
        }
        // Auto-select first match if no match is currently selected.
        if app.search_state.current_match_index.is_none() && !app.search_state.matches.is_empty() {
            app.search_state.current_match_index = Some(0);
        }

        // Apply highlights to lines.
        let search_matches = &app.search_state.matches;
        let current_match = app.search_state.current_match_index;
        let match_style = Style::default()
            .fg(theme.search_match_fg)
            .bg(theme.search_match_bg);
        let current_style = Style::default()
            .fg(theme.search_current_fg)
            .bg(theme.search_current_bg)
            .add_modifier(Modifier::BOLD);

        lines = apply_search_highlights(
            lines,
            search_matches,
            current_match,
            match_style,
            current_style,
        );
    } else if !app.search_state.is_active() {
        // Clear stale matches when search is not active.
        app.search_state.matches.clear();
        app.search_state.current_match_index = None;
    }

    // -- Branch B: pending_scroll -- create snapshot and apply pending action.
    if let Some(pending_action) = app.pending_scroll.take() {
        let total_lines = lines.len();
        let vis_total = crate::tui::app::total_visual_lines(&lines, inner_width);
        let mut scroll = ScrollMode {
            lines: lines.clone(),
            offset: 0,
            total_lines,
            total_visual_lines: vis_total,
            visible_height: inner_height,
            inner_width,
        };

        // Set initial offset to bottom (offset 0 = bottom), then apply action.
        // Offsets are in visual (wrapped) lines.
        let max_offset = vis_total.saturating_sub(inner_height);
        match pending_action {
            crate::tui::app::PendingScroll::Up(n) => {
                scroll.offset = n.min(max_offset);
            }
            crate::tui::app::PendingScroll::Down(_) => {
                // Scroll down from bottom is a no-op (already at bottom).
                scroll.offset = 0;
            }
            crate::tui::app::PendingScroll::ToTop => {
                scroll.offset = max_offset;
            }
            crate::tui::app::PendingScroll::HalfPageUp => {
                let half = inner_height / 2;
                scroll.offset = half.min(max_offset);
            }
            crate::tui::app::PendingScroll::HalfPageDown => {
                // Scroll down from bottom is a no-op (already at bottom).
                scroll.offset = 0;
            }
        }

        app.scroll_mode = Some(scroll);

        // Copy theme color before mutable borrow in scroll_to_current_search_match.
        let logstream_text_color = app.theme_colors.logstream_text;

        // If search is active with a current match, scroll to it.
        if app.search_state.is_active() && app.search_state.current_match_index.is_some() {
            app.scroll_to_current_search_match();
        }

        // Convert scroll.offset (visual lines from bottom) to ratatui scroll (visual lines from top).
        let scroll_ref = app.scroll_mode.as_ref().unwrap();
        let max_visual = scroll_ref.total_visual_lines.saturating_sub(scroll_ref.visible_height);
        let ratatui_scroll = max_visual.saturating_sub(scroll_ref.offset);
        let paragraph = Paragraph::new(scroll_ref.lines.clone())
            .style(Style::default().fg(logstream_text_color))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((ratatui_scroll as u16, 0));

        frame.render_widget(paragraph, area);
        return;
    }

    // -- Branch C: normal auto-scroll to bottom. --
    let vis_total = crate::tui::app::total_visual_lines(&lines, inner_width) as u16;
    let scroll_offset = vis_total.saturating_sub(inner_height as u16);

    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.logstream_text))
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset, 0));

    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Log stream helpers
// ---------------------------------------------------------------------------

/// Parse an ISO 8601 timestamp string to "HH:MM:SS" format.
///
/// Falls back to "--:--:--" if the timestamp is absent or malformed.
/// Supports formats like "2025-01-15T10:30:00Z" and "2025-01-15T10:30:00.123Z".
fn format_timestamp(ts: &Option<String>) -> String {
    let fallback = "--:--:--".to_string();
    let ts = match ts {
        Some(s) => s,
        None => return fallback,
    };

    // Find the 'T' separator in ISO 8601.
    let time_part = match ts.find('T') {
        Some(idx) => &ts[idx + 1..],
        None => return fallback,
    };

    // Extract HH:MM:SS (first 8 characters of the time part).
    if time_part.len() >= 8 && time_part.as_bytes()[2] == b':' && time_part.as_bytes()[5] == b':' {
        time_part[..8].to_string()
    } else {
        fallback
    }
}

/// Return a role indicator character and its color for the given message role.
fn role_indicator(role: &str, theme: &ThemeColors) -> (char, Color) {
    match role {
        "user" => ('>', theme.role_user),
        "assistant" => ('<', theme.role_assistant),
        _ => ('?', theme.role_unknown),
    }
}

/// Return an agent prefix string for subagent entries.
///
/// For subagent entries (`is_sidechain == Some(true)`), returns the last
/// word of the slug (e.g. `"effervescent-soaring-cook"` -> `"[cook]"`).
/// Falls back to `[agent_id_short]` if slug is absent. Returns `None`
/// for main agent entries.
fn agent_prefix(entry: &LogEntry) -> Option<String> {
    if entry.is_sidechain != Some(true) {
        return None;
    }

    if let Some(ref slug) = entry.slug {
        let last_word = slug.rsplit('-').next().unwrap_or(slug);
        Some(format!("[{}]", last_word))
    } else if let Some(ref agent_id) = entry.agent_id {
        let short = if agent_id.len() > 7 {
            &agent_id[..7]
        } else {
            agent_id
        };
        Some(format!("[{}]", short))
    } else {
        Some("[agent]".to_string())
    }
}

/// Deterministic 8-color palette hash for per-agent coloring.
///
/// Uses the agent slug (preferred) or agent_id to pick a consistent
/// color from the theme's agent palette. Main-agent entries (no
/// agent_id/slug) return the theme's main agent color.
fn agent_color(entry: &LogEntry, theme: &ThemeColors) -> Color {
    let key = entry.slug.as_deref().or(entry.agent_id.as_deref());

    match key {
        Some(k) => {
            // Simple hash: sum of bytes mod palette size.
            let hash: usize = k.bytes().map(|b| b as usize).sum();
            theme.agent_palette[hash % theme.agent_palette.len()]
        }
        None => theme.agent_main,
    }
}

/// Extract a human-readable description from a progress entry's `data` field.
///
/// Priority:
/// 1. `data.content` if it is a string
/// 2. `data.status` if it is a string
/// 3. Compact JSON serialization of `data`, truncated to 80 characters
/// 4. Fallback: `"(progress)"` if `data` is absent or null
fn extract_progress_description(entry: &LogEntry) -> String {
    let data = match entry.data {
        Some(ref d) if !d.is_null() => d,
        _ => return "(progress)".to_string(),
    };

    // Priority 1: data.content (string)
    if let Some(content) = data.get("content").and_then(|v| v.as_str()) {
        return content.to_string();
    }

    // Priority 2: data.status (string)
    if let Some(status) = data.get("status").and_then(|v| v.as_str()) {
        return status.to_string();
    }

    // Priority 3: compact JSON, truncated to 80 chars
    let json = serde_json::to_string(data).unwrap_or_else(|_| "(progress)".to_string());
    if json.len() > 80 {
        format!("{}...", &json[..77])
    } else {
        json
    }
}

// ---------------------------------------------------------------------------
// Search helpers
// ---------------------------------------------------------------------------

/// Concatenate the text content of all spans in a line into a single string.
///
/// This is used to compute search matches against the rendered line text.
fn line_to_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Apply search highlights to a list of lines.
///
/// For each search match, splits the affected span(s) and injects highlight
/// styling. The `current_match` index gets a distinct style from other matches.
///
/// This is a post-process approach: build lines normally, then inject highlights.
fn apply_search_highlights(
    lines: Vec<Line<'static>>,
    matches: &[SearchMatch],
    current_match: Option<usize>,
    match_style: Style,
    current_style: Style,
) -> Vec<Line<'static>> {
    if matches.is_empty() {
        return lines;
    }

    // Group matches by line index for efficient processing.
    let mut matches_by_line: std::collections::HashMap<usize, Vec<(usize, &SearchMatch)>> =
        std::collections::HashMap::new();
    for (global_idx, m) in matches.iter().enumerate() {
        matches_by_line
            .entry(m.line_index)
            .or_default()
            .push((global_idx, m));
    }

    lines
        .into_iter()
        .enumerate()
        .map(|(line_idx, line)| {
            if let Some(line_matches) = matches_by_line.get(&line_idx) {
                highlight_line(line, line_matches, current_match, match_style, current_style)
            } else {
                line
            }
        })
        .collect()
}

/// Apply search highlights to a single line.
///
/// Walks through the spans, tracking cumulative byte position, and splits
/// spans at match boundaries to inject highlight styles.
fn highlight_line(
    line: Line<'static>,
    line_matches: &[(usize, &SearchMatch)],
    current_match: Option<usize>,
    match_style: Style,
    current_style: Style,
) -> Line<'static> {
    // Build a sorted list of highlight regions as (byte_start, byte_end, is_current).
    let mut regions: Vec<(usize, usize, bool)> = line_matches
        .iter()
        .map(|(global_idx, m)| {
            let is_current = current_match == Some(*global_idx);
            (m.byte_start, m.byte_start + m.byte_len, is_current)
        })
        .collect();
    regions.sort_by_key(|r| r.0);

    let mut new_spans: Vec<Span<'static>> = Vec::new();
    let mut byte_offset: usize = 0;
    let mut region_idx = 0;

    for span in line.spans {
        let span_text: &str = span.content.as_ref();
        let span_start = byte_offset;
        let span_end = byte_offset + span_text.len();
        let original_style = span.style;

        let mut pos = span_start;

        while pos < span_end && region_idx < regions.len() {
            let (r_start, r_end, is_current) = regions[region_idx];

            if r_start >= span_end {
                // No more matches in this span.
                break;
            }

            if r_end <= pos {
                // This match is before our current position; skip it.
                region_idx += 1;
                continue;
            }

            // Emit the portion before the match (if any).
            if pos < r_start && r_start < span_end {
                let before_start = pos - span_start;
                let before_end = r_start - span_start;
                new_spans.push(Span::styled(
                    span_text[before_start..before_end].to_string(),
                    original_style,
                ));
                pos = r_start;
            }

            // Emit the highlighted portion.
            let hl_start = pos.max(r_start) - span_start;
            let hl_end = r_end.min(span_end) - span_start;
            if hl_start < hl_end {
                let style = if is_current {
                    current_style
                } else {
                    match_style
                };
                new_spans.push(Span::styled(
                    span_text[hl_start..hl_end].to_string(),
                    style,
                ));
            }

            pos = r_end.min(span_end);

            // If we've consumed the entire match region, advance to the next.
            if pos >= r_end {
                region_idx += 1;
            }
        }

        // Emit the remainder of the span after all matches.
        if pos < span_end {
            let remainder_start = pos - span_start;
            new_spans.push(Span::styled(
                span_text[remainder_start..].to_string(),
                original_style,
            ));
        }

        byte_offset = span_end;
    }

    Line::from(new_spans)
}

/// Draw the search input bar at the bottom of the screen.
///
/// Replaces the status bar when search input mode is active.
/// Shows `/ ` prompt followed by the current input buffer and a cursor.
fn draw_search_input_bar(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme_colors;

    let mut spans: Vec<Span> = vec![
        Span::styled(
            "/",
            Style::default()
                .fg(theme.search_prompt)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            app.search_state.input_buffer.clone(),
            Style::default().fg(theme.search_input_fg),
        ),
        // Cursor indicator.
        Span::styled(
            app.symbols.search_cursor.to_string(),
            Style::default()
                .fg(theme.search_input_fg)
                .add_modifier(Modifier::SLOW_BLINK),
        ),
    ];

    // Right-aligned hint.
    let hint = " Enter:search  Esc:cancel";
    let content_len = 1 + app.search_state.input_buffer.len() + 1; // "/" + input + cursor
    let remaining = (area.width as usize).saturating_sub(content_len + hint.len());
    if remaining > 0 {
        spans.push(Span::raw(" ".repeat(remaining)));
        spans.push(Span::styled(
            hint.to_string(),
            Style::default()
                .fg(theme.status_bar_fg)
                .add_modifier(Modifier::DIM),
        ));
    }

    let line = Line::from(spans);
    let paragraph = Paragraph::new(line).style(
        Style::default()
            .bg(theme.status_bar_bg)
            .fg(theme.status_bar_fg),
    );

    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Filter overlay
// ---------------------------------------------------------------------------

/// Draw the filter overlay modal on top of the main UI.
///
/// Draw the filter menu overlay.
///
/// Renders a centered popup showing the list of filter menu items with
/// the currently selected item highlighted. Items include a tool call
/// toggle (checkbox) and agent options (radio buttons).
fn draw_filter_menu(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme_colors;
    let menu = &app.filter_menu;

    if menu.items.is_empty() {
        return;
    }

    // Compute overlay dimensions: centered, compact.
    // Height: 2 (borders) + item count + 2 (title row + footer)
    let content_rows = menu.items.len() + 2; // items + blank separator + footer hints
    let overlay_height = (content_rows as u16 + 2).clamp(6, area.height); // +2 for borders
    let overlay_width = area.width.clamp(20, 40);

    let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Clear the area behind the overlay.
    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .title(" Filter ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.filter_valid_border));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // Render each menu item.
    for idx in 0..menu.items.len() {
        let label = menu.item_label(idx);
        let is_selected = idx == menu.selected;

        let style = if is_selected {
            Style::default()
                .fg(theme.filter_selected_fg)
                .bg(theme.filter_selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.filter_unselected)
        };

        lines.push(Line::from(Span::styled(format!("  {} ", label), style)));
    }

    // Footer hints.
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            " Enter",
            Style::default()
                .fg(theme.filter_shortcut_key)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("/"),
        Span::styled(
            "Space",
            Style::default()
                .fg(theme.filter_shortcut_key)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(":select "),
        Span::styled(
            "Esc",
            Style::default()
                .fg(theme.filter_shortcut_key)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("/"),
        Span::styled(
            "f",
            Style::default()
                .fg(theme.filter_shortcut_key)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(":close"),
    ]));

    let paragraph = Paragraph::new(lines)
        .style(
            Style::default()
                .bg(theme.filter_overlay_bg)
                .fg(theme.filter_overlay_fg),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

/// Draw the help overlay modal showing all keyboard shortcuts.
///
/// Renders a centered popup with a static list of key bindings.
/// Any key press dismisses the overlay (handled in `App::on_key()`).
fn draw_help_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme_colors;

    // Bail out if the terminal is too small to render anything.
    if area.width < 5 || area.height < 5 {
        return;
    }

    // Compute session stats from the ring buffer.
    let stats = compute_session_stats(&app.ring_buffer);

    // ----- Section 1: Symbol & Color Legend ---------------------------------
    // (symbol, color, description)
    let legend: Vec<(&str, Color, &str)> = vec![
        (">", theme.role_user, "User message"),
        ("<", theme.role_assistant, "Assistant message"),
        ("~", theme.role_tool_use, "Tool call"),
        ("?", theme.role_unknown, "Unknown role"),
        (app.symbols.progress_indicator, theme.logstream_progress, "Progress indicator"),
    ];

    // ----- Section 2: Complete Keybinding Reference -------------------------
    let keybindings: Vec<(&str, &str)> = vec![
        ("?", "Toggle this help screen"),
        ("Esc", "Close help / exit scroll/search"),
        ("q", "Quit"),
        ("Ctrl+C", "Quit (force)"),
        ("Tab", "Toggle focus between panels"),
        ("b", "Toggle sidebar"),
        ("Enter", "Confirm session selection"),
        ("f", "Open filter menu"),
        ("/", "Search (type query, Enter to confirm)"),
        ("n / N", "Next / previous search match"),
        ("L", "Load full session history"),
        ("j / Down", "Navigate / scroll down"),
        ("k / Up", "Navigate / scroll up"),
        ("u / d", "Half-page up / down"),
        ("PgUp/PgDn", "Page up / down"),
        ("g / Home", "Scroll to top"),
        ("G / End", "Scroll to bottom (exit scroll)"),
    ];

    // ----- Compute overlay dimensions ---------------------------------------
    // Target: ~70 wide, ~45 tall. Degrade gracefully on small terminals.
    let overlay_width = 70u16.min(area.width.saturating_sub(2));
    // Estimate content height:
    //   title(1) + blank(1) + legend_header(1) + legend rows(5) + note(1) + note(1)
    //   + blank(1) + keybind_header(1) + keybind rows(17)
    //   + blank(1) + stats_header(1) + stats rows(~8)
    //   + blank(1) + footer(1) + borders(2)
    // Roughly: 6 + 2 + 18 + 2 + 10 + 4 = ~42
    let stats_lines_count = 4
        + stats.tool_call_breakdown.len().min(5)
        + if stats.subagent_count > 0 { 1 } else { 0 };
    let content_height = 3   // title + blank + legend header
        + legend.len()       // legend rows
        + 2                  // agent prefix note + timestamp note
        + 2                  // blank + keybinds header
        + keybindings.len()  // keybind rows
        + 2                  // blank + stats header
        + stats_lines_count  // stats rows
        + 2;                 // blank + footer
    let overlay_height = (content_height as u16 + 2).min(area.height); // +2 for borders

    let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Clear the area behind the overlay.
    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    let section_style = Style::default()
        .fg(theme.filter_overlay_fg)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default()
        .fg(theme.filter_shortcut_key)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.filter_overlay_fg);
    let dim_style = Style::default()
        .fg(theme.filter_overlay_fg)
        .add_modifier(Modifier::DIM);

    // ---- Title
    lines.push(Line::from(Span::styled(
        " cc-tail Help",
        section_style,
    )));
    lines.push(Line::from(""));

    // ---- Section 1: Symbol & Color Legend
    lines.push(Line::from(Span::styled(
        " Symbol & Color Legend",
        section_style,
    )));
    for (symbol, color, desc) in &legend {
        lines.push(Line::from(vec![
            Span::styled(
                format!("   {} ", symbol),
                Style::default().fg(*color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(desc.to_string(), text_style),
        ]));
    }
    // Agent prefix and timestamp notes
    lines.push(Line::from(vec![
        Span::styled("   [cook] ", label_style),
        Span::styled("Subagent prefix (last word of slug)", text_style),
    ]));
    lines.push(Line::from(vec![
        Span::styled("   HH:MM  ", dim_style),
        Span::styled("Timestamp format", text_style),
    ]));

    // ---- Section 2: Keybinding Reference
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Keybindings",
        section_style,
    )));
    for (key, desc) in &keybindings {
        lines.push(Line::from(vec![
            Span::styled(
                format!("   {:12}", key),
                label_style,
            ),
            Span::styled(desc.to_string(), text_style),
        ]));
    }

    // ---- Section 3: Session Stats
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Session Stats",
        section_style,
    )));

    // Duration
    let duration_text = stats
        .duration_display
        .as_deref()
        .unwrap_or("--");
    lines.push(Line::from(vec![
        Span::styled("   Duration:   ", label_style),
        Span::styled(duration_text.to_string(), text_style),
    ]));

    // Message counts
    lines.push(Line::from(vec![
        Span::styled("   Messages:   ", label_style),
        Span::styled(
            format!(
                "{} total ({} user, {} assistant)",
                stats.user_message_count + stats.assistant_message_count,
                stats.user_message_count,
                stats.assistant_message_count,
            ),
            text_style,
        ),
    ]));

    // Tool calls
    lines.push(Line::from(vec![
        Span::styled("   Tool calls: ", label_style),
        Span::styled(format!("{}", stats.tool_call_count), text_style),
    ]));

    // Tool breakdown (top 5)
    for (name, count) in stats.tool_call_breakdown.iter().take(5) {
        lines.push(Line::from(vec![
            Span::styled("     ", text_style),
            Span::styled(format!("{}: {}", name, count), dim_style),
        ]));
    }

    // Subagents
    if stats.subagent_count > 0 {
        lines.push(Line::from(vec![
            Span::styled("   Subagents:  ", label_style),
            Span::styled(format!("{}", stats.subagent_count), text_style),
        ]));
    }

    // Entries loaded
    lines.push(Line::from(vec![
        Span::styled("   Entries:    ", label_style),
        Span::styled(format!("{} loaded", stats.entries_loaded), text_style),
    ]));

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Press ? or Esc to close",
        dim_style,
    )));

    let paragraph = Paragraph::new(lines)
        .style(
            Style::default()
                .bg(theme.filter_overlay_bg)
                .fg(theme.filter_overlay_fg),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

/// Separator string used between status bar segments.
const SEPARATOR: &str = " | ";
/// Width of the separator in characters.
const SEPARATOR_WIDTH: usize = 3;

/// Build the keyboard shortcuts segment text.
///
/// Returns the full shortcuts string like `" q:quit Tab:focus b:sidebar /:filter ?:help"`.
fn shortcuts_text() -> String {
    " q:quit Tab:focus b:sidebar f:filter /:search ?:help".to_string()
}

/// Build the session info segment text.
///
/// Returns e.g. `"1/3 abc123..."` or `"no sessions"`.
fn session_info_text(app: &App) -> String {
    if app.sessions.is_empty() {
        "no sessions".to_string()
    } else {
        let current = &app.sessions[app.selected_session_index.min(app.sessions.len() - 1)];
        let short_id = if current.id.len() > 16 {
            format!("{}...", &current.id[..13])
        } else {
            current.id.clone()
        };
        format!(
            "{}/{} {}",
            app.selected_session_index + 1,
            app.sessions.len(),
            short_id
        )
    }
}

/// Build the inactive badge text if the active session is inactive.
///
/// Returns `Some(" INACTIVE ")` if the active session exists and is inactive,
/// `None` otherwise (no active session or session is active).
fn inactive_badge_text(app: &App) -> Option<String> {
    let session_id = app.active_session_id.as_ref()?;
    let session = app.sessions.iter().find(|s| &s.id == session_id)?;
    match session.status() {
        SessionStatus::Inactive => Some(" INACTIVE ".to_string()),
        SessionStatus::Active => None,
    }
}

/// Compute the status bar layout and return the composed `Line`.
///
/// Priority layout algorithm:
/// 1. **Inactive badge** -- always visible (highest priority)
/// 2. **Active filters** -- shown if space permits, truncated if needed
/// 3. **Session info** -- shown if space permits
/// 4. **Keyboard shortcuts** -- hidden first when space is tight (lowest priority)
///
/// Each segment is separated by ` | `. The algorithm greedily allocates
/// space from highest to lowest priority, hiding or truncating lower
/// priority segments when the terminal is too narrow.
fn build_status_bar_line(app: &App, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from(vec![]);
    }

    let theme = &app.theme_colors;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used: usize = 0;

    // -- Priority 0: Status message (transient, highest priority) --
    if let Some(ref msg) = app.status_message {
        let msg_text = format!(" {} ", msg);
        let msg_width = msg_text.len();
        if msg_width <= width {
            spans.push(Span::styled(
                msg_text,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            used += msg_width;
        }
    }

    // -- Priority 1: Inactive badge (always visible) --
    let badge = inactive_badge_text(app);
    let badge_width = badge.as_ref().map_or(0, |b| b.len());

    if let Some(ref badge_text) = badge {
        if used + badge_width <= width {
            spans.push(Span::styled(
                badge_text.clone(),
                Style::default()
                    .fg(theme.status_inactive_fg)
                    .bg(theme.status_inactive_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            used += badge_width;
        }
    }

    // -- Priority 1.1: Full history loaded badge --
    if app.full_history_loaded {
        let full_badge = " FULL ";
        let fb_width = full_badge.len();
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 };
        if used + sep_cost + fb_width <= width {
            if used > 0 {
                spans.push(Span::styled(
                    SEPARATOR.to_string(),
                    Style::default().fg(theme.status_separator),
                ));
                used += SEPARATOR_WIDTH;
            } else {
                spans.push(Span::raw(" ".to_string()));
                used += 1;
            }
            spans.push(Span::styled(
                full_badge.to_string(),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ));
            used += fb_width;
        }
    }

    // -- Priority 1.2: Full-load confirmation prompt --
    if app.full_load_confirm_pending {
        let confirm_text = format!(
            " Session is {:.1} MB. Load full history? (y/n) ",
            app.full_load_pending_size_mb
        );
        let ct_width = confirm_text.len();
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 };
        if used + sep_cost + ct_width <= width {
            if used > 0 {
                spans.push(Span::styled(
                    SEPARATOR.to_string(),
                    Style::default().fg(theme.status_separator),
                ));
                used += SEPARATOR_WIDTH;
            } else {
                spans.push(Span::raw(" ".to_string()));
                used += 1;
            }
            spans.push(Span::styled(
                confirm_text,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            used += ct_width;
        }
    }

    // -- Priority 1.5: Search match counter --
    if let Some(ref counter_text) = app.search_state.match_counter_display() {
        let search_display = format!(" /{} {} ", app.search_state.query, counter_text);
        let sw = search_display.len();
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 };

        if used + sep_cost + sw <= width {
            if used > 0 {
                spans.push(Span::styled(
                    SEPARATOR.to_string(),
                    Style::default().fg(theme.status_separator),
                ));
                used += SEPARATOR_WIDTH;
            } else {
                spans.push(Span::raw(" ".to_string()));
                used += 1;
            }
            spans.push(Span::styled(
                search_display,
                Style::default()
                    .fg(theme.search_prompt)
                    .add_modifier(Modifier::BOLD),
            ));
            used += sw;
        }
    }

    // -- Priority 2: Active filters --
    let filter_display = app.filter_state.display();
    if let Some(ref filter_text) = filter_display {
        let fw = filter_text.len();
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 }; // leading space if first

        if used + sep_cost + fw <= width {
            if used > 0 {
                spans.push(Span::styled(
                    SEPARATOR.to_string(),
                    Style::default().fg(theme.status_separator),
                ));
                used += SEPARATOR_WIDTH;
            } else {
                spans.push(Span::raw(" ".to_string()));
                used += 1;
            }
            spans.push(Span::styled(
                filter_text.clone(),
                Style::default()
                    .fg(theme.status_filter)
                    .add_modifier(Modifier::BOLD),
            ));
            used += fw;
        }
    }

    // -- Priority 3: Session info --
    let session_info = session_info_text(app);
    let si_width = session_info.len();
    {
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 };
        if used + sep_cost + si_width <= width {
            if used > 0 {
                spans.push(Span::styled(
                    SEPARATOR.to_string(),
                    Style::default().fg(theme.status_separator),
                ));
                used += SEPARATOR_WIDTH;
            } else {
                spans.push(Span::raw(" ".to_string()));
                used += 1;
            }
            spans.push(Span::raw(session_info));
            used += si_width;
        }
    }

    // -- Priority 3.5: Project display name --
    if let Some(ref name) = app.project_display_name {
        let proj_text = format!("project:{}", name);
        let pw = proj_text.len();
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 };
        if used + sep_cost + pw <= width {
            if used > 0 {
                spans.push(Span::styled(
                    SEPARATOR.to_string(),
                    Style::default().fg(theme.status_separator),
                ));
                used += SEPARATOR_WIDTH;
            } else {
                spans.push(Span::raw(" ".to_string()));
                used += 1;
            }
            spans.push(Span::styled(
                proj_text,
                Style::default().fg(theme.status_bar_fg),
            ));
            used += pw;
        }
    }

    // -- Priority 4: Keyboard shortcuts (lowest priority, hidden first) --
    let shortcuts = shortcuts_text();
    let sc_width = shortcuts.len();
    {
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 0 };
        if used + sep_cost + sc_width <= width {
            if used > 0 {
                spans.push(Span::styled(
                    SEPARATOR.to_string(),
                    Style::default().fg(theme.status_separator),
                ));
            }
            // Render shortcuts with styled keys
            spans.push(Span::styled(
                " q".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":quit ".to_string()));
            spans.push(Span::styled(
                "Tab".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":focus ".to_string()));
            spans.push(Span::styled(
                "b".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":sidebar ".to_string()));
            spans.push(Span::styled(
                "f".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":filter ".to_string()));
            spans.push(Span::styled(
                "/".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":search ".to_string()));
            spans.push(Span::styled(
                "?".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":help".to_string()));
        }
    }

    Line::from(spans)
}

/// Draw the status bar at the bottom of the screen.
///
/// Uses a priority layout algorithm that adapts to the terminal width:
/// - Inactive badge is always visible (highest priority)
/// - Active filters are shown and truncated if needed
/// - Session info is shown when space permits
/// - Keyboard shortcuts are hidden first (lowest priority)
fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme_colors;
    let width = area.width as usize;
    let bar = build_status_bar_line(app, width);

    let paragraph = Paragraph::new(bar).style(
        Style::default()
            .bg(theme.status_bar_bg)
            .fg(theme.status_bar_fg),
    );

    frame.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Theme;
    use crate::config::AppConfig;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Helper: create a test App with the default (dark) theme.
    fn test_app() -> App {
        App::new(AppConfig::default())
    }

    /// Helper: create a test App with the light theme.
    fn test_app_light() -> App {
        let mut config = AppConfig::default();
        config.theme = Theme::Light;
        App::new(config)
    }

    /// Helper: create a test terminal with given dimensions.
    fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        Terminal::new(backend).unwrap()
    }

    // -- Smoke tests: draw does not panic ------------------------------------

    #[test]
    fn test_draw_default_state_no_panic() {
        let mut app = test_app();
        let mut terminal = test_terminal(80, 24);

        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail");
    }

    #[test]
    fn test_draw_light_theme_no_panic() {
        let mut app = test_app_light();
        let mut terminal = test_terminal(80, 24);

        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with light theme");
    }

    #[test]
    fn test_draw_sidebar_hidden_no_panic() {
        let mut app = test_app();
        app.sidebar_visible = false;

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail");
    }

    #[test]
    fn test_draw_small_terminal_no_panic() {
        let mut app = test_app();
        // Very small terminal -- should not panic
        let mut terminal = test_terminal(10, 3);

        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail even on tiny terminal");
    }

    #[test]
    fn test_draw_with_sessions_no_panic() {
        use crate::session::{Agent, Session};
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut app = test_app();
        app.sessions = vec![
            Session {
                id: "session-abc-123".to_string(),
                agents: vec![Agent {
                    agent_id: None,
                    slug: None,
                    log_path: PathBuf::from("/fake/session-abc-123.jsonl"),
                    is_main: true,
                }],
                last_modified: SystemTime::now(),
            },
            Session {
                id: "session-def-456-very-long-session-id-that-should-be-truncated".to_string(),
                agents: vec![Agent {
                    agent_id: None,
                    slug: None,
                    log_path: PathBuf::from("/fake/session-def-456.jsonl"),
                    is_main: true,
                }],
                last_modified: SystemTime::now() - std::time::Duration::from_secs(3600),
            },
        ];

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with sessions");
    }

    #[test]
    fn test_draw_focus_on_sidebar_no_panic() {
        let mut app = test_app();
        app.focus = Focus::Sidebar;

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail");
    }

    #[test]
    fn test_draw_zero_height_terminal_no_panic() {
        let mut app = test_app();
        // Edge case: zero-height terminal
        let mut terminal = test_terminal(80, 0);

        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail on zero-height terminal");
    }

    #[test]
    fn test_draw_zero_width_terminal_no_panic() {
        let mut app = test_app();
        let mut terminal = test_terminal(0, 24);

        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail on zero-width terminal");
    }

    // -- Sidebar tree layout tests -------------------------------------------

    #[test]
    fn test_draw_with_subagents_no_panic() {
        use crate::session::{Agent, Session};
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut app = test_app();
        app.sessions = vec![Session {
            id: "session-with-agents".to_string(),
            agents: vec![
                Agent {
                    agent_id: None,
                    slug: None,
                    log_path: PathBuf::from("/fake/session-with-agents.jsonl"),
                    is_main: true,
                },
                Agent {
                    agent_id: Some("a0d0bbc".to_string()),
                    slug: Some("effervescent-soaring-cook".to_string()),
                    log_path: PathBuf::from(
                        "/fake/session-with-agents/subagents/agent-a0d0bbc.jsonl",
                    ),
                    is_main: false,
                },
                Agent {
                    agent_id: Some("b1e1ccd".to_string()),
                    slug: None,
                    log_path: PathBuf::from(
                        "/fake/session-with-agents/subagents/agent-b1e1ccd.jsonl",
                    ),
                    is_main: false,
                },
            ],
            last_modified: SystemTime::now(),
        }];

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with subagents");
    }

    #[test]
    fn test_draw_with_new_session_highlight_no_panic() {
        use crate::session::{Agent, Session};
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut app = test_app();
        app.sessions = vec![
            Session {
                id: "new-session".to_string(),
                agents: vec![Agent {
                    agent_id: None,
                    slug: None,
                    log_path: PathBuf::from("/fake/new-session.jsonl"),
                    is_main: true,
                }],
                last_modified: SystemTime::now(),
            },
            Session {
                id: "old-session".to_string(),
                agents: vec![Agent {
                    agent_id: None,
                    slug: None,
                    log_path: PathBuf::from("/fake/old-session.jsonl"),
                    is_main: true,
                }],
                last_modified: SystemTime::now() - std::time::Duration::from_secs(600),
            },
        ];
        app.new_session_ids.insert("new-session".to_string());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with new-session highlight");
    }

    // -- format_relative_time tests ------------------------------------------

    #[test]
    fn test_format_relative_time_now() {
        let time = SystemTime::now();
        let result = format_relative_time(time);
        assert_eq!(result, "now");
    }

    #[test]
    fn test_format_relative_time_seconds() {
        let time = SystemTime::now() - std::time::Duration::from_secs(30);
        let result = format_relative_time(time);
        assert_eq!(result, "30s");
    }

    #[test]
    fn test_format_relative_time_minutes() {
        let time = SystemTime::now() - std::time::Duration::from_secs(300);
        let result = format_relative_time(time);
        assert_eq!(result, "5m");
    }

    #[test]
    fn test_format_relative_time_hours() {
        let time = SystemTime::now() - std::time::Duration::from_secs(7200);
        let result = format_relative_time(time);
        assert_eq!(result, "2h");
    }

    #[test]
    fn test_format_relative_time_days() {
        let time = SystemTime::now() - std::time::Duration::from_secs(259200);
        let result = format_relative_time(time);
        assert_eq!(result, "3d");
    }

    #[test]
    fn test_format_relative_time_weeks() {
        let time = SystemTime::now() - std::time::Duration::from_secs(1209600);
        let result = format_relative_time(time);
        assert_eq!(result, "2w");
    }

    #[test]
    fn test_format_relative_time_future() {
        // A time in the future should show "now"
        let time = SystemTime::now() + std::time::Duration::from_secs(60);
        let result = format_relative_time(time);
        assert_eq!(result, "now");
    }

    // -- format_timestamp tests -----------------------------------------------

    #[test]
    fn test_format_timestamp_valid_iso8601() {
        let ts = Some("2025-01-15T10:30:00Z".to_string());
        assert_eq!(format_timestamp(&ts), "10:30:00");
    }

    #[test]
    fn test_format_timestamp_with_milliseconds() {
        let ts = Some("2025-01-15T10:30:00.123Z".to_string());
        assert_eq!(format_timestamp(&ts), "10:30:00");
    }

    #[test]
    fn test_format_timestamp_with_offset() {
        let ts = Some("2025-01-15T10:30:00+09:00".to_string());
        assert_eq!(format_timestamp(&ts), "10:30:00");
    }

    #[test]
    fn test_format_timestamp_none() {
        assert_eq!(format_timestamp(&None), "--:--:--");
    }

    #[test]
    fn test_format_timestamp_malformed_no_t() {
        let ts = Some("2025-01-15 10:30:00".to_string());
        assert_eq!(format_timestamp(&ts), "--:--:--");
    }

    #[test]
    fn test_format_timestamp_malformed_short_time() {
        let ts = Some("2025-01-15T10:30".to_string());
        assert_eq!(format_timestamp(&ts), "--:--:--");
    }

    #[test]
    fn test_format_timestamp_empty_string() {
        let ts = Some("".to_string());
        assert_eq!(format_timestamp(&ts), "--:--:--");
    }

    // -- role_indicator tests -------------------------------------------------

    #[test]
    fn test_role_indicator_user() {
        let theme = ThemeColors::dark();
        let (ch, color) = role_indicator("user", &theme);
        assert_eq!(ch, '>');
        assert_eq!(color, Color::Blue);
    }

    #[test]
    fn test_role_indicator_assistant() {
        let theme = ThemeColors::dark();
        let (ch, color) = role_indicator("assistant", &theme);
        assert_eq!(ch, '<');
        assert_eq!(color, Color::Green);
    }

    #[test]
    fn test_role_indicator_unknown() {
        let theme = ThemeColors::dark();
        let (ch, color) = role_indicator("something_else", &theme);
        assert_eq!(ch, '?');
        assert_eq!(color, Color::Gray);
    }

    #[test]
    fn test_role_indicator_empty() {
        let theme = ThemeColors::dark();
        let (ch, color) = role_indicator("", &theme);
        assert_eq!(ch, '?');
        assert_eq!(color, Color::Gray);
    }

    #[test]
    fn test_role_indicator_light_theme() {
        let theme = ThemeColors::light();
        let (ch, color) = role_indicator("user", &theme);
        assert_eq!(ch, '>');
        assert_eq!(color, theme.role_user);
    }

    // -- agent_prefix tests ---------------------------------------------------

    #[test]
    fn test_agent_prefix_main_agent_returns_none() {
        let entry = LogEntry {
            is_sidechain: Some(false),
            slug: Some("some-slug".to_string()),
            agent_id: Some("abc".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_prefix(&entry), None);
    }

    #[test]
    fn test_agent_prefix_not_sidechain_none() {
        let entry = LogEntry {
            is_sidechain: None,
            slug: Some("some-slug".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_prefix(&entry), None);
    }

    #[test]
    fn test_agent_prefix_subagent_with_slug() {
        let entry = LogEntry {
            is_sidechain: Some(true),
            slug: Some("effervescent-soaring-cook".to_string()),
            agent_id: Some("a0d0bbc".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_prefix(&entry), Some("[cook]".to_string()));
    }

    #[test]
    fn test_agent_prefix_subagent_single_word_slug() {
        let entry = LogEntry {
            is_sidechain: Some(true),
            slug: Some("cook".to_string()),
            agent_id: Some("a0d0bbc".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_prefix(&entry), Some("[cook]".to_string()));
    }

    #[test]
    fn test_agent_prefix_subagent_no_slug_with_agent_id() {
        let entry = LogEntry {
            is_sidechain: Some(true),
            slug: None,
            agent_id: Some("a0d0bbc".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_prefix(&entry), Some("[a0d0bbc]".to_string()));
    }

    #[test]
    fn test_agent_prefix_subagent_long_agent_id_truncated() {
        let entry = LogEntry {
            is_sidechain: Some(true),
            slug: None,
            agent_id: Some("a0d0bbcdef123".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_prefix(&entry), Some("[a0d0bbc]".to_string()));
    }

    #[test]
    fn test_agent_prefix_subagent_no_slug_no_agent_id() {
        let entry = LogEntry {
            is_sidechain: Some(true),
            slug: None,
            agent_id: None,
            ..LogEntry::default()
        };
        assert_eq!(agent_prefix(&entry), Some("[agent]".to_string()));
    }

    // -- agent_color tests ----------------------------------------------------

    #[test]
    fn test_agent_color_no_agent_returns_main_color() {
        let theme = ThemeColors::dark();
        let entry = LogEntry::default();
        assert_eq!(agent_color(&entry, &theme), Color::White);
    }

    #[test]
    fn test_agent_color_consistent_for_same_slug() {
        let theme = ThemeColors::dark();
        let entry1 = LogEntry {
            slug: Some("effervescent-soaring-cook".to_string()),
            ..LogEntry::default()
        };
        let entry2 = LogEntry {
            slug: Some("effervescent-soaring-cook".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_color(&entry1, &theme), agent_color(&entry2, &theme));
    }

    #[test]
    fn test_agent_color_consistent_for_same_agent_id() {
        let theme = ThemeColors::dark();
        let entry1 = LogEntry {
            agent_id: Some("a0d0bbc".to_string()),
            ..LogEntry::default()
        };
        let entry2 = LogEntry {
            agent_id: Some("a0d0bbc".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(agent_color(&entry1, &theme), agent_color(&entry2, &theme));
    }

    #[test]
    fn test_agent_color_prefers_slug_over_agent_id() {
        let theme = ThemeColors::dark();
        let entry_slug = LogEntry {
            slug: Some("my-slug".to_string()),
            agent_id: Some("some-id".to_string()),
            ..LogEntry::default()
        };
        let entry_slug_only = LogEntry {
            slug: Some("my-slug".to_string()),
            ..LogEntry::default()
        };
        assert_eq!(
            agent_color(&entry_slug, &theme),
            agent_color(&entry_slug_only, &theme)
        );
    }

    #[test]
    fn test_agent_color_different_agents_can_differ() {
        // This is a probabilistic test -- with 8 colors, different strings
        // are likely to produce different colors. We just check it doesn't panic.
        let theme = ThemeColors::dark();
        let entry_a = LogEntry {
            slug: Some("alpha-agent".to_string()),
            ..LogEntry::default()
        };
        let entry_b = LogEntry {
            slug: Some("beta-agent".to_string()),
            ..LogEntry::default()
        };
        // Both should return valid colors (not panic).
        let _color_a = agent_color(&entry_a, &theme);
        let _color_b = agent_color(&entry_b, &theme);
    }

    #[test]
    fn test_agent_color_light_theme_main_agent() {
        let theme = ThemeColors::light();
        let entry = LogEntry::default();
        assert_eq!(agent_color(&entry, &theme), theme.agent_main);
        assert_eq!(agent_color(&entry, &theme), Color::Black);
    }

    #[test]
    fn test_agent_color_light_theme_subagent() {
        let theme = ThemeColors::light();
        let entry = LogEntry {
            slug: Some("test-agent".to_string()),
            ..LogEntry::default()
        };
        let color = agent_color(&entry, &theme);
        // Should be one of the light theme palette colors
        assert!(theme.agent_palette.contains(&color));
    }

    // -- draw_logstream smoke tests with entries ------------------------------

    #[test]
    fn test_draw_logstream_with_entries_no_panic() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();

        // Push some entries into the ring buffer.
        let user_json = r#"{
            "type": "user",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "message": {"role": "user", "content": [{"type": "text", "text": "Hello!"}]}
        }"#;
        let assistant_json = r#"{
            "type": "assistant",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:05Z",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "Here is the response."},
                    {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "src/main.rs"}}
                ]
            }
        }"#;

        app.ring_buffer.push(parse_jsonl_line(user_json).unwrap());
        app.ring_buffer
            .push(parse_jsonl_line(assistant_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with log entries");
    }

    #[test]
    fn test_draw_logstream_filtered_by_session_no_panic() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();
        app.active_session_id = Some("sess-001".to_string());

        let entry_json = r#"{
            "type": "user",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "message": {"role": "user", "content": "Hello!"}
        }"#;
        let other_json = r#"{
            "type": "user",
            "sessionId": "sess-002",
            "timestamp": "2025-01-15T10:31:00Z",
            "message": {"role": "user", "content": "Other session"}
        }"#;

        app.ring_buffer.push(parse_jsonl_line(entry_json).unwrap());
        app.ring_buffer.push(parse_jsonl_line(other_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with session filter");
    }

    #[test]
    fn test_draw_logstream_with_subagent_entry_no_panic() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();

        let subagent_json = r#"{
            "type": "assistant",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "isSidechain": true,
            "agentId": "a0d0bbc",
            "slug": "effervescent-soaring-cook",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Subagent says hello"}]
            }
        }"#;

        app.ring_buffer
            .push(parse_jsonl_line(subagent_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with subagent entry");
    }

    #[test]
    fn test_draw_logstream_skips_progress_entries() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();

        // Push a progress entry -- should be skipped by the renderer.
        let progress_json = r#"{
            "type": "progress",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "data": {"status": "thinking"}
        }"#;

        app.ring_buffer
            .push(parse_jsonl_line(progress_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with only progress entries");
    }

    #[test]
    fn test_draw_logstream_entry_without_message_no_panic() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();

        // A system entry without a message field.
        let json = r#"{"type": "system", "sessionId": "sess-001"}"#;
        app.ring_buffer.push(parse_jsonl_line(json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with messageless entry");
    }

    // -- Status bar layout tests (build_status_bar_line) ----------------------

    /// Helper: collect the text content from a Line by concatenating all spans.
    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Helper: create an inactive session (last modified > 10 min ago).
    fn inactive_session(id: &str) -> crate::session::Session {
        use crate::session::Agent;
        use std::path::PathBuf;

        crate::session::Session {
            id: id.to_string(),
            agents: vec![Agent {
                agent_id: None,
                slug: None,
                log_path: PathBuf::from(format!("/fake/{}.jsonl", id)),
                is_main: true,
            }],
            // 1 hour ago -- well past the 10-minute threshold
            last_modified: SystemTime::now() - std::time::Duration::from_secs(3600),
        }
    }

    /// Helper: create an active session (last modified recently).
    fn active_session(id: &str) -> crate::session::Session {
        use crate::session::Agent;
        use std::path::PathBuf;

        crate::session::Session {
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

    #[test]
    fn test_status_bar_zero_width_returns_empty() {
        let app = test_app();
        let line = build_status_bar_line(&app, 0);
        assert!(line.spans.is_empty());
    }

    #[test]
    fn test_status_bar_wide_terminal_shows_all_segments() {
        let mut app = test_app();
        app.sessions = vec![active_session("sess-001")];
        app.active_session_id = Some("sess-001".to_string());

        // 120 columns: plenty of room for everything
        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        // Should contain session info
        assert!(text.contains("1/1"), "expected session info in: {}", text);
        // Should contain shortcuts
        assert!(text.contains("q:quit"), "expected shortcuts in: {}", text);
        assert!(
            text.contains("Tab:focus"),
            "expected Tab:focus in: {}",
            text
        );
        assert!(
            text.contains("b:sidebar"),
            "expected b:sidebar in: {}",
            text
        );
        // Active session -- no INACTIVE badge
        assert!(
            !text.contains("INACTIVE"),
            "active session should not show INACTIVE in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_inactive_badge_shown() {
        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        app.active_session_id = Some("sess-old".to_string());

        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        assert!(
            text.contains("INACTIVE"),
            "expected INACTIVE badge in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_no_active_session_no_badge() {
        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        // No active session selected
        app.active_session_id = None;

        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        assert!(
            !text.contains("INACTIVE"),
            "no active session should not show INACTIVE badge in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_with_filters_shown() {
        let mut app = test_app();
        app.filter_state.hide_tool_calls = true;

        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        assert!(
            text.contains("no tools"),
            "expected filter display in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_narrow_hides_shortcuts_first() {
        let mut app = test_app();
        app.sessions = vec![active_session("sess-001")];

        // Width just enough for session info but not shortcuts.
        // Session info: " 1/1 sess-001" = 14 chars (with leading space)
        // Shortcuts: " q:quit Tab:focus b:sidebar" = 27 chars + separator = 30 chars
        // So at width 20, session info fits but shortcuts don't.
        let line = build_status_bar_line(&app, 20);
        let text = line_text(&line);

        assert!(
            !text.contains("q:quit"),
            "shortcuts should be hidden at narrow width in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_very_narrow_shows_only_badge() {
        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        app.active_session_id = Some("sess-old".to_string());

        // " INACTIVE " = 10 chars
        // At width 10, only badge should fit
        let line = build_status_bar_line(&app, 10);
        let text = line_text(&line);

        assert!(
            text.contains("INACTIVE"),
            "badge should be visible at width 10 in: {}",
            text
        );
        assert!(
            !text.contains("q:quit"),
            "shortcuts should be hidden at width 10 in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_filter_hidden_on_narrow() {
        let mut app = test_app();
        app.filter_state.hide_tool_calls = true;
        app.filter_state.selected_agent = Some("very-long-agent-id".to_string());

        // "[filter: no tools, agent very-long-agent-id]" = 46 chars
        // At width 15, filter should not fit and be hidden
        let line = build_status_bar_line(&app, 15);
        let _text = line_text(&line);
        // Should not panic regardless
    }

    #[test]
    fn test_status_bar_inactive_badge_always_highest_priority() {
        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        app.active_session_id = Some("sess-old".to_string());
        app.filter_state.hide_tool_calls = true;

        // Width = 12: enough for badge (10) but not much else
        let line = build_status_bar_line(&app, 12);
        let text = line_text(&line);

        assert!(
            text.contains("INACTIVE"),
            "badge must always be visible in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_no_sessions_shows_no_sessions() {
        let app = test_app();
        // No sessions at all

        let line = build_status_bar_line(&app, 80);
        let text = line_text(&line);

        assert!(
            text.contains("no sessions"),
            "expected 'no sessions' in: {}",
            text
        );
    }

    // -- Smoke tests for draw_status_bar with new layout ----------------------

    #[test]
    fn test_draw_with_inactive_session_no_panic() {
        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        app.active_session_id = Some("sess-old".to_string());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with inactive session");
    }

    #[test]
    fn test_draw_with_filters_no_panic() {
        let mut app = test_app();
        app.filter_state.hide_tool_calls = true;

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with active filters");
    }

    #[test]
    fn test_draw_narrow_terminal_with_all_features_no_panic() {
        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        app.active_session_id = Some("sess-old".to_string());
        app.filter_state.hide_tool_calls = true;

        // Very narrow terminal
        let mut terminal = test_terminal(15, 3);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail on narrow terminal with all features");
    }

    #[test]
    fn test_draw_width_1_terminal_no_panic() {
        let mut app = test_app();
        app.sessions = vec![inactive_session("x")];
        app.active_session_id = Some("x".to_string());

        let mut terminal = test_terminal(1, 3);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not panic on width-1 terminal");
    }

    // -- Progress entry rendering tests ---------------------------------------

    #[test]
    fn test_draw_logstream_shows_progress_when_toggled_on() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();


        let progress_json = r#"{
            "type": "progress",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "data": {"status": "thinking"}
        }"#;

        app.ring_buffer
            .push(parse_jsonl_line(progress_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with progress entries in buffer");
    }

    #[test]
    fn test_draw_logstream_progress_with_content_field() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();


        let progress_json = r#"{
            "type": "progress",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "data": {"content": "Delegating: fix the bug"}
        }"#;

        app.ring_buffer
            .push(parse_jsonl_line(progress_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with progress content");
    }

    #[test]
    fn test_draw_logstream_progress_with_no_data() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();


        // Progress entry with no data field
        let progress_json = r#"{
            "type": "progress",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z"
        }"#;

        app.ring_buffer
            .push(parse_jsonl_line(progress_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with progress entry without data");
    }

    // -- extract_progress_description tests -----------------------------------

    #[test]
    fn test_extract_progress_description_content() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(
            r#"{
            "type": "progress",
            "data": {"content": "Delegating: fix the bug"}
        }"#,
        )
        .unwrap();

        assert_eq!(
            extract_progress_description(&entry),
            "Delegating: fix the bug"
        );
    }

    #[test]
    fn test_extract_progress_description_status() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(
            r#"{
            "type": "progress",
            "data": {"status": "thinking"}
        }"#,
        )
        .unwrap();

        assert_eq!(extract_progress_description(&entry), "thinking");
    }

    #[test]
    fn test_extract_progress_description_content_priority_over_status() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(
            r#"{
            "type": "progress",
            "data": {"content": "my content", "status": "my status"}
        }"#,
        )
        .unwrap();

        // content takes priority over status
        assert_eq!(extract_progress_description(&entry), "my content");
    }

    #[test]
    fn test_extract_progress_description_fallback_json() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(
            r#"{
            "type": "progress",
            "data": {"foo": "bar", "baz": 42}
        }"#,
        )
        .unwrap();

        let desc = extract_progress_description(&entry);
        // Should be compact JSON
        assert!(desc.contains("foo"));
        assert!(desc.contains("bar"));
    }

    #[test]
    fn test_extract_progress_description_no_data() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(r#"{"type": "progress"}"#).unwrap();

        assert_eq!(extract_progress_description(&entry), "(progress)");
    }

    #[test]
    fn test_extract_progress_description_null_data() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(r#"{"type": "progress", "data": null}"#).unwrap();

        assert_eq!(extract_progress_description(&entry), "(progress)");
    }

    #[test]
    fn test_extract_progress_description_long_json_truncated() {
        use crate::log_entry::parse_jsonl_line;

        // Build a data field with a very long value
        let long_value = "x".repeat(200);
        let json = format!(
            r#"{{"type": "progress", "data": {{"long_field": "{}"}}}}"#,
            long_value
        );

        let entry = parse_jsonl_line(&json).unwrap();
        let desc = extract_progress_description(&entry);

        assert!(desc.len() <= 80);
        assert!(desc.ends_with("..."));
    }

    // -- Light theme draw smoke tests -----------------------------------------

    #[test]
    fn test_draw_light_theme_with_entries_no_panic() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app_light();

        let user_json = r#"{
            "type": "user",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "message": {"role": "user", "content": [{"type": "text", "text": "Hello!"}]}
        }"#;

        app.ring_buffer.push(parse_jsonl_line(user_json).unwrap());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with light theme and entries");
    }

    #[test]
    fn test_draw_light_theme_with_sessions_no_panic() {
        use crate::session::{Agent, Session};
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut app = test_app_light();
        app.sessions = vec![Session {
            id: "sess-light-test".to_string(),
            agents: vec![Agent {
                agent_id: None,
                slug: None,
                log_path: PathBuf::from("/fake/sess-light-test.jsonl"),
                is_main: true,
            }],
            last_modified: SystemTime::now(),
        }];

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with light theme and sessions");
    }

    // -- status message smoke tests -----------------------------------------

    #[test]
    fn test_draw_with_status_message_no_panic() {
        let mut app = test_app();
        app.status_message = Some("some status message".to_string());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with status message");
    }

    #[test]
    fn test_draw_with_status_message_and_filters_no_panic() {
        let mut app = test_app();
        app.status_message = Some("some status".to_string());
        app.filter_state.hide_tool_calls = true;

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with status message and filters");
    }

    #[test]
    fn test_status_bar_shows_status_message() {
        let mut app = test_app();
        app.status_message = Some("test message".to_string());

        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        assert!(
            text.contains("test message"),
            "expected status message in: {}",
            text
        );
    }

    // -- Help overlay smoke tests ---------------------------------------------

    #[test]
    fn test_draw_with_help_overlay_no_panic() {
        let mut app = test_app();
        app.help_overlay_visible = true;

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with help overlay");
    }

    #[test]
    fn test_draw_help_overlay_small_terminal_no_panic() {
        let mut app = test_app();
        app.help_overlay_visible = true;

        // 20x8 is small but above the 5x5 minimum threshold.
        let mut terminal = test_terminal(20, 8);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with help overlay on small terminal");
    }

    #[test]
    fn test_draw_help_overlay_tiny_terminal_no_panic() {
        let mut app = test_app();
        app.help_overlay_visible = true;

        // Below the minimum threshold: overlay is skipped gracefully.
        let mut terminal = test_terminal(4, 4);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with help overlay on tiny terminal");
    }

    #[test]
    fn test_draw_help_overlay_zero_size_terminal_no_panic() {
        let mut app = test_app();
        app.help_overlay_visible = true;

        let mut terminal = test_terminal(0, 0);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with help overlay on zero-size terminal");
    }

    #[test]
    fn test_draw_help_overlay_light_theme_no_panic() {
        let mut app = test_app_light();
        app.help_overlay_visible = true;

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with help overlay on light theme");
    }

    #[test]
    fn test_draw_help_overlay_width_1_no_panic() {
        let mut app = test_app();
        app.help_overlay_visible = true;

        // Below the 5-column minimum: overlay is skipped gracefully.
        let mut terminal = test_terminal(1, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with help overlay on width-1 terminal");
    }

    #[test]
    fn test_status_bar_includes_help_shortcut() {
        let app = test_app();
        let line = build_status_bar_line(&app, 140);
        let text = line_text(&line);

        assert!(
            text.contains("?:help"),
            "expected ?:help shortcut in: {}",
            text
        );
    }

    // -- Search rendering tests -----------------------------------------------

    #[test]
    fn test_draw_with_search_input_mode_no_panic() {
        let mut app = test_app();
        app.search_state.start_input();
        app.search_state.on_char('t');
        app.search_state.on_char('e');

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with search input mode");
    }

    #[test]
    fn test_draw_with_search_active_mode_no_panic() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();

        let user_json = r#"{
            "type": "user",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "message": {"role": "user", "content": [{"type": "text", "text": "Hello world!"}]}
        }"#;
        app.ring_buffer.push(parse_jsonl_line(user_json).unwrap());

        // Activate search
        app.search_state.start_input();
        app.search_state.on_char('H');
        app.search_state.on_char('e');
        app.search_state.confirm();

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with active search");
    }

    #[test]
    fn test_draw_with_search_active_no_entries_no_panic() {
        let mut app = test_app();
        app.search_state.start_input();
        app.search_state.on_char('x');
        app.search_state.confirm();

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with active search and no entries");
    }

    #[test]
    fn test_status_bar_includes_search_shortcut() {
        let app = test_app();
        let line = build_status_bar_line(&app, 160);
        let text = line_text(&line);

        assert!(
            text.contains("/:search"),
            "expected /:search shortcut in: {}",
            text
        );
    }

    #[test]
    fn test_status_bar_shows_search_counter_when_active() {
        let mut app = test_app();
        app.search_state.mode = crate::search::SearchMode::Active;
        app.search_state.query = "test".to_string();
        app.search_state.matches = vec![
            SearchMatch { line_index: 0, byte_start: 0, byte_len: 4 },
            SearchMatch { line_index: 1, byte_start: 0, byte_len: 4 },
        ];
        app.search_state.current_match_index = Some(0);

        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        assert!(
            text.contains("[1/2]"),
            "expected search counter [1/2] in: {}",
            text
        );
        assert!(
            text.contains("/test"),
            "expected search query /test in: {}",
            text
        );
    }

    #[test]
    fn test_line_to_text_concatenates_spans() {
        let line = Line::from(vec![
            Span::raw("hello "),
            Span::raw("world"),
        ]);
        assert_eq!(line_to_text(&line), "hello world");
    }

    #[test]
    fn test_apply_search_highlights_no_matches_returns_unchanged() {
        let lines = vec![Line::from("hello world")];
        let result = apply_search_highlights(
            lines.clone(),
            &[],
            None,
            Style::default(),
            Style::default(),
        );
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_apply_search_highlights_single_match() {
        let match_style = Style::default().fg(Color::Black).bg(Color::Yellow);
        let current_style = Style::default().fg(Color::Black).bg(Color::LightYellow);

        let lines = vec![Line::from(Span::raw("hello world"))];
        let matches = vec![SearchMatch {
            line_index: 0,
            byte_start: 6,
            byte_len: 5,
        }];

        let result = apply_search_highlights(
            lines,
            &matches,
            Some(0),
            match_style,
            current_style,
        );

        assert_eq!(result.len(), 1);
        let spans = &result[0].spans;
        // Should have at least 2 spans: "hello " and highlighted "world"
        assert!(spans.len() >= 2, "expected at least 2 spans, got {}", spans.len());
    }

    #[test]
    fn test_draw_search_input_bar_no_panic() {
        let mut app = test_app();
        app.search_state.start_input();
        app.search_state.on_char('a');
        app.search_state.on_char('b');
        app.search_state.on_char('c');

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with search input bar");
    }

    #[test]
    fn test_draw_search_input_bar_narrow_terminal_no_panic() {
        let mut app = test_app();
        app.search_state.start_input();
        app.search_state.on_char('x');

        let mut terminal = test_terminal(10, 3);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with search input bar on narrow terminal");
    }
}
