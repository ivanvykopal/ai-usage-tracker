# AI Assistant Usage Overlay — Design

**Date:** 2026-06-30
**Status:** Approved (pending implementation plan)
**Reference:** [abtop](https://github.com/graykode/abtop) — a Rust TUI that monitors AI coding-agent sessions from local file/process state. This project reuses abtop's *collection approach* but presents it as a desktop overlay instead of a terminal app, and is fully self-contained (no abtop dependency).

## 1. Goal

A Windows desktop application, modeled on the NVIDIA GPU usage overlay, that shows live usage of AI coding assistants — **Claude Code, Codex CLI, and Hermes** — as a transparent, always-on-top, draggable floating panel. It runs in the background, surfaces token usage / context-window % / status / memory for every active assistant session at a glance, and stays visible over other windows.

## 2. Non-goals (YAGNI)

- Not a terminal TUI (abtop already is one). The value here is the desktop overlay.
- No API keys, no authentication, no network calls. All data comes from local files and local process/open-port metadata — read-only.
- No editing or modifying of the assistants' files or state.
- No macOS/Linux support in v1 (transparent always-on-top overlays are most native on Windows; cross-platform is out of scope for now).
- No OpenCode support in v1 (abtop covers it; deferred — three agents is the v1 target).
- No historical analytics/dashboards across days. v1 is a live readout.
- No web/cloud sync.

## 3. Key decisions (from brainstorming)

| Decision | Choice | Rationale |
|---|---|---|
| Target OS | Windows only | Cleanest path to a polished transparent always-on-top overlay; user is on Windows/WSL2. |
| Agents | Claude Code, Codex CLI, Hermes | Hermes is a CLI with local on-disk data, so it can be tracked the same way as the others. |
| Data source | Self-contained reimplementation of abtop's collectors | One binary, no abtop dependency, native Hermes support. Port abtop's Rust logic directly to minimize reimplementation bugs. |
| Overlay shape | Floating, draggable, semi-transparent, always-on-top panel + system tray + global hotkey | User-selected. Interactive (clicks work), toggle show/hide from tray. |
| Stack | Tauri v2 (Rust core + web UI) | Closest to abtop: its Rust collectors port almost directly. Tiny native binary via WebView2. |

## 4. Architecture & components

A Tauri v2 desktop app in three layers:

- **Rust core** (`src-tauri/src/`) — collectors that read local AI-assistant state, plus a tick loop that refreshes them into a `Snapshot`.
- **Tauri shell** — one transparent, frameless, always-on-top, draggable window; a system-tray icon; a global hotkey.
- **Web frontend** (`src/`) — a small TS/CSS panel that renders the snapshot and handles drag/interactions.

### Process model

One thread owns the collectors (mirrors abtop's `App`, which is `!Send`); a background tick thread refreshes them every ~1s and emits `snapshot://update` events; the frontend is passive — it renders events and sends commands for user actions.

### Rust core components

Each unit has one job and a clear interface; `transcript` and `collector` carry no Tauri dependency so they are unit-testable in isolation.

| Unit | Responsibility | Depends on |
|---|---|---|
| `model` | Plain data: `AgentSession` (agent, pid, project, started_at, status, model, context %, token counts, turn_count, current_task, mem_mb, git_branch), `SessionStatus` enum, `Snapshot` (sessions + aggregates). | — |
| `collector` trait | `fn collect(&mut self, ctx) -> Vec<AgentSession>` — the single extension point for every agent. | `model`, `process`, `transcript` |
| `ClaudeCollector` | Scans `~/.claude/sessions/*.json` → `projects/<encoded-cwd>/<sid>.jsonl`; incremental transcript parse with offset cache. Ported from abtop `collector/claude.rs`. | trait, `transcript` |
| `CodexCollector` | Codex CLI's local session/transcript paths, ported from abtop `collector/codex.rs`. | trait, `transcript` |
| `HermesCollector` | Same trait; data root + format from config. Parser written against Hermes's real on-disk format (confirmed from a sample during implementation). | trait, `transcript` |
| `process` | `sysinfo` wrapper: pid→(cmd, rss, cpu), parent/child map — drives status + memory. | `sysinfo` |
| `transcript` | Pure JSONL parser: path + last offset → token deltas, context tokens, current_task, model, status signals. No state beyond the file. | `serde_json` |
| `app` | Owns collectors + tick loop. `tick()` (~1s) refreshes sessions; `tick_slow()` (~10s) refreshes config-dir discovery + git. Produces `Snapshot`. | collectors, `process` |
| `config` | Load/save `config.toml` (poll interval, opacity, window position, hotkey, enabled agents, Hermes path). | `toml`, `dirs` |

### Shell + frontend components

| Unit | Responsibility |
|---|---|
| `window` (Tauri) | transparent/frameless/topmost/skipTaskbar window; `startDragging` on title bar; tray icon (Show/Hide, Settings, Quit); global shortcut to toggle. |
| `frontend` (TS) | subscribe to `snapshot://update`; render session rows (agent dot, project, status, context bar, tokens); drag handle; settings panel. Lean vanilla TS + CSS, no heavy framework. |

The key boundary: `transcript` and `collector` are pure logic with no Tauri dependency, so they're unit-testable in isolation. `app` is the only thing that wires collectors to the tick loop and event channel.

## 5. Data flow & error handling

### Startup

Tauri builds the window (transparent, frameless, always-on-top, skips taskbar), creates the tray, registers the global hotkey, and spawns the tick thread. `app::App` is constructed with enabled collectors; the first `tick()` runs immediately so the panel isn't empty on launch.

### Tick loop

A background thread loops:

1. `process::snapshot()` — one `sysinfo` refresh yields the pid table, then derives the parent/child map, RSS/CPU per pid, and `netstat -ano` output → listening ports per pid.
2. Each enabled collector's `collect(&process_ctx)` runs **sequentially** (collectors mutate shared `sysinfo` state; abtop's `App` is `!Send` for this reason — same single-threaded discipline here).
3. `app` merges results, dedupes by `(agent, pid)` / by session_id, drops sessions whose PID died, sorts by `started_at` desc.
4. `app` computes aggregates: total live tokens, tokens-per-agent, count by status.
5. Emits `snapshot://update` via Tauri's event channel → frontend re-renders.
6. Sleeps `poll_interval` (default 1s). Every 10th iteration runs `tick_slow()` to re-discover Claude config dirs and refresh git status (expensive, throttled exactly like abtop).

**Why sequential, not parallel collectors:** the collectors share the `sysinfo` `System` handle (refreshing it for one would invalidate another's view) and the open-files view. abtop keeps `App` on one thread for this reason. We do the same; 1s cadence leaves plenty of headroom.

### Frontend → Rust commands (Tauri IPC)

- `toggle_pin` (keep panel above / let it fall behind)
- `set_opacity(value)`, `set_position(x,y)`, `set_poll_interval(ms)`
- `open_settings`, `hide_panel`, `quit`
- Drag is handled client-side (`getCurrent().window.startDragging()`) — no IPC per mouse-move.

### Error handling — graceful degradation, never crash the panel

- **A collector throws or a path is missing** → that collector returns `[]` for this tick and logs; other collectors still render. A collector that fails 5 consecutive ticks (≈5s at the default interval) is marked "unavailable" and the panel shows a muted row ("Hermes — location not found") rather than disappearing; it auto-recovers the moment a tick succeeds.
- **A single transcript line is malformed** → skip the line, keep parsing the rest (abtop does this — one bad line can't kill the session). Parse errors are counted, not fatal.
- **A session file disappears mid-read** (e.g. `/clear`, session ended) → `fs::read` returns `Err`, we drop the session this tick. The offset cache is evicted when the session_id leaves the active set (ported from abtop's `evict_stale_cache`).
- **File truncated/replaced** → abtop detects via file identity (inode/mtime+size); on change it re-parses from offset 0. We port this — otherwise counters corrupt.
- **`sysinfo`/`netstat` transient failure** → reuse the last good process snapshot for that tick; don't blank the panel.
- **Hotkey registration fails** (already taken) → log a clear message; tray "Show/Hide" still works. Don't block startup.
- **Config file corrupt/missing** → fall back to defaults, write a fresh config, never refuse to start.

**Guiding principle (also abtop's):** read-only, never modify the assistants' files or state, never make API calls, never crash on malformed local data.

## 6. Testing strategy

abtop's collection logic is intricate (incremental JSONL parsing with offset caching, file-identity change detection, status heuristics) and we're porting it — so testing the pure logic is where the value is. The strategy mirrors the Section 4 boundaries.

### Unit tests (pure logic, no Tauri, no filesystem required at runtime)

- **`transcript` parser** — the highest-value tests. Feed canned JSONL fixtures (representing real Claude/Codex/Hermes transcript lines) and assert:
  - Token deltas accumulate correctly across incremental reads (offset → delta → merge).
  - File-identity change triggers a full re-parse from offset 0 (no corrupted counters).
  - File shrink/truncation is handled (abtop's `delta.new_offset < from_offset` branch).
  - A single malformed line is skipped, the rest parse fine.
  - Status signals derived correctly: `last_user_ts_ms > 0` → Thinking; pending `tool_use` → Executing; else Waiting.
  - Context tokens / context-window % computed right per model.
- **`model`** — `Snapshot` aggregation: total tokens, per-agent sums, count-by-status, dedup by `(agent, pid)`.
- **`config`** — load with missing/corrupt file → defaults; round-trip save/load preserves values; Hermes path normalization.
- **Collector path-encoding** — `encode_cwd_path(cwd)` (the `/`-to-`-` encoding abtop uses for project dirs) round-trips for typical Windows cwds, UNC paths, and drive letters.

Fixtures live in `src-tauri/tests/fixtures/` as real transcript snippets (scrubbed of private content). This is where porting bugs hide, so fixtures are the core of the suite.

### Integration tests (real filesystem, still no Tauri)

- **End-to-end collect** — drop a fake `~/.claude/sessions/` + `projects/<enc>/` tree into a temp dir, point a collector at it, assert the produced `AgentSession` has correct project name, tokens, status. Proves the file-walking + transcript wiring together.
- **`/clear` simulation** — write session file, then a fresh transcript with a new sid, assert the live sid is adopted and old counters evicted (abtop issue #68 logic).
- **Dead-PID cleanup** — a session whose pid isn't in the process snapshot is dropped.
- **Multi-profile** — two `~/.claude-*` roots both discovered and collected.

### Manual / QA checklist (not cheaply automatable)

- Panel stays on top of fullscreen apps and other always-on-top windows; drag works across monitors; opacity slider updates live; tray Show/Hide and global hotkey both toggle; click-through behaves correctly on transparent regions; window position persists across restarts.

### What is intentionally not tested

- Tauri window flags and OS-level transparency (not unit-testable; covered by the QA checklist).
- Live process scanning against the user's real machine (flaky in CI).

### Fidelity note

Because we're porting abtop's logic, the fixtures and assertions double as a correctness check that our port matches abtop's documented behavior. Where Hermes differs (its own transcript format), tests are written against a real sample gathered during implementation.

## 7. Open items to confirm during implementation

- **Hermes on-disk format & path.** The user confirmed Hermes is a CLI that writes local data, but the exact data root and transcript/schema are not yet known. Implementation begins by locating a real Hermes data sample (with the user) and writing `HermesCollector` + fixtures against it. Until then, Hermes support is stubbed behind the collector trait and clearly marked.
- **Global hotkey default.** Needs a choice that doesn't collide with common OS/app shortcuts (e.g. `Ctrl+Shift+Space` or `Alt+``); made configurable in `config.toml`.
- **Tauri v2 exact crate versions** for transparent topmost windows — pinned in `Cargo.toml` during scaffolding.
