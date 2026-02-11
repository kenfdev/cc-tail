## Implementation Plan

### Overview

This task has 4 sub-tasks:
1. Add `rstest = "0.23"` to `[dev-dependencies]` in `Cargo.toml`
2. Remove 2 redundant tests from `src/symbols.rs`
3. Remove 8 tests from `src/theme.rs`, add 1 replacement test
4. Remove 3 stdlib-testing tests from `src/tui/mod.rs`

Net change: -12 tests (720 - 12 = 708 tests expected after all changes).

### Step 1: Add rstest dependency to Cargo.toml

**File:** `Cargo.toml`

**Action:** Add `rstest = "0.23"` under `[dev-dependencies]` (after `tempfile = "3"`).

### Step 2: Remove 2 redundant tests from symbols.rs

**File:** `src/symbols.rs`

**Tests to remove:**
- `test_unicode_symbols` (lines 67-74): Duplicates `test_symbols_new_false_is_unicode`
- `test_ascii_symbols` (lines 76-83): Duplicates `test_symbols_new_true_is_ascii`

**Tests to keep:**
- `test_symbols_new_false_is_unicode` (lines 86-92)
- `test_symbols_new_true_is_ascii` (lines 94-101)

### Step 3: Remove 8 tests from theme.rs, add 1 replacement

**File:** `src/theme.rs`

**Tests to remove (8 total):**
1. `test_dark_theme_backward_compatible_borders` (lines 309-314)
2. `test_dark_theme_backward_compatible_sidebar` (lines 316-331)
3. `test_dark_theme_backward_compatible_logstream` (lines 333-345)
4. `test_dark_theme_backward_compatible_agent_palette` (lines 347-359)
5. `test_dark_theme_backward_compatible_status_bar` (lines 361-371)
6. `test_dark_theme_backward_compatible_filter_overlay` (lines 373-387)
7. `test_agent_palette_length` (lines 424-430)
8. `test_theme_colors_clone` (lines 432-437)

**Tests to keep (4):**
1. `test_light_theme_differs_from_dark` (lines 389-397)
2. `test_light_theme_uses_basic_ansi_colors` (lines 399-410)
3. `test_from_theme_dark` (lines 412-416)
4. `test_from_theme_light` (lines 418-422)

**Test to add (1):**
A single `test_dark_theme_is_self_consistent` that checks structural properties: palette length is 8, dark theme is not equal to a default/reset-all struct, and clone works.

### Step 4: Remove 3 tests from tui/mod.rs

**File:** `src/tui/mod.rs`

**Tests to remove:**
1. `test_shutdown_flag_can_be_set_externally` (lines 303-311)
2. `test_shutdown_flag_forces_quit` (lines 313-326)
3. `test_shutdown_flag_shared_across_threads` (lines 328-342)

**Tests to keep (2):**
1. `test_setup_signal_handler_returns_false_initially` (lines 296-301)
2. `test_multiple_signal_handler_calls_independent` (lines 344-357)

### Execution Order

1. Edit `Cargo.toml` -- add rstest
2. Edit `src/symbols.rs` -- remove 2 tests
3. Run `cargo test --lib symbols` -- verify
4. Edit `src/theme.rs` -- remove 8 tests, add 1
5. Run `cargo test --lib theme` -- verify
6. Edit `src/tui/mod.rs` -- remove 3 tests
7. Run `cargo test --lib tui::mod` -- verify
8. Run `cargo test` -- full suite, expect 708 tests
9. Update `docs/feature-progress.md`

### Risks

- **rstest version compatibility**: Should be compatible with Rust edition 2021.
- **Test count**: Task says "7 backward_compatible snapshot tests" but code shows 6. Removing `test_agent_palette_length` to match stated "-12 net" target.
- **Replacement test**: Will add structural property test that isn't brittle.

<!-- DECISION: PLANNED -->
