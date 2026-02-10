//! Session statistics computation for the help overlay.
//!
//! Provides [`SessionStats`] and [`compute_session_stats()`] to derive
//! summary statistics from the entries in a ring buffer: session duration,
//! message counts, tool call breakdown, subagent count, etc.

use std::collections::{HashMap, HashSet};

use crate::log_entry::{EntryType, LogEntry};
use crate::ring_buffer::RingBuffer;

// ---------------------------------------------------------------------------
// SessionStats
// ---------------------------------------------------------------------------

/// Summary statistics computed from loaded log entries.
///
/// All fields are derived from a snapshot of the ring buffer and represent
/// the currently loaded entries, not necessarily the entire session history.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    /// Total number of loaded entries in the ring buffer.
    pub entries_loaded: usize,

    /// Number of user messages.
    pub user_message_count: usize,

    /// Number of assistant messages.
    pub assistant_message_count: usize,

    /// Total number of `tool_use` content blocks across all entries.
    pub tool_call_count: usize,

    /// Breakdown of tool calls by tool name, sorted by count descending.
    pub tool_call_breakdown: Vec<(String, usize)>,

    /// Number of unique subagents (entries with `is_sidechain == true`).
    pub subagent_count: usize,

    /// ISO 8601 timestamp of the earliest entry, if available.
    pub earliest_timestamp: Option<String>,

    /// ISO 8601 timestamp of the latest entry, if available.
    pub latest_timestamp: Option<String>,

    /// Human-readable session duration string (e.g. "2h 15m", "45m 30s").
    /// `None` if timestamps are missing or unparseable.
    pub duration_display: Option<String>,
}

// ---------------------------------------------------------------------------
// Computation
// ---------------------------------------------------------------------------

/// Compute session statistics from the entries in a ring buffer.
///
/// Iterates all entries once and collects counts. Only `tool_use` content
/// blocks are counted for tool stats (not `tool_result`).
pub fn compute_session_stats(ring_buffer: &RingBuffer) -> SessionStats {
    let mut stats = SessionStats::default();

    let mut tool_counts: HashMap<String, usize> = HashMap::new();
    let mut subagent_ids: HashSet<String> = HashSet::new();
    let mut earliest: Option<&str> = None;
    let mut latest: Option<&str> = None;

    for entry in ring_buffer.iter() {
        stats.entries_loaded += 1;

        // Track timestamps.
        if let Some(ref ts) = entry.timestamp {
            let ts_str = ts.as_str();
            match earliest {
                None => earliest = Some(ts_str),
                Some(e) if ts_str < e => earliest = Some(ts_str),
                _ => {}
            }
            match latest {
                None => latest = Some(ts_str),
                Some(l) if ts_str > l => latest = Some(ts_str),
                _ => {}
            }
        }

        // Count by entry type.
        match entry.entry_type {
            EntryType::User => stats.user_message_count += 1,
            EntryType::Assistant => stats.assistant_message_count += 1,
            _ => {}
        }

        // Track subagents.
        if entry.is_sidechain == Some(true) {
            if let Some(ref agent_id) = entry.agent_id {
                subagent_ids.insert(agent_id.clone());
            }
        }

        // Count tool_use blocks in message content.
        count_tool_uses(entry, &mut tool_counts);
    }

    stats.subagent_count = subagent_ids.len();
    stats.tool_call_count = tool_counts.values().sum();

    // Sort tool breakdown by count descending, then name ascending.
    let mut breakdown: Vec<(String, usize)> = tool_counts.into_iter().collect();
    breakdown.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    stats.tool_call_breakdown = breakdown;

    // Store timestamps.
    stats.earliest_timestamp = earliest.map(|s| s.to_string());
    stats.latest_timestamp = latest.map(|s| s.to_string());

    // Compute duration display.
    stats.duration_display = compute_duration_display(earliest, latest);

    stats
}

/// Count `tool_use` content blocks in an entry's message content.
///
/// Only blocks with `"type": "tool_use"` are counted. `tool_result` blocks
/// are explicitly excluded.
fn count_tool_uses(entry: &LogEntry, tool_counts: &mut HashMap<String, usize>) {
    let message = match &entry.message {
        Some(m) => m,
        None => return,
    };

    let blocks = match message.content.as_array() {
        Some(arr) => arr,
        None => return,
    };

    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            let name = block
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unknown");
            *tool_counts.entry(name.to_string()).or_insert(0) += 1;
        }
    }
}

/// Compute a human-readable duration string from two ISO 8601 timestamps.
///
/// Returns `None` if either timestamp is missing or cannot be parsed.
/// Uses simple string parsing rather than a full datetime library to
/// keep dependencies minimal.
fn compute_duration_display(earliest: Option<&str>, latest: Option<&str>) -> Option<String> {
    let start = earliest?;
    let end = latest?;

    let start_secs = parse_iso8601_to_epoch_secs(start)?;
    let end_secs = parse_iso8601_to_epoch_secs(end)?;

    if end_secs < start_secs {
        return None;
    }

    let diff_secs = end_secs - start_secs;
    Some(format_duration_secs(diff_secs))
}

/// Parse a subset of ISO 8601 timestamps to epoch seconds (UTC).
///
/// Handles formats like:
/// - `2025-01-15T10:30:00Z`
/// - `2025-01-15T10:30:00.123Z`
/// - `2025-01-15T10:30:00+00:00`
///
/// Returns `None` for unparseable timestamps. This is a best-effort parser
/// that avoids pulling in `chrono` as a dependency.
fn parse_iso8601_to_epoch_secs(ts: &str) -> Option<u64> {
    // Strip fractional seconds and timezone suffix to get core datetime.
    // Expected: "YYYY-MM-DDTHH:MM:SS..."
    if ts.len() < 19 {
        return None;
    }

    let core = &ts[..19]; // "2025-01-15T10:30:00"

    let year: u64 = core[0..4].parse().ok()?;
    let month: u64 = core[5..7].parse().ok()?;
    let day: u64 = core[8..10].parse().ok()?;
    let hour: u64 = core[11..13].parse().ok()?;
    let minute: u64 = core[14..16].parse().ok()?;
    let second: u64 = core[17..19].parse().ok()?;

    // Validate ranges.
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    // Approximate days from epoch (2000-01-01 as a simpler base, then adjust).
    // We only need relative differences, so absolute accuracy is not critical.
    // Use a simplified calculation that works for dates between 2000-2100.
    let days_from_year = (year - 1970) * 365 + leap_years_since_1970(year);
    let days_from_month = days_in_months_before(month, is_leap_year(year));
    let total_days = days_from_year + days_from_month + (day - 1);

    Some(total_days * 86400 + hour * 3600 + minute * 60 + second)
}

/// Count leap years between 1970 and the given year (exclusive).
fn leap_years_since_1970(year: u64) -> u64 {
    if year <= 1970 {
        return 0;
    }
    let y = year - 1; // count up to year-1
    let count_from_0 = y / 4 - y / 100 + y / 400;
    let count_before_1970 = 1969 / 4 - 1969 / 100 + 1969 / 400;
    count_from_0 - count_before_1970
}

/// Check if a year is a leap year.
fn is_leap_year(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Total days in all months before `month` (1-based).
fn days_in_months_before(month: u64, leap: bool) -> u64 {
    let days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut total = 0u64;
    for m in 1..month {
        total += days[m as usize];
    }
    if leap && month > 2 {
        total += 1;
    }
    total
}

/// Format a duration in seconds as a human-readable string.
///
/// Examples: "0s", "45s", "2m 30s", "1h 15m", "2h 0m", "25h 30m".
fn format_duration_secs(secs: u64) -> String {
    if secs < 60 {
        return format!("{}s", secs);
    }

    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let remaining_secs = secs % 60;

    if hours == 0 {
        if remaining_secs > 0 {
            format!("{}m {}s", minutes, remaining_secs)
        } else {
            format!("{}m", minutes)
        }
    } else {
        format!("{}h {}m", hours, minutes)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_entry::parse_jsonl_line;

    // -- Helper: push a parsed entry into a ring buffer ----------------------

    fn push_entry(buf: &mut RingBuffer, json: &str) {
        let entry = parse_jsonl_line(json).expect("test JSON should be valid");
        buf.push(entry);
    }

    // -- compute_session_stats: empty buffer ---------------------------------

    #[test]
    fn test_empty_buffer_stats() {
        let buf = RingBuffer::new(100_000);
        let stats = compute_session_stats(&buf);

        assert_eq!(stats.entries_loaded, 0);
        assert_eq!(stats.user_message_count, 0);
        assert_eq!(stats.assistant_message_count, 0);
        assert_eq!(stats.tool_call_count, 0);
        assert!(stats.tool_call_breakdown.is_empty());
        assert_eq!(stats.subagent_count, 0);
        assert!(stats.earliest_timestamp.is_none());
        assert!(stats.latest_timestamp.is_none());
        assert!(stats.duration_display.is_none());
    }

    // -- compute_session_stats: message counts --------------------------------

    #[test]
    fn test_message_counts() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "user", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "user", "content": "hi"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "timestamp": "2025-01-15T10:01:00Z", "message": {"role": "assistant", "content": "hello"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "user", "timestamp": "2025-01-15T10:02:00Z", "message": {"role": "user", "content": "bye"}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.entries_loaded, 3);
        assert_eq!(stats.user_message_count, 2);
        assert_eq!(stats.assistant_message_count, 1);
    }

    // -- compute_session_stats: tool call counting ----------------------------

    #[test]
    fn test_tool_call_counting() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "assistant", "content": [
                {"type": "text", "text": "Let me read that file."},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/foo"}},
                {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "ls"}}
            ]}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "timestamp": "2025-01-15T10:01:00Z", "message": {"role": "assistant", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "file contents"},
                {"type": "tool_use", "id": "t3", "name": "Read", "input": {"file_path": "/bar"}}
            ]}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.tool_call_count, 3);
        // tool_result should NOT be counted.

        // Breakdown: Read=2, Bash=1
        assert_eq!(stats.tool_call_breakdown.len(), 2);
        assert_eq!(stats.tool_call_breakdown[0], ("Read".to_string(), 2));
        assert_eq!(stats.tool_call_breakdown[1], ("Bash".to_string(), 1));
    }

    // -- compute_session_stats: tool_result not counted -----------------------

    #[test]
    fn test_tool_result_not_counted() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "result"}
            ]}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.tool_call_count, 0);
        assert!(stats.tool_call_breakdown.is_empty());
    }

    // -- compute_session_stats: subagent counting -----------------------------

    #[test]
    fn test_subagent_counting() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "isSidechain": true, "agentId": "abc", "slug": "cool-agent", "message": {"role": "assistant", "content": "hi"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "isSidechain": true, "agentId": "abc", "slug": "cool-agent", "message": {"role": "assistant", "content": "again"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "isSidechain": true, "agentId": "def", "slug": "other-agent", "message": {"role": "assistant", "content": "hey"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "user", "message": {"role": "user", "content": "main user"}}"#,
        );

        let stats = compute_session_stats(&buf);
        // Two unique subagent IDs: "abc" and "def".
        assert_eq!(stats.subagent_count, 2);
    }

    // -- compute_session_stats: timestamps and duration -----------------------

    #[test]
    fn test_timestamps_and_duration() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "user", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "user", "content": "start"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "timestamp": "2025-01-15T10:30:00Z", "message": {"role": "assistant", "content": "mid"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "user", "timestamp": "2025-01-15T11:15:00Z", "message": {"role": "user", "content": "end"}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(
            stats.earliest_timestamp.as_deref(),
            Some("2025-01-15T10:00:00Z")
        );
        assert_eq!(
            stats.latest_timestamp.as_deref(),
            Some("2025-01-15T11:15:00Z")
        );
        assert_eq!(stats.duration_display.as_deref(), Some("1h 15m"));
    }

    // -- compute_session_stats: no timestamps ---------------------------------

    #[test]
    fn test_no_timestamps() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "user", "message": {"role": "user", "content": "no ts"}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert!(stats.earliest_timestamp.is_none());
        assert!(stats.latest_timestamp.is_none());
        assert!(stats.duration_display.is_none());
    }

    // -- compute_session_stats: progress entries not counted as user/assistant -

    #[test]
    fn test_progress_entries_not_counted_as_messages() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "progress", "timestamp": "2025-01-15T10:00:00Z", "data": {"status": "thinking"}}"#,
        );
        push_entry(
            &mut buf,
            r#"{"type": "user", "timestamp": "2025-01-15T10:01:00Z", "message": {"role": "user", "content": "hi"}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.entries_loaded, 2);
        assert_eq!(stats.user_message_count, 1);
        assert_eq!(stats.assistant_message_count, 0);
    }

    // -- compute_session_stats: string content (no tool blocks) ---------------

    #[test]
    fn test_string_content_no_tool_blocks() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "message": {"role": "assistant", "content": "plain text"}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.tool_call_count, 0);
        assert!(stats.tool_call_breakdown.is_empty());
    }

    // -- format_duration_secs -------------------------------------------------

    #[test]
    fn test_format_duration_zero() {
        assert_eq!(format_duration_secs(0), "0s");
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration_secs(45), "45s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration_secs(120), "2m");
    }

    #[test]
    fn test_format_duration_minutes_and_seconds() {
        assert_eq!(format_duration_secs(150), "2m 30s");
    }

    #[test]
    fn test_format_duration_hours_and_minutes() {
        assert_eq!(format_duration_secs(3900), "1h 5m");
    }

    #[test]
    fn test_format_duration_exact_hour() {
        assert_eq!(format_duration_secs(7200), "2h 0m");
    }

    #[test]
    fn test_format_duration_large() {
        assert_eq!(format_duration_secs(91800), "25h 30m");
    }

    // -- parse_iso8601_to_epoch_secs ------------------------------------------

    #[test]
    fn test_parse_iso8601_basic() {
        let a = parse_iso8601_to_epoch_secs("2025-01-15T10:00:00Z");
        let b = parse_iso8601_to_epoch_secs("2025-01-15T10:30:00Z");
        assert!(a.is_some());
        assert!(b.is_some());
        assert_eq!(b.unwrap() - a.unwrap(), 1800); // 30 minutes
    }

    #[test]
    fn test_parse_iso8601_with_fractional_seconds() {
        let a = parse_iso8601_to_epoch_secs("2025-01-15T10:00:00.123Z");
        assert!(a.is_some());
    }

    #[test]
    fn test_parse_iso8601_too_short() {
        assert!(parse_iso8601_to_epoch_secs("2025").is_none());
    }

    #[test]
    fn test_parse_iso8601_invalid() {
        assert!(parse_iso8601_to_epoch_secs("not-a-timestamp-value").is_none());
    }

    // -- tool_call_breakdown ordering -----------------------------------------

    #[test]
    fn test_tool_call_breakdown_sorted_by_count() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {}},
                {"type": "tool_use", "id": "t2", "name": "Read", "input": {}},
                {"type": "tool_use", "id": "t3", "name": "Read", "input": {}},
                {"type": "tool_use", "id": "t4", "name": "Bash", "input": {}},
                {"type": "tool_use", "id": "t5", "name": "Bash", "input": {}},
                {"type": "tool_use", "id": "t6", "name": "Write", "input": {}}
            ]}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.tool_call_count, 6);
        // Bash=3, Read=2, Write=1
        assert_eq!(stats.tool_call_breakdown[0], ("Bash".to_string(), 3));
        assert_eq!(stats.tool_call_breakdown[1], ("Read".to_string(), 2));
        assert_eq!(stats.tool_call_breakdown[2], ("Write".to_string(), 1));
    }

    // -- tool_use with missing name field ------------------------------------

    #[test]
    fn test_tool_use_missing_name_defaults_to_unknown() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "input": {}}
            ]}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.tool_call_count, 1);
        assert_eq!(stats.tool_call_breakdown[0], ("unknown".to_string(), 1));
    }

    // -- compute_duration_display edge cases ---------------------------------

    #[test]
    fn test_duration_same_timestamp() {
        let mut buf = RingBuffer::new(100_000);
        push_entry(
            &mut buf,
            r#"{"type": "user", "timestamp": "2025-01-15T10:00:00Z", "message": {"role": "user", "content": "only"}}"#,
        );

        let stats = compute_session_stats(&buf);
        assert_eq!(stats.duration_display.as_deref(), Some("0s"));
    }
}
