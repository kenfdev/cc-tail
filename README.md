# cctail

[![Crates.io](https://img.shields.io/crates/v/cctail)](https://crates.io/crates/cctail)
[![CI](https://github.com/kenfdev/cc-tail/actions/workflows/ci.yml/badge.svg)](https://github.com/kenfdev/cc-tail/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A TUI application for monitoring Claude Code sessions in real-time.

<!-- TODO: Add demo GIF using vhs or asciinema -->

## Features

- Real-time monitoring of Claude Code sessions with auto-detection
- Interactive search with match highlighting and n/N navigation
- Filter by agent or hide tool calls
- Session sidebar with subagent tree view
- Full session history load on demand (L)
- Dark/light themes, ASCII fallback (`--ascii`)
- Lightweight `stream` subcommand for piping
- Config file support (`~/.config/cc-tail/config.toml`)
- Help overlay with keybindings, symbol legend, and live session stats

## Installation

### From crates.io

```
cargo install cctail
```

### Quick install (Linux / macOS)

```
curl -fsSL https://raw.githubusercontent.com/kenfdev/cc-tail/main/install.sh | sh
```

Or with a custom install directory:

```
curl -fsSL https://raw.githubusercontent.com/kenfdev/cc-tail/main/install.sh | INSTALL_DIR=~/.local/bin sh
```

## Usage

```
# Launch TUI (auto-detects project and session)
cctail

# Attach to a specific session
cctail --session <id>

# Lightweight streaming mode (single file)
cctail stream --file <path/to/session.jsonl>
```

## Key Bindings

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate sidebar / scroll log |
| `Enter` | Select session |
| `Tab` | Toggle focus (sidebar / log) |
| `b` | Toggle sidebar |
| `/` | Search |
| `n` / `N` | Next / previous search match |
| `f` | Filter menu |
| `L` | Load full session history |
| `u` / `d` | Half-page up / down |
| `g` / `G` | Go to top / bottom |
| `Esc` | Exit mode / close overlay |
| `?` | Help overlay |
| `q` | Quit |

## Configuration

cctail reads an optional config file from `~/.config/cc-tail/config.toml`.
CLI flags override config file values.

```toml
# General
verbose = false
theme = "dark"      # "dark" or "light"
ascii = false       # Use ASCII instead of Unicode symbols

# Display
[display]
timestamps = true
timestamp_format = "%H:%M:%S"
```

## Development

```
make setup
```

This configures git to use the shared hooks in `.githooks/` (auto-format with `cargo fmt`, lint with `cargo clippy` on every commit).

## License

MIT
