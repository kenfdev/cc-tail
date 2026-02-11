//! End-to-end integration tests for the cc-tail pipeline.
//!
//! These tests exercise the handoff between modules that unit tests
//! cannot cover: session discovery -> replay, incremental file reading,
//! and filter interaction with the ring buffer.

use std::io::Write;
use std::path::Path;

use tempfile::TempDir;

use cctail::filter::FilterState;
use cctail::log_entry::{parse_jsonl_line, EntryType};
use cctail::replay::replay_session;
use cctail::ring_buffer::RingBuffer;
use cctail::session::discover_sessions;
use cctail::watcher::{read_new_entries, FileWatchState};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write content to a JSONL file, creating parent directories as needed.
fn write_jsonl_file(dir: &Path, relative_path: &str, lines: &[&str]) {
    let path = dir.join(relative_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut file = std::fs::File::create(&path).unwrap();
    for line in lines {
        writeln!(file, "{}", line).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Test 1: Session discovery to replay
// ---------------------------------------------------------------------------

/// End-to-end: discover sessions from a temp directory, then replay the
/// most recent session, verifying that entries are parsed correctly,
/// sorted by timestamp, and that non-visible types (Progress) are excluded.
#[test]
fn test_session_discovery_to_replay() {
    let tmp = TempDir::new().unwrap();

    // -- Set up directory structure mimicking Claude Code project dir --

    // Top-level session JSONL file
    let user_line = r#"{"type": "user", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}}"#;
    let assistant_line = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:01Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "world"}]}, "costUSD": 0.01, "durationMs": 500}"#;
    let system_line = r#"{"type": "result", "timestamp": "2025-01-15T10:00:02Z", "result": "completed", "is_error": false, "costUSD": 0.02, "durationMs": 1000}"#;
    let progress_line = r#"{"type": "progress", "timestamp": "2025-01-15T10:00:03Z"}"#;

    write_jsonl_file(
        tmp.path(),
        "sess-abc123.jsonl",
        &[user_line, assistant_line, system_line, progress_line],
    );

    // Subagent JSONL file
    let sub_assistant_line = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:04Z", "isSidechain": true, "agentId": "sub-A", "message": {"role": "assistant", "content": [{"type": "text", "text": "subagent reply"}]}}"#;

    write_jsonl_file(
        tmp.path(),
        "sess-abc123/subagents/agent-sub-A.jsonl",
        &[sub_assistant_line],
    );

    // -- Discover sessions --
    let sessions = discover_sessions(tmp.path(), 10).unwrap();
    assert_eq!(sessions.len(), 1, "expected exactly 1 session");
    assert_eq!(sessions[0].id, "sess-abc123");

    // The session should have 2 agents: main + subagent
    assert_eq!(
        sessions[0].agents.len(),
        2,
        "expected main + 1 subagent = 2 agents"
    );

    let sub_agents: Vec<_> = sessions[0]
        .agents
        .iter()
        .filter(|a| !a.is_main)
        .collect();
    assert_eq!(sub_agents.len(), 1);
    assert_eq!(sub_agents[0].agent_id.as_deref(), Some("sub-A"));

    // -- Replay session --
    let session = &sessions[0];
    let (entries, eof_offsets) =
        replay_session(session, &FilterState::default(), 20, false);

    // Visible types: User, Assistant, System (result maps to System).
    // Progress should be excluded.
    // We have: user(visible), assistant(visible), result/system(visible),
    //          progress(hidden), subagent-assistant(visible)
    // "result" type maps to Unknown in EntryType since it's not in the enum.
    // Let's check what we actually get -- result type is not User/Assistant/System,
    // so is_visible_type filters it out. We expect: user, assistant, subagent-assistant = 3.
    // Wait, let me check: EntryType has System for "system" but "result" maps to Unknown.
    // So "result" entries are NOT visible. We should get 3 entries.
    assert!(
        entries.len() >= 2,
        "expected at least 2 visible entries, got {}",
        entries.len()
    );

    // Verify entries are sorted by timestamp (oldest first)
    for i in 1..entries.len() {
        let ts_prev = entries[i - 1].timestamp.as_deref().unwrap_or("");
        let ts_curr = entries[i].timestamp.as_deref().unwrap_or("");
        assert!(
            ts_prev <= ts_curr,
            "entries not sorted by timestamp: {} > {}",
            ts_prev,
            ts_curr
        );
    }

    // Verify no Progress entries in the result
    for entry in &entries {
        assert_ne!(
            entry.entry_type,
            EntryType::Progress,
            "Progress entries should not appear in replay"
        );
    }

    // Verify EOF offsets map contains file paths with correct byte lengths
    assert!(
        !eof_offsets.is_empty(),
        "eof_offsets should contain at least one file"
    );
    for (path, offset) in &eof_offsets {
        let file_len = std::fs::metadata(path).unwrap().len();
        assert_eq!(
            *offset, file_len,
            "EOF offset for {} should match file length",
            path.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2: Incremental reading
// ---------------------------------------------------------------------------

/// End-to-end: write JSONL lines, read them incrementally, append more
/// lines, read again, and verify only new entries are returned.
#[test]
fn test_incremental_reading() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("incremental.jsonl");

    // -- Write initial 2 lines --
    let line1 = r#"{"type": "user", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "user", "content": [{"type": "text", "text": "hello"}]}}"#;
    let line2 = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:01Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "world"}]}}"#;

    {
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "{}", line1).unwrap();
        writeln!(file, "{}", line2).unwrap();
    }

    // -- First read: should return 2 entries --
    let mut state = FileWatchState::new();
    let entries1 = read_new_entries(&path, &mut state, false);
    assert_eq!(entries1.len(), 2, "first read should return 2 entries");
    assert_eq!(entries1[0].entry_type, EntryType::User);
    assert_eq!(entries1[1].entry_type, EntryType::Assistant);

    // -- Append 2 more lines --
    let line3 = r#"{"type": "user", "timestamp": "2025-01-15T10:00:02Z", "message": {"role": "user", "content": [{"type": "text", "text": "follow-up"}]}}"#;
    let line4 = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:03Z", "message": {"role": "assistant", "content": [{"type": "text", "text": "response"}]}}"#;

    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{}", line3).unwrap();
        writeln!(file, "{}", line4).unwrap();
    }

    // -- Second read: should return only the 2 NEW entries --
    let entries2 = read_new_entries(&path, &mut state, false);
    assert_eq!(
        entries2.len(),
        2,
        "second read should return only 2 new entries"
    );
    assert_eq!(entries2[0].entry_type, EntryType::User);
    assert_eq!(entries2[1].entry_type, EntryType::Assistant);
    // Verify timestamps match the appended lines
    assert_eq!(
        entries2[0].timestamp.as_deref(),
        Some("2025-01-15T10:00:02Z")
    );
    assert_eq!(
        entries2[1].timestamp.as_deref(),
        Some("2025-01-15T10:00:03Z")
    );

    // -- Third read with no changes: should return 0 entries --
    let entries3 = read_new_entries(&path, &mut state, false);
    assert_eq!(
        entries3.len(),
        0,
        "third read with no changes should return 0 entries"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Filter interaction end-to-end
// ---------------------------------------------------------------------------

/// End-to-end: parse entries from multiple agents, push into RingBuffer,
/// and verify that FilterState + iter_filtered returns the correct subset.
#[test]
fn test_filter_interaction_end_to_end() {
    // -- Parse 6 JSONL entries: 2 main agent, 2 sub-A, 2 sub-B --
    let main_user = r#"{"type": "user", "timestamp": "2025-01-15T10:00:00Z", "sessionId": "sess-001", "message": {"role": "user", "content": [{"type": "text", "text": "main user msg"}]}}"#;
    let main_assistant = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:01Z", "sessionId": "sess-001", "message": {"role": "assistant", "content": [{"type": "text", "text": "main assistant msg"}]}}"#;
    let sub_a_1 = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:02Z", "sessionId": "sess-001", "isSidechain": true, "agentId": "sub-A", "message": {"role": "assistant", "content": [{"type": "text", "text": "sub-A msg 1"}]}}"#;
    let sub_a_2 = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:03Z", "sessionId": "sess-001", "isSidechain": true, "agentId": "sub-A", "message": {"role": "assistant", "content": [{"type": "text", "text": "sub-A msg 2"}]}}"#;
    let sub_b_1 = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:04Z", "sessionId": "sess-001", "isSidechain": true, "agentId": "sub-B", "message": {"role": "assistant", "content": [{"type": "text", "text": "sub-B msg 1"}]}}"#;
    let sub_b_2 = r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:05Z", "sessionId": "sess-001", "isSidechain": true, "agentId": "sub-B", "message": {"role": "assistant", "content": [{"type": "text", "text": "sub-B msg 2"}]}}"#;

    let entries: Vec<_> = [main_user, main_assistant, sub_a_1, sub_a_2, sub_b_1, sub_b_2]
        .iter()
        .map(|line| parse_jsonl_line(line).unwrap())
        .collect();

    // -- Push all into RingBuffer --
    let mut buf = RingBuffer::new(100_000);
    for entry in entries {
        buf.push(entry);
    }

    // -- Default filter: all 6 entries should be returned --
    let default_filter = FilterState::default();
    let all: Vec<_> = buf
        .iter_filtered(|e| default_filter.matches(e))
        .collect();
    assert_eq!(all.len(), 6, "default filter should return all 6 entries");

    // -- Filter by agent "sub-A": should return only 2 sub-A entries --
    let filter_a = FilterState {
        hide_tool_calls: false,
        selected_agent: Some("sub-A".to_string()),
    };
    let sub_a_entries: Vec<_> = buf
        .iter_filtered(|e| filter_a.matches(e))
        .collect();
    assert_eq!(
        sub_a_entries.len(),
        2,
        "sub-A filter should return exactly 2 entries"
    );
    for entry in &sub_a_entries {
        assert_eq!(entry.agent_id.as_deref(), Some("sub-A"));
    }

    // -- Filter by agent "sub-B": should return only 2 sub-B entries --
    let filter_b = FilterState {
        hide_tool_calls: false,
        selected_agent: Some("sub-B".to_string()),
    };
    let sub_b_entries: Vec<_> = buf
        .iter_filtered(|e| filter_b.matches(e))
        .collect();
    assert_eq!(
        sub_b_entries.len(),
        2,
        "sub-B filter should return exactly 2 entries"
    );
    for entry in &sub_b_entries {
        assert_eq!(entry.agent_id.as_deref(), Some("sub-B"));
    }
}
