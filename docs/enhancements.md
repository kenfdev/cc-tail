# cc-tail Enhancement Spec

## Overview

This spec covers the next round of enhancements for cc-tail, focused on making the log viewer more useful, more intuitive, and more enjoyable to use. The goal: someone discovers cc-tail, tries it, and posts it on Reddit because they genuinely love it.

---

## Feature 1: Full History Load (`L` key)

### Problem

cc-tail uses a ring buffer with a size limit. Older log entries are dropped as new ones arrive, meaning users can't scroll back to see what happened earlier in a long session.

### Behavior

- **Trigger**: Press `L` to load the full session history from the JSONL file on disk.
- **Default**: On startup, cc-tail continues to use the current ring buffer behavior (stream recent entries).
- **On load**: Read the entire JSONL log file and replace the ring buffer contents with all entries. The user's current viewport position is preserved (they are not auto-scrolled).
- **Visual feedback**: While loading, show an inline indicator (e.g., `Loading full history...`) in the log area. Once complete, show a brief confirmation (e.g., `Loaded 1,842 entries`).
- **Memory**: Load the full file into memory. JSONL session files are typically under 10MB. If the file is exceptionally large (>50MB), show a warning before loading and let the user confirm.
- **Interaction with search**: Once fully loaded, search operates on all loaded entries.

### Key binding

| Key | Action |
|-----|--------|
| `L` | Load full session history from disk |

---

## Feature 2: Search (`/` key)

### Problem

There's no way to find specific content in the log. Users have to visually scan through potentially thousands of lines.

### Behavior

- **Trigger**: Press `/` to open search mode.
- **Input**: A single-line input bar appears at the **bottom** of the screen (vim/less style). The prompt shows `/` followed by the user's typed query.
- **Matching**: Plain text substring matching (case-insensitive). No regex support.
- **Scope**: Search operates only on **visible** (non-filtered) content. If tool calls are hidden by the filter, search does not match against hidden content.
- **Highlighting**:
  - All matches on screen are highlighted with a background color (e.g., dark yellow/amber background).
  - The **current** match (the one the cursor is on) uses a **distinct, brighter** highlight (e.g., bright yellow background with bold text) to differentiate it from other matches.
- **Navigation**:
  - `n` moves to the next match (forward).
  - `N` moves to the previous match (backward).
  - Scrolling uses **vim-style scrolloff**: the matched line is kept a few lines (3-5) from the top/bottom edge of the viewport, not hard-pinned to center.
  - Wraps around: going past the last match wraps to the first, and vice versa.
- **Match counter**: Show a counter in the bottom bar, e.g., `[3/17]` indicating the current match index out of total matches.
- **Exit**: Press `Escape` to exit search mode **and** clear all highlights immediately. The viewport stays where it is.
- **Empty query**: If the user presses Enter with no query, search mode exits with no effect.

### Key bindings

| Key | Action |
|-----|--------|
| `/` | Open search input bar |
| `Enter` | Confirm search query, jump to first match |
| `n` | Jump to next match |
| `N` | Jump to previous match |
| `Escape` | Exit search, clear all highlights |

---

## Feature 3: Filter Menu (`f` key)

### Problem

The current filter UX is obscure. Users don't understand what filtering options are available or how to use them. The existing word-based filter is more complex than needed.

### Behavior

- **Trigger**: Press `f` to open the filter menu.
- **Menu style**: A **center overlay** modal (consistent with other popups in cc-tail). The background is slightly dimmed.
- **Menu options**:
  1. **Hide/Show Tool Calls** (binary toggle)
     - When enabled, all tool use entries (lines with `~` indicator) are completely hidden from the view.
     - No collapsed indicator or trace is left behind -- the view shows only conversation text cleanly.
  2. **Filter by Agent** (when subagents are present)
     - Shows a list of agents detected in the current session (main agent + any subagents with their slug names).
     - Selecting an agent shows **only** entries from that agent.
     - An "All Agents" option resets the filter.
     - This option only appears in the menu when the session actually contains subagent entries.
- **Interaction with search**: Active filters affect search scope. Search only matches against visible (non-filtered) content.
- **Visual indicator**: When any filter is active, show a subtle indicator in the bottom status area (e.g., `[filter: no tools]` or `[filter: agent cook]`).
- **Exit menu**: Press `Escape` or `f` again to close the menu without changing anything.

### Key bindings

| Key | Action |
|-----|--------|
| `f` | Open/close filter menu |
| `Up/Down` or `j/k` | Navigate menu options |
| `Enter` | Select/toggle the highlighted option |
| `Escape` | Close menu without changes |

---

## Feature 4: Improved Help Screen (`?` key)

### Problem

Users don't know what the symbols (`>`, `<`, `~`, `?`) mean or what colors represent. The current help screen (if any) doesn't explain the visual language of the UI.

### Behavior

The `?` key opens a help overlay with **three sections**:

#### Section 1: Symbol & Color Legend

| Symbol | Color | Meaning |
|--------|-------|---------|
| `>` | Blue | User message |
| `<` | Green | Assistant (Claude) message |
| `~` | Yellow | Tool call |
| `?` | Gray | Unknown message type |
| `▶` | Dark Gray | Progress update |

Also explain:
- Agent prefixes like `[cook]` appear for subagent messages, with distinct colors per agent.
- Timestamps are shown in `HH:MM:SS` format in dark gray.

#### Section 2: Key Bindings

A complete reference of all keybindings, including:

| Key | Action |
|-----|--------|
| `?` | Toggle help screen |
| `/` | Search |
| `n` / `N` | Next / previous search match |
| `f` | Open filter menu |
| `L` | Load full session history |
| `j` / `k` | Scroll down / up |
| `Ctrl+d` / `Ctrl+u` | Half-page down / up |
| `g` / `G` | Jump to top / bottom |
| `q` | Quit |
| (other existing bindings) | (as applicable) |

#### Section 3: Session Stats

Display computed stats about the current session:

- **Session duration**: Time between first and last entry (e.g., `Duration: 14m 32s`)
- **Message count**: Total user messages, assistant messages (e.g., `Messages: 8 user, 12 assistant`)
- **Tool calls**: Total tool call count with breakdown by tool name (e.g., `Tools: 47 total (Read: 18, Edit: 12, Bash: 9, Grep: 5, other: 3)`)
- **Agents**: Number of subagents spawned (e.g., `Agents: 3 subagents`)
- **Log entries**: Total entries loaded (e.g., `Entries: 342 loaded (of 1,842 total)` -- showing buffer vs full file)

### Key bindings

| Key | Action |
|-----|--------|
| `?` | Toggle help screen |
| `Escape` | Close help screen |

---

## Feature 5: Unicode with ASCII Fallback

### Problem

Some terminals don't render Unicode characters properly.

### Behavior

- **Default**: Use Unicode characters where they improve the UI (e.g., `▶` for progress).
- **Fallback**: Detect terminal capability or provide a `--ascii` CLI flag. When ASCII mode is active, replace Unicode characters with ASCII equivalents:
  - `▶` becomes `>`
  - Box-drawing characters (`─`, `┄`) become `-`
  - Any other Unicode decorations use ASCII equivalents
- **Detection**: If possible, check the `TERM` or `LANG` environment variables. If uncertain, default to Unicode (modern terminals handle it fine).

---

## Design Principles

These principles apply across all features:

1. **Inline over chrome**: Status information flows inline with content rather than occupying dedicated UI bars (exception: search input bar at bottom, which is temporary).
2. **Vim-familiar**: Key bindings follow vim/less conventions where possible (`/`, `n`, `N`, `j`, `k`, `g`, `G`).
3. **Filter and search are independent layers**: Filters control what's visible; search operates on the visible set. They compose cleanly.
4. **Minimal by default, powerful on demand**: The default view stays clean. Power features (full load, filter, search) are one keypress away but don't clutter the default experience.
5. **Center overlay for menus**: All modal interactions (help, filter menu) use center-overlay style for consistency.

---

## Out of Scope (Considered and Deferred)

The following ideas were discussed and intentionally excluded from this round:

| Idea | Reason |
|------|--------|
| Inline breadcrumbs (e.g., "-- reading files --") | cc-tail only reads logs; can't derive intent from tool patterns reliably |
| Syntax highlighting for code blocks | Complexity vs. value tradeoff; lower priority |
| Session timeline minimap | Implementation effort too high for this round |
| File tree heatmap | Not aligned with cc-tail's core purpose as a log viewer |
| Cost/token tracking | JSONL logs don't contain usage data; unreliable to estimate |
| Regex search | Plain text covers 90% of use cases; avoids confusion |
| Per-turn cost annotations | No token data in logs (see cost tracking above) |
| Status line / persistent bar | User prefers inline information over dedicated UI chrome |

---

## Implementation Priority

Suggested order based on user impact and dependency chain:

1. **Help screen improvements** (low effort, immediately improves onboarding)
2. **Filter menu** (replaces confusing current filter, enables agent filtering)
3. **Search** (high-value feature, depends on clear filter semantics)
4. **Full history load** (unblocks search over full sessions)
5. **ASCII fallback** (polish, can be done anytime)
