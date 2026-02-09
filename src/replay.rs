//! Session replay for catching up on recent messages.
//!
//! On startup (or session switch) the TUI needs to display the last N
//! visible messages from the session's JSONL files so the user has
//! context. This module reads each agent's log file line-by-line,
//! applies the same visibility rules used for live tailing, and returns
//! the most recent entries together with per-file EOF offsets that the
//! watcher can use to avoid re-processing replayed lines.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use crate::filter::FilterState;
use crate::log_entry::{parse_jsonl_line, EntryType, LogEntry};
use crate::session::Session;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of visible messages to replay on session init / switch.
pub const DEFAULT_REPLAY_COUNT: usize = 20;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Replay the most recent visible messages from a session's JSONL files.
///
/// Scans every agent log file in `session`, parses each line, applies the
/// visibility filter (entry type must be User, Assistant, or System *and*
/// `filter.matches()` must pass), collects all visible entries, sorts them
/// by timestamp, and returns the last `max_visible` entries.
///
/// Also returns a map from each file path to its total byte length (the
/// EOF offset). The caller can hand these offsets to the file watcher so
/// that it starts tailing from where replay left off, avoiding duplicate
/// entries.
///
/// # Arguments
///
/// * `session`          - The session whose agent log files should be read.
/// * `filter`           - The current filter state; entries that don't pass are
///   excluded from the visible set.
/// * `max_visible`      - Maximum number of visible entries to return (default 20).
/// * `verbose`          - If `true`, emit diagnostic messages to stderr for
///   missing/unreadable files and parse errors.
///
/// # Returns
///
/// A tuple of `(entries, eof_offsets)` where:
/// - `entries` is a `Vec<LogEntry>` of at most `max_visible` entries sorted
///   by timestamp (oldest first).
/// - `eof_offsets` is a `HashMap<PathBuf, u64>` mapping each agent's log
///   file path to its byte length at the time of reading.
pub fn replay_session(
    session: &Session,
    filter: &FilterState,
    max_visible: usize,
    verbose: bool,
) -> (Vec<LogEntry>, HashMap<PathBuf, u64>) {
    let mut all_visible: Vec<LogEntry> = Vec::new();
    let mut eof_offsets: HashMap<PathBuf, u64> = HashMap::new();

    for agent in &session.agents {
        let path = &agent.log_path;

        // Open the file; skip gracefully if missing or unreadable.
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                if verbose {
                    eprintln!("cc-tail: replay: skipping {}: {}", path.display(), e);
                }
                continue;
            }
        };

        // Record the file's byte length as the EOF offset.
        let file_len = match file.metadata() {
            Ok(m) => m.len(),
            Err(e) => {
                if verbose {
                    eprintln!("cc-tail: replay: could not stat {}: {}", path.display(), e);
                }
                continue;
            }
        };
        eof_offsets.insert(path.clone(), file_len);

        // Read line-by-line, collecting visible entries.
        let reader = BufReader::new(file);
        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    if verbose {
                        eprintln!("cc-tail: replay: read error in {}: {}", path.display(), e);
                    }
                    continue;
                }
            };

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Parse the JSONL line; skip malformed lines.
            let entry = match parse_jsonl_line(trimmed) {
                Ok(e) => e,
                Err(e) => {
                    if verbose {
                        eprintln!(
                            "cc-tail: replay: skipping malformed line in {}: {}",
                            path.display(),
                            e
                        );
                    }
                    continue;
                }
            };

            // Visibility check: entry type must be User, Assistant, or System.
            if !is_visible_type(&entry) {
                continue;
            }

            // Filter check: must pass the current filter state.
            if !filter.matches(&entry) {
                continue;
            }

            all_visible.push(entry);
        }
    }

    // Sort all visible entries by timestamp (ISO 8601 string comparison).
    // Entries without timestamps sort to the beginning.
    all_visible.sort_by(|a, b| {
        let ts_a = a.timestamp.as_deref().unwrap_or("");
        let ts_b = b.timestamp.as_deref().unwrap_or("");
        ts_a.cmp(ts_b)
    });

    // Take the last `max_visible` entries.
    let start = all_visible.len().saturating_sub(max_visible);
    let result = all_visible.split_off(start);

    (result, eof_offsets)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the entry type is one that should be shown to the user
/// during replay: User, Assistant, or System. Progress and
/// `FileHistorySnapshot` entries are always hidden.
pub(crate) fn is_visible_type(entry: &LogEntry) -> bool {
    matches!(
        entry.entry_type,
        EntryType::User | EntryType::Assistant | EntryType::System
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Agent;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use std::time::SystemTime;
    use tempfile::TempDir;

    // -- Helpers ----------------------------------------------------------

    /// Create a session with agents pointing at the given log file paths.
    fn make_session(id: &str, log_paths: Vec<PathBuf>) -> Session {
        let agents = log_paths
            .into_iter()
            .enumerate()
            .map(|(i, path)| Agent {
                agent_id: if i == 0 {
                    None
                } else {
                    Some(format!("agent-{}", i))
                },
                slug: None,
                log_path: path,
                is_main: i == 0,
            })
            .collect();

        Session {
            id: id.to_string(),
            agents,
            last_modified: SystemTime::now(),
        }
    }

    /// Write JSONL lines to a file.
    fn write_jsonl(path: &Path, lines: &[&str]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{}", line).unwrap();
        }
    }

    /// Build a JSONL line for a user entry with a timestamp.
    fn user_line(ts: &str, text: &str) -> String {
        format!(
            r#"{{"type": "user", "timestamp": "{}", "message": {{"role": "user", "content": [{{"type": "text", "text": "{}"}}]}}}}"#,
            ts, text
        )
    }

    /// Build a JSONL line for an assistant entry with a timestamp.
    fn assistant_line(ts: &str, text: &str) -> String {
        format!(
            r#"{{"type": "assistant", "timestamp": "{}", "message": {{"role": "assistant", "content": [{{"type": "text", "text": "{}"}}]}}}}"#,
            ts, text
        )
    }

    /// Build a JSONL line for a system entry with a timestamp.
    fn system_line(ts: &str, text: &str) -> String {
        format!(
            r#"{{"type": "system", "timestamp": "{}", "message": {{"role": "user", "content": "{}"}}}}"#,
            ts, text
        )
    }

    /// Build a JSONL line for a progress entry (not visible).
    fn progress_line(ts: &str) -> String {
        format!(
            r#"{{"type": "progress", "timestamp": "{}", "data": {{"status": "thinking"}}}}"#,
            ts
        )
    }

    fn default_filter() -> FilterState {
        FilterState::default()
    }

    // =====================================================================
    // Test 1: Basic replay (single file, 20+ entries -> returns last 20)
    // =====================================================================

    #[test]
    fn test_basic_replay_returns_last_20() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        // Write 25 user entries with sequential timestamps.
        let lines: Vec<String> = (0..25)
            .map(|i| {
                user_line(
                    &format!("2025-01-15T10:{:02}:00Z", i),
                    &format!("msg-{}", i),
                )
            })
            .collect();
        let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        write_jsonl(&log_path, &line_refs);

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, false);

        assert_eq!(entries.len(), 20);
        // Should be the last 20 (indices 5..25)
        assert_eq!(
            entries[0].timestamp.as_deref(),
            Some("2025-01-15T10:05:00Z")
        );
        assert_eq!(
            entries[19].timestamp.as_deref(),
            Some("2025-01-15T10:24:00Z")
        );
    }

    // =====================================================================
    // Test 2: Multi-agent interleaving (sorted by timestamp)
    // =====================================================================

    #[test]
    fn test_multi_agent_interleaving() {
        let tmp = TempDir::new().unwrap();
        let main_path = tmp.path().join("main.jsonl");
        let sub_path = tmp.path().join("subagent.jsonl");

        write_jsonl(
            &main_path,
            &[
                &user_line("2025-01-15T10:00:00Z", "user-msg-1"),
                &assistant_line("2025-01-15T10:02:00Z", "main-reply-1"),
            ],
        );

        write_jsonl(
            &sub_path,
            &[
                &assistant_line("2025-01-15T10:01:00Z", "sub-reply-1"),
                &assistant_line("2025-01-15T10:03:00Z", "sub-reply-2"),
            ],
        );

        let session = make_session("s1", vec![main_path, sub_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, false);

        assert_eq!(entries.len(), 4);
        // Verify sorted by timestamp
        assert_eq!(
            entries[0].timestamp.as_deref(),
            Some("2025-01-15T10:00:00Z")
        );
        assert_eq!(
            entries[1].timestamp.as_deref(),
            Some("2025-01-15T10:01:00Z")
        );
        assert_eq!(
            entries[2].timestamp.as_deref(),
            Some("2025-01-15T10:02:00Z")
        );
        assert_eq!(
            entries[3].timestamp.as_deref(),
            Some("2025-01-15T10:03:00Z")
        );
    }

    // =====================================================================
    // Test 3: Filter interaction (filter reduces visible entries)
    // =====================================================================

    #[test]
    fn test_filter_reduces_visible_entries() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(
            &log_path,
            &[
                &user_line("2025-01-15T10:00:00Z", "hello world"),
                &assistant_line("2025-01-15T10:01:00Z", "goodbye world"),
                &user_line("2025-01-15T10:02:00Z", "hello again"),
            ],
        );

        let mut filter = FilterState::default();
        filter.set_pattern("hello");

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &filter, 20, false);

        // Only entries matching "hello" should be returned
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].timestamp.as_deref(),
            Some("2025-01-15T10:00:00Z")
        );
        assert_eq!(
            entries[1].timestamp.as_deref(),
            Some("2025-01-15T10:02:00Z")
        );
    }

    // =====================================================================
    // Test 4: Fewer than max entries
    // =====================================================================

    #[test]
    fn test_fewer_than_max_entries() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(
            &log_path,
            &[
                &user_line("2025-01-15T10:00:00Z", "msg-1"),
                &assistant_line("2025-01-15T10:01:00Z", "msg-2"),
            ],
        );

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, false);

        // Only 2 visible entries, should return all 2
        assert_eq!(entries.len(), 2);
    }

    // =====================================================================
    // Test 5: Empty file
    // =====================================================================

    #[test]
    fn test_empty_file() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("empty.jsonl");
        write_jsonl(&log_path, &[]);

        let session = make_session("s1", vec![log_path.clone()]);
        let (entries, offsets) = replay_session(&session, &default_filter(), 20, false);

        assert!(entries.is_empty());
        // EOF offset should still be recorded (0 for empty file)
        assert_eq!(offsets.get(&log_path), Some(&0));
    }

    // =====================================================================
    // Test 6: Missing file
    // =====================================================================

    #[test]
    fn test_missing_file() {
        let log_path = PathBuf::from("/nonexistent/path/session.jsonl");

        let session = make_session("s1", vec![log_path.clone()]);
        let (entries, offsets) = replay_session(&session, &default_filter(), 20, false);

        assert!(entries.is_empty());
        // Missing file should not have an EOF offset
        assert!(!offsets.contains_key(&log_path));
    }

    // =====================================================================
    // Test 7: Mixed entry types (only User/Assistant/System visible)
    // =====================================================================

    #[test]
    fn test_mixed_entry_types_visibility() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(
            &log_path,
            &[
                &user_line("2025-01-15T10:00:00Z", "user msg"),
                &progress_line("2025-01-15T10:01:00Z"),
                &assistant_line("2025-01-15T10:02:00Z", "assistant msg"),
                &format!(
                    r#"{{"type": "file-history-snapshot", "timestamp": "2025-01-15T10:03:00Z"}}"#
                ),
                &system_line("2025-01-15T10:04:00Z", "system msg"),
                &format!(
                    r#"{{"type": "queue-operation", "timestamp": "2025-01-15T10:05:00Z", "data": {{}}}}"#
                ),
            ],
        );

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, false);

        // Only User, Assistant, and System should be visible
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].entry_type, EntryType::User);
        assert_eq!(entries[1].entry_type, EntryType::Assistant);
        assert_eq!(entries[2].entry_type, EntryType::System);
    }

    // =====================================================================
    // Test 8: No-timestamp ordering (entries without timestamps sort first)
    // =====================================================================

    #[test]
    fn test_no_timestamp_ordering() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(
            &log_path,
            &[
                &user_line("2025-01-15T10:02:00Z", "with-ts-2"),
                r#"{"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "no-ts"}]}}"#,
                &user_line("2025-01-15T10:01:00Z", "with-ts-1"),
            ],
        );

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, false);

        assert_eq!(entries.len(), 3);
        // Entry without timestamp should sort first (empty string < any timestamp)
        assert_eq!(entries[0].timestamp, None);
        assert_eq!(
            entries[1].timestamp.as_deref(),
            Some("2025-01-15T10:01:00Z")
        );
        assert_eq!(
            entries[2].timestamp.as_deref(),
            Some("2025-01-15T10:02:00Z")
        );
    }

    // =====================================================================
    // Test 9: EOF offset correctness
    // =====================================================================

    #[test]
    fn test_eof_offset_correctness() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        let line1 = user_line("2025-01-15T10:00:00Z", "msg-1");
        let line2 = assistant_line("2025-01-15T10:01:00Z", "msg-2");
        write_jsonl(&log_path, &[&line1, &line2]);

        let expected_len = std::fs::metadata(&log_path).unwrap().len();

        let session = make_session("s1", vec![log_path.clone()]);
        let (_entries, offsets) = replay_session(&session, &default_filter(), 20, false);

        assert_eq!(offsets.get(&log_path), Some(&expected_len));
    }

    // =====================================================================
    // Test 10: Multiple files with EOF offsets
    // =====================================================================

    #[test]
    fn test_multiple_files_eof_offsets() {
        let tmp = TempDir::new().unwrap();
        let main_path = tmp.path().join("main.jsonl");
        let sub_path = tmp.path().join("sub.jsonl");

        write_jsonl(
            &main_path,
            &[&user_line("2025-01-15T10:00:00Z", "main-msg")],
        );
        write_jsonl(
            &sub_path,
            &[&assistant_line("2025-01-15T10:01:00Z", "sub-msg")],
        );

        let main_len = std::fs::metadata(&main_path).unwrap().len();
        let sub_len = std::fs::metadata(&sub_path).unwrap().len();

        let session = make_session("s1", vec![main_path.clone(), sub_path.clone()]);
        let (_entries, offsets) = replay_session(&session, &default_filter(), 20, false);

        assert_eq!(offsets.len(), 2);
        assert_eq!(offsets.get(&main_path), Some(&main_len));
        assert_eq!(offsets.get(&sub_path), Some(&sub_len));
    }

    // =====================================================================
    // Test 11: Malformed lines are skipped
    // =====================================================================

    #[test]
    fn test_malformed_lines_skipped() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(
            &log_path,
            &[
                &user_line("2025-01-15T10:00:00Z", "good-1"),
                "this is not valid json",
                "{broken json",
                &user_line("2025-01-15T10:01:00Z", "good-2"),
            ],
        );

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, false);

        assert_eq!(entries.len(), 2);
    }

    // =====================================================================
    // Test 12: max_visible = 0 returns empty
    // =====================================================================

    #[test]
    fn test_max_visible_zero() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(&log_path, &[&user_line("2025-01-15T10:00:00Z", "msg-1")]);

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 0, false);

        assert!(entries.is_empty());
    }

    // =====================================================================
    // Test 13: max_visible = 1 returns only the most recent
    // =====================================================================

    #[test]
    fn test_max_visible_one() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(
            &log_path,
            &[
                &user_line("2025-01-15T10:00:00Z", "older"),
                &user_line("2025-01-15T10:01:00Z", "newer"),
            ],
        );

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 1, false);

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].timestamp.as_deref(),
            Some("2025-01-15T10:01:00Z")
        );
    }

    // =====================================================================
    // Test 14: Role filter interaction
    // =====================================================================

    #[test]
    fn test_role_filter_interaction() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(
            &log_path,
            &[
                &user_line("2025-01-15T10:00:00Z", "user msg"),
                &assistant_line("2025-01-15T10:01:00Z", "assistant msg"),
                &system_line("2025-01-15T10:02:00Z", "system msg"),
            ],
        );

        let mut filter = FilterState::default();
        filter.enabled_roles.insert("assistant".to_string());

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &filter, 20, false);

        // Only assistant-role entries should pass the role filter
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_type, EntryType::Assistant);
    }

    // =====================================================================
    // Test 15: Session with no agents
    // =====================================================================

    #[test]
    fn test_session_with_no_agents() {
        let session = Session {
            id: "empty".to_string(),
            agents: vec![],
            last_modified: SystemTime::now(),
        };

        let (entries, offsets) = replay_session(&session, &default_filter(), 20, false);

        assert!(entries.is_empty());
        assert!(offsets.is_empty());
    }

    // =====================================================================
    // Test 16: Whitespace-only lines are skipped
    // =====================================================================

    #[test]
    fn test_whitespace_only_lines_skipped() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        // Manually write content with blank lines
        let mut file = fs::File::create(&log_path).unwrap();
        writeln!(file, "{}", user_line("2025-01-15T10:00:00Z", "msg-1")).unwrap();
        writeln!(file).unwrap(); // blank line
        writeln!(file, "   ").unwrap(); // whitespace-only line
        writeln!(file, "{}", user_line("2025-01-15T10:01:00Z", "msg-2")).unwrap();

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, false);

        assert_eq!(entries.len(), 2);
    }

    // =====================================================================
    // Test 17: is_visible_type helper
    // =====================================================================

    #[test]
    fn test_is_visible_type() {
        let user = parse_jsonl_line(r#"{"type": "user"}"#).unwrap();
        let assistant = parse_jsonl_line(r#"{"type": "assistant"}"#).unwrap();
        let system = parse_jsonl_line(r#"{"type": "system"}"#).unwrap();
        let progress = parse_jsonl_line(r#"{"type": "progress"}"#).unwrap();
        let fhs = parse_jsonl_line(r#"{"type": "file-history-snapshot"}"#).unwrap();
        let queue = parse_jsonl_line(r#"{"type": "queue-operation"}"#).unwrap();
        let unknown = parse_jsonl_line(r#"{"type": "some-future-type"}"#).unwrap();

        assert!(is_visible_type(&user));
        assert!(is_visible_type(&assistant));
        assert!(is_visible_type(&system));
        assert!(!is_visible_type(&progress));
        assert!(!is_visible_type(&fhs));
        assert!(!is_visible_type(&queue));
        assert!(!is_visible_type(&unknown));
    }

    // =====================================================================
    // Test 18: Verbose mode emits diagnostics (no panic)
    // =====================================================================

    #[test]
    fn test_verbose_mode_with_missing_file() {
        let log_path = PathBuf::from("/nonexistent/verbose-test.jsonl");
        let session = make_session("s1", vec![log_path]);

        // Should not panic; verbose=true just prints to stderr
        let (entries, _offsets) = replay_session(&session, &default_filter(), 20, true);
        assert!(entries.is_empty());
    }

    // =====================================================================
    // Test 19: Large replay count with few entries
    // =====================================================================

    #[test]
    fn test_large_max_visible_with_few_entries() {
        let tmp = TempDir::new().unwrap();
        let log_path = tmp.path().join("session.jsonl");

        write_jsonl(&log_path, &[&user_line("2025-01-15T10:00:00Z", "only-msg")]);

        let session = make_session("s1", vec![log_path]);
        let (entries, _offsets) = replay_session(&session, &default_filter(), 1000, false);

        assert_eq!(entries.len(), 1);
    }

    // =====================================================================
    // Test 20: DEFAULT_REPLAY_COUNT constant value
    // =====================================================================

    #[test]
    fn test_default_replay_count() {
        assert_eq!(DEFAULT_REPLAY_COUNT, 20);
    }
}
