# AI Assistant Usage Overlay

A Windows desktop overlay (like NVIDIA's GPU overlay) showing live usage of Claude Code, Codex CLI, and Hermes: tokens, context-window %, status, memory. Transparent, always-on-top, draggable. Read-only — no API keys, no network.

## Build Requirements

- Rust 1.70+ (via rustup)
- Node.js 18+ (for frontend tooling if needed)
- Windows 10/11 (native Windows build; WebView2 is included with Windows 10+)

## Development

### Prerequisites

Install Rust via [rustup](https://rustup.rs/):
```powershell
winget install Rustlang.Rustup
```

### Run in Dev Mode

From the project root:
```powershell
cd src-tauri
cargo tauri dev
```

This will:
1. Build the Rust backend
2. Launch the transparent overlay window
3. Watch for file changes

### Build Release

```powershell
cd src-tauri
cargo tauri build
```

The installer will be at `src-tauri/target/release/bundle/msi/` or `.exe`.

## Configuration

Config file: `%APPDATA%\ai-usage-overlay\config.toml`

```toml
poll_interval_ms = 1000
opacity = 0.85
hotkey = "Ctrl+Shift+Space"
enabled_agents = ["claude", "codex", "hermes"]
# hermes_data_dir = "C:/Users/you/AppData/Local/hermes"
```

## Features

- **Claude Code**: Reads `~/.claude/sessions/*.json` and transcripts from `~/.claude/projects/`
- **Codex CLI**: Reads today's rollouts from `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, including live 5h/weekly rate-limit windows from `token_count` events
- **Hermes**: Placeholder — needs data format specification

### 5-hour / weekly usage limits

Codex reports its own 5h/weekly rate-limit usage inside its transcript, so it shows up automatically. Claude Code does not expose this in the transcript — to see it here, configure a Claude Code [StatusLine hook](https://docs.claude.com/en/docs/claude-code/statusline) that writes `~/.claude/abtop-rate-limits.json`:

```json
{
  "five_hour": { "used_percentage": 42.0, "resets_at": 1730000000 },
  "seven_day": { "used_percentage": 18.5, "resets_at": 1730500000 }
}
```

Without that file, the rate-limit row is simply omitted for Claude sessions.

## Key Bindings

| Key | Action |
|-----|--------|
| `Ctrl+Shift+Space` | Toggle show/hide |
| Drag titlebar | Move window |
| Opacity slider | Adjust transparency |
| Tray icon | Show/Hide/Quit |

## Manual QA Checklist

Run with at least one `claude` or `codex` session active:

- [ ] Panel appears top-left on launch, transparent, above other windows
- [ ] Stays on top of fullscreened browser/editor
- [ ] Dragging the title bar moves the window
- [ ] Opacity slider updates transparency live
- [ ] `Ctrl+Shift+Space` toggles show/hide
- [ ] Session rows update ~1s; tokens accumulate as agents work
- [ ] Status changes (Waiting/Thinking/Executing) reflect agent activity
- [ ] Closing an agent session removes its row within ~1s
- [ ] Quitting via tray exits cleanly
- [ ] With no agents running, panel shows "No active AI assistants"
- [ ] Deleting config file → app starts with defaults

## Architecture

- **Rust core** (`src-tauri/src/`): Collectors read local AI assistant state, tick loop produces snapshots
- **Tauri shell**: Transparent frameless topmost window, tray icon, global hotkey
- **Frontend** (`dist/`): Vanilla JS/CSS rendering snapshot events

## License

MIT
