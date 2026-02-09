# cctail

A TUI application for monitoring Claude Code sessions in real-time.

## Installation

### From crates.io

```
cargo install cctail
```

### Pre-built binaries

Download from [GitHub Releases](https://github.com/kenfdev/cc-tail/releases):

- `cctail-x86_64-apple-darwin` (macOS Intel)
- `cctail-aarch64-apple-darwin` (macOS Apple Silicon)
- `cctail-x86_64-unknown-linux-gnu` (Linux x86_64)
- `cctail-aarch64-unknown-linux-gnu` (Linux ARM64)

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
