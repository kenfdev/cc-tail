use serde_json::Value;

use crate::tool_summary::summarize_tool_use;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single rendered line derived from a content block.
///
/// Each variant carries a ready-to-display `String`:
/// - `Text` — a line of plain text from a `"text"` content block.
/// - `ToolUse` — a one-line summary of a `"tool_use"` content block.
/// - `Unknown` — an indicator for an unrecognised block type, showing
///   the type label and the serialised size of the block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderedLine {
    Text(String),
    ToolUse(String),
    Unknown(String),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a `message.content` value into a list of [`RenderedLine`]s.
///
/// Handles the three shapes that `content` can take:
/// - **Array of objects**: iterates in order, dispatching each block by its
///   `"type"` field.
/// - **String**: wraps as `RenderedLine::Text` lines (split on newlines).
/// - **Null / other**: returns an empty `Vec`.
pub fn render_content_blocks(content: &Value) -> Vec<RenderedLine> {
    match content {
        Value::Array(blocks) => render_array(blocks),
        Value::String(s) => split_text_lines(s),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Private helpers — array dispatch
// ---------------------------------------------------------------------------

/// Render an array of content blocks.
fn render_array(blocks: &[Value]) -> Vec<RenderedLine> {
    let mut lines = Vec::new();
    for block in blocks {
        // Non-object array elements are silently skipped.
        let obj = match block.as_object() {
            Some(o) => o,
            None => continue,
        };

        let block_type = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        match block_type {
            "text" => {
                if let Some(text_field) = obj.get("text") {
                    if let Some(s) = text_field.as_str() {
                        lines.extend(split_text_lines(s));
                    }
                    // If "text" is present but not a string, skip.
                }
                // If "text" key is missing entirely, skip.
            }
            "tool_use" => {
                let name = obj
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let input = obj.get("input").unwrap_or(&Value::Null);
                let summary = summarize_tool_use(name, input);
                lines.push(RenderedLine::ToolUse(summary));
            }
            "tool_result" => {
                // Explicitly skipped per spec.
            }
            _ => {
                let size_bytes = serde_json::to_string(block)
                    .map(|s| s.len())
                    .unwrap_or(0);
                let label = format!("[{}] ({})", block_type, format_size(size_bytes));
                lines.push(RenderedLine::Unknown(label));
            }
        }
    }
    lines
}

/// Split a string on newlines and wrap each line as `RenderedLine::Text`.
fn split_text_lines(s: &str) -> Vec<RenderedLine> {
    s.split('\n').map(|l| RenderedLine::Text(l.to_string())).collect()
}

// ---------------------------------------------------------------------------
// Private helpers — size formatting
// ---------------------------------------------------------------------------

/// Format a byte count as a human-readable size string.
///
/// - `< 1024`       → `"NB"`    (e.g. `"42B"`)
/// - `>= 1024, < 1M` → `"N.NKB"` (e.g. `"12.3KB"`)
/// - `>= 1M`         → `"N.NMB"` (e.g. `"1.5MB"`)
fn format_size(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * 1024;

    if bytes < KB {
        format!("{}B", bytes)
    } else if bytes < MB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // 1. Text block tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_text_block_single_line() {
        let content = json!([{"type": "text", "text": "Hello, world!"}]);
        let result = render_content_blocks(&content);
        assert_eq!(result, vec![RenderedLine::Text("Hello, world!".to_string())]);
    }

    #[test]
    fn test_text_block_multi_line() {
        let content = json!([{"type": "text", "text": "line1\nline2\nline3"}]);
        let result = render_content_blocks(&content);
        assert_eq!(
            result,
            vec![
                RenderedLine::Text("line1".to_string()),
                RenderedLine::Text("line2".to_string()),
                RenderedLine::Text("line3".to_string()),
            ]
        );
    }

    #[test]
    fn test_text_block_empty_text() {
        let content = json!([{"type": "text", "text": ""}]);
        let result = render_content_blocks(&content);
        assert_eq!(result, vec![RenderedLine::Text("".to_string())]);
    }

    #[test]
    fn test_text_block_missing_text_field() {
        let content = json!([{"type": "text"}]);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_text_block_text_field_is_not_string() {
        let content = json!([{"type": "text", "text": 42}]);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // 2. tool_use block tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_use_happy_path() {
        let content = json!([{
            "type": "tool_use",
            "id": "tool-1",
            "name": "Read",
            "input": {"file_path": "src/main.rs"}
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::ToolUse(s) => {
                assert!(s.contains("Read"));
                assert!(s.contains("src/main.rs"));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_use_bash_command() {
        let content = json!([{
            "type": "tool_use",
            "id": "tool-2",
            "name": "Bash",
            "input": {"command": "cargo test"}
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::ToolUse(s) => {
                assert!(s.contains("Bash"));
                assert!(s.contains("cargo test"));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_use_missing_name() {
        let content = json!([{
            "type": "tool_use",
            "id": "tool-3",
            "input": {"file_path": "foo.rs"}
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], RenderedLine::ToolUse(_)));
    }

    #[test]
    fn test_tool_use_missing_input() {
        let content = json!([{
            "type": "tool_use",
            "id": "tool-4",
            "name": "Read"
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::ToolUse(s) => {
                assert!(s.contains("Read"));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_use_missing_both_name_and_input() {
        let content = json!([{
            "type": "tool_use",
            "id": "tool-5"
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], RenderedLine::ToolUse(_)));
    }

    // -----------------------------------------------------------------------
    // 3. tool_result block tests (should be skipped)
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_result_is_skipped() {
        let content = json!([{
            "type": "tool_result",
            "tool_use_id": "tool-1",
            "content": "file contents here"
        }]);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_tool_result_among_other_blocks() {
        let content = json!([
            {"type": "text", "text": "before"},
            {"type": "tool_result", "tool_use_id": "tool-1", "content": "result"},
            {"type": "text", "text": "after"}
        ]);
        let result = render_content_blocks(&content);
        assert_eq!(
            result,
            vec![
                RenderedLine::Text("before".to_string()),
                RenderedLine::Text("after".to_string()),
            ]
        );
    }

    // -----------------------------------------------------------------------
    // 4. Unknown block tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_unknown_block_thinking() {
        let content = json!([{
            "type": "thinking",
            "thinking": "Let me consider this..."
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::Unknown(s) => {
                assert!(s.starts_with("[thinking]"));
                assert!(s.contains('B') || s.contains("KB") || s.contains("MB"));
            }
            other => panic!("expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_block_server_tool_use() {
        let content = json!([{
            "type": "server_tool_use",
            "name": "some_server_tool",
            "input": {}
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::Unknown(s) => {
                assert!(s.starts_with("[server_tool_use]"));
            }
            other => panic!("expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_block_image() {
        let content = json!([{
            "type": "image",
            "source": {"type": "base64", "data": "abc123"}
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::Unknown(s) => {
                assert!(s.starts_with("[image]"));
            }
            other => panic!("expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_block_missing_type_field() {
        let content = json!([{"data": "some data"}]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::Unknown(s) => {
                assert!(s.starts_with("[unknown]"));
            }
            other => panic!("expected Unknown, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // 5. Mixed content array tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_mixed_content_preserves_order() {
        let content = json!([
            {"type": "text", "text": "Hello"},
            {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "Goodbye"}
        ]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 4);
        assert!(matches!(&result[0], RenderedLine::Text(s) if s == "Hello"));
        assert!(matches!(&result[1], RenderedLine::ToolUse(_)));
        assert!(matches!(&result[2], RenderedLine::Unknown(_)));
        assert!(matches!(&result[3], RenderedLine::Text(s) if s == "Goodbye"));
    }

    #[test]
    fn test_mixed_with_tool_result_skipped() {
        let content = json!([
            {"type": "text", "text": "Start"},
            {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a.rs"}},
            {"type": "tool_result", "tool_use_id": "t1", "content": "file data"},
            {"type": "text", "text": "End"}
        ]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 3);
        assert!(matches!(&result[0], RenderedLine::Text(s) if s == "Start"));
        assert!(matches!(&result[1], RenderedLine::ToolUse(_)));
        assert!(matches!(&result[2], RenderedLine::Text(s) if s == "End"));
    }

    #[test]
    fn test_multiple_text_blocks_with_newlines() {
        let content = json!([
            {"type": "text", "text": "a\nb"},
            {"type": "text", "text": "c\nd"}
        ]);
        let result = render_content_blocks(&content);
        assert_eq!(
            result,
            vec![
                RenderedLine::Text("a".to_string()),
                RenderedLine::Text("b".to_string()),
                RenderedLine::Text("c".to_string()),
                RenderedLine::Text("d".to_string()),
            ]
        );
    }

    // -----------------------------------------------------------------------
    // 6. Non-array content tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_content_is_string() {
        let content = json!("System prompt text");
        let result = render_content_blocks(&content);
        assert_eq!(
            result,
            vec![RenderedLine::Text("System prompt text".to_string())]
        );
    }

    #[test]
    fn test_content_is_string_multiline() {
        let content = json!("line1\nline2");
        let result = render_content_blocks(&content);
        assert_eq!(
            result,
            vec![
                RenderedLine::Text("line1".to_string()),
                RenderedLine::Text("line2".to_string()),
            ]
        );
    }

    #[test]
    fn test_content_is_null() {
        let content = Value::Null;
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_content_is_number() {
        let content = json!(42);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_content_is_boolean() {
        let content = json!(true);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_content_is_object() {
        let content = json!({"unexpected": "shape"});
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------------
    // 7. Size formatting tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_size_zero_bytes() {
        assert_eq!(format_size(0), "0B");
    }

    #[test]
    fn test_format_size_small_bytes() {
        assert_eq!(format_size(42), "42B");
    }

    #[test]
    fn test_format_size_just_under_1kb() {
        assert_eq!(format_size(1023), "1023B");
    }

    #[test]
    fn test_format_size_exactly_1kb() {
        assert_eq!(format_size(1024), "1.0KB");
    }

    #[test]
    fn test_format_size_kilobytes() {
        // 12.3 * 1024 = 12595.2 -> 12595
        assert_eq!(format_size(12595), "12.3KB");
    }

    #[test]
    fn test_format_size_just_under_1mb() {
        assert_eq!(format_size(1024 * 1024 - 1), "1024.0KB");
    }

    #[test]
    fn test_format_size_exactly_1mb() {
        assert_eq!(format_size(1024 * 1024), "1.0MB");
    }

    #[test]
    fn test_format_size_megabytes() {
        // 1.5 * 1024 * 1024 = 1572864
        assert_eq!(format_size(1572864), "1.5MB");
    }

    #[test]
    fn test_format_size_large_megabytes() {
        // 10 * 1024 * 1024 = 10485760
        assert_eq!(format_size(10485760), "10.0MB");
    }

    // -----------------------------------------------------------------------
    // 8. Edge case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_array() {
        let content = json!([]);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_non_object_array_elements_skipped() {
        let content = json!([42, "string", true, null, [1, 2]]);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_mixed_object_and_non_object_elements() {
        let content = json!([
            "not an object",
            {"type": "text", "text": "valid"},
            42
        ]);
        let result = render_content_blocks(&content);
        assert_eq!(result, vec![RenderedLine::Text("valid".to_string())]);
    }

    #[test]
    fn test_all_blocks_skipped() {
        let content = json!([
            {"type": "tool_result", "tool_use_id": "t1", "content": "data1"},
            {"type": "tool_result", "tool_use_id": "t2", "content": "data2"}
        ]);
        let result = render_content_blocks(&content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_unknown_block_size_is_reasonable() {
        let content = json!([{
            "type": "thinking",
            "thinking": "x"
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        // The serialised block should be small (< 1024 bytes), so expect "NB)" format.
        match &result[0] {
            RenderedLine::Unknown(s) => {
                assert!(s.contains("[thinking]"));
                assert!(s.contains("B)"));
            }
            other => panic!("expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_block_large_data() {
        // Create a block with a large data field to push size over 1KB.
        let big_data = "x".repeat(2000);
        let content = json!([{
            "type": "image",
            "data": big_data
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::Unknown(s) => {
                assert!(s.starts_with("[image]"));
                assert!(s.contains("KB"));
            }
            other => panic!("expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn test_content_empty_string() {
        let content = json!("");
        let result = render_content_blocks(&content);
        assert_eq!(result, vec![RenderedLine::Text("".to_string())]);
    }

    #[test]
    fn test_text_block_with_trailing_newline() {
        let content = json!([{"type": "text", "text": "hello\n"}]);
        let result = render_content_blocks(&content);
        assert_eq!(
            result,
            vec![
                RenderedLine::Text("hello".to_string()),
                RenderedLine::Text("".to_string()),
            ]
        );
    }

    #[test]
    fn test_tool_use_grep_with_pattern() {
        let content = json!([{
            "type": "tool_use",
            "id": "t1",
            "name": "Grep",
            "input": {"pattern": "TODO", "path": "src/"}
        }]);
        let result = render_content_blocks(&content);
        assert_eq!(result.len(), 1);
        match &result[0] {
            RenderedLine::ToolUse(s) => {
                assert!(s.contains("Grep"));
                assert!(s.contains("TODO"));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }
    }
}
