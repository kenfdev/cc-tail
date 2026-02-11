
## 2026-02-08 - Task #3, Step: review

**Trigger:** REJECTED

**Lesson:** The security reviewer rejected the implementation due to two medium-severity issues: (1) No sanitization of ANSI escape sequences and control characters in tool names and input values — crafted inputs could inject terminal escape sequences that manipulate display or execute commands on vulnerable terminals. (2) Sensitive data exposure — file paths, bash commands, URLs, and search queries are output verbatim without redaction, potentially leaking credentials, API keys, and tokens in logs/terminal output. The implementation should sanitize control characters from all user-controlled strings and consider optional redaction of common secret patterns.
## 2026-02-08 - Task #7, Step: review

**Trigger:** REJECTED

**Lesson:** Security review identified unbounded memory allocation in `read_to_string` (no cap on bytes read per call) and unbounded `incomplete_line_buf` growth. When reading from files incrementally, always cap the maximum bytes read per call and limit buffer sizes to prevent OOM from pathological inputs.

## 2026-02-09 - Task #132, Step: review

**Trigger:** REJECTED

**Lesson:** `build_stream_command()` in `src/tmux.rs` constructs a shell command string via `format!` without quoting the binary path or log file path. Since `tmux split-window` passes this string to `sh -c`, file paths with spaces or shell metacharacters can cause word-splitting or arbitrary command execution. Shell-quote all interpolated values in command strings that will be interpreted by a shell.

## 2026-02-10 - Task #152, Step: review

**Trigger:** REJECTED

**Lesson:** Unicode case-folding via `to_lowercase()` can change the byte length of strings. When using byte positions from a lowercased copy to index into the original text, this creates a desynchronization that can cause panics at non-UTF-8 byte boundaries. When implementing case-insensitive substring matching that returns byte positions, ensure positions map back to the original text, not the lowered copy. This is a general pattern: any text transformation that changes string length invalidates byte-offset math.

## 2026-02-10 - Task #179, Step: review

**Trigger:** REJECTED

**Lesson:** The `find_matches()` function performs case-insensitive search by lowercasing both text and query, then returns byte offsets from the lowercased text. These offsets are later used to slice the original (non-lowercased) text in `highlight_line()`. When multi-byte Unicode characters change byte length during case conversion (e.g., German ß→ss, Turkish İ→i̇), the byte offsets become misaligned, causing potential panics. Byte offsets from string transformations must always be validated against or computed from the target string they will be applied to.

## 2026-02-11 - Task #cc-tail-9qn, Step: review

**Trigger:** REJECTED

**Lesson:** Code quality review rejected because changing `FileWatchState::new()` from `pub(crate)` to `pub` triggered the `clippy::new_without_default` lint. When widening visibility of structs with `new()` methods, always check if a `Default` impl is needed to satisfy clippy lints.
