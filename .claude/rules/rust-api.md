---
paths:
  - "src/**/*.rs"
---

# Rust API Rules

## Run clippy after changing public API surface

After changing struct/method visibility (e.g., `pub(crate)` → `pub`), run `cargo clippy` to catch lints that only apply at the new visibility level — such as `new_without_default` for public types with `new()` methods.

*Rationale:* Some clippy lints are conditional on visibility. A seemingly trivial visibility change can introduce new lint violations.
