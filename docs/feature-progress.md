# cc-tail Feature Progress

Tracking document for v1 implementation. Each task follows the **Plan → Implement → Review** workflow.

Status legend: `[ ]` not started · `[~]` in progress · `[x]` done

---

## 1. Project Scaffolding & CLI

Set up the Rust project, define CLI flags/subcommands with clap, and wire up config file loading.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Cargo workspace structure, dependency list, clap derive API design, config TOML schema |
| Implement | `[x]` | `Cargo.toml`, `main.rs` (57 lines), `cli.rs` (74 lines, clap derive), `config.rs` (525 lines). All flags implemented: `--project`, `--session`, `--verbose`, `--theme`, `--config`. `stream` subcommand with `--file`, `--replay`, `--verbose`, `--theme`. Config file parsing with `toml` crate, CLI-overrides-config precedence. |
| Review | `[x]` | 18 unit tests in config.rs covering TOML parsing, merging, defaults, unknown-key tolerance, missing-file fallback. |

---

## 2. JSONL Parsing & Data Model

Implement the hybrid parsing model (typed struct + `serde_json::Value` for content blocks) and the `LogEntry` type.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Define `LogEntry` struct fields, `#[serde(default)]` strategy, content block enum vs Value trade-off |
| Implement | `[x]` | `log_entry.rs` (452 lines). `LogEntry` struct with hybrid parsing (array/string content). `EntryType` enum: User, Assistant, Progress, FileHistorySnapshot, System, QueueOperation, Unknown. `parse_jsonl_line() -> Result<LogEntry>`. Malformed line handling (silent skip / verbose stderr warning). |
| Review | `[x]` | 16+ unit tests covering known fields extraction, unknown fields tolerance, malformed line handling, edge cases (empty content array, missing optional fields). |

---

## 3. Tool Call Summarization

Extract one-line input-only summaries from `tool_use` content blocks for each known tool type.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Define summary format per tool (Read, Bash, Edit, Write, Glob, Grep, Task), fallback strategy |
| Implement | `[x]` | `tool_summary.rs` (961 lines). `summarize_tool_use(name, input) -> String` handles 9+ tools (Read, Bash, Edit, Write, Glob, Grep, Task, WebSearch, WebFetch, Skill). Includes ANSI sanitization + secret redaction for security. Fallback to bare tool name on extraction failure. |
| Review | `[x]` | 80+ unit tests covering each tool type, input extraction failure fallback, unknown tool names, security (sanitization, redaction, truncation). |

---

## 4. Content Block Rendering Logic

Render `message.content` arrays: text blocks, tool_use summaries, and unknown block type indicators (type + size).

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Define rendering rules per block type, ordering guarantees, size calculation for unknown blocks |
| Implement | `[x]` | `content_render.rs` (733 lines). `render_content_blocks()` handles text/tool_use/tool_result/unknown blocks. `has_renderable_content()` optimization. `RenderedLine` enum. Preserves original array order. |
| Review | `[x]` | 30+ unit tests covering mixed block arrays, unknown block types, empty content, size formatting. |

---

## 5. Project Path Auto-Detection

Convert CWD to Claude Code's escaped log path format and locate the correct `~/.claude/projects/` directory.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Escaped path algorithm, parent-walk strategy, git-root fallback, `--project` override, most-specific match rule |
| Implement | `[x]` | `project_path.rs` (597 lines). `detect_project_path()` with 5-level strategy: explicit override → CWD → parent walk → git root → error. `escape_path()` for `~/.claude/projects/` mapping. Single directory result, longest path wins. |
| Review | `[x]` | 30+ unit tests covering path escaping, parent walking, git root detection, explicit override, ambiguous path edge cases. |

---

## 6. Session Discovery & Management

Discover sessions from JSONL files, track subagent relationships, and support auto-attach and `--session` prefix match.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Session struct design, subagent association (directory structure), sorting by recency, prefix matching |
| Implement | `[x]` | `session.rs` (1,069 lines). `Session`/`Agent` structs. `discover_sessions()` scans for `*.jsonl` + `{id}/subagents/*.jsonl`. `resolve_session()` with prefix matching. `SessionStatus` (Active/Inactive). Sort by mtime, limit to 20. Auto-attach to most recent. Active/inactive threshold (10 min). |
| Review | `[x]` | 34 unit tests covering discovery, filtering, classification, prefix match edge cases, empty directory handling. |

---

## 7. File Watching & Incremental Reading

Watch the project directory with `notify` crate and implement byte-cursor incremental reading with incomplete-line buffering.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | notify crate configuration (native watchers only, recursive), per-file state tracking, channel design |
| Implement | `[x]` | `watcher.rs` (1,191 lines). Recursive watcher on project dir filtered to `*.jsonl`. Per-file `FileWatchState { byte_offset, incomplete_line_buf }`. On event: read from offset → split on `\n` → parse complete lines → buffer incomplete tail → send `LogEntry` via `tokio::sync::mpsc`. File truncation detection. MAX_READ_BYTES (64MB) and MAX_INCOMPLETE_LINE_BUF (10MB) limits. New subagent file detection. |
| Review | `[x]` | 23 unit tests covering incremental reading, incomplete line buffering, multi-event sequences, new file detection. |

---

## 8. Ring Buffer (Byte-Budget)

Implement a 50MB byte-budget ring buffer for `LogEntry` storage with oldest-first eviction and full re-render support.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Byte accounting strategy (per-entry size estimation), eviction policy, re-filtering API |
| Implement | `[x]` | `ring_buffer.rs`. `RingBuffer` with `DEFAULT_BYTE_BUDGET` (50MB). O(1) eviction accounting. `push()` with LRU eviction, `iter()`, `iter_filtered(filter)`, `byte_size()`. Track cumulative byte size, evict oldest when budget exceeded. |
| Review | `[x]` | Unit tests covering eviction behavior, capacity limits, filter-and-iterate, mixed entry sizes. |

---

## 9. TUI Foundation & Layout

Set up ratatui with crossterm, implement the three-panel layout (sidebar, log stream, status bar), and the main event loop.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Layout proportions, focus model (sidebar vs log stream), tick rate, channel draining strategy |
| Implement | `[x]` | `tui/mod.rs` (374 lines), `tui/app.rs` (1,772 lines), `tui/event.rs` (200 lines). Terminal setup/restore with panic hook. Main loop: poll crossterm events + drain mpsc channel → update state → render. Layout: sidebar (30 cols fixed) + log stream (flex) + status bar (1 row). Focus toggle with `Tab`. Sidebar toggle with `b`. App state struct with 15+ fields. Default focus on Sidebar; `Enter` confirms session selection without switching focus. |
| Review | `[x]` | Implementation wired and functional. Manual testing: resize behavior, focus switching, clean terminal restore on exit. |

---

## 10. Sidebar Widget

Render the session list with agent children, navigation, selection, and new-session notification highlight.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Widget structure, selection state, scroll behavior, highlight styling |
| Implement | `[x]` | `tui/ui.rs` (part of 2,590 lines). `draw_sidebar()` renders session list as tree: session headers with `●` active marker + timestamp + status, agents as indented children with full 3-word slugs. `j`/`k` navigation, `Enter` to select. New session visual highlight. Last 20 sessions limit. Session ID prefix (6 chars). Scrolling support. |
| Review | `[x]` | Tests in ui.rs. Manual testing: navigation wrapping, many sessions, long slug names, new session appearance. |

---

## 11. Log Stream Widget

Render the interleaved message stream with timestamps, emoji/ASCII role indicators, agent prefixes, and per-agent colors.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Line layout, color scheme (dark/light), per-agent color hashing, word-wrap strategy |
| Implement | `[x]` | `tui/ui.rs` (part of 2,590 lines). `draw_logstream()` renders filtered log entries. Role indicators (`>`, `<`, `?`). Timestamps (HH:MM:SS). Agent prefixes: none for main, `[last-word-of-slug]` for subagents. Tool summaries. Line wrapping. Focused/unfocused border styles. Full text output. |
| Review | `[x]` | Manual testing: color rendering, long messages, mixed agents, theme switching. |

---

## 12. Status Bar

Implement the dynamic priority status bar showing active filters, inactive badge, and keyboard shortcuts.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Priority layout algorithm, badge styling, filter display format |
| Implement | `[x]` | `tui/ui.rs`. `draw_status_bar()` displays project name, session count, filter status, active session. Dynamic key hints (q:quit, /:filter, etc.). Priority layout for narrow terminals. |
| Review | `[x]` | Manual testing: narrow terminal behavior, filter display, inactive badge appearance/disappearance. |

---

## 13. Filter Menu (`f` key)

Simple menu-style filter overlay with two filter dimensions: tool call hiding and agent filtering. Replaces the previous complex regex/role/agent filter overlay.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Two-level filtering: entry-level (agent) + line-level (tool call hiding). Menu-style overlay with radio buttons for agents and checkbox for tool calls. |
| Implement | `[x]` | **Rewritten**: `filter.rs` (~90 lines, 21 tests): Simple `FilterState { hide_tool_calls, selected_agent }` with `is_active()`, `matches()`, `is_tool_line_visible()`, `display()`. Removed `MessageFilter` trait, `RegexFilter`, `RoleFilter`, `AgentFilter`, `AndFilter`. `tui/filter_overlay.rs` (~190 lines, 30 tests): `FilterMenuState` with `FilterMenuItem` enum (ToolCallToggle, AgentAll, Agent), `MenuAction` (Consumed, Close, Selected). Navigate j/k, select Enter/Space, close Esc/f. `tui/app.rs`: `f` key opens menu, `/` unbound (reserved for search). `ActiveFilters` struct removed. `apply_filter_from_menu()` applies immediately on selection. `tui/ui.rs`: `draw_filter_menu()` centered popup with highlighted selection. Tool call line hiding in `draw_logstream()` via `RenderedLine::ToolUse` skip. Status bar shows `[filter: no tools]`, `[filter: agent cook]`, etc. Shortcuts changed `/:filter` to `f:filter`. `replay.rs`: Compatible with new FilterState; 2 tests rewritten. 614 tests total, zero clippy warnings. |
| Review | `[x]` | 51 new unit tests across filter.rs (21) and filter_overlay.rs (30). Integration tests in app.rs. Smoke tests in ui.rs. All 614 tests pass, `cargo clippy` clean. |

---

## 14. Session Replay

On startup and session switch, replay the last 20 visible messages from the JSONL file(s), then continue live tailing.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Replay algorithm (full scan, collect last N visible across all agent files, interleave chronologically) |
| Implement | `[x]` | `replay.rs`. `replay_session()` reads all agent files, applies visibility filter (User/Assistant/System), returns last 20 messages + EOF offsets. Watcher starts from replay offset. Integration in app.rs. |
| Review | `[x]` | Tests for replay count correctness, multi-agent interleaving, filter interaction, empty session. |

---

## 15. Progress Entry Toggle (REMOVED)

The `p` keyboard shortcut and progress entry toggle feature have been removed. Progress entries are now always hidden.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | State management for toggle, interaction with filter system, display format |
| Implement | `[x]` | **REMOVED**: `progress_visible` field, `p` key handler, and `toggle_progress_visible()` method removed from App. `is_visible_type()` hardcodes `Progress => false`. Progress entries always hidden. |
| Review | `[x]` | Feature removed; all references cleaned from replay.rs, stream.rs, app.rs, ui.rs. |

---

## 16. Theme Support

Implement dark and light themes with reasonable ANSI color defaults and per-agent deterministic color assignment.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Theme struct design, color mapping for each element, 256-color vs 16-color fallback strategy |
| Implement | `[x]` | `theme.rs`. `Theme` enum (Dark/Light). `ThemeColors` struct with 40+ color fields. `from_theme()` constructor. Both themes use 16-color ANSI palette. Applied in ui.rs rendering. `--theme` CLI flag + config file support. |
| Review | `[x]` | Manual testing on dark/light terminals, 16-color terminal fallback. |

---

## 17. `stream` Subcommand

Implement the lightweight non-TUI streaming mode that tails a single JSONL file to stdout.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Output format, replay logic, TTY detection for emoji/ANSI, reuse of parsing pipeline |
| Implement | `[x]` | `stream.rs` (200+ lines). `StreamArgs` struct (`--file`, `--replay`, `--verbose`, `--theme`). `run_stream()` entry point with `replay_phase()` + `live_tail_phase()`. TTY detection: emoji + ANSI colors for interactive, ASCII `[H]`/`[A]` + no colors for piped. Reuses JSONL parser, tool summarizer, content block renderer. |
| Review | `[x]` | Manual testing: output format verification (TTY vs piped), replay count, live tailing behavior. |

---

## 18. tmux Integration (REMOVED)

The tmux pane-splitting feature has been removed. Subagent output is viewed inline in the log stream only.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Feature removed |
| Implement | `[x]` | **REMOVED**: `tmux.rs` deleted, all tmux references removed from config, app, ui, and docs. |
| Review | `[x]` | Feature removed; all references cleaned. |

---

## 19. Signal Handling & Graceful Shutdown

Handle SIGINT/SIGTERM for clean terminal restoration.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Signal handler setup, cleanup sequence, quit confirmation when panes active |
| Implement | `[x]` | `tui/mod.rs`. `setup_signal_handler()` listens for SIGINT/SIGTERM via `tokio::signal`. `Arc<AtomicBool>` shutdown flag. Panic hook restores terminal. `WatcherHandle.shutdown()` signal propagation. Graceful terminal restore. `q` key quit handling. |
| Review | `[x]` | Manual testing: Ctrl+C cleanup, `q` with/without panes, terminal state after exit. |

---

## 20. Help Overlay (Enhanced)

Rich three-section help screen with symbol legend, keybinding reference, and live session stats.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Three-section overlay: symbol/color legend, complete keybinding reference (including future keys), session stats computed from ring buffer |
| Implement | `[x]` | **New file: `session_stats.rs` (~250 lines)**: `SessionStats` struct + `compute_session_stats()`. Pure logic, 23 unit tests. Counts messages (user/assistant), tool calls with per-tool breakdown (only `tool_use`, not `tool_result`), subagent count, session duration from timestamps. ISO 8601 parser (no chrono dependency). **`tui/ui.rs`**: Rewrote `draw_help_overlay()` with 3 sections: (1) Symbol & Color Legend showing `>/</?/~/\u{25b6}` with colors + agent prefix + timestamp notes, (2) Complete Keybinding Reference with 17 entries including future keys (`n/N`, `f`, `L`), (3) Session Stats (duration, message counts, tool call count + top-5 breakdown, subagent count, entries loaded). ~70x45 overlay, degrades gracefully on small terminals. **`tui/app.rs`**: Changed help key handling -- `?` toggles, `Esc` closes (replaced "any key dismisses"). Updated 6 tests. **`main.rs`**: Added `mod session_stats`. Total: 635 tests passing, zero clippy warnings. |
| Review | `[x]` | 23 new unit tests in session_stats.rs. Existing help overlay smoke tests in ui.rs updated. All 635 tests pass, `cargo clippy -- -D warnings` clean. |

---

## 21. Log Stream Scroll Mode

Freeze the log stream and scroll through history with keyboard/mouse, using a two-phase entry model and focus-aware key dispatch.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Two-phase scroll entry (pending_scroll → render snapshot → active scroll_mode), focus-aware key dispatch, mouse scroll support |
| Implement | `[x]` | `tui/app.rs`: `PendingScroll` enum (Up/Down/ToTop/HalfPageUp/HalfPageDown), `ScrollMode` struct, `scroll_mode`/`pending_scroll` fields on App, `enter_scroll_mode()`/`exit_scroll_mode()`/`apply_scroll()`/`is_in_scroll_mode()`/`on_mouse()` methods. Focus-aware `on_key()` dispatch (Up/Down/j/k/PageUp/PageDown/u/d/g/G/Home/End/Esc). `u`/`d` keys for half-page scroll. Scroll reset on session switch and filter apply. `tui/event.rs`: `Mouse(MouseEvent)` variant in `AppEvent`, crossterm mouse event propagation. `tui/mod.rs`: mouse dispatch in event loop. `tui/ui.rs`: three-branch `draw_logstream()` (scroll_mode active → render snapshot; pending_scroll → build lines, snapshot, apply; normal → auto-scroll). Dynamic title `[SCROLL mode - Esc:exit]`. Help overlay updated with scroll keybindings. 30+ tests. |
| Review | `[x]` | Code quality review: APPROVED (well-structured, 50+ new unit tests, 659 total passing, zero clippy warnings). Security review: APPROVED (no vulnerabilities found). |

---

## 22. CI & Distribution

Set up GitHub Actions for build/test on macOS + Linux, and release binary publishing.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | CI matrix (macOS + Linux, x86_64 + aarch64), release workflow triggers, artifact naming |
| Implement | `[x]` | `.github/workflows/ci.yml`: 3-target matrix (macos-latest, ubuntu-latest, ubuntu-24.04-arm) with fmt, clippy, test, build. Linux targets use musl (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`) for fully static binaries. `.github/workflows/release.yml`: triggered on `v*` tags, builds release binaries with `--target` flag, creates GitHub Release via `softprops/action-gh-release`. `install.sh`: maps Linux to `linux-musl` target. `Cargo.toml`: renamed to `cctail`, added repository/readme/keywords/categories metadata. `README.md`: installation (cargo install + binary download), usage, key bindings. |
| Review | `[x]` | `cargo fmt --check` passes, `cargo clippy -- -D warnings` passes, `cargo test` (659 tests OK), `cargo publish --dry-run` succeeds. |

---

## 23. Search with Highlighting, Navigation, and Match Counter (Feature 2)

Vim/less-style search with case-insensitive substring matching, in-place highlighting, `n`/`N` navigation, and a `[3/17]` match counter.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Three-state machine (Inactive/Input/Active), post-process highlight approach, reuse status bar for input, force scroll mode on search confirm |
| Implement | `[x]` | **New file: `src/search.rs` (~250 lines)**: `SearchState`, `SearchMode`, `SearchMatch`, `find_matches()` (case-insensitive non-overlapping substring matching), state machine methods (`start_input`, `on_char`, `on_backspace`, `confirm`, `cancel`, `next_match`, `prev_match`, `match_counter_display`). 30 unit tests. **`src/main.rs`**: Added `mod search`. **`src/theme.rs`**: 6 new search color fields (`search_match_bg/fg`, `search_current_bg/fg`, `search_input_fg`, `search_prompt`) for both dark and light themes. **`src/tui/app.rs`**: Added `search_state: SearchState` field, search input/active mode key handling (`/` opens input, `Enter` confirms, `Esc` cancels, `n`/`N` navigate, `Ctrl+C` cancels input), `force_scroll_mode_for_search()`, `scroll_to_current_search_match()`, `cancel_search()`. Search cancelled on filter change and session switch. 15 new unit tests. **`src/tui/ui.rs`**: `draw_search_input_bar()` replaces status bar during input mode, `line_to_text()`, `apply_search_highlights()` (post-process span splitting), `highlight_line()` (per-line match highlighting with current match distinction). Search match computation in `draw_logstream()`. Match counter `[n/N]` in status bar. Updated shortcuts to include `/:search`. Help overlay updated with search keybindings. 10 new rendering tests. Total: 682 tests, zero clippy warnings. |
| Review | `[x]` | 55 new unit tests across search.rs (30), app.rs (15), ui.rs (10). All 682 tests pass, `cargo clippy -- -D warnings` clean. |

---

## 24. Full History Load (`L` key) (Feature 1)

Load the entire session history on demand with `Shift+L`, replacing the default 20-message replay with all visible entries.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | `L` key handler, file size check with 50 MB confirmation threshold, y/n/Esc confirmation flow, scroll position preservation |
| Implement | `[x]` | **`src/replay.rs`**: Added `session_file_size()` (sum agent file sizes, handles missing files) and `load_full_session()` (calls `replay_session` with `usize::MAX`). 3 new tests. **`src/tui/app.rs`**: Added `full_history_loaded`, `full_load_confirm_pending`, `full_load_pending_size_mb` fields. `L` key handler with active session check, size threshold check (50 MB), confirmation prompt interceptor (y/n/Esc before all other handlers). `perform_full_history_load()` replaces ring buffer, restores scroll position, cancels search, sets flag. `get_active_session()` helper. Session switch resets `full_history_loaded`. 10 new tests. **`src/tui/ui.rs`**: "FULL" badge (green bg) in status bar when loaded. Confirmation prompt text in status bar. `L` added to help overlay keybindings. Total: 701 tests, zero clippy warnings. |
| Review | `[x]` | 13 new unit tests across replay.rs (3), app.rs (10). All 701 tests pass, `cargo clippy -- -D warnings` clean. |

---

## 25. ASCII Fallback (`--ascii` flag) (Feature 5)

Provide an `--ascii` CLI flag (and config file option) that replaces Unicode glyphs with ASCII-safe characters for terminal compatibility.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | `Symbols` struct with unicode/ascii modes, `--ascii` CLI flag, config file support, replace 5 hardcoded Unicode references in ui.rs |
| Implement | `[x]` | **New file: `src/symbols.rs` (~100 lines)**: `Symbols` struct with `active_marker`, `tree_connector`, `progress_indicator`, `search_cursor` fields. `unicode()`, `ascii()`, and `new(bool)` constructors. 4 unit tests. **`src/cli.rs`**: Added `--ascii` flag (default false). **`src/config.rs`**: Added `ascii: bool` to `AppConfig` and `ascii: Option<bool>` to `FileConfig`. Wired through `build_config` (file then CLI override). Updated test helper. **`src/main.rs`**: Added `mod symbols`. **`src/tui/app.rs`**: Added `symbols: Symbols` field, initialized from `config.ascii`. 2 new tests. **`src/tui/ui.rs`**: Replaced 5 hardcoded Unicode references with `app.symbols.*`: sidebar active marker (line 142), tree connector (line 190), progress indicator (line 371), search cursor (line 853), help legend progress (line 1017). Total: 701 tests, zero clippy warnings. |
| Review | `[x]` | 6 new unit tests across symbols.rs (4), app.rs (2). All 701 tests pass, `cargo clippy -- -D warnings` clean. |

---

## 26. Search Highlight Bug Fixes (focused match color + Esc cleanup)

Fix two search-related bugs: (1) focused match has no distinct color when navigating with n/N, and (2) highlights persist after Esc cancels search.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Root cause: Branch A renders stale snapshots with baked-in highlights. Fix via snapshot invalidation on n/N/Esc, auto-select first match, scroll-to-match after snapshot creation, and improved current-match colors. |
| Implement | `[x]` | **`src/tui/app.rs`**: Added `invalidate_scroll_snapshot()` method that converts active scroll mode back to pending scroll, preserving offset. Updated `n`/`N` handlers to call `invalidate_scroll_snapshot()` instead of `scroll_to_current_search_match()`. Updated `Esc` handler in search active mode to call `invalidate_scroll_snapshot()` after cancel. **`src/tui/ui.rs`**: Added auto-selection of first match when `current_match_index` is `None` and matches exist. In Branch B, moved `scroll_mode` assignment before paragraph creation, added `scroll_to_current_search_match()` call after snapshot creation when search is active. Resolved borrow checker issue by copying `logstream_text` color before mutable borrow. **`src/theme.rs`**: Changed dark theme `search_current_bg` from `LightYellow` to `Magenta`, `search_current_fg` from `Black` to `White`. Changed light theme `search_current_bg` from `LightYellow` to `Blue`, `search_current_fg` from `Black` to `White`. **UTF-8 safety fix in `src/search.rs`**: Rewrote `find_matches()` to build a byte-offset mapping (`build_lower_to_orig_map`) from lowercased text positions back to original text positions, preventing panics when characters change byte length during case conversion (e.g., Turkish İ U+0130: 2 bytes orig -> 3 bytes lowered, German ẞ U+1E9E: 3 bytes orig -> 2 bytes lowered). Added `map_lower_to_orig()` with binary search lookup. 10 new UTF-8 tests covering emoji, CJK, Turkish İ, German ẞ, accented characters, and comprehensive boundary safety. All 711 tests pass. |
| Review | `[x]` | Code quality review: APPROVED. Security review: APPROVED. UTF-8 byte boundary fix applied with `build_lower_to_orig_map()` and `map_lower_to_orig()` helpers. 10 new multi-byte character tests (emoji, CJK, Turkish I, German eszett, accented characters). Additional fix: `force_scroll_mode_for_search()` now invalidates the scroll snapshot when already in scroll mode, so search highlights appear immediately on Enter (previously showed `[0/0]` until pressing `n`). All 711 tests pass, zero clippy warnings. |

---

## 27. Visual-line scroll fix (wrap-aware search and scroll)

Fix scroll system to use visual (wrapped) line counts instead of logical line counts. This fixes search scroll-to-match not showing the current match when lines wrap, and improves general scroll accuracy.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Root cause: `ScrollMode` used logical line counts (`lines.len()`) but ratatui `Paragraph::scroll()` with `Wrap` operates on visual (wrapped) lines. Mismatch causes viewport to miss the current search match. |
| Implement | `[x]` | **`src/tui/app.rs`**: Added `total_visual_lines` and `inner_width` fields to `ScrollMode`. Added `wrapped_line_height()`, `total_visual_lines()`, `visual_line_position()` helper functions. Updated `apply_scroll()` to use `total_visual_lines` for max_offset. Updated `scroll_to_current_search_match()` to convert logical line index to visual line position. 2 new tests (`test_visual_line_helpers`, `test_scroll_to_search_match_with_wrapping`). **`src/tui/ui.rs`**: Branch A, B, and C all now use visual line counts for ratatui scroll offset calculation. Branch B computes `total_visual_lines` when creating snapshot. All 713 tests pass. |
| Review | `[x]` | Code quality review: APPROVED. Clippy warning fixed (manual `div_ceil` → `.div_ceil()`). All 713 tests pass. |

---

## Summary

| # | Feature | Plan | Implement | Review |
|---|---------|------|-----------|--------|
| 1 | Project Scaffolding & CLI | `[x]` | `[x]` | `[x]` |
| 2 | JSONL Parsing & Data Model | `[x]` | `[x]` | `[x]` |
| 3 | Tool Call Summarization | `[x]` | `[x]` | `[x]` |
| 4 | Content Block Rendering Logic | `[x]` | `[x]` | `[x]` |
| 5 | Project Path Auto-Detection | `[x]` | `[x]` | `[x]` |
| 6 | Session Discovery & Management | `[x]` | `[x]` | `[x]` |
| 7 | File Watching & Incremental Reading | `[x]` | `[x]` | `[x]` |
| 8 | Ring Buffer (Byte-Budget) | `[x]` | `[x]` | `[x]` |
| 9 | TUI Foundation & Layout | `[x]` | `[x]` | `[x]` |
| 10 | Sidebar Widget | `[x]` | `[x]` | `[x]` |
| 11 | Log Stream Widget | `[x]` | `[x]` | `[x]` |
| 12 | Status Bar | `[x]` | `[x]` | `[x]` |
| 13 | Filter Menu (`f` key) | `[x]` | `[x]` | `[x]` |
| 14 | Session Replay | `[x]` | `[x]` | `[x]` |
| 15 | Progress Entry Toggle (REMOVED) | `[x]` | `[x]` | `[x]` |
| 16 | Theme Support | `[x]` | `[x]` | `[x]` |
| 17 | `stream` Subcommand | `[x]` | `[x]` | `[x]` |
| 18 | tmux Integration (REMOVED) | `[x]` | `[x]` | `[x]` |
| 19 | Signal Handling & Graceful Shutdown | `[x]` | `[x]` | `[x]` |
| 20 | Help Overlay (Enhanced) | `[x]` | `[x]` | `[x]` |
| 21 | Log Stream Scroll Mode | `[x]` | `[x]` | `[x]` |
| 22 | CI & Distribution | `[x]` | `[x]` | `[x]` |
| 23 | Search (Feature 2) | `[x]` | `[x]` | `[x]` |
| 24 | Full History Load (Feature 1) | `[x]` | `[x]` | `[x]` |
| 25 | ASCII Fallback (Feature 5) | `[x]` | `[x]` | `[x]` |
| 26 | Search Highlight Bug Fixes | `[x]` | `[x]` | `[x]` |
| 27 | Visual-line scroll fix (wrap-aware) | `[x]` | `[x]` | `[x]` |
