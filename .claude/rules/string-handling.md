---
paths:
  - "src/**/*.rs"
---

# String Handling Rules

## Never use byte offsets across string transformations

Byte offsets computed from a transformed string (e.g., after `to_lowercase()`, `to_uppercase()`, Unicode normalization) must never be applied to the original or any other string.

If you need case-insensitive matching with original-text byte positions, compute positions directly on the original string — for example, using `char`-level iteration with byte tracking, or a library that preserves source positions.

**Why:** Unicode case folding can change byte lengths (e.g., German `ß` → `ss`, Turkish `İ` → `i̇`). Indexing with misaligned offsets causes panics at non-UTF-8 byte boundaries.

*Scope:* All string search/matching code that returns byte positions.
