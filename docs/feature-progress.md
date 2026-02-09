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
| Implement | `[x]` | `tui/mod.rs` (374 lines), `tui/app.rs` (1,772 lines), `tui/event.rs` (200 lines). Terminal setup/restore with panic hook. Main loop: poll crossterm events + drain mpsc channel → update state → render. Layout: sidebar (30 cols fixed) + log stream (flex) + status bar (1 row). Focus toggle with `Tab`. Sidebar toggle with `b`. App state struct with 15+ fields. |
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
| Implement | `[x]` | `tui/ui.rs`. `draw_status_bar()` displays project name, session count, filter status, active session, tmux status. Dynamic key hints (q:quit, /:filter, etc.). Priority layout for narrow terminals. |
| Review | `[x]` | Manual testing: narrow terminal behavior, filter display, inactive badge appearance/disappearance. |

---

## 13. Filter System & Overlay

Implement the filter trait, concrete filters (text regex, role, agent), AND combinator, the `/` overlay UI, and retroactive filtering.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | `MessageFilter` trait design, combinator pattern, overlay widget layout, real-time regex validation |
| Implement | `[x]` | `filter.rs` + `tui/filter_overlay.rs` (851 lines). `MessageFilter` trait + `RegexFilter`, `RoleFilter`, `AgentFilter`. `FilterState` with composition. `draw_filter_overlay()` modal with pattern input, role/agent toggles. Keyboard navigation (Tab/Enter/Esc). Real-time regex validation (green/red border). Retroactive: on filter change, re-iterate ring buffer. Update status bar. |
| Review | `[x]` | Unit tests for filter matching logic. Manual testing: overlay UX, regex validation feedback, retroactive re-render. |

---

## 14. Session Replay

On startup and session switch, replay the last 20 visible messages from the JSONL file(s), then continue live tailing.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Replay algorithm (full scan, collect last N visible across all agent files, interleave chronologically) |
| Implement | `[x]` | `replay.rs`. `replay_session()` reads all agent files, applies visibility filter (User/Assistant/System), returns last 20 messages + EOF offsets. Watcher starts from replay offset. Integration in app.rs. |
| Review | `[x]` | Tests for replay count correctness, multi-agent interleaving, filter interaction, empty session. |

---

## 15. Progress Entry Toggle

Toggle visibility of `progress` type entries via `p` key (independent of `--verbose`).

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | State management for toggle, interaction with filter system, display format |
| Implement | `[x]` | `progress_visible` flag in App. `p` key handler toggles visibility. `replay_session()` respects `progress_visible` parameter. Toggle triggers ring buffer re-render. `file-history-snapshot` always hidden. |
| Review | `[x]` | Manual testing: toggle behavior, interaction with other filters. |

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

## 18. tmux Integration

Spawn per-agent tmux panes running `cc-tail stream`, manage pane lifecycle, and handle cleanup.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | `Multiplexer` trait design, `TmuxBackend` implementation, pane lifecycle on session switch, cleanup strategy |
| Implement | `[x]` | `tmux.rs`. `TmuxManager` struct. `Multiplexer` trait for backends. `TmuxBackend` with pane spawning. `$TMUX` env detection. Session naming with hash (`cc-tail-<project-hash>`). Layout application (tiled). Pane lifecycle management. Error handling (NotInstalled, NotInsideTmux). Track pane IDs for cleanup. Non-tmux: show info message. |
| Review | `[x]` | Manual testing: pane creation, layout, new subagent pane, session switch behavior, cleanup on exit. |

---

## 19. Signal Handling & Graceful Shutdown

Handle SIGINT/SIGTERM for clean terminal restoration and tmux pane cleanup.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Signal handler setup, cleanup sequence, quit confirmation when panes active |
| Implement | `[x]` | `tui/mod.rs`. `setup_signal_handler()` listens for SIGINT/SIGTERM via `tokio::signal`. `Arc<AtomicBool>` shutdown flag. Panic hook restores terminal. `WatcherHandle.shutdown()` signal propagation. Graceful terminal restore. `q` key quit handling. |
| Review | `[x]` | Manual testing: Ctrl+C cleanup, `q` with/without panes, terminal state after exit. |

---

## 20. Help Overlay

Show a static keyboard shortcut reference on `?` key press.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Overlay layout, shortcut list content |
| Implement | `[x]` | `tui/app.rs` + `tui/ui.rs`. `help_overlay_visible` flag in App. `draw_help_overlay()` renders keybindings modal. `?` key handler toggles. Displays 20+ keybindings. Clear widget for modal overlay. Any key dismisses. |
| Review | `[x]` | Manual testing: overlay appearance, dismissal, terminal size edge cases. |

---

## 21. Log Stream Scroll Mode

Freeze the log stream and scroll through history with keyboard/mouse, using a two-phase entry model and focus-aware key dispatch.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | Two-phase scroll entry (pending_scroll → render snapshot → active scroll_mode), focus-aware key dispatch, mouse scroll support |
| Implement | `[x]` | `tui/app.rs`: `PendingScroll` enum, `ScrollMode` struct, `scroll_mode`/`pending_scroll` fields on App, `enter_scroll_mode()`/`exit_scroll_mode()`/`apply_scroll()`/`is_in_scroll_mode()`/`on_mouse()` methods. Focus-aware `on_key()` dispatch (Up/Down/j/k/PageUp/PageDown/g/G/Home/End/Esc). Scroll reset on session switch and filter apply. `tui/event.rs`: `Mouse(MouseEvent)` variant in `AppEvent`, crossterm mouse event propagation. `tui/mod.rs`: mouse dispatch in event loop. `tui/ui.rs`: three-branch `draw_logstream()` (scroll_mode active → render snapshot; pending_scroll → build lines, snapshot, apply; normal → auto-scroll). Dynamic title `[SCROLL mode - Esc:exit]`. Help overlay updated with scroll keybindings. 30+ tests. |
| Review | `[x]` | Code quality review: APPROVED (well-structured, 50+ new unit tests, 648 total passing, zero clippy warnings). Security review: APPROVED (no vulnerabilities found). |

---

## 22. CI & Distribution

Set up GitHub Actions for build/test on macOS + Linux, and release binary publishing.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[x]` | CI matrix (macOS + Linux, x86_64 + aarch64), release workflow triggers, artifact naming |
| Implement | `[x]` | `.github/workflows/ci.yml`: 3-target matrix (macos-latest, ubuntu-latest, ubuntu-24.04-arm) with fmt, clippy, test, build. Linux targets use musl (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`) for fully static binaries. `.github/workflows/release.yml`: triggered on `v*` tags, builds release binaries with `--target` flag, creates GitHub Release via `softprops/action-gh-release`. `install.sh`: maps Linux to `linux-musl` target. `Cargo.toml`: renamed to `cctail`, added repository/readme/keywords/categories metadata. `README.md`: installation (cargo install + binary download), usage, key bindings. |
| Review | `[x]` | `cargo fmt --check` passes, `cargo clippy -- -D warnings` passes, `cargo test` (648 tests OK), `cargo publish --dry-run` succeeds. |

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
| 13 | Filter System & Overlay | `[x]` | `[x]` | `[x]` |
| 14 | Session Replay | `[x]` | `[x]` | `[x]` |
| 15 | Progress Entry Toggle | `[x]` | `[x]` | `[x]` |
| 16 | Theme Support | `[x]` | `[x]` | `[x]` |
| 17 | `stream` Subcommand | `[x]` | `[x]` | `[x]` |
| 18 | tmux Integration | `[x]` | `[x]` | `[x]` |
| 19 | Signal Handling & Graceful Shutdown | `[x]` | `[x]` | `[x]` |
| 20 | Help Overlay | `[x]` | `[x]` | `[x]` |
| 21 | Log Stream Scroll Mode | `[x]` | `[x]` | `[x]` |
| 22 | CI & Distribution | `[x]` | `[x]` | `[x]` |
