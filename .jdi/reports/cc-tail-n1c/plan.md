## Implementation Plan: Consolidate Test Groups with rstest Parameterization

### Overview

Consolidate repetitive test groups into parameterized `rstest` tests across 2 files (`src/tool_summary.rs` and `src/content_render.rs`). The `rstest = "0.23"` crate is already in `[dev-dependencies]` in `Cargo.toml`. Current baseline: 708 passing tests.

---

### File 1: `src/tool_summary.rs`

Both the test module imports (`use super::*;` and `use serde_json::json;`) will need `use rstest::rstest;` added.

#### Group A: Wrong-type fields fallback (-2 net tests)

**Existing tests (3 separate `#[test]` fns):**
1. `test_read_file_path_is_number`: `("Read", json!({"file_path": 42}), "[Read]")`
2. `test_bash_command_is_array`: `("Bash", json!({"command": ["ls", "-la"]}), "[Bash]")`
3. `test_grep_pattern_is_bool`: `("Grep", json!({"pattern": true, "path": "src/"}), "[Grep]")`

**Proposed rstest:**
```rust
#[rstest]
#[case("Read", json!({"file_path": 42}), "[Read]")]
#[case("Bash", json!({"command": ["ls", "-la"]}), "[Bash]")]
#[case("Grep", json!({"pattern": true, "path": "src/"}), "[Grep]")]
fn test_wrong_type_fields_fallback(
    #[case] tool: &str,
    #[case] input: Value,
    #[case] expected: &str,
) {
    assert_eq!(summarize_tool_use(tool, &input), expected);
}
```

#### Group B: truncate_chars unit tests (-5 net tests)

**Existing tests (6 separate `#[test]` fns):**
1. `test_truncate_chars_empty`: `("", 10, "")`
2. `test_truncate_chars_under`: `("hello", 10, "hello")`
3. `test_truncate_chars_exact`: `("hello", 5, "hello")`
4. `test_truncate_chars_over`: `("hello world", 5, "hello…")`
5. `test_truncate_chars_zero_max`: `("hello", 0, "…")`
6. `test_truncate_chars_multibyte`: `("あいうえお", 3, "あいう…")`

**Proposed rstest:**
```rust
#[rstest]
#[case("", 10, "")]
#[case("hello", 10, "hello")]
#[case("hello", 5, "hello")]
#[case("hello world", 5, "hello…")]
#[case("hello", 0, "…")]
#[case("あいうえお", 3, "あいう…")]
fn test_truncate_chars(
    #[case] input: &str,
    #[case] max: usize,
    #[case] expected: &str,
) {
    assert_eq!(truncate_chars(input, max), expected);
}
```

#### Group C: Env-var redaction tests (-5 net tests)

**Existing tests (6 separate `#[test]` fns):**
1. `test_redact_api_key_env_var`: `("export API_KEY=supersecretvalue123", "export API_KEY=[REDACTED]")`
2. `test_redact_password_env_var`: `("PASSWORD=hunter2", "PASSWORD=[REDACTED]")`
3. `test_redact_secret_env_var`: `("SECRET=topsecretvalue", "SECRET=[REDACTED]")`
4. `test_redact_secret_key_env_var`: `("SECRET_KEY=mykey123", "SECRET_KEY=[REDACTED]")`
5. `test_redact_db_password_env_var`: `("DB_PASSWORD=p@ssw0rd!", "DB_PASSWORD=[REDACTED]")`
6. `test_redact_aws_secret`: `("AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "AWS_SECRET_ACCESS_KEY=[REDACTED]")`

**Proposed rstest:**
```rust
#[rstest]
#[case("export API_KEY=supersecretvalue123", "export API_KEY=[REDACTED]")]
#[case("PASSWORD=hunter2", "PASSWORD=[REDACTED]")]
#[case("SECRET=topsecretvalue", "SECRET=[REDACTED]")]
#[case("SECRET_KEY=mykey123", "SECRET_KEY=[REDACTED]")]
#[case("DB_PASSWORD=p@ssw0rd!", "DB_PASSWORD=[REDACTED]")]
#[case("AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "AWS_SECRET_ACCESS_KEY=[REDACTED]")]
fn test_redact_env_var_secrets(
    #[case] input: &str,
    #[case] expected: &str,
) {
    assert_eq!(redact_secrets(input), expected);
}
```

**tool_summary.rs subtotal: 15 tests become 3 functions. Net: -12.**

---

### File 2: `src/content_render.rs`

The test module imports will need `use rstest::rstest;` added.

#### Group D: format_size tests (-8 net tests)

**Existing tests (9 separate `#[test]` fns):**
1. `test_format_size_zero_bytes`: `(0, "0B")`
2. `test_format_size_small_bytes`: `(42, "42B")`
3. `test_format_size_just_under_1kb`: `(1023, "1023B")`
4. `test_format_size_exactly_1kb`: `(1024, "1.0KB")`
5. `test_format_size_kilobytes`: `(12595, "12.3KB")`
6. `test_format_size_just_under_1mb`: `(1024 * 1024 - 1, "1024.0KB")`
7. `test_format_size_exactly_1mb`: `(1024 * 1024, "1.0MB")`
8. `test_format_size_megabytes`: `(1572864, "1.5MB")`
9. `test_format_size_large_megabytes`: `(10485760, "10.0MB")`

**Proposed rstest:**
```rust
#[rstest]
#[case(0, "0B")]
#[case(42, "42B")]
#[case(1023, "1023B")]
#[case(1024, "1.0KB")]
#[case(12595, "12.3KB")]
#[case(1024 * 1024 - 1, "1024.0KB")]
#[case(1024 * 1024, "1.0MB")]
#[case(1572864, "1.5MB")]
#[case(10485760, "10.0MB")]
fn test_format_size(
    #[case] bytes: usize,
    #[case] expected: &str,
) {
    assert_eq!(format_size(bytes), expected);
}
```

#### Group E: has_renderable_content tests (-10 net tests)

**True cases (6 tests):**
1. `test_has_renderable_content_string`: `json!("hello")`
2. `test_has_renderable_content_empty_string`: `json!("")`
3. `test_has_renderable_content_text_block`: `json!([{"type": "text", "text": "hi"}])`
4. `test_has_renderable_content_tool_use_block`: `json!([{"type": "tool_use", "name": "Read", "input": {}}])`
5. `test_has_renderable_content_tool_result_with_text`: `json!([{"type": "tool_result", "tool_use_id": "t1", "content": "data"}, {"type": "text", "text": "visible"}])`
6. `test_has_renderable_content_unknown_type`: `json!([{"type": "thinking", "thinking": "hmm"}])`

**False cases (6 tests):**
1. `test_has_renderable_content_tool_result_only`: `json!([{"type": "tool_result", "tool_use_id": "t1", "content": "data"}])`
2. `test_has_renderable_content_multiple_tool_results`: `json!([{"type": "tool_result", "tool_use_id": "t1", "content": "data1"}, {"type": "tool_result", "tool_use_id": "t2", "content": "data2"}])`
3. `test_has_renderable_content_empty_array`: `json!([])`
4. `test_has_renderable_content_null`: `json!(null)`
5. `test_has_renderable_content_number`: `json!(42)`
6. `test_has_renderable_content_non_object_elements`: `json!([42, "string", true])`

**Proposed rstest (2 functions):**
```rust
#[rstest]
#[case(json!("hello"))]
#[case(json!(""))]
#[case(json!([{"type": "text", "text": "hi"}]))]
#[case(json!([{"type": "tool_use", "name": "Read", "input": {}}]))]
#[case(json!([{"type": "tool_result", "tool_use_id": "t1", "content": "data"}, {"type": "text", "text": "visible"}]))]
#[case(json!([{"type": "thinking", "thinking": "hmm"}]))]
fn test_has_renderable_content_true(#[case] content: Value) {
    assert!(has_renderable_content(&content));
}

#[rstest]
#[case(json!([{"type": "tool_result", "tool_use_id": "t1", "content": "data"}]))]
#[case(json!([{"type": "tool_result", "tool_use_id": "t1", "content": "data1"}, {"type": "tool_result", "tool_use_id": "t2", "content": "data2"}]))]
#[case(json!([]))]
#[case(json!(null))]
#[case(json!(42))]
#[case(json!([42, "string", true]))]
fn test_has_renderable_content_false(#[case] content: Value) {
    assert!(!has_renderable_content(&content));
}
```

**content_render.rs subtotal: 21 tests become 3 functions. Net: -18.**

---

### Grand Total

- tool_summary.rs: 15 tests -> 3 functions = **-12 net**
- content_render.rs: 21 tests -> 3 functions = **-18 net**
- **Total: -30 net functions** (rstest generates individual test cases at runtime so actual `cargo test` count remains at 708)

### Risks and Edge Cases

1. **rstest `#[case]` with `json!()` macro**: Works because `#[case]` accepts any expression.
2. **Expressions in `#[case]`**: `1024 * 1024 - 1` is valid Rust and rstest evaluates as expression.
3. **`Value::Null` vs `json!(null)`**: Produces the same value, works in `#[case]`.
4. **Multibyte strings**: UTF-8 strings and ellipsis character work fine in `#[case]`.

### Implementation Order

1. Edit `src/tool_summary.rs` → run `cargo test`
2. Edit `src/content_render.rs` → run `cargo test`
3. Update `docs/feature-progress.md`

<!-- DECISION: PLANNED -->
