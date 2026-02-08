# cc-tail Feature Progress

Tracking document for v1 implementation. Each task follows the **Plan → Implement → Review** workflow.

Status legend: `[ ]` not started · `[~]` in progress · `[x]` done

---

## 1. Project Scaffolding & CLI

Set up the Rust project, define CLI flags/subcommands with clap, and wire up config file loading.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Cargo workspace structure, dependency list, clap derive API design, config TOML schema |
| Implement | `[ ]` | `Cargo.toml`, `main.rs` entry point, clap args (`--project`, `--session`, `--verbose`, `--theme`, `--config`), `stream` subcommand args (`--file`, `--replay`, `--verbose`, `--theme`), config file parsing with `toml` crate, CLI-overrides-config precedence |
| Review | `[ ]` | Verify `--help` output, config defaults, unknown-key tolerance, missing-file graceful fallback |

---

## 2. JSONL Parsing & Data Model

Implement the hybrid parsing model (typed struct + `serde_json::Value` for content blocks) and the `LogEntry` type.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Define `LogEntry` struct fields, `#[serde(default)]` strategy, content block enum vs Value trade-off |
| Implement | `[ ]` | `LogEntry` struct with `type`, `sessionId`, `timestamp`, `message.role`, `message.content` (as `Value`), `isSidechain`, `agentId`, `slug`, `uuid`/`parentUuid`. Parsing function `parse_jsonl_line() -> Result<LogEntry>`. Malformed line handling (silent skip / verbose stderr warning). |
| Review | `[ ]` | Unit tests: known fields extraction, unknown fields tolerance, malformed line handling, edge cases (empty content array, missing optional fields) |

---

## 3. Tool Call Summarization

Extract one-line input-only summaries from `tool_use` content blocks for each known tool type.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Define summary format per tool (Read, Bash, Edit, Write, Glob, Grep, Task), fallback strategy |
| Implement | `[ ]` | `summarize_tool_use(name, input) -> String` function. Extract relevant input fields (file path, command, pattern). Fallback to bare tool name on extraction failure. |
| Review | `[ ]` | Unit tests for each tool type, input extraction failure fallback, unknown tool names |

---

## 4. Content Block Rendering Logic

Render `message.content` arrays: text blocks, tool_use summaries, and unknown block type indicators (type + size).

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Define rendering rules per block type, ordering guarantees, size calculation for unknown blocks |
| Implement | `[ ]` | `render_content_blocks(content: &[Value]) -> Vec<RenderedLine>`. Preserve original array order. Text blocks rendered fully, tool_use → summary line, unknown blocks → `[type] (size)`. |
| Review | `[ ]` | Unit tests: mixed block arrays, unknown block types, empty content, size formatting |

---

## 5. Project Path Auto-Detection

Convert CWD to Claude Code's escaped log path format and locate the correct `~/.claude/projects/` directory.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Escaped path algorithm, parent-walk strategy, git-root fallback, `--project` override, most-specific match rule |
| Implement | `[ ]` | `detect_project_path(cwd, explicit_project) -> Result<PathBuf>`. Steps: escape CWD → check exists → walk parents → git root → require `--project`. Single directory result, longest path wins. |
| Review | `[ ]` | Unit tests: path escaping, parent walking, git root detection, explicit override, ambiguous path edge cases |

---

## 6. Session Discovery & Management

Discover sessions from JSONL files, track subagent relationships, and support auto-attach and `--session` prefix match.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Session struct design, subagent association (directory structure), sorting by recency, prefix matching |
| Implement | `[ ]` | `Session` struct (id, agents, last_modified). Scan project directory for `*.jsonl` + `{id}/subagents/*.jsonl`. Sort by mtime, limit to 20. Auto-attach to most recent. `--session` prefix match. Active/inactive threshold (10 min). |
| Review | `[ ]` | Integration tests with synthetic directory structures, prefix match edge cases, empty directory handling |

---

## 7. File Watching & Incremental Reading

Watch the project directory with `notify` crate and implement byte-cursor incremental reading with incomplete-line buffering.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | notify crate configuration (native watchers only, recursive), per-file state tracking, channel design |
| Implement | `[ ]` | Recursive watcher on project dir filtered to `*.jsonl`. Per-file `WatchState { byte_offset, incomplete_line_buf }`. On event: read from offset → split on `\n` → parse complete lines → buffer incomplete tail → send `LogEntry` via `tokio::sync::mpsc`. New subagent file detection. |
| Review | `[ ]` | Integration tests: incomplete line buffering, multi-event sequences, new file detection. Manual testing for watcher latency. |

---

## 8. Ring Buffer (Byte-Budget)

Implement a 50MB byte-budget ring buffer for `LogEntry` storage with oldest-first eviction and full re-render support.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Byte accounting strategy (per-entry size estimation), eviction policy, re-filtering API |
| Implement | `[ ]` | `RingBuffer<LogEntry>` with `push()`, `iter()`, `iter_filtered(filter)`, `byte_size()`. Track cumulative byte size, evict oldest when budget exceeded. Entry size = serialized JSON size or estimated struct size. |
| Review | `[ ]` | Unit tests: eviction behavior, capacity limits, filter-and-iterate, mixed entry sizes |

---

## 9. TUI Foundation & Layout

Set up ratatui with crossterm, implement the three-panel layout (sidebar, log stream, status bar), and the main event loop.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Layout proportions, focus model (sidebar vs log stream), tick rate, channel draining strategy |
| Implement | `[ ]` | Terminal setup/restore. Main loop: poll crossterm events + drain mpsc channel → update state → render. Layout: sidebar (fixed width or %) + log stream (flex) + status bar (1 row). Focus toggle with `Tab`. Sidebar toggle with `b`. |
| Review | `[ ]` | Manual testing: resize behavior, focus switching, clean terminal restore on exit |

---

## 10. Sidebar Widget

Render the session list with agent children, navigation, selection, and new-session notification highlight.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Widget structure, selection state, scroll behavior, highlight styling |
| Implement | `[ ]` | Sidebar widget rendering: sessions sorted by recency, active session `●` marker, agents as indented children with full 3-word slugs, `j`/`k` navigation, `Enter` to select. New session visual highlight. Last 20 sessions limit. Session ID prefix (6 chars) + timestamp display. |
| Review | `[ ]` | Manual testing: navigation wrapping, many sessions, long slug names, new session appearance |

---

## 11. Log Stream Widget

Render the interleaved message stream with timestamps, emoji/ASCII role indicators, agent prefixes, and per-agent colors.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Line layout, color scheme (dark/light), per-agent color hashing, word-wrap strategy |
| Implement | `[ ]` | Log stream widget: render `LogEntry` items from ring buffer. Timestamps (HH:MM:SS). Role indicators (`👤`/`🤖`). Agent prefixes: none for main, `[last-word-of-slug]` for subagents. Deterministic hash → 8-color palette for agent colors. Blue=human, green=assistant, yellow=tool. Full text output, ratatui wrapping. |
| Review | `[ ]` | Manual testing: color rendering, long messages, mixed agents, theme switching |

---

## 12. Status Bar

Implement the dynamic priority status bar showing active filters, inactive badge, and keyboard shortcuts.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Priority layout algorithm, badge styling, filter display format |
| Implement | `[ ]` | Status bar widget: inactive badge (always visible), active filter display, keyboard shortcuts (space-permitting). Narrow terminal: hide shortcuts first, truncate filters, badge always visible. Inactive detection: session file mtime > 10 min ago. |
| Review | `[ ]` | Manual testing: narrow terminal behavior, filter display, inactive badge appearance/disappearance |

---

## 13. Filter System & Overlay

Implement the filter trait, concrete filters (text regex, role, agent), AND combinator, the `/` overlay UI, and retroactive filtering.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | `MessageFilter` trait design, combinator pattern, overlay widget layout, real-time regex validation |
| Implement | `[ ]` | `MessageFilter` trait + `RegexFilter`, `RoleFilter`, `AgentFilter`, `AndFilter`. Filter overlay: pattern input with green/red border validation, role toggles, agent toggles (snapshot on open). `Enter` to apply, `Esc` to cancel, `Tab` between fields. Retroactive: on filter change, re-iterate ring buffer. Update status bar. |
| Review | `[ ]` | Unit tests: filter matching logic, AND combinator. Manual testing: overlay UX, regex validation feedback, retroactive re-render |

---

## 14. Session Replay

On startup and session switch, replay the last 20 visible messages from the JSONL file(s), then continue live tailing.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Replay algorithm (full scan, collect last N visible across all agent files, interleave chronologically) |
| Implement | `[ ]` | `replay_session(session, filter, n=20) -> Vec<LogEntry>`. Scan main JSONL + all subagent JSONLs. Parse all entries, filter, sort by timestamp, take last 20. Populate ring buffer. Set byte cursors to EOF for live tailing. |
| Review | `[ ]` | Integration tests: replay count correctness, multi-agent interleaving, filter interaction, empty session |

---

## 15. Progress Entry Toggle

Toggle visibility of `progress` type entries via `p` key (independent of `--verbose`).

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | State management for toggle, interaction with filter system, display format |
| Implement | `[ ]` | `progress_visible: bool` state toggle on `p` keypress. Progress entries rendered as `▶ Delegating: <task description>`. Hidden by default. Toggle triggers ring buffer re-render. `file-history-snapshot` always hidden. |
| Review | `[ ]` | Manual testing: toggle behavior, interaction with other filters |

---

## 16. Theme Support

Implement dark and light themes with reasonable ANSI color defaults and per-agent deterministic color assignment.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Theme struct design, color mapping for each element, 256-color vs 16-color fallback strategy |
| Implement | `[ ]` | `Theme` enum/struct with color definitions for: timestamps (dim), human (blue), assistant (green), tool (yellow), agent prefix colors, sidebar highlight, status bar. Dark/light variants. `--theme` flag + config file support. 256-color with 16-color fallback. |
| Review | `[ ]` | Manual testing on dark/light terminals, 16-color terminal fallback |

---

## 17. `stream` Subcommand

Implement the lightweight non-TUI streaming mode that tails a single JSONL file to stdout.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Output format, replay logic, TTY detection for emoji/ANSI, reuse of parsing pipeline |
| Implement | `[ ]` | `cc-tail stream --file <path> --replay 20`. Replay last N visible messages then live tail. TTY detection: emoji + ANSI colors for interactive, ASCII `[H]`/`[A]` + no colors for piped. Reuse JSONL parser, tool summarizer, content block renderer. Output to stdout. |
| Review | `[ ]` | Integration tests: output format verification (TTY vs piped), replay count, live tailing behavior |

---

## 18. tmux Integration

Spawn per-agent tmux panes running `cc-tail stream`, manage pane lifecycle, and handle cleanup.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | `Multiplexer` trait design, `TmuxBackend` implementation, pane lifecycle on session switch, cleanup strategy |
| Implement | `[ ]` | `Multiplexer` trait + `TmuxBackend`. `$TMUX` env detection. `t` key: create `cc-tail-<project-hash>` tmux session, one pane per agent running `cc-tail stream --file <path> --replay 0`. Auto-tile layout. New subagent → auto-spawn pane. Session switch: keep old panes, `t` again replaces. Track pane IDs for cleanup. Non-tmux: show info message. |
| Review | `[ ]` | Manual testing: pane creation, layout, new subagent pane, session switch behavior, cleanup on exit |

---

## 19. Signal Handling & Graceful Shutdown

Handle SIGINT/SIGTERM for clean terminal restoration and tmux pane cleanup.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Signal handler setup, cleanup sequence, quit confirmation when panes active |
| Implement | `[ ]` | Signal handlers for SIGINT + SIGTERM. Cleanup: restore terminal state, kill all tracked tmux panes, remove cc-tail tmux session. `q` key: if tmux panes active, confirm "Quit and close N panes? (y/n)"; otherwise quit immediately. |
| Review | `[ ]` | Manual testing: Ctrl+C cleanup, `q` with/without panes, terminal state after exit |

---

## 20. Help Overlay

Show a static keyboard shortcut reference on `?` key press.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | Overlay layout, shortcut list content |
| Implement | `[ ]` | `?` key opens centered overlay listing all shortcuts. Any key dismisses. Static content, no contextual info. |
| Review | `[ ]` | Manual testing: overlay appearance, dismissal, terminal size edge cases |

---

## 21. CI & Distribution

Set up GitHub Actions for build/test on macOS + Linux, and release binary publishing.

| Phase | Status | Notes |
|-------|--------|-------|
| Plan | `[ ]` | CI matrix (macOS + Linux, x86_64 + aarch64), release workflow triggers, artifact naming |
| Implement | `[ ]` | GitHub Actions workflow: `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check` on push/PR. Release workflow: build binaries for 4 targets (macOS x86_64/aarch64, Linux x86_64/aarch64) using native runners, upload to GitHub Releases. `Cargo.toml` metadata for crates.io publishing. |
| Review | `[ ]` | Verify CI passes, release artifacts downloadable, `cargo install cc-tail` works |

---

## Summary

| # | Feature | Plan | Implement | Review |
|---|---------|------|-----------|--------|
| 1 | Project Scaffolding & CLI | `[ ]` | `[ ]` | `[ ]` |
| 2 | JSONL Parsing & Data Model | `[ ]` | `[ ]` | `[ ]` |
| 3 | Tool Call Summarization | `[ ]` | `[ ]` | `[ ]` |
| 4 | Content Block Rendering Logic | `[ ]` | `[ ]` | `[ ]` |
| 5 | Project Path Auto-Detection | `[ ]` | `[ ]` | `[ ]` |
| 6 | Session Discovery & Management | `[ ]` | `[ ]` | `[ ]` |
| 7 | File Watching & Incremental Reading | `[ ]` | `[ ]` | `[ ]` |
| 8 | Ring Buffer (Byte-Budget) | `[ ]` | `[ ]` | `[ ]` |
| 9 | TUI Foundation & Layout | `[ ]` | `[ ]` | `[ ]` |
| 10 | Sidebar Widget | `[ ]` | `[ ]` | `[ ]` |
| 11 | Log Stream Widget | `[ ]` | `[ ]` | `[ ]` |
| 12 | Status Bar | `[ ]` | `[ ]` | `[ ]` |
| 13 | Filter System & Overlay | `[ ]` | `[ ]` | `[ ]` |
| 14 | Session Replay | `[ ]` | `[ ]` | `[ ]` |
| 15 | Progress Entry Toggle | `[ ]` | `[ ]` | `[ ]` |
| 16 | Theme Support | `[ ]` | `[ ]` | `[ ]` |
| 17 | `stream` Subcommand | `[ ]` | `[ ]` | `[ ]` |
| 18 | tmux Integration | `[ ]` | `[ ]` | `[ ]` |
| 19 | Signal Handling & Graceful Shutdown | `[ ]` | `[ ]` | `[ ]` |
| 20 | Help Overlay | `[ ]` | `[ ]` | `[ ]` |
| 21 | CI & Distribution | `[ ]` | `[ ]` | `[ ]` |
