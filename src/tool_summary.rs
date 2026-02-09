use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of characters for Bash command summaries before truncation.
const BASH_CMD_MAX_CHARS: usize = 80;

// ---------------------------------------------------------------------------
// Compiled regex patterns (compiled once, reused across calls)
// ---------------------------------------------------------------------------

/// Matches ANSI escape sequences: ESC followed by `[` then any number of
/// parameter/intermediate bytes and a final byte, or other common OSC/CSI forms.
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Covers CSI sequences (ESC[...X), OSC sequences (ESC]...BEL/ST), and
    // simple two-character escape sequences (ESC followed by a letter).
    Regex::new(r"\x1b\[[0-9;]*[A-Za-z]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[A-Za-z]").unwrap()
});

/// Matches common secret patterns and redacts the sensitive portion.
/// Each pattern captures a prefix group and a secret-value group.
static SECRET_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Bearer tokens: "Bearer <token>"
        Regex::new(r"(?i)(Bearer\s+)\S+").unwrap(),
        // OpenAI-style API keys: sk-... (at least 8 chars after prefix)
        Regex::new(r"(sk-)[A-Za-z0-9_\-]{8,}").unwrap(),
        // GitHub personal access tokens: ghp_... (at least 8 chars)
        Regex::new(r"(ghp_)[A-Za-z0-9_\-]{8,}").unwrap(),
        // GitHub OAuth tokens: gho_...
        Regex::new(r"(gho_)[A-Za-z0-9_\-]{8,}").unwrap(),
        // GitHub user-to-server tokens: ghu_...
        Regex::new(r"(ghu_)[A-Za-z0-9_\-]{8,}").unwrap(),
        // GitHub server-to-server tokens: ghs_...
        Regex::new(r"(ghs_)[A-Za-z0-9_\-]{8,}").unwrap(),
        // GitHub refresh tokens: ghr_...
        Regex::new(r"(ghr_)[A-Za-z0-9_\-]{8,}").unwrap(),
        // token= in URLs/query strings
        Regex::new(r"(?i)(token=)[^\s&]+").unwrap(),
        // Environment variable assignments with sensitive names
        Regex::new(r"(?i)((?:API_KEY|SECRET|PASSWORD|ACCESS_TOKEN|AUTH_TOKEN|SECRET_KEY|PRIVATE_KEY|DB_PASSWORD|DATABASE_URL|AWS_SECRET_ACCESS_KEY)=)\S+").unwrap(),
    ]
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Produce a one-line, input-only summary string for a `tool_use` content block.
///
/// The function inspects the tool `name` and extracts the most relevant field(s)
/// from `input` (a `serde_json::Value` expected to be an object).
///
/// # Security
/// * Sanitizes ANSI escape sequences and control characters from output.
/// * Redacts common secret patterns (API keys, tokens, passwords).
///
/// # Guarantees
/// * Never panics.
/// * Always returns a non-empty `String`.
/// * Unicode-safe truncation (never splits a multi-byte character).
pub fn summarize_tool_use(name: &str, input: &Value) -> String {
    let sanitized_name = sanitize_control_chars(name);
    let raw = match sanitized_name.as_str() {
        "Read" => summarize_single_key(&sanitized_name, input, "file_path"),
        "Bash" => summarize_bash(input),
        "Edit" => summarize_single_key(&sanitized_name, input, "file_path"),
        "Write" => summarize_single_key(&sanitized_name, input, "file_path"),
        "Glob" => summarize_single_key(&sanitized_name, input, "pattern"),
        "Grep" => summarize_grep(input),
        "Task" => summarize_single_key(&sanitized_name, input, "description"),
        "WebSearch" => summarize_single_key(&sanitized_name, input, "query"),
        "WebFetch" => summarize_single_key(&sanitized_name, input, "url"),
        "Skill" => summarize_single_key(&sanitized_name, input, "skill"),
        _ => format!("[{}]", sanitized_name),
    };
    redact_secrets(&raw)
}

// ---------------------------------------------------------------------------
// Security helpers
// ---------------------------------------------------------------------------

/// Strip ANSI escape sequences and control characters from a string.
///
/// Removes:
/// * ANSI escape sequences (CSI, OSC, and simple ESC sequences)
/// * Control characters in 0x00-0x1F (except `\n` 0x0A and `\t` 0x09)
/// * The DEL character (0x7F)
fn sanitize_control_chars(s: &str) -> String {
    // First strip ANSI escape sequences
    let without_ansi = ANSI_RE.replace_all(s, "");
    // Then strip remaining control characters (except \n and \t)
    without_ansi
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !(c.is_control() || c == '\x7f'))
        .collect()
}

/// Detect and redact common secret patterns in a string.
///
/// Replaces the sensitive value portion with `[REDACTED]`, preserving the
/// prefix so the user can see *what kind* of secret was present.
fn redact_secrets(s: &str) -> String {
    let mut result = s.to_string();
    for pattern in SECRET_PATTERNS.iter() {
        result = pattern.replace_all(&result, "${1}[REDACTED]").to_string();
    }
    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a single string key from `input` and format as `[ToolName] <value>`.
/// Falls back to `[ToolName]` if the key is missing, not a string, or empty.
fn summarize_single_key(tool: &str, input: &Value, key: &str) -> String {
    match input.get(key).and_then(Value::as_str) {
        Some(v) if !v.is_empty() => {
            let sanitized = sanitize_control_chars(v);
            if sanitized.is_empty() {
                format!("[{}]", tool)
            } else {
                format!("[{}] {}", tool, sanitized)
            }
        }
        _ => format!("[{}]", tool),
    }
}

/// Summarize a Bash tool call: `[Bash] <command>`, truncated to
/// [`BASH_CMD_MAX_CHARS`] characters with an ellipsis if longer.
fn summarize_bash(input: &Value) -> String {
    match input.get("command").and_then(Value::as_str) {
        Some(cmd) if !cmd.is_empty() => {
            let sanitized = sanitize_control_chars(cmd);
            if sanitized.is_empty() {
                "[Bash]".to_string()
            } else {
                let truncated = truncate_chars(&sanitized, BASH_CMD_MAX_CHARS);
                format!("[Bash] {}", truncated)
            }
        }
        _ => "[Bash]".to_string(),
    }
}

/// Summarize a Grep tool call: `[Grep] "<pattern>" in <path>` when a path
/// is present, or `[Grep] "<pattern>"` when only a pattern is given.
fn summarize_grep(input: &Value) -> String {
    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .map(sanitize_control_chars);
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .map(sanitize_control_chars);

    match (pattern.as_deref(), path.as_deref()) {
        (Some(p), Some(d)) if !p.is_empty() && !d.is_empty() => {
            format!("[Grep] \"{}\" in {}", p, d)
        }
        (Some(p), _) if !p.is_empty() => {
            format!("[Grep] \"{}\"", p)
        }
        _ => "[Grep]".to_string(),
    }
}

/// Truncate a string to at most `max` characters, appending `…` if truncated.
///
/// Uses `.chars()` iteration so that we never split a multi-byte codepoint.
fn truncate_chars(s: &str, max: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{}…", truncated)
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
    // Per-tool happy paths
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_happy_path() {
        let input = json!({"file_path": "src/main.rs"});
        assert_eq!(summarize_tool_use("Read", &input), "[Read] src/main.rs");
    }

    #[test]
    fn test_bash_happy_path() {
        let input = json!({"command": "cargo test auth"});
        assert_eq!(summarize_tool_use("Bash", &input), "[Bash] cargo test auth");
    }

    #[test]
    fn test_edit_happy_path() {
        let input = json!({"file_path": "src/lib.rs", "old_string": "foo", "new_string": "bar"});
        assert_eq!(summarize_tool_use("Edit", &input), "[Edit] src/lib.rs");
    }

    #[test]
    fn test_write_happy_path() {
        let input = json!({"file_path": "tests/new_test.rs", "content": "fn test() {}"});
        assert_eq!(
            summarize_tool_use("Write", &input),
            "[Write] tests/new_test.rs"
        );
    }

    #[test]
    fn test_glob_happy_path() {
        let input = json!({"pattern": "**/*.rs"});
        assert_eq!(summarize_tool_use("Glob", &input), "[Glob] **/*.rs");
    }

    #[test]
    fn test_grep_happy_path_with_path() {
        let input = json!({"pattern": "TODO", "path": "src/"});
        assert_eq!(
            summarize_tool_use("Grep", &input),
            "[Grep] \"TODO\" in src/"
        );
    }

    #[test]
    fn test_grep_happy_path_without_path() {
        let input = json!({"pattern": "TODO"});
        assert_eq!(summarize_tool_use("Grep", &input), "[Grep] \"TODO\"");
    }

    #[test]
    fn test_task_happy_path() {
        let input = json!({"description": "Explore: \"investigate log format\""});
        assert_eq!(
            summarize_tool_use("Task", &input),
            "[Task] Explore: \"investigate log format\""
        );
    }

    #[test]
    fn test_websearch_happy_path() {
        let input = json!({"query": "rust tui library"});
        assert_eq!(
            summarize_tool_use("WebSearch", &input),
            "[WebSearch] rust tui library"
        );
    }

    #[test]
    fn test_webfetch_happy_path() {
        let input = json!({"url": "https://example.com"});
        assert_eq!(
            summarize_tool_use("WebFetch", &input),
            "[WebFetch] https://example.com"
        );
    }

    #[test]
    fn test_skill_happy_path() {
        let input = json!({"skill": "agent-browser"});
        assert_eq!(summarize_tool_use("Skill", &input), "[Skill] agent-browser");
    }

    // -----------------------------------------------------------------------
    // Unknown tool names
    // -----------------------------------------------------------------------

    #[test]
    fn test_unknown_tool() {
        let input = json!({});
        assert_eq!(summarize_tool_use("KillShell", &input), "[KillShell]");
    }

    #[test]
    fn test_unknown_tool_with_input() {
        let input = json!({"some_key": "some_value"});
        assert_eq!(
            summarize_tool_use("SomeFutureTool", &input),
            "[SomeFutureTool]"
        );
    }

    // -----------------------------------------------------------------------
    // Fallback tests: missing fields
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_missing_file_path() {
        let input = json!({});
        assert_eq!(summarize_tool_use("Read", &input), "[Read]");
    }

    #[test]
    fn test_bash_missing_command() {
        let input = json!({});
        assert_eq!(summarize_tool_use("Bash", &input), "[Bash]");
    }

    #[test]
    fn test_edit_missing_file_path() {
        let input = json!({"old_string": "foo", "new_string": "bar"});
        assert_eq!(summarize_tool_use("Edit", &input), "[Edit]");
    }

    #[test]
    fn test_write_missing_file_path() {
        let input = json!({"content": "hello"});
        assert_eq!(summarize_tool_use("Write", &input), "[Write]");
    }

    #[test]
    fn test_glob_missing_pattern() {
        let input = json!({});
        assert_eq!(summarize_tool_use("Glob", &input), "[Glob]");
    }

    #[test]
    fn test_grep_missing_pattern() {
        let input = json!({"path": "src/"});
        assert_eq!(summarize_tool_use("Grep", &input), "[Grep]");
    }

    #[test]
    fn test_grep_empty_pattern() {
        let input = json!({"pattern": "", "path": "src/"});
        assert_eq!(summarize_tool_use("Grep", &input), "[Grep]");
    }

    #[test]
    fn test_grep_empty_path() {
        let input = json!({"pattern": "TODO", "path": ""});
        assert_eq!(summarize_tool_use("Grep", &input), "[Grep] \"TODO\"");
    }

    #[test]
    fn test_task_missing_description() {
        let input = json!({});
        assert_eq!(summarize_tool_use("Task", &input), "[Task]");
    }

    #[test]
    fn test_websearch_missing_query() {
        let input = json!({});
        assert_eq!(summarize_tool_use("WebSearch", &input), "[WebSearch]");
    }

    #[test]
    fn test_webfetch_missing_url() {
        let input = json!({});
        assert_eq!(summarize_tool_use("WebFetch", &input), "[WebFetch]");
    }

    #[test]
    fn test_skill_missing_skill() {
        let input = json!({});
        assert_eq!(summarize_tool_use("Skill", &input), "[Skill]");
    }

    // -----------------------------------------------------------------------
    // Fallback tests: empty strings
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_empty_file_path() {
        let input = json!({"file_path": ""});
        assert_eq!(summarize_tool_use("Read", &input), "[Read]");
    }

    #[test]
    fn test_bash_empty_command() {
        let input = json!({"command": ""});
        assert_eq!(summarize_tool_use("Bash", &input), "[Bash]");
    }

    // -----------------------------------------------------------------------
    // Fallback tests: wrong types
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_file_path_is_number() {
        let input = json!({"file_path": 42});
        assert_eq!(summarize_tool_use("Read", &input), "[Read]");
    }

    #[test]
    fn test_bash_command_is_array() {
        let input = json!({"command": ["ls", "-la"]});
        assert_eq!(summarize_tool_use("Bash", &input), "[Bash]");
    }

    #[test]
    fn test_grep_pattern_is_bool() {
        let input = json!({"pattern": true, "path": "src/"});
        assert_eq!(summarize_tool_use("Grep", &input), "[Grep]");
    }

    // -----------------------------------------------------------------------
    // Null / empty input tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_null_input() {
        let input = Value::Null;
        assert_eq!(summarize_tool_use("Read", &input), "[Read]");
        assert_eq!(summarize_tool_use("Bash", &input), "[Bash]");
        assert_eq!(summarize_tool_use("Grep", &input), "[Grep]");
        assert_eq!(summarize_tool_use("Unknown", &input), "[Unknown]");
    }

    #[test]
    fn test_empty_object_input() {
        let input = json!({});
        assert_eq!(summarize_tool_use("Read", &input), "[Read]");
        assert_eq!(summarize_tool_use("Bash", &input), "[Bash]");
        assert_eq!(summarize_tool_use("Grep", &input), "[Grep]");
    }

    #[test]
    fn test_input_is_string_not_object() {
        let input = json!("just a string");
        assert_eq!(summarize_tool_use("Read", &input), "[Read]");
        assert_eq!(summarize_tool_use("Bash", &input), "[Bash]");
    }

    // -----------------------------------------------------------------------
    // Bash truncation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_bash_under_limit() {
        // 79 chars -- under the 80-char limit
        let cmd = "a".repeat(79);
        let input = json!({"command": cmd});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, format!("[Bash] {}", cmd));
        assert!(!result.contains('…'));
    }

    #[test]
    fn test_bash_at_limit() {
        // Exactly 80 chars -- should NOT be truncated
        let cmd = "a".repeat(80);
        let input = json!({"command": cmd});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, format!("[Bash] {}", cmd));
        assert!(!result.contains('…'));
    }

    #[test]
    fn test_bash_over_limit() {
        // 81 chars -- should be truncated to 80 + ellipsis
        let cmd = "a".repeat(81);
        let input = json!({"command": cmd});
        let result = summarize_tool_use("Bash", &input);
        let expected = format!("[Bash] {}…", "a".repeat(80));
        assert_eq!(result, expected);
    }

    #[test]
    fn test_bash_truncation_unicode_safe() {
        // Build a string of 79 ASCII chars + a multi-byte emoji (4 bytes).
        // Total chars = 80, which is at the limit, so no truncation.
        let cmd = format!("{}{}", "x".repeat(79), "\u{1F600}"); // 😀
        assert_eq!(cmd.chars().count(), 80);
        let input = json!({"command": cmd});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, format!("[Bash] {}", cmd));
        assert!(!result.contains('…'));
    }

    #[test]
    fn test_bash_truncation_unicode_over_limit() {
        // 80 ASCII + 1 emoji = 81 chars, should truncate to 80 chars + ellipsis.
        // The emoji should be dropped cleanly (not split).
        let cmd = format!("{}{}", "x".repeat(80), "\u{1F600}");
        assert_eq!(cmd.chars().count(), 81);
        let input = json!({"command": cmd});
        let result = summarize_tool_use("Bash", &input);
        let expected = format!("[Bash] {}…", "x".repeat(80));
        assert_eq!(result, expected);
    }

    // -----------------------------------------------------------------------
    // truncate_chars unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_truncate_chars_empty() {
        assert_eq!(truncate_chars("", 10), "");
    }

    #[test]
    fn test_truncate_chars_under() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_chars_exact() {
        assert_eq!(truncate_chars("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_chars_over() {
        assert_eq!(truncate_chars("hello world", 5), "hello…");
    }

    #[test]
    fn test_truncate_chars_zero_max() {
        assert_eq!(truncate_chars("hello", 0), "…");
    }

    #[test]
    fn test_truncate_chars_multibyte() {
        // Each Japanese character is 1 char (3 bytes in UTF-8)
        let s = "あいうえお"; // 5 chars
        assert_eq!(truncate_chars(s, 3), "あいう…");
    }

    // -----------------------------------------------------------------------
    // Security: sanitize_control_chars unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sanitize_plain_text_unchanged() {
        assert_eq!(sanitize_control_chars("hello world"), "hello world");
    }

    #[test]
    fn test_sanitize_strips_csi_color_codes() {
        // ESC[31m = red, ESC[0m = reset
        assert_eq!(sanitize_control_chars("\x1b[31mERROR\x1b[0m"), "ERROR");
    }

    #[test]
    fn test_sanitize_strips_csi_cursor_movement() {
        // ESC[2J = clear screen, ESC[H = cursor home
        assert_eq!(sanitize_control_chars("\x1b[2Jhello\x1b[H"), "hello");
    }

    #[test]
    fn test_sanitize_strips_osc_title_sequence() {
        // OSC sequence to set terminal title: ESC ] 0 ; title BEL
        assert_eq!(
            sanitize_control_chars("\x1b]0;malicious title\x07safe text"),
            "safe text"
        );
    }

    #[test]
    fn test_sanitize_strips_null_bytes() {
        assert_eq!(sanitize_control_chars("hello\x00world"), "helloworld");
    }

    #[test]
    fn test_sanitize_strips_bell_character() {
        assert_eq!(sanitize_control_chars("ding\x07dong"), "dingdong");
    }

    #[test]
    fn test_sanitize_strips_backspace() {
        assert_eq!(sanitize_control_chars("abc\x08\x08\x08secret"), "abcsecret");
    }

    #[test]
    fn test_sanitize_strips_carriage_return() {
        // CR could be used to overwrite visible text in terminal
        assert_eq!(sanitize_control_chars("visible\rsecret"), "visiblesecret");
    }

    #[test]
    fn test_sanitize_preserves_newline() {
        assert_eq!(sanitize_control_chars("line1\nline2"), "line1\nline2");
    }

    #[test]
    fn test_sanitize_preserves_tab() {
        assert_eq!(sanitize_control_chars("col1\tcol2"), "col1\tcol2");
    }

    #[test]
    fn test_sanitize_strips_del_character() {
        assert_eq!(sanitize_control_chars("hello\x7fworld"), "helloworld");
    }

    #[test]
    fn test_sanitize_mixed_control_and_ansi() {
        // Mix of ANSI color, null byte, bell, and normal text
        assert_eq!(
            sanitize_control_chars("\x1b[31m\x00alert\x07\x1b[0m ok"),
            "alert ok"
        );
    }

    #[test]
    fn test_sanitize_only_control_chars_returns_empty() {
        assert_eq!(sanitize_control_chars("\x00\x01\x02\x03"), "");
    }

    // -----------------------------------------------------------------------
    // Security: redact_secrets unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_redact_openai_api_key() {
        assert_eq!(
            redact_secrets("key is sk-abc123defghijklmnop"),
            "key is sk-[REDACTED]"
        );
    }

    #[test]
    fn test_redact_github_pat() {
        assert_eq!(
            redact_secrets("using ghp_1234567890abcdef"),
            "using ghp_[REDACTED]"
        );
    }

    #[test]
    fn test_redact_github_oauth_token() {
        assert_eq!(
            redact_secrets("token gho_abcdefghijklmnop"),
            "token gho_[REDACTED]"
        );
    }

    #[test]
    fn test_redact_bearer_token() {
        assert_eq!(
            redact_secrets("Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.payload.sig"),
            "Authorization: Bearer [REDACTED]"
        );
    }

    #[test]
    fn test_redact_bearer_case_insensitive() {
        assert_eq!(
            redact_secrets("header: bearer my-secret-token"),
            "header: bearer [REDACTED]"
        );
    }

    #[test]
    fn test_redact_token_in_url() {
        assert_eq!(
            redact_secrets("https://api.example.com/data?token=abc123secret&page=1"),
            "https://api.example.com/data?token=[REDACTED]&page=1"
        );
    }

    #[test]
    fn test_redact_api_key_env_var() {
        assert_eq!(
            redact_secrets("export API_KEY=supersecretvalue123"),
            "export API_KEY=[REDACTED]"
        );
    }

    #[test]
    fn test_redact_password_env_var() {
        assert_eq!(redact_secrets("PASSWORD=hunter2"), "PASSWORD=[REDACTED]");
    }

    #[test]
    fn test_redact_secret_env_var() {
        assert_eq!(redact_secrets("SECRET=topsecretvalue"), "SECRET=[REDACTED]");
    }

    #[test]
    fn test_redact_secret_key_env_var() {
        assert_eq!(
            redact_secrets("SECRET_KEY=mykey123"),
            "SECRET_KEY=[REDACTED]"
        );
    }

    #[test]
    fn test_redact_db_password_env_var() {
        assert_eq!(
            redact_secrets("DB_PASSWORD=p@ssw0rd!"),
            "DB_PASSWORD=[REDACTED]"
        );
    }

    #[test]
    fn test_redact_aws_secret() {
        assert_eq!(
            redact_secrets("AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"),
            "AWS_SECRET_ACCESS_KEY=[REDACTED]"
        );
    }

    #[test]
    fn test_redact_preserves_non_secret_text() {
        let text = "cargo test --release -- --nocapture";
        assert_eq!(redact_secrets(text), text);
    }

    #[test]
    fn test_redact_short_sk_prefix_not_matched() {
        // "sk-" followed by fewer than 8 chars should NOT be redacted
        assert_eq!(redact_secrets("sk-short"), "sk-short");
    }

    #[test]
    fn test_redact_multiple_secrets_in_one_string() {
        assert_eq!(
            redact_secrets("API_KEY=secret123 PASSWORD=hunter2"),
            "API_KEY=[REDACTED] PASSWORD=[REDACTED]"
        );
    }

    // -----------------------------------------------------------------------
    // Security: ANSI escape sequences in tool names
    // -----------------------------------------------------------------------

    #[test]
    fn test_ansi_in_tool_name_unknown_tool() {
        let input = json!({});
        let result = summarize_tool_use("\x1b[31mEvil\x1b[0m", &input);
        assert_eq!(result, "[Evil]");
        assert!(!result.contains('\x1b'));
    }

    #[test]
    fn test_ansi_in_tool_name_strips_sequences() {
        // Even if someone injects ANSI into a tool name that doesn't match
        // known tools, the output should be clean.
        let input = json!({});
        let result = summarize_tool_use("\x1b[2J\x1b[HMalicious", &input);
        assert_eq!(result, "[Malicious]");
    }

    // -----------------------------------------------------------------------
    // Security: ANSI escape sequences in input values
    // -----------------------------------------------------------------------

    #[test]
    fn test_ansi_in_file_path_value() {
        let input = json!({"file_path": "\x1b[31m/etc/passwd\x1b[0m"});
        let result = summarize_tool_use("Read", &input);
        assert_eq!(result, "[Read] /etc/passwd");
    }

    #[test]
    fn test_ansi_in_bash_command_value() {
        let input = json!({"command": "\x1b[31mrm -rf /\x1b[0m"});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, "[Bash] rm -rf /");
    }

    #[test]
    fn test_ansi_in_grep_pattern_and_path() {
        let input = json!({
            "pattern": "\x1b[1mTODO\x1b[0m",
            "path": "\x1b[32msrc/\x1b[0m"
        });
        let result = summarize_tool_use("Grep", &input);
        assert_eq!(result, "[Grep] \"TODO\" in src/");
    }

    #[test]
    fn test_ansi_in_url_value() {
        let input = json!({"url": "\x1b[4mhttps://evil.com\x1b[0m"});
        let result = summarize_tool_use("WebFetch", &input);
        assert_eq!(result, "[WebFetch] https://evil.com");
    }

    // -----------------------------------------------------------------------
    // Security: control characters in input values
    // -----------------------------------------------------------------------

    #[test]
    fn test_null_bytes_in_file_path() {
        let input = json!({"file_path": "/etc/\x00passwd"});
        let result = summarize_tool_use("Read", &input);
        assert_eq!(result, "[Read] /etc/passwd");
    }

    #[test]
    fn test_carriage_return_injection_in_command() {
        // CR injection: display one thing, actually different
        let input = json!({"command": "safe-cmd\rmalicious-cmd"});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, "[Bash] safe-cmdmalicious-cmd");
    }

    #[test]
    fn test_backspace_overwrite_in_command() {
        let input = json!({"command": "good\x08\x08\x08\x08evil"});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, "[Bash] goodevil");
    }

    #[test]
    fn test_input_with_only_control_chars_falls_back() {
        // If after sanitization the value is empty, should fall back
        let input = json!({"file_path": "\x00\x01\x02"});
        let result = summarize_tool_use("Read", &input);
        assert_eq!(result, "[Read]");
    }

    #[test]
    fn test_bash_with_only_ansi_falls_back() {
        let input = json!({"command": "\x1b[31m\x1b[0m"});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, "[Bash]");
    }

    // -----------------------------------------------------------------------
    // Security: sensitive data in file paths, commands, and URLs
    // -----------------------------------------------------------------------

    #[test]
    fn test_sensitive_data_in_file_path() {
        let input = json!({"file_path": "/home/user/.config/API_KEY=sk-abc123defghijklmnop"});
        let result = summarize_tool_use("Read", &input);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("abc123defghijklmnop"));
    }

    #[test]
    fn test_sensitive_data_in_bash_command() {
        let input = json!({"command": "curl -H 'Authorization: Bearer eyJtoken123' https://api.example.com"});
        let result = summarize_tool_use("Bash", &input);
        assert!(result.contains("Bearer [REDACTED]"));
        assert!(!result.contains("eyJtoken123"));
    }

    #[test]
    fn test_sensitive_data_in_bash_env_export() {
        let input = json!({"command": "export API_KEY=mysecretkey123"});
        let result = summarize_tool_use("Bash", &input);
        assert!(result.contains("API_KEY=[REDACTED]"));
        assert!(!result.contains("mysecretkey123"));
    }

    #[test]
    fn test_sensitive_data_in_url() {
        let input = json!({"url": "https://api.example.com/v1?token=secret_token_value"});
        let result = summarize_tool_use("WebFetch", &input);
        assert!(result.contains("token=[REDACTED]"));
        assert!(!result.contains("secret_token_value"));
    }

    #[test]
    fn test_sensitive_data_in_grep_pattern() {
        let input = json!({"pattern": "PASSWORD=hunter2"});
        let result = summarize_tool_use("Grep", &input);
        assert!(result.contains("PASSWORD=[REDACTED]"));
        assert!(!result.contains("hunter2"));
    }

    // -----------------------------------------------------------------------
    // Security: combined sanitization + redaction
    // -----------------------------------------------------------------------

    #[test]
    fn test_combined_ansi_and_secret_in_command() {
        // ANSI escape wrapping a secret
        let input = json!({"command": "export \x1b[31mAPI_KEY=supersecret123\x1b[0m"});
        let result = summarize_tool_use("Bash", &input);
        // ANSI should be stripped AND secret should be redacted
        assert!(!result.contains('\x1b'));
        assert!(!result.contains("supersecret123"));
        assert!(result.contains("API_KEY=[REDACTED]"));
    }

    #[test]
    fn test_combined_control_chars_and_secret_in_path() {
        let input = json!({"file_path": "/home/\x00user/.env\rAPI_KEY=leaked_key_value"});
        let result = summarize_tool_use("Read", &input);
        assert!(!result.contains('\x00'));
        assert!(!result.contains('\r'));
        assert!(!result.contains("leaked_key_value"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_combined_ansi_bearer_in_url() {
        let input = json!({"url": "https://api.com/\x1b[1mdata?token=my_secret_token\x1b[0m"});
        let result = summarize_tool_use("WebFetch", &input);
        assert!(!result.contains('\x1b'));
        assert!(!result.contains("my_secret_token"));
        assert!(result.contains("token=[REDACTED]"));
    }

    #[test]
    fn test_no_false_positive_redaction_on_normal_commands() {
        // Ensure common non-secret commands are not redacted
        let input = json!({"command": "cargo test --release"});
        let result = summarize_tool_use("Bash", &input);
        assert_eq!(result, "[Bash] cargo test --release");
    }

    #[test]
    fn test_no_false_positive_redaction_on_normal_paths() {
        let input = json!({"file_path": "src/config/settings.rs"});
        let result = summarize_tool_use("Read", &input);
        assert_eq!(result, "[Read] src/config/settings.rs");
    }
}
