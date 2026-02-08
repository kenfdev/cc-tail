//! Lightweight non-TUI streaming mode that tails a single JSONL file to stdout.
//!
//! `cc-tail stream --file <path> --replay 20` replays the last N visible
//! messages then live-tails new entries as they are appended. TTY detection
//! controls formatting: emoji + ANSI colors for interactive terminals,
//! ASCII role tags + no colors for piped output.
//!
//! This module is intentionally independent of the TUI (no ratatui imports).
//! It reuses the shared JSONL parser, content-block renderer, and tool
//! summarizer.

use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::cli::{StreamArgs, Theme};
use crate::content_render::{render_content_blocks, RenderedLine};
use crate::log_entry::{parse_jsonl_line, EntryType, LogEntry};
use crate::replay::is_visible_type;
use crate::watcher::{read_new_entries, FileWatchState};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Runtime configuration for the stream mode.
struct StreamConfig {
    /// Path to the JSONL file to tail.
    path: PathBuf,
    /// Number of visible messages to replay before live tailing.
    replay_count: usize,
    /// Show progress entries and parse errors.
    verbose: bool,
    /// ANSI color codes (empty strings when piping).
    colors: AnsiColors,
    /// Whether stdout is a terminal (controls emoji vs ASCII indicators).
    is_tty: bool,
}

// ---------------------------------------------------------------------------
// ANSI color helpers
// ---------------------------------------------------------------------------

/// ANSI escape codes for terminal coloring.
///
/// When stdout is not a TTY (piped), all fields are empty strings so that
/// no escape sequences leak into downstream consumers.
struct AnsiColors {
    /// Timestamp color (dim/gray).
    timestamp: &'static str,
    /// User role color.
    role_user: &'static str,
    /// Assistant role color.
    role_assistant: &'static str,
    /// System role color.
    role_system: &'static str,
    /// Tool use color.
    tool_use: &'static str,
    /// Default text color.
    text: &'static str,
    /// Reset all attributes.
    reset: &'static str,
}

impl AnsiColors {
    /// Build color codes for an interactive TTY.
    fn for_tty(theme: &Theme) -> Self {
        match theme {
            Theme::Dark => Self {
                timestamp: "\x1b[90m",   // bright black (gray)
                role_user: "\x1b[34m",   // blue
                role_assistant: "\x1b[32m", // green
                role_system: "\x1b[33m", // yellow
                tool_use: "\x1b[33m",    // yellow
                text: "\x1b[0m",         // default
                reset: "\x1b[0m",
            },
            Theme::Light => Self {
                timestamp: "\x1b[90m",      // gray
                role_user: "\x1b[34m",      // blue
                role_assistant: "\x1b[32m", // green
                role_system: "\x1b[35m",    // magenta
                tool_use: "\x1b[35m",       // magenta
                text: "\x1b[0m",            // default
                reset: "\x1b[0m",
            },
        }
    }

    /// No-op color codes for piped (non-TTY) output.
    fn for_pipe() -> Self {
        Self {
            timestamp: "",
            role_user: "",
            role_assistant: "",
            role_system: "",
            tool_use: "",
            text: "",
            reset: "",
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the stream mode: replay recent messages then live-tail.
///
/// Returns `Ok(())` on clean shutdown (Ctrl+C or broken pipe).
pub async fn run_stream(args: &StreamArgs) -> Result<(), Box<dyn std::error::Error>> {
    let is_tty = std::io::stdout().is_terminal();
    let theme = args.theme.clone().unwrap_or(Theme::Dark);
    let colors = if is_tty {
        AnsiColors::for_tty(&theme)
    } else {
        AnsiColors::for_pipe()
    };

    let config = StreamConfig {
        path: args.file.clone(),
        replay_count: args.replay,
        verbose: args.verbose,
        colors,
        is_tty,
    };

    // Validate the file exists.
    if !config.path.exists() {
        eprintln!("cc-tail: file not found: {}", config.path.display());
        std::process::exit(1);
    }

    // Phase 1: Replay
    let eof_offset = replay_phase(&config)?;

    // Phase 2: Live tail
    live_tail_phase(&config, eof_offset).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 1: Replay
// ---------------------------------------------------------------------------

/// Read the entire file, filter to visible entries, print the last N.
///
/// Returns the byte offset at EOF so that live tailing can start from there.
fn replay_phase(config: &StreamConfig) -> Result<u64, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(&config.path)?;
    let file_len = file.metadata()?.len();
    let reader = BufReader::new(file);

    let mut all_visible: Vec<LogEntry> = Vec::new();

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                if config.verbose {
                    eprintln!("cc-tail: read error: {}", e);
                }
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let entry = match parse_jsonl_line(trimmed) {
            Ok(e) => e,
            Err(e) => {
                if config.verbose {
                    eprintln!("cc-tail: skipping malformed line: {}", e);
                }
                continue;
            }
        };

        if !is_visible_type(&entry, config.verbose) {
            continue;
        }

        all_visible.push(entry);
    }

    // Take the last `replay_count` entries.
    let start = all_visible.len().saturating_sub(config.replay_count);
    let replay_entries = &all_visible[start..];

    let stdout = io::stdout();
    let mut out = stdout.lock();

    for entry in replay_entries {
        if print_entry(&mut out, entry, config).is_err() {
            // BrokenPipe — exit cleanly.
            std::process::exit(0);
        }
    }

    Ok(file_len)
}

// ---------------------------------------------------------------------------
// Phase 2: Live tailing
// ---------------------------------------------------------------------------

/// Watch the file for changes and print new visible entries as they appear.
///
/// Listens for SIGINT and SIGTERM alongside file-change events using
/// `tokio::select!`, allowing graceful shutdown when a signal is received.
async fn live_tail_phase(
    config: &StreamConfig,
    start_offset: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = FileWatchState::new_with_offset(start_offset);

    // Set up a channel for notify events.
    let (tx, mut rx) = mpsc::channel::<()>(64);

    // Determine the parent directory to watch.
    let watch_path = config
        .path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let target_path = config.path.canonicalize().unwrap_or(config.path.clone());

    // Create the filesystem watcher.
    let target_clone = target_path.clone();
    let mut watcher = notify::RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) => {
                        // Check if any of the event paths match our target file.
                        for path in &event.paths {
                            let canonical = path
                                .canonicalize()
                                .unwrap_or_else(|_| path.clone());
                            if canonical == target_clone {
                                let _ = tx.blocking_send(());
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
        },
        notify::Config::default(),
    )?;

    watcher.watch(&watch_path, RecursiveMode::NonRecursive)?;

    // Set up signal listeners for graceful shutdown.
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    let stdout = io::stdout();

    // Process events until interrupted by signal or channel close.
    loop {
        tokio::select! {
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(()) => {
                        let entries = read_new_entries(&config.path, &mut state, config.verbose);
                        let mut out = stdout.lock();
                        for entry in &entries {
                            if !is_visible_type(entry, config.verbose) {
                                continue;
                            }
                            if print_entry(&mut out, entry, config).is_err() {
                                // BrokenPipe — exit cleanly.
                                std::process::exit(0);
                            }
                        }
                    }
                    None => break, // Channel closed.
                }
            }
            _ = sigint.recv() => {
                break; // SIGINT received — exit cleanly.
            }
            _ = sigterm.recv() => {
                break; // SIGTERM received — exit cleanly.
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Print a single log entry to the given writer.
///
/// Returns `Err` on I/O failure (typically `BrokenPipe`).
fn print_entry<W: Write>(
    out: &mut W,
    entry: &LogEntry,
    config: &StreamConfig,
) -> io::Result<()> {
    let ts = format_timestamp(entry.timestamp.as_deref());
    let role = entry
        .message
        .as_ref()
        .and_then(|m| m.role.as_deref())
        .unwrap_or("");

    // Print the header line: timestamp + role indicator
    let (role_label, role_color) = role_indicator(role, &entry.entry_type, config);
    writeln!(
        out,
        "{}{}{} {}{}{}",
        config.colors.timestamp, ts, config.colors.reset,
        role_color, role_label, config.colors.reset,
    )?;

    // Print content lines
    if let Some(ref msg) = entry.message {
        let rendered = render_content_blocks(&msg.content);
        for line in &rendered {
            match line {
                RenderedLine::Text(text) => {
                    writeln!(
                        out,
                        "  {}{}{}",
                        config.colors.text, text, config.colors.reset,
                    )?;
                }
                RenderedLine::ToolUse(summary) => {
                    writeln!(
                        out,
                        "  {}{}{}",
                        config.colors.tool_use, summary, config.colors.reset,
                    )?;
                }
                RenderedLine::Unknown(label) => {
                    writeln!(out, "  {}", label)?;
                }
            }
        }
    }

    out.flush()?;
    Ok(())
}

/// Return the role indicator string and its ANSI color code.
///
/// In TTY mode, uses emoji indicators. In pipe mode, uses ASCII tags.
fn role_indicator<'a>(
    role: &str,
    entry_type: &EntryType,
    config: &'a StreamConfig,
) -> (String, &'a str) {
    match entry_type {
        EntryType::User => {
            let label = if config.is_tty {
                "\u{1f9d1}".to_string() // 🧑
            } else {
                "[H]".to_string()
            };
            (label, config.colors.role_user)
        }
        EntryType::Assistant => {
            let label = if config.is_tty {
                "\u{1f916}".to_string() // 🤖
            } else {
                "[A]".to_string()
            };
            (label, config.colors.role_assistant)
        }
        EntryType::System => {
            let label = if config.is_tty {
                "\u{2699}\u{fe0f}".to_string() // ⚙️
            } else {
                "[S]".to_string()
            };
            (label, config.colors.role_system)
        }
        EntryType::Progress => {
            let label = if config.is_tty {
                "\u{23f3}".to_string() // ⏳
            } else {
                "[P]".to_string()
            };
            (label, config.colors.timestamp)
        }
        _ => {
            let label = format!("[{}]", role);
            (label, config.colors.text)
        }
    }
}

/// Format an ISO 8601 timestamp to `HH:MM:SS`.
///
/// Extracts the time portion from timestamps like `2025-01-15T14:30:12Z`
/// or `2025-01-15T14:30:12.123Z`. Returns `"--:--:--"` if the timestamp
/// is missing or does not contain a recognizable time component.
fn format_timestamp(ts: Option<&str>) -> String {
    match ts {
        Some(s) => {
            // Look for the 'T' separator in ISO 8601
            if let Some(t_pos) = s.find('T') {
                let time_part = &s[t_pos + 1..];
                // Extract HH:MM:SS (first 8 characters)
                if time_part.len() >= 8 && time_part.as_bytes()[2] == b':' && time_part.as_bytes()[5] == b':' {
                    return time_part[..8].to_string();
                }
            }
            "--:--:--".to_string()
        }
        None => "--:--:--".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_entry::parse_jsonl_line;

    // -- format_timestamp tests -----------------------------------------------

    #[test]
    fn test_format_timestamp_iso8601() {
        assert_eq!(
            format_timestamp(Some("2025-01-15T14:30:12Z")),
            "14:30:12"
        );
    }

    #[test]
    fn test_format_timestamp_with_millis() {
        assert_eq!(
            format_timestamp(Some("2025-01-15T14:30:12.123Z")),
            "14:30:12"
        );
    }

    #[test]
    fn test_format_timestamp_with_offset() {
        assert_eq!(
            format_timestamp(Some("2025-01-15T14:30:12+09:00")),
            "14:30:12"
        );
    }

    #[test]
    fn test_format_timestamp_none() {
        assert_eq!(format_timestamp(None), "--:--:--");
    }

    #[test]
    fn test_format_timestamp_no_t_separator() {
        assert_eq!(format_timestamp(Some("not a timestamp")), "--:--:--");
    }

    #[test]
    fn test_format_timestamp_short_time() {
        // 'T' present but time part too short
        assert_eq!(format_timestamp(Some("2025-01-15T14")), "--:--:--");
    }

    #[test]
    fn test_format_timestamp_empty() {
        assert_eq!(format_timestamp(Some("")), "--:--:--");
    }

    // -- role_indicator tests -------------------------------------------------

    fn make_config_tty() -> StreamConfig {
        StreamConfig {
            path: PathBuf::from("/dev/null"),
            replay_count: 0,
            verbose: false,
            colors: AnsiColors::for_tty(&Theme::Dark),
            is_tty: true,
        }
    }

    fn make_config_pipe() -> StreamConfig {
        StreamConfig {
            path: PathBuf::from("/dev/null"),
            replay_count: 0,
            verbose: false,
            colors: AnsiColors::for_pipe(),
            is_tty: false,
        }
    }

    #[test]
    fn test_role_indicator_user_tty() {
        let config = make_config_tty();
        let (label, _color) = role_indicator("user", &EntryType::User, &config);
        assert_eq!(label, "\u{1f9d1}");
    }

    #[test]
    fn test_role_indicator_user_pipe() {
        let config = make_config_pipe();
        let (label, _color) = role_indicator("user", &EntryType::User, &config);
        assert_eq!(label, "[H]");
    }

    #[test]
    fn test_role_indicator_assistant_tty() {
        let config = make_config_tty();
        let (label, _color) = role_indicator("assistant", &EntryType::Assistant, &config);
        assert_eq!(label, "\u{1f916}");
    }

    #[test]
    fn test_role_indicator_assistant_pipe() {
        let config = make_config_pipe();
        let (label, _color) = role_indicator("assistant", &EntryType::Assistant, &config);
        assert_eq!(label, "[A]");
    }

    #[test]
    fn test_role_indicator_system_tty() {
        let config = make_config_tty();
        let (label, _color) = role_indicator("user", &EntryType::System, &config);
        assert_eq!(label, "\u{2699}\u{fe0f}");
    }

    #[test]
    fn test_role_indicator_system_pipe() {
        let config = make_config_pipe();
        let (label, _color) = role_indicator("user", &EntryType::System, &config);
        assert_eq!(label, "[S]");
    }

    #[test]
    fn test_role_indicator_progress_tty() {
        let config = make_config_tty();
        let (label, _color) = role_indicator("", &EntryType::Progress, &config);
        assert_eq!(label, "\u{23f3}");
    }

    #[test]
    fn test_role_indicator_progress_pipe() {
        let config = make_config_pipe();
        let (label, _color) = role_indicator("", &EntryType::Progress, &config);
        assert_eq!(label, "[P]");
    }

    // -- print_entry tests ----------------------------------------------------

    #[test]
    fn test_print_entry_user_pipe() {
        let config = make_config_pipe();
        let entry = parse_jsonl_line(
            r#"{"type": "user", "timestamp": "2025-01-15T10:30:00Z", "message": {"role": "user", "content": [{"type": "text", "text": "fix the bug"}]}}"#,
        )
        .unwrap();

        let mut buf = Vec::new();
        print_entry(&mut buf, &entry, &config).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("10:30:00"));
        assert!(output.contains("[H]"));
        assert!(output.contains("fix the bug"));
    }

    #[test]
    fn test_print_entry_assistant_pipe() {
        let config = make_config_pipe();
        let entry = parse_jsonl_line(
            r#"{"type": "assistant", "timestamp": "2025-01-15T10:30:15Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "I'll investigate."}]}}"#,
        )
        .unwrap();

        let mut buf = Vec::new();
        print_entry(&mut buf, &entry, &config).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("10:30:15"));
        assert!(output.contains("[A]"));
        assert!(output.contains("I'll investigate."));
    }

    #[test]
    fn test_print_entry_tool_use_pipe() {
        let config = make_config_pipe();
        let entry = parse_jsonl_line(
            r#"{"type": "assistant", "timestamp": "2025-01-15T10:30:16Z", "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "src/auth/mod.rs"}}]}}"#,
        )
        .unwrap();

        let mut buf = Vec::new();
        print_entry(&mut buf, &entry, &config).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("[Read]"));
        assert!(output.contains("src/auth/mod.rs"));
    }

    #[test]
    fn test_print_entry_tty_has_ansi() {
        let config = make_config_tty();
        let entry = parse_jsonl_line(
            r#"{"type": "user", "timestamp": "2025-01-15T10:30:00Z", "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}}"#,
        )
        .unwrap();

        let mut buf = Vec::new();
        print_entry(&mut buf, &entry, &config).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should contain ANSI escape sequences
        assert!(output.contains("\x1b["));
        assert!(output.contains("\u{1f9d1}")); // 🧑 emoji
    }

    #[test]
    fn test_print_entry_no_message() {
        let config = make_config_pipe();
        let entry = parse_jsonl_line(
            r#"{"type": "system", "timestamp": "2025-01-15T10:00:00Z"}"#,
        )
        .unwrap();

        let mut buf = Vec::new();
        print_entry(&mut buf, &entry, &config).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // Should still print the header line
        assert!(output.contains("10:00:00"));
        assert!(output.contains("[S]"));
    }

    #[test]
    fn test_print_entry_string_content() {
        let config = make_config_pipe();
        let entry = parse_jsonl_line(
            r#"{"type": "system", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "user", "content": "System prompt text"}}"#,
        )
        .unwrap();

        let mut buf = Vec::new();
        print_entry(&mut buf, &entry, &config).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("System prompt text"));
    }

    // -- AnsiColors tests -----------------------------------------------------

    #[test]
    fn test_ansi_colors_pipe_all_empty() {
        let colors = AnsiColors::for_pipe();
        assert!(colors.timestamp.is_empty());
        assert!(colors.role_user.is_empty());
        assert!(colors.role_assistant.is_empty());
        assert!(colors.role_system.is_empty());
        assert!(colors.tool_use.is_empty());
        assert!(colors.text.is_empty());
        assert!(colors.reset.is_empty());
    }

    #[test]
    fn test_ansi_colors_tty_dark_has_escapes() {
        let colors = AnsiColors::for_tty(&Theme::Dark);
        assert!(colors.timestamp.contains("\x1b["));
        assert!(colors.role_user.contains("\x1b["));
        assert!(colors.role_assistant.contains("\x1b["));
        assert!(colors.reset.contains("\x1b["));
    }

    #[test]
    fn test_ansi_colors_tty_light_has_escapes() {
        let colors = AnsiColors::for_tty(&Theme::Light);
        assert!(colors.timestamp.contains("\x1b["));
        assert!(colors.role_user.contains("\x1b["));
        assert!(colors.role_assistant.contains("\x1b["));
        assert!(colors.reset.contains("\x1b["));
    }

    // -- Integration-style tests with replay ----------------------------------

    #[test]
    fn test_replay_phase_empty_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();

        let config = StreamConfig {
            path,
            replay_count: 20,
            verbose: false,
            colors: AnsiColors::for_pipe(),
            is_tty: false,
        };

        let offset = replay_phase(&config).unwrap();
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_replay_phase_returns_eof_offset() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("session.jsonl");
        let content = r#"{"type": "user", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}}
{"type": "assistant", "timestamp": "2025-01-15T10:01:00Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}}
"#;
        std::fs::write(&path, content).unwrap();
        let expected_len = content.len() as u64;

        let config = StreamConfig {
            path,
            replay_count: 20,
            verbose: false,
            colors: AnsiColors::for_pipe(),
            is_tty: false,
        };

        let offset = replay_phase(&config).unwrap();
        assert_eq!(offset, expected_len);
    }

    #[test]
    fn test_replay_phase_limits_to_replay_count() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("session.jsonl");

        // Write 5 entries, replay only last 2
        let mut content = String::new();
        for i in 0..5 {
            content.push_str(&format!(
                r#"{{"type": "user", "timestamp": "2025-01-15T10:{:02}:00Z", "message": {{"role": "user", "content": [{{"type": "text", "text": "msg-{}"}}]}}}}"#,
                i, i
            ));
            content.push('\n');
        }
        std::fs::write(&path, &content).unwrap();

        let config = StreamConfig {
            path,
            replay_count: 2,
            verbose: false,
            colors: AnsiColors::for_pipe(),
            is_tty: false,
        };

        // This test verifies the function runs without error.
        // Actual output goes to stdout which we can't easily capture here,
        // but print_entry is tested separately.
        let offset = replay_phase(&config).unwrap();
        assert_eq!(offset, content.len() as u64);
    }

    // -- FileWatchState::new_with_offset tests --------------------------------

    #[test]
    fn test_file_watch_state_new_with_offset() {
        let state = FileWatchState::new_with_offset(1024);
        assert_eq!(state.byte_offset, 1024);
    }

    #[test]
    fn test_file_watch_state_new_with_zero_offset() {
        let state = FileWatchState::new_with_offset(0);
        assert_eq!(state.byte_offset, 0);
    }
}
