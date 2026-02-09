# cc-tail

A TUI application for monitoring Claude Code sessions in real-time.

## Problem

When running Claude Code in non-interactive mode (e.g. via shell scripts in a ralph loop), there is no visibility into what's happening inside Claude sessions. The raw JSONL log files exist but are not human-readable. Users need a way to monitor active Claude conversations in real-time.

## Solution

A Rust TUI application (built on `ratatui`) that watches Claude Code's JSONL log directory, detects active sessions, parses log entries, and renders them as a rich, color-formatted chat-like stream. Features a sidebar for session/agent browsing and runtime-changeable filters.

---

## Core Mental Model

- **Session** = one Claude Code conversation = main agent + its subagents (they share a `sessionId`)
- **cc-tail shows one session at a time** — multiple concurrent sessions are not shown simultaneously
- **Session switching** happens via the sidebar — users browse and select which session to monitor
- **Subagents within a session** are viewed inline (interleaved)

---

## Claude Code Log Architecture

Claude Code stores conversation logs at `~/.claude/projects/`. Each project directory uses an escaped path format (replace `/` with `-`, strip leading `-`). **Subdirectories create separate escaped paths** — running Claude Code from `/Users/foo/myproject/src` produces a different project directory than `/Users/foo/myproject`.

```
~/.claude/projects/-Users-foo-myproject/
├── {sessionId}.jsonl                         # main session log
├── {sessionId}/subagents/
│   ├── agent-{agentId}.jsonl                 # subagent logs
│   └── agent-{agentId}.jsonl
└── {anotherSessionId}.jsonl                  # another concurrent session
```

### JSONL Entry Schema

Each line is a JSON object with these key fields:

| Field | Description |
|---|---|
| `type` | Entry type: `"user"`, `"assistant"`, `"progress"`, `"file-history-snapshot"` |
| `sessionId` | UUID identifying the session (shared between main and its subagents) |
| `uuid` / `parentUuid` | Message threading chain |
| `isSidechain` | `false` for main session, `true` for subagent |
| `agentId` | Subagent identifier (e.g. `"a0d0bbc"`) — absent on main session |
| `slug` | Human-readable subagent name (e.g. `"effervescent-soaring-cook"`) |
| `timestamp` | ISO 8601 timestamp |
| `message.role` | `"user"` or `"assistant"` |
| `message.content` | Array of content blocks: `text`, `tool_use`, `tool_result` |
| `message.model` | Model used (e.g. `"claude-opus-4-6"`, `"claude-haiku-4-5-20251001"`) |

### Session & Subagent Relationship

- All subagents inherit the parent session's `sessionId`
- Subagent logs live under `{sessionId}/subagents/agent-{agentId}.jsonl`
- Selecting a session automatically includes all its subagents

---

## CLI Interface

```
cc-tail [OPTIONS]
cc-tail stream [OPTIONS]
```

### Default Mode (TUI)

Launches the TUI and begins monitoring Claude Code sessions.

| Flag | Default | Description |
|---|---|---|
| `--project <path>` | Auto-detect from cwd | Path to the project directory (actual code path, not log path). cc-tail converts internally to the `~/.claude/projects/` equivalent. |
| `--session <id>` | Most recent | Attach to a specific session UUID (prefix match supported). Default: auto-attach to the most recently active session. |
| `--verbose` | false | Show progress entries and additional metadata. Also shows JSONL parse errors with raw line content. Writes debug info to stderr (redirect with `2>debug.log`). |
| `--theme <theme>` | dark | Color theme: `dark` or `light`. Reasonable defaults for each terminal background. |
| `--config <path>` | `~/.config/cc-tail/config.toml` | Path to config file |

### `stream` Subcommand

A lightweight, non-TUI streaming mode that tails a single JSONL file and outputs formatted log lines to stdout. Designed for standalone use: users can invoke directly for custom workflows, scripting, or piping.

```
cc-tail stream --file <path> [OPTIONS]
```

| Flag | Default | Description |
|---|---|---|
| `--file <path>` | Required | Path to a specific `.jsonl` file to tail |
| `--replay <n>` | 20 | Number of visible messages to replay from the file before live tailing |
| `--verbose` | false | Show progress entries and parse errors |
| `--theme <theme>` | dark | Color theme for ANSI output |

#### Output Behavior

- **Interactive TTY**: ANSI colors and emoji characters (👤, 🤖)
- **Piped / non-TTY**: ANSI colors stripped, emoji replaced with ASCII equivalents (`👤` → `[H]`, `🤖` → `[A]`). Auto-detected, no flag needed.

---

## TUI Layout

```
┌─ Sessions ──────┬─ Log Stream ──────────────────────────────────────┐
│                  │                                                   │
│ ● abc123  14:32 │ 14:30:12 👤 Human:                               │
│   ├─ main       │   fix the bug in the authentication module        │
│   ├─ cook       │                                                   │
│   └─ swift      │ 14:30:15 🤖 Assistant:                           │
│                  │   I'll investigate the authentication module.     │
│   def456  14:10 │                                                   │
│   ├─ main       │ 14:30:16 [Read] src/auth/mod.rs                  │
│                  │ 14:30:18 [Read] src/auth/jwt.rs                  │
│   ghi789  13:55 │ 14:30:20 [Edit] src/auth/jwt.rs:45               │
│   ├─ main       │ 14:30:22 [Bash] cargo test auth                  │
│   └─ pilot      │                                                   │
│                  │ 14:30:25 🤖 Assistant:                           │
│                  │   Fixed the JWT token validation. The expiry      │
│                  │   check was using UTC instead of local time.      │
│                  │                                                   │
│                  │ [cook] 14:30:16 👤 Human:                        │
│                  │   Explore the auth module structure...            │
│                  │                                                   │
│                  │ [cook] 14:30:18 🤖 Assistant:                    │
│                  │   The auth module is organized as follows...      │
│                  │                                                   │
├──────────────────┴──────────────────────────────────────────────────-┤
│ j/k:navigate  Enter:select  Tab:focus  /:filter  ?:help              │
└─────────────────────────────────────────────────────────────────────-┘
```

### Panels

| Panel | Description |
|---|---|
| **Sidebar** (left) | Session list with agents as indented children. Shows session ID prefix (6 chars) and timestamp of last message. Active session highlighted. Toggleable with `b` key. |
| **Log Stream** (main) | Live message stream for the selected session. All agents interleaved chronologically with `[slug]` prefixes. Main agent messages have no prefix. |
| **Status Bar** (bottom) | Dynamic priority layout: active filters and inactive badge always shown; keyboard shortcuts shown when space permits. On narrow terminals, shortcuts are hidden first, then filter display truncated; inactive badge is always visible. |

### Sidebar Behavior

- Shows the **last 20 sessions** sorted by most-recently-active first. Older sessions are hidden (accessible via `--session` flag).
- Active session (the one being viewed) is highlighted with `●`
- Agents shown as indented children under each session
- **Sidebar shows full 3-word slugs** (e.g. `effervescent-soaring-cook`) — provides a reference for mapping to abbreviated `[cook]` prefixes in the log stream
- Navigate with `j`/`k`, select with `Enter`
- **New session notification**: when a new session starts, it appears in the sidebar with a visual highlight (e.g. bold or accent color) to draw attention. No auto-switch — the user decides when to switch. **Sidebar-only** — no status bar badge when sidebar is hidden. This is intentional to avoid notification overload.

### Sidebar Toggle

Press `b` to toggle sidebar visibility. Useful in narrow terminals. When hidden, the log stream takes the full terminal width.

### Log Stream Scroll Mode

When the log stream has focus, pressing `Up`/`k`, `PgUp`, `g`/`Home`, or scrolling the mouse wheel up **enters scroll mode**. In scroll mode:

- The log stream **freezes** at a snapshot of the current content. New entries continue to accumulate in the ring buffer but are not displayed until scroll mode exits.
- The title bar shows `[SCROLL mode - Esc:exit]` to indicate the frozen state.
- Use `Up`/`k`, `Down`/`j`, `PgUp`, `PgDn` to navigate within the snapshot.
- `g`/`Home` jumps to the top; `G`/`End`/`Esc` exits scroll mode and returns to live tailing.
- Mouse wheel scroll is also supported (scroll up enters scroll mode; scroll down navigates when already in scroll mode).
- Scroll mode is automatically exited when: the user switches sessions, applies a new filter, or presses `G`/`End`/`Esc`.

**Implementation**: uses a two-phase entry model. When scroll mode is first triggered, the render phase takes a snapshot of the current rendered lines and stores them in a `ScrollMode` struct. Subsequent scroll actions operate on this frozen snapshot without re-reading the ring buffer.

---

## Display

### Message Types Shown

| Log Type | Default Visibility | Display Format |
|---|---|---|
| `user` messages | Shown | `👤 Human: <text>` |
| `assistant` text | Shown | `🤖 Assistant: <text>` |
| Tool calls | Shown (summary) | `[Bash] cargo test auth` |
| `progress` entries | Hidden (toggleable with `p` key or `--verbose`) | `▶ Delegating: <task description>` |
| `file-history-snapshot` | Always hidden | — |
| Unknown content blocks | Shown (type + size) | `[thinking] (12.3KB)`, `[image] (png)` |

Unknown or unrecognized content block types (including `thinking`, `server_tool_use`, `image`) are rendered as a one-line type indicator with size only. No content preview. Size is calculated from the text length of the block's content. This is forward-compatible as Claude adds new block types.

### Content Block Rendering Order

Content blocks within a single message are rendered **in their original order** as they appear in the `message.content` array. Unknown blocks appear inline between text blocks as one-line type indicators. No reordering or collapsing.

### Tool Call Rendering

Tool calls are rendered as **input-only summaries** — a single line extracted from the `tool_use` input block. No result parsing. No progressive rendering or pending state tracking in v1.

When a `tool_use` block is encountered in the content array, cc-tail renders a single summary line from the tool input:

| Tool | Summary Format |
|---|---|
| Read | `[Read] src/main.rs` |
| Bash | `[Bash] cargo test auth` |
| Edit | `[Edit] src/lib.rs:42` |
| Write | `[Write] tests/new_test.rs` |
| Glob | `[Glob] **/*.rs` |
| Grep | `[Grep] "TODO" in src/` |
| Task | `[Task] Explore: "investigate log format"` |

All tool summaries are exactly one line regardless of the tool output size. Summaries are derived from `tool_use` input fields only (command, file path, pattern). **No tool_result parsing** — exit codes, line counts, and match counts are not extracted. If input extraction fails, fall back to just the tool name: `[Read]`.

### Visual Style

- **Timestamps** on every message (HH:MM:SS format)
- **Color coding**: ANSI 256-color palette with automatic fallback to 16-color for limited terminals. Blue for human messages, green for assistant, yellow for tool calls, dim for timestamps. Reasonable defaults — no hand-tuned palettes for v1, will iterate based on feedback.
- **Agent prefixes**: main agent messages have no prefix. Subagent messages prefixed with abbreviated `[slug]` (last word of the three-word slug, e.g. `[cook]` for `effervescent-soaring-cook`) in the log stream.
- **Per-agent colors**: assigned via **deterministic hash** of agentId/slug. Same agent always gets the same color across session switches. Uses a curated palette of 8 visually distinct colors. Hash collisions (two agents sharing a color) are accepted as rare — the `[slug]` prefix disambiguates.
- **Full message output** — no truncation of text content blocks, terminal wrapping handled by ratatui
- **Theme support**: `--theme dark` (default) and `--theme light`. Also settable in config file.
- **Inactive session indicator**: when the current session's log file has not been modified for 10+ minutes, the status bar shows a dim `inactive` badge.

### Agent Display in Log Stream

Within a single session, all agents (main + subagents) are interleaved chronologically:

```
14:30:12 👤 Human:
  fix the bug in the authentication module

14:30:15 🤖 Assistant:
  I'll investigate the authentication module.

14:30:16 [Read] src/auth/mod.rs
[cook] 14:30:16 👤 Human:
  Explore the auth module structure...

14:30:18 [Read] src/auth/jwt.rs
[cook] 14:30:18 🤖 Assistant:
  The auth module is organized as follows...

14:30:20 [Edit] src/auth/jwt.rs:45
14:30:22 [Bash] cargo test auth
```

Messages are displayed in **arrival order** — no reordering or buffering. The filesystem watcher delivers events, and they are rendered in the order received. Agent switches are distinguished by color and `[slug]` prefix alone — no separator lines.

---

## Internal Architecture

### Async / TUI Boundary

cc-tail uses a **channel-based architecture** to bridge the async file-watching world and the synchronous TUI render loop:

- **Async watcher tasks** (tokio) monitor JSONL files and parse new entries
- Parsed `LogEntry` structs are sent via **`tokio::sync::mpsc`** channel to the TUI thread
- **TUI thread** drains the channel each tick, appends entries to the ring buffer, and re-renders
- Clean separation: no locks on the hot rendering path, no shared mutable state between watcher and renderer

```
┌──────────────┐     mpsc channel     ┌──────────────┐
│ File Watcher │ ──── LogEntry ─────► │  TUI Thread  │
│ (tokio task) │                      │  (ratatui)   │
│              │                      │              │
│ File Watcher │ ──── LogEntry ─────► │ drain ch     │
│ (tokio task) │                      │ → ring buf   │
│    ...       │                      │ → render     │
└──────────────┘                      └──────────────┘
```

### Self-Debugging

In `--verbose` mode, cc-tail writes debug information to **stderr**. Since ratatui uses stdout for rendering, stderr remains available for diagnostics. Users redirect with `cc-tail --verbose 2>debug.log` to capture watcher events, parse failures, and session detection logic.

---

## Data Model

### LogEntry Storage

cc-tail stores parsed `LogEntry` structs (not pre-rendered lines) in a **byte-budget ring buffer** capped at **50MB**. This design enables:

- **Retroactive filtering**: when filters change, the entire buffer is re-rendered against the new filter criteria
- **Re-rendering on resize**: terminal size changes re-flow all buffered content
- **Theme switching**: changing themes re-renders all buffered content with new colors
- **Bounded memory**: large entries (e.g., 50KB code blocks) reduce the total count, while small entries allow more history. Memory stays predictable regardless of content mix.

Entries beyond the 50MB byte budget are evicted oldest-first. For typical sessions with average-sized messages, this provides ~thousands of entries (~hours of session activity).

### JSONL Parsing Strategy

- Use event-driven file watching with a **per-file byte cursor and incomplete-line buffer**: track a `u64` byte offset per file, read from last offset to EOF on each notify event, split on `\n`, buffer any trailing incomplete line until the next event
- **Malformed line handling**: silently skip any line that fails JSON parsing (whether truncated, corrupted, or truly malformed). In `--verbose` mode, write a warning to stderr with the parse error and truncated raw line content. This covers both incomplete writes and crash-corrupted entries uniformly.
- Use a **hybrid parsing model**: typed Rust struct with `#[serde(default)]` for known top-level fields (`type`, `sessionId`, `timestamp`, `message.role`, `isSidechain`, `agentId`, `slug`), and `serde_json::Value` for the `message.content` array. This provides type safety for common operations while remaining forward-compatible with Claude schema changes to content block types.
- No special handling needed for oversized entries — Claude Code caps tool output before logging

---

## Keyboard Shortcuts

Vim-style key bindings:

| Key | Context | Action |
|---|---|---|
| `j` / `k` | Sidebar focused | Navigate up/down in session/agent list |
| `j` / `Down` | Log stream focused | Scroll down (when in scroll mode) |
| `k` / `Up` | Log stream focused | Scroll up / enter scroll mode |
| `PgUp` | Log stream focused | Page scroll up / enter scroll mode |
| `PgDn` | Log stream focused | Page scroll down (when in scroll mode) |
| `g` / `Home` | Log stream focused | Scroll to top / enter scroll mode |
| `G` / `End` | Log stream focused | Exit scroll mode (return to live tail) |
| `Esc` | Log stream focused | Exit scroll mode (if active) |
| `Enter` | Sidebar focused | Switch to the highlighted session |
| `Tab` | Global | Toggle focus between sidebar and log stream |
| `/` | Global | Open filter input overlay |
| `b` | Global | Toggle sidebar visibility |
| `p` | Global | Toggle progress entry visibility (independent of `--verbose`) |
| `q` | Global | Quit cc-tail |
| `?` | Global | Show help overlay — static list of all keyboard shortcuts. No contextual info. |

### Filter Input (`/`)

Opens an overlay at the bottom of the screen:

```
┌─ Filter ────────────────────────────────────────────┐
│ Pattern: error|panic                                 │
│ Role: [all] [user] [assistant]                       │
│ Agent: [all] [main] [cook] [swift]                   │
│                                                      │
│ Enter:apply  Esc:cancel  Tab:next field              │
└──────────────────────────────────────────────────────┘
```

- **Pattern**: always interpreted as regex. Matched against all visible text — message content, tool names, file paths, command strings, rendered summaries. **Real-time validation**: the input border turns green for valid regex, red for invalid. This uses `Regex::new()` on each keystroke.
- **Role**: toggle between all/user/assistant
- **Agent**: toggle specific agents on/off. **Agent list is a snapshot** taken when the overlay opens. If a new agent spawns while the overlay is open, it won't appear until the overlay is reopened.
- Filters combine with AND logic
- Active filters shown in the status bar
- **Retroactive filtering**: changing filters re-evaluates the entire ring buffer, not just new messages
- Press `Esc` to cancel, `Enter` to apply

---

## Startup Behavior

### Session Auto-Attach

On launch, cc-tail auto-attaches to the most recently active session (determined by file modification time). Use `--session <id>` to attach to a specific session (prefix match supported).

### Session Replay

On startup, cc-tail replays the **last 20 visible messages** from the selected session to provide context, then continues with live tailing. The 20-message count is **total across all agents** (main + subagents combined), interleaved chronologically. "Visible" means messages that pass the current display filters.

**File reading strategy for replay**: full sequential scan of the JSONL file. For large files (100MB+), this may take 1-2 seconds on modern hardware — an acceptable brief pause for a monitoring tool.

### Session Switching

When switching sessions via the sidebar, cc-tail performs a fresh replay from the JSONL file (same as startup: last 20 visible messages total). No session state is cached — switching away and back re-reads the file. This keeps memory usage predictable.

### Active Session Detection

A session is considered "active" if its log file was modified within the last 10 minutes. This threshold accounts for pauses in ralph loops where the user may be reviewing output before continuing.

When all sessions go inactive, cc-tail continues displaying the current session's output with a dim `inactive` badge in the status bar. No auto-switch or overlay — the user sees stale output and understands it's stale.

### Empty State

If no active log files are found, the log stream panel displays:

```
Waiting for Claude Code sessions...
(watching ~/.claude/projects/-Users-foo-myproject/)
```

Continues watching indefinitely until log files appear or the user quits.

### Project Auto-Detection

1. Determine the current working directory
2. Convert to the escaped path format (replace `/` with `-`, strip leading `-`)
3. Check if `~/.claude/projects/<escaped-path>/` exists
4. If not found, walk up parent directories and try each (for when running from a subdirectory)
5. If still not found, detect the **git root** via `git rev-parse --show-toplevel`, convert to escaped path, and try that (covers monorepo cases where Claude was started from the repo root)
6. If still not found, **require `--project` flag** — do not guess or try to match all projects
7. If `--project` is specified, use that path directly
8. When multiple directories could match, pick the **most specific** (longest escaped path). **Strictly one directory** — no merging across project directories.

**Path collision note**: The escaped path algorithm can produce ambiguous results in rare cases (e.g., `/Users/foo/my-project` and `/Users/foo/my/project` both produce `Users-foo-my-project`). This is accepted as extremely rare — Claude Code itself has the same behavior.

---

## Filtering Architecture

Filters are set exclusively through the TUI at runtime (no CLI filter flags). They are combinable (AND logic when multiple are active).

### v1 Filters

| Filter | Description |
|---|---|
| Text content (regex) | Regex pattern matched against all visible text — message content, tool names, file paths, command strings, rendered summaries |
| Message role | Filter by role: `user`, `assistant`, or all |
| Agent name | Filter by agent: `main`, specific subagent slugs, or all |

### Internal Design

Filters implement a Rust trait:

```rust
trait MessageFilter: Send + Sync {
    fn matches(&self, entry: &LogEntry) -> bool;
}
```

Combined via `AndFilter` / `OrFilter` wrapper structs. New filters are added by implementing the trait. Filter changes trigger a full re-render of the ring buffer.

### Future Filters (v2+)

- Filter by model (opus/haiku/sonnet)
- Filter by entry type (user/assistant/tool/progress)
- Inverse text filter (exclude regex)
- Filter expression language (e.g. `role:assistant AND model:opus`)

---

## File Watching

### Strategy

Watch the entire `~/.claude/projects/<project>/` directory tree **recursively** using the `notify` crate (**native watchers only**: FSEvents on macOS, inotify on Linux). Accept the ~1-2 second FSEvents coalescing latency — this is a monitoring tool, not an interactive one, and the delay is acceptable for the use case. No polling fallback for exotic filesystems (NFS/FUSE/Docker volumes are unsupported).

Only process files matching the `*.jsonl` glob pattern — ignore memory files, config files, and other non-log files that Claude Code stores in the same directory.

### File Read Strategy

On each notify event for a watched file:

1. Read from the tracked byte offset to EOF
2. Split on `\n`
3. Parse each complete line as JSON
4. Buffer any trailing incomplete line (no terminating `\n`) for the next event
5. Update the byte offset

This handles the race condition where cc-tail reads a file mid-write by Claude Code. Incomplete lines are naturally completed on the next write event.

### New Subagent Detection

When a new `*.jsonl` file appears under `{sessionId}/subagents/`, cc-tail:
1. Detects the new file via the recursive watcher — **immediately adds** the subagent to the sidebar and begins tailing. No waiting for correlation with the parent's Task tool_use block.
2. Begins tailing the new file for live updates

---

## Error Handling

### Malformed JSONL Lines

- In normal mode: silently skip any line that fails JSON parsing (don't crash on bad data)
- In `--verbose` mode: write a warning to stderr with the parse error and truncated raw line content
- Wait for complete lines (newline-terminated) before attempting to parse

### Signal Handling

- **SIGINT (Ctrl+C)**: graceful shutdown — restore terminal state, exit
- **SIGTERM**: same as SIGINT
- **SIGKILL**: terminal state may be corrupted — user runs `reset` to fix

---

## Configuration

### File Location

`~/.config/cc-tail/config.toml` (XDG-compliant). cc-tail **does not create** a default config file — if no file exists, hardcoded defaults are used. Users create the file manually when they want to customize.

### Config Parsing

Unknown keys in the config file are **silently ignored**. This provides forward compatibility — old config files work with new versions that add keys, and typos in key names silently fall through to defaults.

### Default Configuration

```toml
# Default verbosity (show progress entries)
verbose = false

# Color theme: "dark" or "light"
theme = "dark"

[display]
# Show timestamps
timestamps = true
# Timestamp format
timestamp_format = "%H:%M:%S"
```

CLI flags override config file values.

---

## Technical Stack

| Component | Choice | Rationale |
|---|---|---|
| Language | Rust | Single binary, fast startup, strong ecosystem for TUI and async |
| TUI framework | `ratatui` + `crossterm` | De facto standard for Rust TUI apps. Handles layout, widgets, terminal state management. |
| Filesystem watching | `notify` crate | Native watchers only (FSEvents on macOS, inotify on Linux). No polling fallback. |
| JSON parsing | `serde` + `serde_json` | Hybrid model: typed structs for top-level fields, `serde_json::Value` for content blocks |
| CLI parsing | `clap` | Derive macros for flags and subcommands, auto-generated help |
| Async runtime | `tokio` | Concurrent file watching, channel-based communication with TUI thread |
| Config parsing | `toml` crate | Native TOML support |
| Regex | `regex` crate | For text content filtering with real-time validation |

### Platform Support

- **macOS** (aarch64, x86_64): primary target
- **Linux** (aarch64, x86_64): supported
- **Windows**: not supported. WSL users can use the Linux binary.

### Rust Version

Target **latest stable** Rust. No MSRV guarantee. Document in README.

---

## Testing Strategy

### Unit Tests

- JSONL parser: validate extraction of all known fields from hybrid struct + `serde_json::Value`
- Tool summarizer: verify one-line input-only summaries for each tool type, including fallback when input extraction fails
- Filter logic: test `MessageFilter` trait implementations (regex, role, agent) and combinators
- Project path detection: test escaped path conversion, parent walking, git root fallback
- Ring buffer: test byte-budget eviction, re-filtering after filter change

### Integration Tests

- Feed synthetic JSONL files and verify parsed `LogEntry` sequences
- Test incomplete-line buffering with partial writes
- Test session discovery and subagent detection
- Test `cc-tail stream` output format against known JSONL input (both TTY and piped modes)

**No file watcher tests** — unit test the parsing pipeline only. The `notify` crate's behavior with filesystem timing is inherently platform-dependent and flaky in CI. Manual testing validates watcher integration.

No TUI rendering tests in v1 — trust ratatui for rendering correctness, test the data pipeline.

---

## Distribution

### cargo install

Publish to crates.io as `cc-tail` (name verified available):
```
cargo install cc-tail
```

Requires Rust toolchain on the user's machine.

### Pre-built Binaries

Publish on GitHub Releases using **GitHub Actions with native runners** (macOS + Linux matrix, no cross-compilation):
- `cc-tail-x86_64-apple-darwin`
- `cc-tail-aarch64-apple-darwin`
- `cc-tail-x86_64-unknown-linux-gnu`
- `cc-tail-aarch64-unknown-linux-gnu`

---

## Scope: v1 vs Future

### v1 (Initial Release)

- Full TUI with sidebar (session/agent list) + log stream + status bar
- `cc-tail stream` subcommand for lightweight single-file tailing (public, documented)
- Auto-detect TTY for emoji (emoji in terminal, ASCII `[H]`/`[A]` when piped)
- Channel-based async architecture (tokio mpsc → TUI thread)
- Single-session focus with session switching via sidebar
- Sidebar shows last 20 sessions (sorted by recency) with full 3-word agent slugs
- Abbreviated `[slug]` prefixes (last word) in the log stream
- Vim-style keyboard navigation
- Runtime-changeable filters (text regex, role, agent) via `/` overlay with real-time regex validation
- Agent list in filter overlay is a snapshot (taken when overlay opens)
- Retroactive filtering over byte-budget ring buffer (50MB cap)
- Inline interleaved agent output in arrival order (no chronological reordering)
- Per-agent colors via deterministic hash (8-color curated palette)
- Input-only tool call summaries (no tool_result parsing, no pending state)
- Content blocks rendered in original array order (unknown blocks inline)
- Progress entry toggle via `p` key (independent of `--verbose`)
- Project auto-detection from cwd with parent-walk and git-root fallback + `--project` override (strict cwd match, single directory)
- Session auto-attach (most recently active) + `--session` override
- Session replay on startup and switch (last 20 visible messages total across all agents, full file scan)
- Immediate subagent detection (no correlation with parent Task tool_use)
- Unknown content block type indicators (type + size only, no content preview)
- New session notification in sidebar only (visual highlight, no status bar badge)
- Inactive session indicator in status bar (10-minute threshold)
- Dark/light theme support with reasonable defaults (no hand-tuned palettes)
- Full text output (no truncation of long content blocks)
- Toggleable sidebar
- Scroll mode (freeze log stream, navigate history with keyboard/mouse)
- Static help overlay (`?` key) — shortcuts only
- Dynamic priority status bar (filters > shortcuts on narrow terminals)
- Config file (`~/.config/cc-tail/config.toml`) — not auto-created, unknown keys ignored
- Verbose mode writes debug info to stderr
- Silent skip of malformed JSONL lines
- Hybrid JSONL parsing (typed top-level struct + Value for content blocks)
- Native filesystem watchers only (no polling fallback)
- Signal handling (SIGINT + SIGTERM cleanup)
- Unit + integration test suite (no watcher tests, no TUI rendering tests)
- macOS + Linux only (no Windows)
- CI via GitHub Actions with native runners (macOS + Linux matrix)
- Distribution via cargo install + GitHub Release binaries
- Latest stable Rust, no MSRV guarantee

### Future (v2+)

- Progressive tool call rendering (show pending state, update on result)
- Tool result parsing (exit codes, line counts, match counts)
- `--raw` / streaming mode for non-TTY output (pipe-friendly)
- Additional filters: `--model`, `--type`, `--exclude`
- Filter expression language
- Desktop/terminal notifications
- Collapsible tool call detail (expand to see full input/output)
- Session timeline view / summary mode
- Process-based active session detection (supplement mtime)
- Configurable replay count
- Config hot-reloading
- `cc-tail sessions` subcommand (list sessions as JSON for scripting)
- `cc-tail init` subcommand (generate default config file)
- First-human-message subtitle in sidebar session list
- Hand-tuned ANSI 256-color palettes per theme
- Status bar badge for new sessions when sidebar is hidden
- Polling fallback for exotic filesystems
- Windows support
