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

use crate::content_render::{render_content_blocks, RenderedLine};
use crate::log_entry::{EntryType, LogEntry};
use crate::session::SessionStatus;
use crate::theme::ThemeColors;
use crate::tui::app::{App, Focus};
use crate::tui::filter_overlay::FilterOverlayFocus;

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

    draw_status_bar(frame, app, status_area);

    // Draw filter overlay on top of everything when visible.
    if app.filter_overlay.visible {
        draw_filter_overlay(frame, app, size);
    }

    // Draw help overlay on top of everything when visible.
    if app.help_overlay_visible {
        draw_help_overlay(frame, app, size);
    }

    // Draw quit confirmation overlay when pending.
    if app.quit_confirm_pending {
        draw_quit_confirmation(frame, size);
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

        // Active marker: "● " for active sessions, "  " for inactive.
        let marker = match session.status() {
            SessionStatus::Active => Span::styled(
                "\u{25cf} ",
                Style::default()
                    .fg(theme.sidebar_active_marker)
                    .add_modifier(Modifier::BOLD),
            ),
            SessionStatus::Inactive => Span::styled(
                "  ",
                Style::default().fg(theme.sidebar_inactive_marker),
            ),
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

            // Indent: "  \u{2514} slug-name" (2 spaces + corner + space + slug)
            let prefix = "  \u{2514} ";
            let available = max_width.saturating_sub(prefix.len());
            let truncated_slug = if slug_display.len() > available {
                format!(
                    "{}...",
                    &slug_display[..available.saturating_sub(3)]
                )
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
/// to the bottom.
fn draw_logstream(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme_colors;
    let focused = app.focus == Focus::LogStream;
    let border_style = if focused {
        Style::default().fg(theme.border_focused)
    } else {
        Style::default().fg(theme.border_unfocused)
    };

    let block = Block::default()
        .title(" Log Stream ")
        .borders(Borders::ALL)
        .border_style(border_style);

    // Collect filtered entries.
    let filter_state = &app.filter_state;
    let progress_visible = app.progress_visible;

    // Entry-type visibility predicate: User, Assistant, System are always
    // visible; Progress is visible only when toggled on via `p` key.
    // FileHistorySnapshot and other types are always hidden.
    let is_type_visible = |e: &LogEntry| -> bool {
        match e.entry_type {
            EntryType::User | EntryType::Assistant | EntryType::System => true,
            EntryType::Progress => progress_visible,
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
        let paragraph = Paragraph::new("Waiting for log entries...")
            .style(Style::default().fg(theme.logstream_placeholder))
            .block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    // Build styled lines for all entries.
    let mut lines: Vec<Line> = Vec::new();

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
                    format!("\u{25b6} {}", description),
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
            for (i, rendered_line) in rendered.iter().enumerate() {
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

                // Only show agent prefix on the first line of each entry.
                if i == 0 {
                    if let Some(ref ps) = prefix_span {
                        spans.push(ps.clone());
                    }
                }

                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    text.to_string(),
                    Style::default().fg(color),
                ));

                lines.push(Line::from(spans));
            }
        }
    }

    // Compute scroll to auto-scroll to the bottom.
    let inner_height = block.inner(area).height;
    let total_lines = lines.len() as u16;
    let scroll_offset = total_lines.saturating_sub(inner_height);

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
    if time_part.len() >= 8
        && time_part.as_bytes()[2] == b':'
        && time_part.as_bytes()[5] == b':'
    {
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
    let key = entry
        .slug
        .as_deref()
        .or(entry.agent_id.as_deref());

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
// Filter overlay
// ---------------------------------------------------------------------------

/// Draw the filter overlay modal on top of the main UI.
///
/// Renders a centered popup with:
/// - A regex pattern input field (with red border for invalid patterns)
/// - Role toggles (if any roles are known)
/// - Agent toggles (if any agents are known)
/// - Footer with key hints
fn draw_filter_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme_colors;
    let overlay = &app.filter_overlay;

    // Compute overlay dimensions: centered, at most 60 cols x 20 rows.
    let overlay_width = area.width.clamp(20, 60);
    let overlay_height = area.height.clamp(6, 20);

    let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    // Clear the area behind the overlay.
    frame.render_widget(Clear, overlay_area);

    // Outer block
    let border_color = if !overlay.pattern_valid {
        theme.filter_invalid
    } else {
        theme.filter_valid_border
    };

    let block = Block::default()
        .title(" Filter (Enter=apply, Esc=cancel) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Split inner area into sections.
    let mut lines: Vec<Line> = Vec::new();

    // -- Pattern input section --
    let focused_on_pattern = overlay.focus == FilterOverlayFocus::PatternInput;
    let pattern_label_style = if focused_on_pattern {
        Style::default()
            .fg(theme.filter_focused_label)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.filter_unfocused_label)
    };

    let cursor_indicator = if focused_on_pattern { "|" } else { "" };

    // Build the pattern display with cursor
    let pattern_display = if focused_on_pattern {
        let before = &overlay.pattern_input[..overlay.cursor_pos];
        let after = &overlay.pattern_input[overlay.cursor_pos..];
        format!("{}{}{}", before, cursor_indicator, after)
    } else {
        overlay.pattern_input.clone()
    };

    let pattern_style = if !overlay.pattern_valid {
        Style::default().fg(theme.filter_invalid)
    } else {
        Style::default().fg(theme.filter_valid_text)
    };

    lines.push(Line::from(vec![
        Span::styled(" Pattern: ", pattern_label_style),
        Span::styled(pattern_display, pattern_style),
    ]));

    if !overlay.pattern_valid {
        lines.push(Line::from(vec![Span::styled(
            "          (invalid regex)",
            Style::default().fg(theme.filter_invalid),
        )]));
    }

    lines.push(Line::from(""));

    // -- Role toggles section --
    if !overlay.role_options.is_empty() {
        let focused_on_roles = overlay.focus == FilterOverlayFocus::RoleToggles;
        let role_label_style = if focused_on_roles {
            Style::default()
                .fg(theme.filter_focused_label)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.filter_unfocused_label)
        };
        lines.push(Line::from(Span::styled(
            " Roles (Space=toggle):",
            role_label_style,
        )));

        for (i, opt) in overlay.role_options.iter().enumerate() {
            let checkbox = if opt.enabled { "[x]" } else { "[ ]" };
            let is_selected = focused_on_roles && i == overlay.role_selected;
            let style = if is_selected {
                Style::default()
                    .fg(theme.filter_selected_fg)
                    .bg(theme.filter_selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.filter_unselected)
            };
            lines.push(Line::from(Span::styled(
                format!("   {} {}", checkbox, opt.name),
                style,
            )));
        }

        lines.push(Line::from(""));
    }

    // -- Agent toggles section --
    if !overlay.agent_options.is_empty() {
        let focused_on_agents = overlay.focus == FilterOverlayFocus::AgentToggles;
        let agent_label_style = if focused_on_agents {
            Style::default()
                .fg(theme.filter_focused_label)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.filter_unfocused_label)
        };
        lines.push(Line::from(Span::styled(
            " Agents (Space=toggle, m=main):",
            agent_label_style,
        )));

        // Main agent toggle
        let main_checkbox = if overlay.include_main { "[x]" } else { "[ ]" };
        let main_style = if focused_on_agents {
            Style::default().fg(theme.filter_main_focused)
        } else {
            Style::default().fg(theme.filter_main_unfocused)
        };
        lines.push(Line::from(Span::styled(
            format!("   {} main", main_checkbox),
            main_style,
        )));

        for (i, opt) in overlay.agent_options.iter().enumerate() {
            let checkbox = if opt.enabled { "[x]" } else { "[ ]" };
            let is_selected = focused_on_agents && i == overlay.agent_selected;
            let style = if is_selected {
                Style::default()
                    .fg(theme.filter_selected_fg)
                    .bg(theme.filter_selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.filter_unselected)
            };
            lines.push(Line::from(Span::styled(
                format!("   {} {}", checkbox, opt.display_name),
                style,
            )));
        }

        lines.push(Line::from(""));
    }

    // -- Footer hints --
    lines.push(Line::from(vec![
        Span::styled(
            " Tab",
            Style::default()
                .fg(theme.filter_shortcut_key)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(":next section "),
        Span::styled(
            "Enter",
            Style::default()
                .fg(theme.filter_shortcut_key)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(":apply "),
        Span::styled(
            "Esc",
            Style::default()
                .fg(theme.filter_shortcut_key)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(":cancel"),
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

    // Shortcut entries: (key_text, description)
    let shortcuts: Vec<(&str, &str)> = vec![
        ("q", "Quit"),
        ("Ctrl+C", "Quit (force)"),
        ("?", "Show this help"),
        ("Tab", "Toggle focus between panels"),
        ("b", "Toggle sidebar"),
        ("/", "Open filter overlay"),
        ("p", "Toggle progress entries"),
        ("t", "Open tmux panes"),
        ("j / Down", "Select next session"),
        ("k / Up", "Select previous session"),
        ("Enter", "Confirm session selection"),
    ];

    // Compute overlay dimensions.
    // Width: enough for the widest line, clamped to terminal width.
    let content_width: u16 = 44; // "  Ctrl+C     Quit (force)" is ~28, title is ~18; 44 gives nice padding
    let overlay_width = (content_width + 4).min(area.width); // +4 for borders + padding, clamped to area
    // Height: title(1) + blank(1) + shortcuts(11) + blank(1) + footer(1) + borders(2) = 17
    let content_lines = shortcuts.len() as u16 + 4; // title + blank + shortcuts + blank + footer
    let overlay_height = (content_lines + 2).min(area.height); // +2 for borders, clamped to area

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

    // Title
    lines.push(Line::from(Span::styled(
        " Keyboard Shortcuts",
        Style::default()
            .fg(theme.filter_overlay_fg)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Shortcut rows
    for (key, desc) in &shortcuts {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:12}", key),
                Style::default()
                    .fg(theme.filter_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                desc.to_string(),
                Style::default().fg(theme.filter_overlay_fg),
            ),
        ]));
    }

    // Footer
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Press any key to close",
        Style::default()
            .fg(theme.filter_overlay_fg)
            .add_modifier(Modifier::DIM),
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
/// Returns the full shortcuts string like `" q:quit Tab:focus b:sidebar /:filter p:progress t:tmux ?:help"`.
fn shortcuts_text() -> String {
    " q:quit Tab:focus b:sidebar /:filter p:progress t:tmux ?:help".to_string()
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

    // -- Priority 1.5: tmux pane count --
    let tmux_pane_count = app.tmux_manager.pane_count();
    if tmux_pane_count > 0 {
        let tmux_text = format!("tmux:{}", tmux_pane_count);
        let tw = tmux_text.len();
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 };
        if used + sep_cost + tw <= width {
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
                tmux_text,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            used += tw;
        }
    }

    // -- Priority 2: Active filters --
    let filter_display = app.active_filters.display();
    if let Some(ref full_filter) = filter_display {
        let sep_cost = if used > 0 { SEPARATOR_WIDTH } else { 1 }; // leading space if first
        let available = width.saturating_sub(used + sep_cost);

        if available >= 4 {
            // Enough room for at least "x..."
            let truncated = app.active_filters.display_truncated(available);
            if let Some(filter_text) = truncated {
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
                let fw = filter_text.len();
                spans.push(Span::styled(
                    filter_text,
                    Style::default()
                        .fg(theme.status_filter)
                        .add_modifier(Modifier::BOLD),
                ));
                used += fw;
            }
        } else if available > 0 && full_filter.len() <= available {
            // Fits without truncation
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
            let fw = full_filter.len();
            spans.push(Span::styled(
                full_filter.clone(),
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
                "/".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":filter ".to_string()));
            spans.push(Span::styled(
                "p".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":progress ".to_string()));
            spans.push(Span::styled(
                "t".to_string(),
                Style::default()
                    .fg(theme.status_shortcut_key)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(":tmux ".to_string()));
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

/// Draw the quit confirmation overlay when tmux panes are active.
///
/// Shows a centered dialog asking the user to confirm quitting,
/// which will also kill all tmux panes.
fn draw_quit_confirmation(frame: &mut Frame, area: Rect) {
    let overlay_width = area.width.clamp(10, 50);
    let overlay_height = area.height.clamp(3, 7);

    let x = area.x + (area.width.saturating_sub(overlay_width)) / 2;
    let y = area.y + (area.height.saturating_sub(overlay_height)) / 2;
    let overlay_area = Rect::new(x, y, overlay_width, overlay_height);

    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .title(" Quit? ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let lines = vec![
        Line::from("Kill tmux panes and quit?"),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":yes  "),
            Span::styled(
                "n",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(":no"),
        ]),
    ];

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(Color::Black).fg(Color::White))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, inner);
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

    let paragraph = Paragraph::new(bar)
        .style(Style::default().bg(theme.status_bar_bg).fg(theme.status_bar_fg));

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
                    log_path: PathBuf::from("/fake/session-with-agents/subagents/agent-a0d0bbc.jsonl"),
                    is_main: false,
                },
                Agent {
                    agent_id: Some("b1e1ccd".to_string()),
                    slug: None,
                    log_path: PathBuf::from("/fake/session-with-agents/subagents/agent-b1e1ccd.jsonl"),
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

        app.ring_buffer
            .push(parse_jsonl_line(entry_json).unwrap());
        app.ring_buffer
            .push(parse_jsonl_line(other_json).unwrap());

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
        use crate::tui::app::ActiveFilters;

        let mut app = test_app();
        app.active_filters = ActiveFilters {
            pattern: Some("error".to_string()),
            level: None,
        };

        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        assert!(
            text.contains("filter:error"),
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
    fn test_status_bar_filter_truncated_on_narrow() {
        use crate::tui::app::ActiveFilters;

        let mut app = test_app();
        app.active_filters = ActiveFilters {
            pattern: Some("very_long_pattern_that_wont_fit".to_string()),
            level: None,
        };

        // "filter:very_long_pattern_that_wont_fit" = 38 chars
        // At width 25, filter should be truncated with "..."
        let line = build_status_bar_line(&app, 25);
        let text = line_text(&line);

        if text.contains("filter:") {
            // If filter is shown, it should be truncated
            assert!(
                text.contains("..."),
                "long filter should be truncated with ... in: {}",
                text
            );
        }
        // Either way, should not panic
    }

    #[test]
    fn test_status_bar_inactive_badge_always_highest_priority() {
        use crate::tui::app::ActiveFilters;

        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        app.active_session_id = Some("sess-old".to_string());
        app.active_filters = ActiveFilters {
            pattern: Some("err".to_string()),
            level: None,
        };

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
        use crate::tui::app::ActiveFilters;

        let mut app = test_app();
        app.active_filters = ActiveFilters {
            pattern: Some("test".to_string()),
            level: Some("user".to_string()),
        };

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with active filters");
    }

    #[test]
    fn test_draw_narrow_terminal_with_all_features_no_panic() {
        use crate::tui::app::ActiveFilters;

        let mut app = test_app();
        app.sessions = vec![inactive_session("sess-old")];
        app.active_session_id = Some("sess-old".to_string());
        app.active_filters = ActiveFilters {
            pattern: Some("error".to_string()),
            level: None,
        };

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
        app.progress_visible = true;

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
            .expect("draw should not fail with progress entries visible");
    }

    #[test]
    fn test_draw_logstream_progress_with_content_field() {
        use crate::log_entry::parse_jsonl_line;

        let mut app = test_app();
        app.progress_visible = true;

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
        app.progress_visible = true;

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

        let entry = parse_jsonl_line(r#"{
            "type": "progress",
            "data": {"content": "Delegating: fix the bug"}
        }"#)
        .unwrap();

        assert_eq!(
            extract_progress_description(&entry),
            "Delegating: fix the bug"
        );
    }

    #[test]
    fn test_extract_progress_description_status() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(r#"{
            "type": "progress",
            "data": {"status": "thinking"}
        }"#)
        .unwrap();

        assert_eq!(extract_progress_description(&entry), "thinking");
    }

    #[test]
    fn test_extract_progress_description_content_priority_over_status() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(r#"{
            "type": "progress",
            "data": {"content": "my content", "status": "my status"}
        }"#)
        .unwrap();

        // content takes priority over status
        assert_eq!(extract_progress_description(&entry), "my content");
    }

    #[test]
    fn test_extract_progress_description_fallback_json() {
        use crate::log_entry::parse_jsonl_line;

        let entry = parse_jsonl_line(r#"{
            "type": "progress",
            "data": {"foo": "bar", "baz": 42}
        }"#)
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

    // -- Status bar includes p:progress shortcut ------------------------------

    #[test]
    fn test_status_bar_includes_progress_shortcut() {
        let app = test_app();
        let line = build_status_bar_line(&app, 120);
        let text = line_text(&line);

        assert!(
            text.contains("p:progress"),
            "expected p:progress shortcut in: {}",
            text
        );
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

    // -- tmux integration smoke tests -----------------------------------------

    #[test]
    fn test_draw_with_status_message_no_panic() {
        let mut app = test_app();
        app.status_message = Some("tmux: spawned 3 panes".to_string());

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with status message");
    }

    #[test]
    fn test_draw_with_quit_confirmation_no_panic() {
        let mut app = test_app();
        app.quit_confirm_pending = true;

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with quit confirmation");
    }

    #[test]
    fn test_draw_quit_confirmation_small_terminal_no_panic() {
        let mut app = test_app();
        app.quit_confirm_pending = true;

        let mut terminal = test_terminal(15, 5);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with quit confirmation on small terminal");
    }

    #[test]
    fn test_draw_with_status_message_and_filters_no_panic() {
        use crate::tui::app::ActiveFilters;

        let mut app = test_app();
        app.status_message = Some("Not inside tmux".to_string());
        app.active_filters = ActiveFilters {
            pattern: Some("error".to_string()),
            level: None,
        };

        let mut terminal = test_terminal(80, 24);
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("draw should not fail with status message and filters");
    }

    #[test]
    fn test_status_bar_includes_tmux_shortcut() {
        let app = test_app();
        let line = build_status_bar_line(&app, 130);
        let text = line_text(&line);

        assert!(
            text.contains("t:tmux"),
            "expected t:tmux shortcut in: {}",
            text
        );
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
}
