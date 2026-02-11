---
paths:
  - "src/**/*.rs"
---

# Security Rules

## Sanitize external data at shell and terminal boundaries

When interpolating external or user-controlled strings into shell commands or terminal output:

- **Shell commands:** Use proper quoting or argument arrays — never raw `format!` interpolation into `sh -c` strings. Paths with spaces or metacharacters cause word-splitting or arbitrary command execution.
- **Terminal output:** Strip or escape ANSI escape sequences and control characters from untrusted data before rendering. Crafted inputs can manipulate terminal display or execute commands on vulnerable terminals.

*Rationale:* Unsanitized strings crossing the boundary from external data into shell/terminal contexts enable injection attacks.

## Bound all I/O reads from external sources

When reading data from files, network streams, or any external source:

- Always enforce an upper bound on bytes read per call and on cumulative buffer size.
- Use `read` with a fixed buffer or `.take(limit)` to cap reads.
- Never use unbounded `read_to_string` on untrusted input.

*Rationale:* Without size limits, pathological or malicious input causes OOM.
