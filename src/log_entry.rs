use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Entry type enum
// ---------------------------------------------------------------------------

/// The `type` field in a JSONL log entry.
///
/// Uses kebab-case to match the JSON values produced by Claude Code.
/// The `Unknown` variant acts as a catch-all for forward-compatibility:
/// any unrecognised type string deserializes to `Unknown` instead of
/// failing, thanks to `#[serde(other)]`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryType {
    User,
    Assistant,
    Progress,
    FileHistorySnapshot,
    System,
    QueueOperation,
    /// Catch-all for entry types not yet modelled.
    #[default]
    #[serde(other)]
    Unknown,
}

// ---------------------------------------------------------------------------
// Message struct
// ---------------------------------------------------------------------------

/// The `message` object embedded inside a log entry.
///
/// `content` is kept as a raw `serde_json::Value` so that we remain
/// forward-compatible with new content-block shapes without needing
/// to update this struct every time Claude adds a block type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Message {
    pub role: Option<String>,
    pub content: Value,
    pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// LogEntry struct
// ---------------------------------------------------------------------------

/// A single parsed JSONL log entry.
///
/// Field naming in the JSON is inconsistent (mix of camelCase and
/// snake_case-ish), so each field carries an explicit `#[serde(rename)]`
/// or `#[serde(alias)]` where necessary.
///
/// `#[serde(default)]` at the struct level ensures that any missing
/// field gets its `Default` value instead of causing a parse error.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LogEntry {
    /// The entry type (renamed from the JSON key `"type"`).
    #[serde(rename = "type")]
    pub entry_type: EntryType,

    /// Session UUID shared across main and subagent logs.
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,

    /// ISO 8601 timestamp.
    pub timestamp: Option<String>,

    /// Unique identifier for this entry.
    pub uuid: Option<String>,

    /// Parent entry UUID (message threading).
    #[serde(rename = "parentUuid")]
    pub parent_uuid: Option<String>,

    /// `true` for subagent entries.
    #[serde(rename = "isSidechain")]
    pub is_sidechain: Option<bool>,

    /// Subagent identifier (e.g. `"a0d0bbc"`). Absent on main session.
    #[serde(rename = "agentId")]
    pub agent_id: Option<String>,

    /// Human-readable subagent name (e.g. `"effervescent-soaring-cook"`).
    pub slug: Option<String>,

    /// The message payload.
    pub message: Option<Message>,

    /// Opaque data payload used by some entry types (e.g. progress).
    pub data: Option<Value>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a single JSONL line into a `LogEntry`.
///
/// Returns `Err` for malformed JSON. The caller decides whether to skip
/// or warn (e.g. silent skip in normal mode, stderr warning in verbose).
pub fn parse_jsonl_line(line: &str) -> Result<LogEntry, serde_json::Error> {
    serde_json::from_str(line)
}

// ---------------------------------------------------------------------------
// Byte-size estimation
// ---------------------------------------------------------------------------

impl LogEntry {
    /// Returns an estimated byte size by re-serializing the entry to JSON.
    ///
    /// Used for ring-buffer accounting (byte-budget eviction). The result
    /// is an approximation — it may differ slightly from the original line
    /// due to whitespace normalisation and key ordering, but is accurate
    /// enough for budget tracking.
    pub fn estimated_byte_size(&self) -> usize {
        serde_json::to_string(self).map(|s| s.len()).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- 1. Parse assistant entry with all known fields -----------------------

    #[test]
    fn test_parse_assistant_entry_all_fields() {
        let json = r#"{
            "type": "assistant",
            "sessionId": "sess-001",
            "timestamp": "2025-01-15T10:30:00Z",
            "uuid": "uuid-aaa",
            "parentUuid": "uuid-parent",
            "isSidechain": false,
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Hello!"}],
                "model": "claude-opus-4-6"
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::Assistant);
        assert_eq!(entry.session_id.as_deref(), Some("sess-001"));
        assert_eq!(entry.timestamp.as_deref(), Some("2025-01-15T10:30:00Z"));
        assert_eq!(entry.uuid.as_deref(), Some("uuid-aaa"));
        assert_eq!(entry.parent_uuid.as_deref(), Some("uuid-parent"));
        assert_eq!(entry.is_sidechain, Some(false));

        let msg = entry.message.as_ref().unwrap();
        assert_eq!(msg.role.as_deref(), Some("assistant"));
        assert_eq!(msg.model.as_deref(), Some("claude-opus-4-6"));
        assert!(msg.content.is_array());
        assert_eq!(msg.content.as_array().unwrap().len(), 1);
    }

    // -- 2. Parse user entry --------------------------------------------------

    #[test]
    fn test_parse_user_entry() {
        let json = r#"{
            "type": "user",
            "sessionId": "sess-002",
            "timestamp": "2025-01-15T10:29:00Z",
            "uuid": "uuid-bbb",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "Fix the bug"}]
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::User);
        assert_eq!(entry.session_id.as_deref(), Some("sess-002"));
        let msg = entry.message.as_ref().unwrap();
        assert_eq!(msg.role.as_deref(), Some("user"));
        assert!(msg.content.is_array());
    }

    // -- 3. Parse progress entry ----------------------------------------------

    #[test]
    fn test_parse_progress_entry() {
        let json = r#"{
            "type": "progress",
            "sessionId": "sess-003",
            "timestamp": "2025-01-15T10:31:00Z",
            "data": {"status": "thinking"}
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::Progress);
        assert_eq!(entry.session_id.as_deref(), Some("sess-003"));
        assert!(entry.data.is_some());
        assert_eq!(entry.data.as_ref().unwrap()["status"], "thinking");
    }

    // -- 4. Parse file-history-snapshot entry (minimal fields) ----------------

    #[test]
    fn test_parse_file_history_snapshot_minimal() {
        let json = r#"{"type": "file-history-snapshot"}"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::FileHistorySnapshot);
        assert_eq!(entry.session_id, None);
        assert_eq!(entry.timestamp, None);
        assert_eq!(entry.message, None);
    }

    // -- 5. Parse system entry ------------------------------------------------

    #[test]
    fn test_parse_system_entry() {
        let json = r#"{
            "type": "system",
            "sessionId": "sess-005",
            "timestamp": "2025-01-15T10:00:00Z",
            "message": {
                "role": "user",
                "content": "System prompt text"
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::System);
        assert_eq!(entry.session_id.as_deref(), Some("sess-005"));
        let msg = entry.message.as_ref().unwrap();
        assert!(msg.content.is_string());
        assert_eq!(msg.content.as_str().unwrap(), "System prompt text");
    }

    // -- 6. Parse queue-operation entry ---------------------------------------

    #[test]
    fn test_parse_queue_operation_entry() {
        let json = r#"{
            "type": "queue-operation",
            "sessionId": "sess-006",
            "data": {"operation": "enqueue", "item": "task-1"}
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::QueueOperation);
        assert!(entry.data.is_some());
    }

    // -- 7. Unknown entry type falls to Unknown variant -----------------------

    #[test]
    fn test_unknown_entry_type() {
        let json = r#"{"type": "some-future-type", "sessionId": "sess-007"}"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::Unknown);
        assert_eq!(entry.session_id.as_deref(), Some("sess-007"));
    }

    // -- 8. Unknown/extra JSON fields are silently ignored --------------------

    #[test]
    fn test_extra_fields_ignored() {
        let json = r#"{
            "type": "assistant",
            "sessionId": "sess-008",
            "unknownField": "should be ignored",
            "anotherExtra": 42,
            "nested": {"deep": true}
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::Assistant);
        assert_eq!(entry.session_id.as_deref(), Some("sess-008"));
    }

    // -- 9. Malformed JSON returns Err ----------------------------------------

    #[test]
    fn test_malformed_json_returns_err() {
        let bad_json = r#"{"type": "user", broken"#;
        let result = parse_jsonl_line(bad_json);
        assert!(result.is_err());
    }

    // -- 10. Empty string returns Err -----------------------------------------

    #[test]
    fn test_empty_string_returns_err() {
        let result = parse_jsonl_line("");
        assert!(result.is_err());
    }

    // -- 11. Subagent entry (with agentId, slug, parentUuid) ------------------

    #[test]
    fn test_subagent_entry() {
        let json = r#"{
            "type": "assistant",
            "sessionId": "sess-011",
            "uuid": "uuid-sub-1",
            "parentUuid": "uuid-main-1",
            "isSidechain": true,
            "agentId": "a0d0bbc",
            "slug": "effervescent-soaring-cook",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Subagent response"}],
                "model": "claude-haiku-4-5-20251001"
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::Assistant);
        assert_eq!(entry.is_sidechain, Some(true));
        assert_eq!(entry.agent_id.as_deref(), Some("a0d0bbc"));
        assert_eq!(entry.slug.as_deref(), Some("effervescent-soaring-cook"));
        assert_eq!(entry.parent_uuid.as_deref(), Some("uuid-main-1"));
        let msg = entry.message.as_ref().unwrap();
        assert_eq!(msg.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    }

    // -- 12. Default values for missing optional fields -----------------------

    #[test]
    fn test_default_values_for_missing_fields() {
        let json = r#"{"type": "user"}"#;

        let entry = parse_jsonl_line(json).unwrap();
        assert_eq!(entry.entry_type, EntryType::User);
        assert_eq!(entry.session_id, None);
        assert_eq!(entry.timestamp, None);
        assert_eq!(entry.uuid, None);
        assert_eq!(entry.parent_uuid, None);
        assert_eq!(entry.is_sidechain, None);
        assert_eq!(entry.agent_id, None);
        assert_eq!(entry.slug, None);
        assert!(entry.message.is_none());
        assert!(entry.data.is_none());
    }

    // -- 13. Content as string value ------------------------------------------

    #[test]
    fn test_content_as_string() {
        let json = r#"{
            "type": "system",
            "message": {
                "role": "user",
                "content": "A plain string content"
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        let msg = entry.message.as_ref().unwrap();
        assert!(msg.content.is_string());
        assert_eq!(msg.content.as_str().unwrap(), "A plain string content");
    }

    // -- 14. Content as array value -------------------------------------------

    #[test]
    fn test_content_as_array() {
        let json = r#"{
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "First block"},
                    {"type": "tool_use", "id": "tool-1", "name": "Read", "input": {}},
                    {"type": "tool_result", "tool_use_id": "tool-1", "content": "file contents"}
                ]
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        let msg = entry.message.as_ref().unwrap();
        assert!(msg.content.is_array());
        let arr = msg.content.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "tool_use");
        assert_eq!(arr[2]["type"], "tool_result");
    }

    // -- 15. Empty content array ----------------------------------------------

    #[test]
    fn test_empty_content_array() {
        let json = r#"{
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": []
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        let msg = entry.message.as_ref().unwrap();
        assert!(msg.content.is_array());
        assert!(msg.content.as_array().unwrap().is_empty());
    }

    // -- 16. estimated_byte_size returns reasonable value ---------------------

    #[test]
    fn test_estimated_byte_size() {
        let json = r#"{
            "type": "assistant",
            "sessionId": "sess-016",
            "timestamp": "2025-01-15T10:30:00Z",
            "uuid": "uuid-016",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Hello, world!"}],
                "model": "claude-opus-4-6"
            }
        }"#;

        let entry = parse_jsonl_line(json).unwrap();
        let size = entry.estimated_byte_size();

        // The re-serialized form should be non-zero and roughly in the
        // expected range (the compact JSON is ~200-300 bytes).
        assert!(size > 0, "estimated_byte_size should be positive");
        assert!(
            size < 1000,
            "estimated_byte_size should be reasonable for a small entry, got {}",
            size
        );

        // Sanity-check: a minimal entry should be smaller than one with content.
        let minimal_json = r#"{"type": "user"}"#;
        let minimal_entry = parse_jsonl_line(minimal_json).unwrap();
        let minimal_size = minimal_entry.estimated_byte_size();
        assert!(
            minimal_size < size,
            "minimal entry ({}) should be smaller than full entry ({})",
            minimal_size,
            size
        );
    }
}
