# cctail

A TUI application for monitoring Claude Code sessions in real-time.

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

### Key Bindings

| Key | Action |
|-----|--------|
| `j`/`k` | Navigate sidebar / scroll log |
| `Enter` | Select session |
| `Tab` | Toggle focus |
| `/` | Open filter overlay |
| `b` | Toggle sidebar |
| `p` | Toggle progress entries |
| `t` | Spawn tmux panes |
| `?` | Help |
| `q` | Quit |

## License

MIT
