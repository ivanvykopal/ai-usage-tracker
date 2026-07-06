# AI Assistant Usage Overlay

A Windows desktop overlay (like NVIDIA's GPU overlay) showing live usage of Claude Code, Codex CLI, and Hermes: tokens, context-window %, status, memory. Transparent, always-on-top, draggable. Read-only and local-first — no new API keys, and only one outbound network call (Claude's account-level usage, see below; opt-out available).

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
claude_usage_enabled = true
# hermes_data_dir = "C:/Users/you/AppData/Local/hermes"
```

## Features

- **Claude Code**: Reads `~/.claude/sessions/*.json` and transcripts from `~/.claude/projects/`
- **Codex CLI**: Reads today's rollouts from `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, including live 5h/weekly rate-limit windows from `token_count` events
- **Hermes**: Placeholder — needs data format specification

### 5-hour / weekly / monthly usage limits

A usage-limit bar at the top of the panel always shows each provider's
account-level quota usage, independent of whether a session is currently
running. Only the windows a provider actually reports are shown — e.g. a
Codex free-plan account only has a monthly window, so only "month" appears
for it; a plan with 5h/weekly windows shows those instead.

- **Codex** reports this in its own transcript, so it appears automatically
  once you've used Codex at least once that day. Which window(s) show up
  (5h, weekly, monthly) depends on your plan — the free plan reports only a
  single ~30-day "monthly" window.
- **Claude** usage is fetched directly from Anthropic's own usage API
  (`api.anthropic.com/api/oauth/usage`), reusing the OAuth token Claude Code
  already stores in `~/.claude/.credentials.json` after you log in — no new
  credentials, no API key. **This is the one network call this app makes;**
  everything else is fully local and read-only. It polls every 3 minutes and
  only ever talks to Anthropic's own endpoint.
  - To disable this and keep the app strictly network-free, set
    `claude_usage_enabled = false` in `config.toml`. Claude's usage row will
    then only populate if you separately configure a StatusLine hook to write
    `~/.claude/abtop-rate-limits.json`:
    ```json
    {
      "five_hour": { "used_percentage": 42.0, "resets_at": 1730000000 },
      "seven_day": { "used_percentage": 18.5, "resets_at": 1730500000 }
    }
    ```
  - If you're on Bedrock/Vertex/API-key auth (no OAuth token present), the
    app automatically falls back to the hook file regardless of this setting.
- A provider row is simply omitted until usage data has been obtained at
  least once — no "N/A" placeholders.

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
