# AI Assistant Usage Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Windows desktop overlay (Tauri v2 + Rust + web UI) that shows live usage of Claude Code, Codex CLI, and Hermes as a transparent, always-on-top, draggable floating panel.

**Architecture:** A Rust core collects AI-assistant session state from local files/processes (self-contained port of abtop's approach) into a `Snapshot`; a background tick thread emits it to a lean TS frontend over Tauri events; the shell is one transparent frameless topmost window with a tray icon and global hotkey.

**Tech Stack:** Tauri v2, Rust (serde, serde_json, sysinfo, toml, dirs, chrono), vanilla TypeScript + CSS (Vite dev server, no framework).

## Global Constraints

- **Windows-only.** Window flags, tray, and global-hotkey code target Windows. Use `std::path` and `dirs::home_dir()` — on Windows the home dir resolves to `%USERPROFILE%`.
- **Read-only.** Never write to or modify any AI-assistant's files. The only file the app writes is its own `config.toml`.
- **No network, no API keys, no auth.** All data is local file/process metadata.
- **Self-contained.** No `abtop` dependency. Collection logic is reimplemented.
- **Poll interval default 1s.** Configurable via `config.toml`.
- **Graceful degradation.** A missing/malformed data source for one agent never crashes the panel or blanks other agents.
- **TDD.** Pure Rust logic (`transcript`, `model`, `config`, collectors) is test-first. The Tauri shell/frontend are verified by a manual QA checklist.
- **Commits:** one commit per task, conventional-commit messages, end with `Co-Authored-By: Claude <noreply@anthropic.com>`.

## v1 scope (deviation from abtop's richness, by design)

abtop tracks MCP servers, subagents, chat messages, tool calls, file accesses, git stats, open ports, multi-profile discovery, and an async desktop-rollout scanner. **v1 omits all of that.** The approved overlay UI shows only: agent, project, status, context-window %, tokens in/out, memory. So v1 ports the *core* path only: discover sessions from local files, parse transcripts for tokens/context/status, scan processes for memory. Deferred items are listed in §"Out of scope (future)".

## File Structure

```
usage-tracker/
  package.json                  # frontend: vite + @tauri-apps/cli, no runtime deps
  tsconfig.json
  vite.config.ts
  index.html
  src/                          # frontend (vanilla TS + CSS)
    main.ts                     # entry: wire events → render, wire commands → buttons
    render.ts                   # pure: Snapshot → HTML string for session rows
    style.css                   # transparent panel, drag handle, bars
  src-tauri/
    Cargo.toml
    tauri.conf.json
    build.rs
    src/
      main.rs                   # tauri entry (windows subsystem), calls lib::run()
      lib.rs                    # tauri builder: window/tray/hotkey setup, commands, tick thread
      config.rs                 # Config load/save (TDD)                      [pure]
      model.rs                  # AgentSession, SessionStatus, Snapshot (TDD) [pure]
      transcript.rs             # IncrementalReader: file-identity + offset IO [pure]
      process.rs                # sysinfo wrapper: ProcessSnapshot             [thin]
      collector.rs              # Collector trait + ProcessContext             [pure-ish]
      claude.rs                 # ClaudeCollector + claude line parser (TDD)
      codex.rs                  # CodexCollector + codex line parser (TDD)
      hermes.rs                 # HermesCollector (sample-contingent — see Task 12)
      app.rs                    # App: owns collectors, tick(), to_snapshot()
    tests/
      fixtures/
        claude_session.json
        claude_transcript.jsonl
        codex_rollout.jsonl
```

Boundary rule: `config.rs`, `model.rs`, `transcript.rs`, `collector.rs`, `claude.rs`, `codex.rs`, `hermes.rs` have **no** `tauri` dependency — they are unit-testable in isolation. Only `lib.rs`/`main.rs`/`process.rs` touch the OS/Tauri.

---

### Task 1: Scaffold the Tauri v2 project

**Files:**
- Create: `package.json`, `tsconfig.json`, `vite.config.ts`, `index.html`, `src/main.ts`, `src/style.css`
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: a runnable `cargo tauri dev` / `npm run tauri dev` skeleton with a single window and a Rust `greet` command, proving the toolchain works.

- [ ] **Step 1: Scaffold with the official template**

Run (from the project root):
```bash
npm create tauri-app@latest -- --template vanilla-ts --manager npm --name usage-tracker --identifier com.usage-tracker.app
```
If the interactive prompt appears, choose: package manager `npm`, frontend `vanilla-ts`, UI template `Vanilla`, identifier `com.usage-tracker.app`. The scaffolder writes into the current directory.

- [ ] **Step 2: Verify the toolchain**

Run:
```bash
npm install
npm run tauri dev
```
Expected: a window opens showing the default Tauri welcome page; no build errors. Stop the dev server (`Ctrl+C`).

- [ ] **Step 3: Add Rust dependencies to `src-tauri/Cargo.toml`**

Open `src-tauri/Cargo.toml` and ensure the `[dependencies]` section contains (use versions the scaffolder already pinned for `tauri`/`serde`; add the rest):
```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-global-shortcut = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sysinfo = "0.32"
toml = "0.8"
dirs = "5"
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 4: Verify it still builds**

Run: `npm run tauri dev`
Expected: builds and opens with no errors. Stop it.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: scaffold Tauri v2 app with core dependencies

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 2: Config module (TDD)

**Files:**
- Create: `src-tauri/src/config.rs`
- Create: `src-tauri/tests/config_test.rs`

**Interfaces:**
- Produces: `pub struct Config { pub poll_interval_ms: u64, pub opacity: f32, pub window_x: Option<i32>, pub window_y: Option<i32>, pub hotkey: String, pub enabled_agents: Vec<String>, pub hermes_data_dir: Option<PathBuf> }`, `pub fn default_config() -> Config`, `pub fn load_config(path: &Path) -> Config`, `pub fn save_config(path: &Path, cfg: &Config) -> std::io::Result<()>`.
- Consumes: nothing (first task).

- [ ] **Step 1: Write the failing test**

`src-tauri/tests/config_test.rs`:
```rust
use std::fs;
use usage_tracker::config::{default_config, load_config, save_config, Config};

#[test]
fn missing_file_returns_defaults() {
    let cfg = load_config(std::path::Path::new("/nonexistent/config.toml"));
    assert_eq!(cfg.poll_interval_ms, 1000);
    assert!((cfg.opacity - 0.85).abs() < 0.001);
    assert_eq!(cfg.hotkey, "Ctrl+Shift+Space");
    assert_eq!(cfg.enabled_agents, vec!["claude", "codex", "hermes"]);
}

#[test]
fn round_trip_preserves_values() {
    let dir = tempfile_dir();
    let path = dir.join("config.toml");
    let mut cfg = default_config();
    cfg.poll_interval_ms = 2000;
    cfg.opacity = 0.5;
    cfg.hermes_data_dir = Some(dir.join("hermes-data"));
    save_config(&path, &cfg).unwrap();
    let loaded = load_config(&path);
    assert_eq!(loaded.poll_interval_ms, 2000);
    assert!((loaded.opacity - 0.5).abs() < 0.001);
    assert_eq!(loaded.hermes_data_dir, Some(dir.join("hermes-data")));
}

#[test]
fn corrupt_file_falls_back_to_defaults() {
    let dir = tempfile_dir();
    let path = dir.join("config.toml");
    fs::write(&path, "this is = = not valid toml {{{").unwrap();
    let cfg = load_config(&path);
    assert_eq!(cfg.poll_interval_ms, 1000); // default
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("utt-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    dir
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test config_test`
Expected: FAIL — `usage_tracker::config` module not found / unresolved import.

- [ ] **Step 3: Write minimal implementation**

`src-tauri/src/config.rs`:
```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub poll_interval_ms: u64,
    pub opacity: f32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub hotkey: String,
    pub enabled_agents: Vec<String>,
    pub hermes_data_dir: Option<PathBuf>,
}

pub fn default_config() -> Config {
    Config {
        poll_interval_ms: 1000,
        opacity: 0.85,
        window_x: None,
        window_y: None,
        hotkey: "Ctrl+Shift+Space".to_string(),
        enabled_agents: vec!["claude".into(), "codex".into(), "hermes".into()],
        hermes_data_dir: None,
    }
}

pub fn load_config(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Config>(&text).unwrap_or_else(|_| default_config()),
        Err(_) => default_config(),
    }
}

pub fn save_config(path: &Path, cfg: &Config) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, text)
}
```

Expose the module from the library crate. In `src-tauri/src/lib.rs` (replace whatever the scaffold put there) add:
```rust
pub mod config;
```
Keep the scaffold's `run()` function intact for now.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test config_test`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config.rs src-tauri/src/lib.rs src-tauri/tests/config_test.rs
git commit -m "feat(config): load/save TOML config with graceful fallback

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 3: Model — AgentSession, SessionStatus, Snapshot (TDD)

**Files:**
- Create: `src-tauri/src/model.rs`
- Create: `src-tauri/tests/model_test.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod model;`)

**Interfaces:**
- Produces:
  - `pub enum SessionStatus { Waiting, Thinking, Executing, Done, Unknown }` (derives `Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize`)
  - `pub struct AgentSession { pub agent_cli: String, pub pid: u32, pub session_id: String, pub cwd: String, pub project_name: String, pub started_at: i64, pub status: SessionStatus, pub model: String, pub context_percent: f64, pub total_input_tokens: u64, pub total_output_tokens: u64, pub total_cache_read: u64, pub turn_count: u32, pub current_task: String, pub mem_mb: u64 }` (derives `Debug, Clone, Serialize, Deserialize`)
  - `pub struct Snapshot { pub sessions: Vec<AgentSession>, pub total_tokens: u64, pub by_agent_tokens: std::collections::HashMap<String,u64>, pub by_status: std::collections::HashMap<SessionStatus,u32> }`
  - `pub fn build_snapshot(sessions: Vec<AgentSession>) -> Snapshot`
- Consumes: nothing.

- [ ] **Step 1: Write the failing test**

`src-tauri/tests/model_test.rs`:
```rust
use usage_tracker::model::{build_snapshot, AgentSession, SessionStatus};
use std::collections::HashMap;

fn session(agent: &str, pid: u32, inp: u64, out: u64, status: SessionStatus) -> AgentSession {
    AgentSession {
        agent_cli: agent.into(), pid, session_id: format!("{agent}-{pid}"),
        cwd: "/proj".into(), project_name: "proj".into(), started_at: 0,
        status, model: "m".into(), context_percent: 0.0,
        total_input_tokens: inp, total_output_tokens: out, total_cache_read: 0,
        turn_count: 0, current_task: String::new(), mem_mb: 0,
    }
}

#[test]
fn snapshot_aggregates_tokens_by_agent() {
    let sessions = vec![
        session("claude", 1, 100, 10, SessionStatus::Executing),
        session("claude", 2, 200, 20, SessionStatus::Waiting),
        session("codex", 3, 50, 5, SessionStatus::Thinking),
    ];
    let snap = build_snapshot(sessions);
    assert_eq!(snap.total_tokens, 100 + 10 + 200 + 20 + 50 + 5);
    let mut want = HashMap::new();
    want.insert("claude".to_string(), 100 + 10 + 200 + 20);
    want.insert("codex".to_string(), 50 + 5);
    assert_eq!(snap.by_agent_tokens, want);
}

#[test]
fn snapshot_counts_by_status() {
    let sessions = vec![
        session("claude", 1, 0, 0, SessionStatus::Executing),
        session("claude", 2, 0, 0, SessionStatus::Executing),
        session("codex", 3, 0, 0, SessionStatus::Waiting),
    ];
    let snap = build_snapshot(sessions);
    assert_eq!(snap.by_status.get(&SessionStatus::Executing), Some(&2));
    assert_eq!(snap.by_status.get(&SessionStatus::Waiting), Some(&1));
    assert_eq!(snap.by_status.get(&SessionStatus::Thinking), Some(&0));
}

#[test]
fn snapshot_serializes_to_json() {
    let snap = build_snapshot(vec![session("claude", 1, 1, 1, SessionStatus::Waiting)]);
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("\"agent_cli\":\"claude\""));
    assert!(json.contains("\"total_tokens\":2"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test model_test`
Expected: FAIL — `model` module not found.

- [ ] **Step 3: Write minimal implementation**

`src-tauri/src/model.rs`:
```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Waiting,
    Thinking,
    Executing,
    Done,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub agent_cli: String,
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub project_name: String,
    pub started_at: i64,
    pub status: SessionStatus,
    pub model: String,
    pub context_percent: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read: u64,
    pub turn_count: u32,
    pub current_task: String,
    pub mem_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub sessions: Vec<AgentSession>,
    pub total_tokens: u64,
    pub by_agent_tokens: HashMap<String, u64>,
    pub by_status: HashMap<SessionStatus, u32>,
}

pub fn build_snapshot(sessions: Vec<AgentSession>) -> Snapshot {
    let mut total_tokens: u64 = 0;
    let mut by_agent_tokens: HashMap<String, u64> = HashMap::new();
    let mut by_status: HashMap<SessionStatus, u32> = HashMap::new();
    for s in &sessions {
        let t = s.total_input_tokens + s.total_output_tokens;
        total_tokens += t;
        *by_agent_tokens.entry(s.agent_cli.clone()).or_insert(0) += t;
        *by_status.entry(s.status).or_insert(0) += 1;
    }
    Snapshot { sessions, total_tokens, by_agent_tokens, by_status }
}
```

Add `pub mod model;` to `src-tauri/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test model_test`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/model.rs src-tauri/src/lib.rs src-tauri/tests/model_test.rs
git commit -m "feat(model): AgentSession, SessionStatus, Snapshot with aggregation

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 4: Incremental transcript reader (TDD)

The agent-agnostic IO piece: read only newly-appended JSONL lines since last call, re-parse from scratch if the file's identity (mtime+size) changed. Used by every collector so the panel never re-reads an entire transcript every tick.

**Files:**
- Create: `src-tauri/src/transcript.rs`
- Create: `src-tauri/tests/transcript_test.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod transcript;`)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub struct FileIdentity { pub mtime_ms: i64, pub size: u64 }`
  - `pub struct IncrementalReader { pub offset: u64, pub identity: Option<FileIdentity> }`
  - `impl IncrementalReader { pub fn new() -> Self; pub fn read_new_lines(&mut self, path: &Path) -> Vec<String> }`
- Consumes: nothing.

- [ ] **Step 1: Write the failing test**

`src-tauri/tests/transcript_test.rs`:
```rust
use std::fs;
use std::path::PathBuf;
use usage_tracker::transcript::IncrementalReader;

fn tmp(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("utt-tr-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

#[test]
fn reads_all_lines_on_first_call() {
    let p = tmp("a.jsonl", "{\"i\":1}\n{\"i\":2}\n");
    let mut r = IncrementalReader::new();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":1}".to_string(), "{\"i\":2}".to_string()]);
}

#[test]
fn reads_only_appended_lines_on_subsequent_call() {
    let p = tmp("b.jsonl", "{\"i\":1}\n");
    let mut r = IncrementalReader::new();
    let _ = r.read_new_lines(&p);
    // append
    let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    use std::io::Write; write!(f, "{{\"i\":2}}\n").unwrap();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":2}".to_string()]);
}

#[test]
fn reparses_from_zero_when_file_identity_changes() {
    let p = tmp("c.jsonl", "{\"i\":1}\n");
    let mut r = IncrementalReader::new();
    let _ = r.read_new_lines(&p);
    // truncate + rewrite (identity changes: size shrinks)
    fs::write(&p, "{\"i\":9}\n").unwrap();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":9}".to_string()]);
}

#[test]
fn skips_malformed_line_keeps_rest() {
    // malformed middle line still yields the good ones around it
    let p = tmp("d.jsonl", "{\"i\":1}\nnot json\n{\"i\":2}\n");
    let mut r = IncrementalReader::new();
    let lines = r.read_new_lines(&p);
    // read_new_lines returns raw lines; parsing is the caller's job.
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1], "not json");
}

#[test]
fn missing_file_returns_empty() {
    let mut r = IncrementalReader::new();
    assert!(r.read_new_lines(std::path::Path::new("/no/such/file.jsonl")).is_empty());
}

#[test]
fn trailing_partial_line_is_not_returned() {
    // no trailing newline → last line is incomplete, must not be returned yet
    let p = tmp("e.jsonl", "{\"i\":1}\n{\"i\":2}");
    let mut r = IncrementalReader::new();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":1}".to_string()]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test transcript_test`
Expected: FAIL — `transcript` module not found.

- [ ] **Step 3: Write minimal implementation**

`src-tauri/src/transcript.rs`:
```rust
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct FileIdentity {
    pub mtime_ms: i64,
    pub size: u64,
}

pub struct IncrementalReader {
    pub offset: u64,
    pub identity: Option<FileIdentity>,
}

impl IncrementalReader {
    pub fn new() -> Self {
        Self { offset: 0, identity: None }
    }

    /// Returns complete JSONL lines appended since the last call. If the
    /// file's identity (mtime+size) changed, re-reads from offset 0. A
    /// trailing line without a newline is held back until it completes.
    pub fn read_new_lines(&mut self, path: &Path) -> Vec<String> {
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let size = meta.len();
        let mtime_ms = meta.modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let identity = FileIdentity { mtime_ms, size };

        let from_zero = match self.identity {
            None => true,
            Some(prev) => prev != identity,
        };
        let start = if from_zero { 0 } else { self.offset };

        let mut file = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut buf = String::new();
        if file.read_to_string(&mut buf).is_err() {
            return Vec::new();
        }

        // Only return lines terminated by '\n'. A trailing fragment without
        // '\n' is incomplete (the writer is mid-line); hold it back.
        let ends_with_newline = buf.ends_with('\n');
        let mut lines: Vec<String> = buf.split('\n').map(|s| s.to_string()).collect();
        if ends_with_newline {
            // split on trailing '\n' yields a final empty element; drop it
            if lines.last().map(|s| s.is_empty()).unwrap_or(false) {
                lines.pop();
            }
        } else if !lines.is_empty() {
            // drop the incomplete trailing fragment, rewind offset to before it
            let fragment_len = lines.last().map(|s| s.len() as u64).unwrap_or(0);
            self.offset = start + buf.len() as u64 - fragment_len;
            self.identity = Some(identity);
            lines.pop();
            return lines;
        }

        self.offset = start + buf.len() as u64;
        self.identity = Some(identity);
        lines
    }
}

impl Default for IncrementalReader {
    fn default() -> Self { Self::new() }
}
```

Add `pub mod transcript;` to `src-tauri/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test transcript_test`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/transcript.rs src-tauri/src/lib.rs src-tauri/tests/transcript_test.rs
git commit -m "feat(transcript): incremental JSONL reader with identity-change detection

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 5: Process snapshot wrapper (sysinfo)

A thin wrapper, not heavily unit-tested (flaky against the live OS); covered by the manual QA checklist and a basic smoke test.

**Files:**
- Create: `src-tauri/src/process.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod process;`)

**Interfaces:**
- Produces:
  - `#[derive(Debug, Clone)] pub struct ProcInfo { pub pid: u32, pub command: String, pub rss_kb: u64, pub cpu: f32, pub parent_pid: Option<u32> }`
  - `pub struct ProcessSnapshot { pub procs: HashMap<u32, ProcInfo>, pub children: HashMap<u32, Vec<u32>>, pub ports_by_pid: HashMap<u32, Vec<u16>> }`
  - `pub fn snapshot() -> ProcessSnapshot` — refreshes sysinfo + runs `netstat -ano`, returns the maps.
  - `pub fn has_active_descendant(pid: u32, snap: &ProcessSnapshot) -> bool` — true if any descendant CPU > 5%.
- Consumes: `sysinfo`.

- [ ] **Step 1: Write a smoke test**

`src-tauri/tests/process_test.rs`:
```rust
use usage_tracker::process::{snapshot, has_active_descendant};

#[test]
fn snapshot_includes_self_and_has_no_panics() {
    let snap = snapshot();
    let me = std::process::id();
    assert!(snap.procs.contains_key(&me), "self process must appear");
    // children map is well-formed (no panic iterating it)
    for (_pid, kids) in &snap.children {
        for _k in kids { /* touch */ }
    }
}

#[test]
fn has_active_descendant_returns_bool_without_panic() {
    let snap = snapshot();
    let _ = has_active_descendant(std::process::id(), &snap);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test process_test`
Expected: FAIL — `process` module not found.

- [ ] **Step 3: Write minimal implementation**

`src-tauri/src/process.rs`:
```rust
use std::collections::HashMap;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub command: String,
    pub rss_kb: u64,
    pub cpu: f32,
    pub parent_pid: Option<u32>,
}

pub struct ProcessSnapshot {
    pub procs: HashMap<u32, ProcInfo>,
    pub children: HashMap<u32, Vec<u32>>,
    pub ports_by_pid: HashMap<u32, Vec<u16>>,
}

pub fn snapshot() -> ProcessSnapshot {
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes();

    let mut procs: HashMap<u32, ProcInfo> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, p) in sys.processes() {
        let pid_u = pid.as_u32();
        procs.insert(pid_u, ProcInfo {
            pid: pid_u,
            command: p.name().to_string(),
            rss_kb: p.memory(),
            cpu: p.cpu_usage(),
            parent_pid: p.parent().map(|pp| pp.as_u32()),
        });
        if let Some(ppid) = p.parent().map(|pp| pp.as_u32()) {
            children.entry(ppid).or_default().push(pid_u);
        }
    }
    let ports_by_pid = listening_ports_windows();
    ProcessSnapshot { procs, children, ports_by_pid }
}

/// True if any descendant of `pid` has CPU > 5%.
pub fn has_active_descendant(pid: u32, snap: &ProcessSnapshot) -> bool {
    let mut stack: Vec<u32> = snap.children.get(&pid).cloned().unwrap_or_default();
    while let Some(c) = stack.pop() {
        if let Some(info) = snap.procs.get(&c) {
            if info.cpu > 5.0 { return true; }
        }
        if let Some(grandkids) = snap.children.get(&c) {
            stack.extend(grandkids);
        }
    }
    false
}

/// `netstat -ano` → pid → listening TCP ports. Empty on any failure.
fn listening_ports_windows() -> HashMap<u32, Vec<u16>> {
    let out = std::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output();
    let Ok(out) = out else { return HashMap::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map: HashMap<u32, Vec<u16>> = HashMap::new();
    for line in text.lines() {
        // lines look like:  TCP    0.0.0.0:8080   0.0.0.0:0   LISTENING   1234
        if !line.contains("LISTENING") { continue; }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 { continue; }
        let local = parts[1];
        let port = match local.rsplit_once(':').map(|(_, p)| p.parse::<u16>()) {
            Some(Ok(p)) => p,
            _ => continue,
        };
        let pid: u32 = match parts[parts.len() - 1].parse() { Ok(p) => p, Err(_) => continue };
        map.entry(pid).or_default().push(port);
    }
    map
}
```

Add `pub mod process;` to `src-tauri/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test process_test`
Expected: PASS (2 tests). If `sysinfo` API differs at the pinned version (e.g. `memory()` units), fix to match — the smoke test only checks presence, not values.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/process.rs src-tauri/src/lib.rs src-tauri/tests/process_test.rs
git commit -m "feat(process): sysinfo + netstat wrapper for pid/mem/cpu/ports

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: Collector trait

**Files:**
- Create: `src-tauri/src/collector.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod collector;`)

**Interfaces:**
- Produces:
  - `pub struct ProcessContext<'a> { pub procs: &'a HashMap<u32,ProcInfo>, pub children: &'a HashMap<u32,Vec<u32>>, pub ports: &'a HashMap<u32,Vec<u16>> }`
  - `pub trait Collector { fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession>; fn name(&self) -> &str; }`
- Consumes: `model::AgentSession`, `process::ProcInfo`.

- [ ] **Step 1: Write minimal implementation (no test — pure trait definition)**

`src-tauri/src/collector.rs`:
```rust
use crate::model::AgentSession;
use crate::process::ProcInfo;
use std::collections::HashMap;

pub struct ProcessContext<'a> {
    pub procs: &'a HashMap<u32, ProcInfo>,
    pub children: &'a HashMap<u32, Vec<u32>>,
    pub ports: &'a HashMap<u32, Vec<u16>>,
}

pub trait Collector {
    fn name(&self) -> &str;
    fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession>;
}
```

Add `pub mod collector;` to `src-tauri/src/lib.rs`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/collector.rs src-tauri/src/lib.rs
git commit -m "feat(collector): Collector trait + ProcessContext

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: ClaudeCollector (TDD against a fake `.claude` tree)

Claude Code writes `~/.claude/sessions/{pid}.json` (a small header with `pid`, `cwd`, `session_id`, `startedAt`) and a transcript at `~/.claude/projects/<encoded-cwd>/<session_id>.jsonl` where `<encoded-cwd>` is the cwd with `/` (and `:`) replaced by `-`. Each transcript line is a JSON object; the ones we care about carry `type:"assistant"`/`type:"user"` and a `message.usage` block with `input_tokens`, `output_tokens`, `cache_read_input_tokens`, plus an `index`/context size we approximate.

**Files:**
- Create: `src-tauri/src/claude.rs`
- Create: `src-tauri/tests/fixtures/claude_session.json`
- Create: `src-tauri/tests/fixtures/claude_transcript.jsonl`
- Create: `src-tauri/tests/claude_test.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod claude;`)

**Interfaces:**
- Produces:
  - `pub fn encode_cwd_path(cwd: &str) -> String`
  - `pub struct ClaudeCollector { config_dir: PathBuf, readers: HashMap<String, IncrementalReader> }`
  - `impl ClaudeCollector { pub fn new(config_dir: PathBuf) -> Self; }`
  - `impl Collector for ClaudeCollector`
- Consumes: `model`, `transcript::IncrementalReader`, `collector::{Collector, ProcessContext}`, `process::ProcInfo`.

- [ ] **Step 1: Create the fixtures**

`src-tauri/tests/fixtures/claude_session.json` (a `~/.claude/sessions/4242.json` analogue; cwd `/proj` encodes to `-proj`):
```json
{ "pid": 4242, "cwd": "/proj", "session_id": "abc-123", "startedAt": 1700000000000 }
```

`src-tauri/tests/fixtures/claude_transcript.jsonl` (two assistant turns with usage; one user turn):
```jsonl
{"type":"user","message":{"role":"user","content":"do the thing"},"timestamp":"2023-11-14T22:13:20Z"}
{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-4-6","usage":{"input_tokens":1200,"output_tokens":80,"cache_read_input_tokens":5000},"timestamp":"2023-11-14T22:13:25Z"}}
{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-4-6","usage":{"input_tokens":7000,"output_tokens":150,"cache_read_input_tokens":5000},"timestamp":"2023-11-14T22:13:30Z"}}
```

- [ ] **Step 2: Write the failing test**

`src-tauri/tests/claude_test.rs`:
```rust
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use usage_tracker::claude::{ClaudeCollector, encode_cwd_path};
use usage_tracker::collector::{Collector, ProcessContext};
use usage_tracker::process::ProcInfo;

fn build_fake_claude_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-claude-{}", std::process::id()));
    let _ = fs::create_dir_all(root.join("sessions"));
    let enc = encode_cwd_path("/proj");
    let _ = fs::create_dir_all(root.join("projects").join(&enc));
    fs::write(
        root.join("sessions").join("4242.json"),
        include_str!("fixtures/claude_session.json"),
    ).unwrap();
    fs::write(
        root.join("projects").join(enc).join("abc-123.jsonl"),
        include_str!("fixtures/claude_transcript.jsonl"),
    ).unwrap();
    root
}

fn empty_ctx() -> ProcessContext<'static> {
    // SAFETY of lifetime: we leak two empty maps so the ctx outlives the call.
    static EMPTY_PROCS: std::sync::OnceLock<HashMap<u32, ProcInfo>> = std::sync::OnceLock::new();
    static EMPTY_KIDS: std::sync::OnceLock<HashMap<u32, Vec<u32>>> = std::sync::OnceLock::new();
    static EMPTY_PORTS: std::sync::OnceLock<HashMap<u32, Vec<u16>>> = std::sync::OnceLock::new();
    let procs = EMPTY_PROCS.get_or_init(HashMap::new);
    let kids = EMPTY_KIDS.get_or_init(HashMap::new);
    let ports = EMPTY_PORTS.get_or_init(HashMap::new);
    ProcessContext { procs, children: kids, ports }
}

#[test]
fn encode_cwd_replaces_slash_and_colon() {
    assert_eq!(encode_cwd_path("/proj"), "-proj");
    assert_eq!(encode_cwd_path("C:\\Users\\me"), "C--Users-me");
}

#[test]
fn collects_session_with_accumulated_tokens_and_project_name() {
    let root = build_fake_claude_root();
    // Pretend pid 4242 is alive as a 'claude' process.
    let mut procs = HashMap::new();
    procs.insert(4242, ProcInfo { pid: 4242, command: "claude".into(), rss_kb: 50_000, cpu: 0.0, parent_pid: None });
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports };

    let mut c = ClaudeCollector::new(root);
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.agent_cli, "claude");
    assert_eq!(s.pid, 4242);
    assert_eq!(s.session_id, "abc-123");
    assert_eq!(s.project_name, "proj");
    assert_eq!(s.model, "claude-sonnet-4-6");
    // input accumulates: 1200 + 7000 = 8200 ; output: 80 + 150 = 230 ; cache_read: 10000
    assert_eq!(s.total_input_tokens, 8200);
    assert_eq!(s.total_output_tokens, 230);
    assert_eq!(s.total_cache_read, 10000);
}

#[test]
fn dead_pid_session_is_dropped() {
    let root = build_fake_claude_root();
    let ctx = empty_ctx(); // no procs → pid 4242 not alive
    let mut c = ClaudeCollector::new(root);
    let sessions = c.collect(&ctx);
    assert!(sessions.is_empty(), "session whose pid is not alive must be dropped");
}
```

> **Note on the `OnceLock` lifetime trick:** `ProcessContext` borrows the maps. In tests we need a `'static` borrow; leaking empty maps via `OnceLock` is the simplest way. For the non-empty case in the second test, local maps with explicit lifetimes work because the ctx is used immediately.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test claude_test`
Expected: FAIL — `claude` module not found.

- [ ] **Step 4: Write minimal implementation**

`src-tauri/src/claude.rs`:
```rust
use crate::collector::{Collector, ProcessContext};
use crate::model::{AgentSession, SessionStatus};
use crate::process::{has_active_descendant, ProcInfo};
use crate::transcript::IncrementalReader;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct SessionFile {
    #[serde(default)] pid: u32,
    #[serde(default)] cwd: String,
    #[serde(default)] session_id: String,
    #[serde(default, rename = "startedAt")] started_at: i64,
}

pub fn encode_cwd_path(cwd: &str) -> String {
    cwd.chars().map(|c| match c {
        '/' | '\\' | ':' => '-',
        _ => c,
    }).collect()
}

pub struct ClaudeCollector {
    config_dir: PathBuf,
    readers: HashMap<String, IncrementalReader>,
    // accumulated parse state per session_id
    state: HashMap<String, ParseState>,
}

#[derive(Default, Clone)]
struct ParseState {
    model: String,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    last_user_ts_ms: i64,
    pending_tool: bool,
    current_task: String,
    last_context_tokens: u64,
}

impl ClaudeCollector {
    pub fn new(config_dir: PathBuf) -> Self {
        Self { config_dir, readers: HashMap::new(), state: HashMap::new() }
    }
}

impl Collector for ClaudeCollector {
    fn name(&self) -> &str { "claude" }

    fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession> {
        let sessions_dir = self.config_dir.join("sessions");
        let mut out = Vec::new();
        let entries = match fs::read_dir(&sessions_dir) {
            Ok(e) => e,
            Err(_) => return out,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Ok(sf) = serde_json::from_str::<SessionFile>(&text) else { continue };

            // Only keep sessions whose pid is alive and looks like claude.
            let alive = ctx.procs.get(&sf.pid)
                .map(|p| p.command.contains("claude"))
                .unwrap_or(false);
            if !alive { continue; }

            let project_dir = self.config_dir.join("projects").join(encode_cwd_path(&sf.cwd));
            let transcript = project_dir.join(format!("{}.jsonl", sf.session_id));
            if transcript.exists() {
                let reader = self.readers.entry(sf.session_id.clone()).or_default();
                let lines = reader.read_new_lines(&transcript);
                let st = self.state.entry(sf.session_id.clone()).or_default();
                for line in lines {
                    apply_claude_line(&line, st);
                }
            }
            let st = self.state.get(&sf.session_id).cloned().unwrap_or_default();
            let proc = ctx.procs.get(&sf.pid);
            let mem_mb = proc.map(|p| p.rss_kb / 1024).unwrap_or(0);
            let status = derive_status(&st, sf.pid, ctx);
            let project_name = sf.cwd.rsplit(['/', '\\']).next().unwrap_or("?").to_string();

            out.push(AgentSession {
                agent_cli: "claude".into(),
                pid: sf.pid,
                session_id: sf.session_id.clone(),
                cwd: sf.cwd.clone(),
                project_name,
                started_at: sf.started_at,
                status,
                model: st.model.clone(),
                context_percent: 0.0, // context window % is model-dependent; v1 leaves 0 unless known
                total_input_tokens: st.total_input,
                total_output_tokens: st.total_output,
                total_cache_read: st.total_cache_read,
                turn_count: 0,
                current_task: st.current_task.clone(),
                mem_mb,
            });
        }
        // Evict state for sessions no longer present (pid died / file gone).
        self.state.retain(|_, _| true); // placeholder keep-all; eviction handled by alive check
        out.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        out
    }
}

fn derive_status(st: &ParseState, pid: u32, ctx: &ProcessContext) -> SessionStatus {
    let active_child = has_active_descendant(pid, ctx);
    if active_child || st.pending_tool { SessionStatus::Executing }
    else if st.last_user_ts_ms > 0 { SessionStatus::Thinking }
    else { SessionStatus::Waiting }
}

/// Mutates accumulated parse state from one transcript JSON line.
fn apply_claude_line(line: &str, st: &mut ParseState) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "user" => {
            st.last_user_ts_ms = v.get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_iso_to_ms)
                .unwrap_or(st.last_user_ts_ms);
            st.pending_tool = false;
        }
        "assistant" => {
            if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                st.total_input += u.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                st.total_output += u.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                st.total_cache_read += u.get("cache_read_input_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                st.last_context_tokens = st.total_input + st.total_cache_read;
            }
            if let Some(m) = v.get("message") {
                st.model = m.get("model").and_then(|m| m.as_str()).unwrap_or(&st.model).to_string();
                // pending_tool if this assistant turn contained a tool_use not yet answered
                let has_tool_use = m.get("content").and_then(|c| c.as_array())
                    .map(|arr| arr.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use")))
                    .unwrap_or(false);
                st.pending_tool = has_tool_use;
                if has_tool_use {
                    st.current_task = m.get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| arr.iter().find_map(|b| {
                            if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                b.get("name").and_then(|n| n.as_str()).map(String::from)
                            } else { None }
                        }))
                        .unwrap_or_default();
                }
            }
            st.last_user_ts_ms = 0; // assistant replied → no longer generating
        }
        _ => {}
    }
}

fn parse_iso_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok()
        .map(|dt| dt.timestamp_millis())
}
```

Add `pub mod claude;` to `src-tauri/src/lib.rs`.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --test claude_test`
Expected: PASS (3 tests). If `serde_json::Value` field access differs, adjust — the assertions only check token accumulation, project name, and dead-pid drop.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/claude.rs src-tauri/src/lib.rs src-tauri/tests/claude_test.rs src-tauri/tests/fixtures/claude_session.json src-tauri/tests/fixtures/claude_transcript.jsonl
git commit -m "feat(claude): ClaudeCollector scanning ~/.claude sessions + transcripts

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: CodexCollector (TDD against a fake `.codex` tree)

Codex CLI writes rollouts to `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`. Each line is a JSON event; we care about `type:"session_meta"` (cwd, model, session_id, cli_version), `type:"event_msg"` with `payload.type` in `{task_started, user_message, token_count, task_complete}`, and `type:"response_item"` (assistant turns, tool calls). v1 simplification: we discover rollouts by scanning today's dir for recently-modified `rollout-*.jsonl` files (drop abtop's PID→fd open-file mapping and desktop async scanner — YAGNI for v1). A rollout is "live" if its mtime is within the last 5 minutes.

**Files:**
- Create: `src-tauri/src/codex.rs`
- Create: `src-tauri/tests/fixtures/codex_rollout.jsonl`
- Create: `src-tauri/tests/codex_test.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod codex;`)

**Interfaces:**
- Produces:
  - `pub struct CodexCollector { sessions_dir: PathBuf, readers: HashMap<PathBuf, IncrementalReader>, state: HashMap<PathBuf, ParseState> }`
  - `impl CodexCollector { pub fn new(sessions_dir: PathBuf) -> Self; }`
  - `impl Collector for CodexCollector`
- Consumes: `model`, `transcript`, `collector`, `process`.

- [ ] **Step 1: Create the fixture**

`src-tauri/tests/fixtures/codex_rollout.jsonl`:
```jsonl
{"type":"session_meta","session_id":"codex-1","cwd":"/webapp","model":"gpt-5-codex","cli_version":"0.1","git":{"branch":"main"}}
{"type":"event_msg","payload":{"type":"user_message","message":"build it"}}
{"type":"event_msg","payload":{"type":"token_count","input_tokens":900,"output_tokens":40,"rate_limit":{"remaining":5000}}}
{"type":"response_item","payload":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}]}
{"type":"event_msg","payload":{"type":"token_count","input_tokens":3000,"output_tokens":120}}
```
> Note: in real Codex rollouts `token_count` carries cumulative-or-delta counts depending on version. v1 treats each `token_count` as a **delta** and sums them, matching the assertion below. If a real sample shows cumulative counts, switch to `max(seen)` — flagged in Task 12's discovery step.

- [ ] **Step 2: Write the failing test**

`src-tauri/tests/codex_test.rs`:
```rust
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use chrono::Local;
use usage_tracker::codex::CodexCollector;
use usage_tracker::collector::{Collector, ProcessContext};

fn build_fake_codex_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-codex-{}", std::process::id()));
    let now = Local::now();
    let day_dir = root.join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    fs::write(
        day_dir.join("rollout-codex-1.jsonl"),
        include_str!("fixtures/codex_rollout.jsonl"),
    ).unwrap();
    root
}

#[test]
fn collects_recent_codex_rollout_with_tokens_and_project() {
    let root = build_fake_codex_root();
    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports };

    let mut c = CodexCollector::new(root.join("sessions"));
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1, "exactly one recent rollout expected");
    let s = &sessions[0];
    assert_eq!(s.agent_cli, "codex");
    assert_eq!(s.session_id, "codex-1");
    assert_eq!(s.project_name, "webapp");
    assert_eq!(s.model, "gpt-5-codex");
    // deltas summed: 900+3000 input, 40+120 output
    assert_eq!(s.total_input_tokens, 3900);
    assert_eq!(s.total_output_tokens, 160);
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --test codex_test`
Expected: FAIL — `codex` module not found.

- [ ] **Step 4: Write minimal implementation**

`src-tauri/src/codex.rs`:
```rust
use crate::collector::{Collector, ProcessContext};
use crate::model::{AgentSession, SessionStatus};
use crate::process::ProcInfo;
use crate::transcript::IncrementalReader;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const RECENT_AGE_SECS: u64 = 300; // 5 min

pub struct CodexCollector {
    sessions_dir: PathBuf,
    readers: HashMap<PathBuf, IncrementalReader>,
    state: HashMap<PathBuf, CodexState>,
}

#[derive(Default, Clone)]
struct CodexState {
    session_id: String,
    cwd: String,
    model: String,
    total_input: u64,
    total_output: u64,
    last_user: bool,
    pending_tool: bool,
    current_task: String,
    task_complete: bool,
}

impl CodexCollector {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir, readers: HashMap::new(), state: HashMap::new() }
    }

    fn today_dir(&self) -> Option<PathBuf> {
        let now = chrono::Local::now();
        let d = self.sessions_dir
            .join(now.format("%Y").to_string())
            .join(now.format("%m").to_string())
            .join(now.format("%d").to_string());
        if d.exists() { Some(d) } else { None }
    }
}

impl Collector for CodexCollector {
    fn name(&self) -> &str { "codex" }

    fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
        let mut out = Vec::new();
        let Some(today) = self.today_dir() else { return out };
        let entries = match fs::read_dir(&today) { Ok(e) => e, Err(_) => return out };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") { continue; }
            if !is_recent(&path, RECENT_AGE_SECS) { continue; }

            let reader = self.readers.entry(path.clone()).or_default();
            let lines = reader.read_new_lines(&path);
            let st = self.state.entry(path.clone()).or_default();
            for line in lines { apply_codex_line(&line, st); }
            let st = self.state.get(&path).cloned().unwrap_or_default();

            let project_name = st.cwd.rsplit(['/', '\\']).next().unwrap_or("?").to_string();
            let status = if st.task_complete { SessionStatus::Done }
                else if st.pending_tool { SessionStatus::Executing }
                else if st.last_user { SessionStatus::Thinking }
                else { SessionStatus::Waiting };

            out.push(AgentSession {
                agent_cli: "codex".into(),
                pid: 0, // v1 doesn't map rollout→pid
                session_id: st.session_id.clone(),
                cwd: st.cwd.clone(),
                project_name,
                started_at: 0,
                status,
                model: st.model.clone(),
                context_percent: 0.0,
                total_input_tokens: st.total_input,
                total_output_tokens: st.total_output,
                total_cache_read: 0,
                turn_count: 0,
                current_task: st.current_task.clone(),
                mem_mb: 0,
            });
        }
        out
    }
}

fn apply_codex_line(line: &str, st: &mut CodexState) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "session_meta" => {
            st.session_id = v.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
            st.cwd = v.get("cwd").and_then(|s| s.as_str()).unwrap_or("").to_string();
            st.model = v.get("model").and_then(|s| s.as_str()).unwrap_or("").to_string();
        }
        "event_msg" => {
            let pty = v.get("payload").and_then(|p| p.get("type")).and_then(|t| t.as_str()).unwrap_or("");
            match pty {
                "user_message" => { st.last_user = true; st.pending_tool = false; }
                "token_count" => {
                    let p = v.get("payload").unwrap();
                    st.total_input += p.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                    st.total_output += p.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                    st.last_user = false;
                }
                "task_complete" => { st.task_complete = true; st.last_user = false; }
                "task_started" => { st.task_complete = false; }
                _ => {}
            }
        }
        "response_item" => {
            // assistant turn or tool call
            st.last_user = false;
            let has_tool = v.get("payload").and_then(|p| p.as_array())
                .map(|arr| arr.iter().any(|b| {
                    b.get("type").and_then(|t| t.as_str()) == Some("function_call")
                })).unwrap_or(false);
            st.pending_tool = has_tool;
            if has_tool {
                st.current_task = "function_call".to_string();
            }
        }
        _ => {}
    }
}

fn is_recent(path: &Path, max_age_secs: u64) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    let Ok(m) = meta.modified() else { return false };
    let age = SystemTime::now().duration_since(m).unwrap_or(Duration::ZERO);
    age.as_secs() <= max_age_secs
}
```

Add `pub mod codex;` to `src-tauri/src/lib.rs`.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --test codex_test`
Expected: PASS (1 test).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/codex.rs src-tauri/src/lib.rs src-tauri/tests/codex_test.rs src-tauri/tests/fixtures/codex_rollout.jsonl
git commit -m "feat(codex): CodexCollector scanning ~/.codex/sessions today rollouts

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: App — tick loop + Snapshot aggregation

Wires collectors into an `App`, runs `tick()`, produces a `Snapshot`. No Tauri yet.

**Files:**
- Create: `src-tauri/src/app.rs`
- Create: `src-tauri/tests/app_test.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod app;`)

**Interfaces:**
- Produces:
  - `pub struct App { collectors: Vec<Box<dyn Collector>>, }`
  - `impl App { pub fn new(collectors: Vec<Box<dyn Collector>>) -> Self; pub fn tick(&mut self) -> Snapshot; }`
- Consumes: `collector::Collector`, `process::snapshot`, `model::build_snapshot`.

- [ ] **Step 1: Write the failing test**

`src-tauri/tests/app_test.rs`:
```rust
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use usage_tracker::app::App;
use usage_tracker::claude::ClaudeCollector;
use usage_tracker::collector::Collector;
use usage_tracker::model::SessionStatus;

fn build_fake_claude_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-app-{}", std::process::id()));
    let _ = fs::create_dir_all(root.join("sessions"));
    let _ = fs::create_dir_all(root.join("projects").join("-proj"));
    fs::write(root.join("sessions").join("4242.json"),
        r#"{ "pid": 4242, "cwd": "/proj", "session_id": "abc", "startedAt": 1700000000000 }"#).unwrap();
    fs::write(root.join("projects").join("-proj").join("abc.jsonl"),
        r#"{"type":"assistant","message":{"role":"assistant","model":"m","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":0}}}"#).unwrap();
    root
}

#[test]
fn tick_builds_snapshot_from_collectors() {
    let root = build_fake_claude_root();
    // inject a live claude pid by faking sysinfo: we can't easily, so this test
    // asserts the no-live-session path returns an empty-but-valid snapshot.
    let mut app = App::new(vec![Box::new(ClaudeCollector::new(root))]);
    let snap = app.tick();
    // pid 4242 is not actually running → no sessions, but snapshot is well-formed
    assert!(snap.sessions.is_empty());
    assert_eq!(snap.total_tokens, 0);
    let _ = SessionStatus::Waiting; // enum is in scope / importable
}
```

> **Note:** fully exercising a *live* session through `App::tick` requires faking `sysinfo`, which is out of scope (covered by the manual QA checklist against a real `claude` process). This test pins the wiring and the empty-snapshot contract.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test app_test`
Expected: FAIL — `app` module not found.

- [ ] **Step 3: Write minimal implementation**

`src-tauri/src/app.rs`:
```rust
use crate::collector::{Collector, ProcessContext};
use crate::model::{build_snapshot, Snapshot};
use crate::process;

pub struct App {
    collectors: Vec<Box<dyn Collector>>,
}

impl App {
    pub fn new(collectors: Vec<Box<dyn Collector>>) -> Self {
        Self { collectors }
    }

    pub fn tick(&mut self) -> Snapshot {
        let ps = process::snapshot();
        let ctx = ProcessContext {
            procs: &ps.procs,
            children: &ps.children,
            ports: &ps.ports_by_pid,
        };
        let mut sessions = Vec::new();
        for c in &mut self.collectors {
            // A collector that panics must not kill the tick loop.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.collect(&ctx)));
            if let Ok(s) = result {
                sessions.extend(s);
            }
            // on panic: skip this collector this tick, keep going
        }
        // dedupe by (agent_cli, session_id), last wins
        sessions.sort_by_key(|s| (s.agent_cli.clone(), s.session_id.clone()));
        sessions.dedup_by_key(|s| (s.agent_cli.clone(), s.session_id.clone()));
        build_snapshot(sessions)
    }
}
```

Add `pub mod app;` to `src-tauri/src/lib.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test app_test`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/app.rs src-tauri/src/lib.rs src-tauri/tests/app_test.rs
git commit -m "feat(app): tick loop wiring collectors into a Snapshot

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: Tauri shell — transparent topmost window, tray, hotkey, tick thread

This task has no unit tests (OS-level window behavior is on the manual QA checklist). It is verified by running the app.

**Files:**
- Create: `src-tauri/src/commands.rs` (Tauri commands)
- Modify: `src-tauri/src/lib.rs` (the `run()` function)
- Modify: `src-tauri/tauri.conf.json` (window flags, tray, permissions)
- Modify: `src-tauri/capabilities/default.json` (permissions for global-shortcut, window, tray)

**Interfaces:**
- Produces: a running overlay window; Tauri commands `toggle_visibility`, `set_opacity`, `set_poll_interval`, `quit`; emits `snapshot://update` events.
- Consumes: `app::App`, `config`.

- [ ] **Step 1: Configure the window in `tauri.conf.json`**

In the `"app"` → `"windows"` array, set the single window to:
```json
{
  "label": "overlay",
  "title": "AI Assistants",
  "width": 320,
  "height": 260,
  "decorations": false,
  "transparent": true,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "resizable": false,
  "visible": true,
  "x": 24,
  "y": 24
}
```
Add to `"app"`:
```json
"trayIcon": {
  "id": "main",
  "iconPath": "icons/icon.png",
  "tooltip": "AI Assistant Usage"
}
```
(Ensure an `src-tauri/icons/icon.png` exists — copy the scaffold's default icon if needed.)

- [ ] **Step 2: Set capabilities/permissions**

In `src-tauri/capabilities/default.json`, ensure `"permissions"` includes:
```json
"core:default",
"core:window:allow-start-dragging",
"core:window:allow-set-always-on-top",
"core:window:allow-show",
"core:window:allow-hide",
"core:window:allow-set-position",
"global-shortcut:allow-register",
"global-shortcut:allow-unregister"
```

- [ ] **Step 3: Write the commands + run() in `lib.rs`**

Replace the scaffold `run()` in `src-tauri/src/lib.rs` with:
```rust
pub mod app;
pub mod claude;
pub mod codex;
pub mod collector;
pub mod config;
pub mod hermes;
pub mod model;
pub mod process;
pub mod transcript;

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, Emitter};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

struct AppState {
    app: Mutex<app::App>,
    config: Mutex<config::Config>,
    config_path: PathBuf,
}

#[tauri::command]
fn toggle_visibility(window: tauri::Window) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.show();
    }
}

#[tauri::command]
fn set_opacity(window: tauri::Window, opacity: f32) {
    let _ = window.set_opacity(opacity.clamp(0.1, 1.0));
}

#[tauri::command]
fn set_poll_interval(state: tauri::State<AppState>, ms: u64) {
    if let Ok(mut cfg) = state.config.lock() {
        cfg.poll_interval_ms = ms;
        let _ = config::save_config(&state.config_path, &cfg);
    }
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn build_collectors(cfg: &config::Config) -> Vec<Box<dyn collector::Collector>> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut v: Vec<Box<dyn collector::Collector>> = Vec::new();
    if cfg.enabled_agents.iter().any(|a| a == "claude") {
        v.push(Box::new(claude::ClaudeCollector::new(home.join(".claude"))));
    }
    if cfg.enabled_agents.iter().any(|a| a == "codex") {
        v.push(Box::new(codex::CodexCollector::new(home.join(".codex").join("sessions"))));
    }
    if cfg.enabled_agents.iter().any(|a| a == "hermes") {
        if let Some(dir) = &cfg.hermes_data_dir {
            v.push(Box::new(hermes::HermesCollector::new(dir.clone())));
        }
    }
    v
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config_path = dirs::config_dir()
        .unwrap_or_default()
        .join("usage-tracker")
        .join("config.toml");
    let cfg = config::load_config(&config_path);
    let collectors = build_collectors(&cfg);
    let app_state = AppState {
        app: Mutex::new(app::App::new(collectors)),
        config: Mutex::new(cfg.clone()),
        config_path: config_path.clone(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(app_state)
        .setup(move |app_handle| {
            // initial opacity
            if let Some(w) = app_handle.get_webview_window("overlay") {
                let _ = w.set_opacity(cfg.opacity);
            }

            // global hotkey: Ctrl+Shift+Space toggles visibility
            let shortcut: Shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            let app_handle_clone = app_handle.clone();
            let _ = app_handle.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    if let Some(w) = app_handle_clone.get_webview_window("overlay") {
                        if w.is_visible().unwrap_or(false) { let _ = w.hide(); }
                        else { let _ = w.show(); }
                    }
                }
            });

            // tick thread
            let app_handle = app_handle.clone();
            std::thread::spawn(move || loop {
                let interval = {
                    let state: tauri::State<AppState> = app_handle.state();
                    state.config.lock().map(|c| c.poll_interval_ms).unwrap_or(1000)
                };
                let snapshot = {
                    let state: tauri::State<AppState> = app_handle.state();
                    let mut a = state.app.lock().unwrap();
                    a.tick()
                };
                let _ = app_handle.emit("snapshot://update", &snapshot);
                std::thread::sleep(std::time::Duration::from_millis(interval.max(200)));
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            toggle_visibility, set_opacity, set_poll_interval, quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Create `src-tauri/src/hermes.rs` as a stub so the build compiles (filled in Task 12):
```rust
use crate::collector::{Collector, ProcessContext};
use crate::model::AgentSession;
use std::path::PathBuf;

pub struct HermesCollector { _data_dir: PathBuf }

impl HermesCollector {
    pub fn new(data_dir: PathBuf) -> Self { Self { _data_dir: data_dir } }
}

impl Collector for HermesCollector {
    fn name(&self) -> &str { "hermes" }
    fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> { Vec::new() }
}
```

- [ ] **Step 4: Verify it builds and runs**

Run: `npm run tauri dev`
Expected: a transparent, borderless, always-on-top window appears in the top-left; the tray icon is present; `Ctrl+Shift+Space` toggles visibility. Stop it.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(shell): transparent topmost window, tray, hotkey, tick thread

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 11: Frontend panel (render snapshot, drag, settings)

**Files:**
- Modify: `index.html`, `src/main.ts`, `src/style.css`
- Create: `src/render.ts`

No unit tests; verified visually by running the app (manual QA checklist).

- [ ] **Step 1: Write `src/render.ts` (pure Snapshot → HTML)**

```typescript
export interface AgentSession {
  agent_cli: string; pid: number; session_id: string; cwd: string;
  project_name: string; started_at: number; status: string; model: string;
  context_percent: number; total_input_tokens: number; total_output_tokens: number;
  total_cache_read: number; turn_count: number; current_task: string; mem_mb: number;
}
export interface Snapshot {
  sessions: AgentSession[]; total_tokens: number;
  by_agent_tokens: Record<string, number>; by_status: Record<string, number>;
}

const STATUS_LABEL: Record<string, string> = {
  waiting: "Waiting", thinking: "Thinking", executing: "Executing", done: "Done", unknown: "Unknown",
};

export function renderSnapshot(s: Snapshot): string {
  if (s.sessions.length === 0) {
    return `<div class="empty">No active AI assistants</div>`;
  }
  const rows = s.sessions.map(sess => {
    const bar = Math.min(100, Math.round(sess.context_percent));
    return `
      <div class="row">
        <div class="head">
          <span class="dot dot-${sess.agent_cli}"></span>
          <span class="agent">${sess.agent_cli}</span>
          <span class="proj">${escapeHtml(sess.project_name)}</span>
          <span class="status status-${sess.status}">${STATUS_LABEL[sess.status] ?? sess.status}</span>
        </div>
        <div class="bar"><div class="bar-fill" style="width:${bar}%"></div></div>
        <div class="meta">
          <span>↓${fmt(sess.total_input_tokens)}</span>
          <span>↑${fmt(sess.total_output_tokens)}</span>
          <span>${sess.mem_mb}MB</span>
          <span class="task">${escapeHtml(sess.current_task || "")}</span>
        </div>
      </div>`;
  }).join("");
  return `<div class="rows">${rows}</div>
          <div class="footer">total ${fmt(s.total_tokens)} tok · ${s.sessions.length} live</div>`;
}

function fmt(n: number): string {
  if (n >= 1000) return (n / 1000).toFixed(1) + "k";
  return String(n);
}
function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, c => ({ "&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;" }[c]!));
}
```

- [ ] **Step 2: Write `src/main.ts` (wire events + drag + commands)**

```typescript
import { renderSnapshot, type Snapshot } from "./render";

const panel = document.getElementById("panel")!;
const content = document.getElementById("content")!;

// Drag via the title bar using Tauri's native startDragging
const titlebar = document.getElementById("titlebar")!;
titlebar.addEventListener("mousedown", () => {
  // @ts-ignore
  window.__TAURI__.window.getCurrent().window.startDragging();
});

document.getElementById("hide-btn")!.addEventListener("click", () => {
  // @ts-ignore
  window.__TAURI__.core.invoke("toggle_visibility");
});

// Subscribe to snapshot updates
// @ts-ignore
window.__TAURI__.event.listen("snapshot://update", (e: { payload: Snapshot }) => {
  content.innerHTML = renderSnapshot(e.payload);
});

// Opacity slider
const opacity = document.getElementById("opacity") as HTMLInputElement | null;
opacity?.addEventListener("input", () => {
  // @ts-ignore
  window.__TAURI__.core.invoke("set_opacity", { opacity: parseFloat(opacity.value) });
});
```

- [ ] **Step 3: Write `index.html`**

```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <link rel="stylesheet" href="/src/style.css" />
  </head>
  <body>
    <div id="panel">
      <div id="titlebar">
        <span>AI Assistants</span>
        <button id="hide-btn" title="Hide">_</button>
      </div>
      <div id="content"><div class="empty">Loading…</div></div>
      <div id="settings">
        <label>Opacity <input id="opacity" type="range" min="0.3" max="1" step="0.05" value="0.85" /></label>
      </div>
    </div>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
```

- [ ] **Step 4: Write `src/style.css` (transparent panel)**

```css
:root { color-scheme: dark; }
* { box-sizing: border-box; margin: 0; padding: 0; }
html, body { background: transparent; font-family: "Segoe UI", system-ui, sans-serif; }
body { -webkit-user-select: none; user-select: none; }

#panel {
  width: 100vw; height: 100vh;
  background: rgba(20, 22, 28, 0.82);
  border: 1px solid rgba(255,255,255,0.12);
  border-radius: 10px;
  color: #e6e6e6;
  font-size: 12px;
  display: flex; flex-direction: column;
  overflow: hidden;
}
#titlebar {
  display: flex; justify-content: space-between; align-items: center;
  padding: 6px 10px; cursor: grab;
  background: rgba(255,255,255,0.05);
  font-weight: 600;
}
#titlebar:active { cursor: grabbing; }
#hide-btn {
  background: transparent; border: none; color: #aaa; cursor: pointer; font-size: 14px;
}
#content { flex: 1; overflow-y: auto; padding: 6px 10px; }
.row { padding: 6px 0; border-bottom: 1px solid rgba(255,255,255,0.06); }
.head { display: flex; align-items: center; gap: 6px; }
.agent { font-weight: 600; }
.proj { opacity: 0.6; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.status { font-size: 10px; padding: 1px 5px; border-radius: 8px; background: rgba(255,255,255,0.1); }
.status-executing { background: rgba(80,200,120,0.25); }
.status-thinking { background: rgba(80,160,255,0.25); }
.status-waiting { background: rgba(160,160,160,0.2); }
.status-done { background: rgba(120,120,120,0.2); }
.dot { width: 8px; height: 8px; border-radius: 50%; }
.dot-claude { background: #d97757; }
.dot-codex { background: #4ade80; }
.dot-hermes { background: #818cf8; }
.bar { height: 4px; background: rgba(255,255,255,0.1); border-radius: 2px; margin: 4px 0; }
.bar-fill { height: 100%; background: #6aa0ff; border-radius: 2px; }
.meta { display: flex; gap: 10px; font-size: 10px; opacity: 0.7; }
.task { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.footer { padding: 4px 10px; opacity: 0.5; font-size: 10px; }
#settings { padding: 4px 10px; border-top: 1px solid rgba(255,255,255,0.06); }
#settings input { width: 120px; vertical-align: middle; }
.empty { padding: 20px; text-align: center; opacity: 0.5; }
```

- [ ] **Step 5: Verify visually**

Run: `npm run tauri dev`
Expected: the transparent panel shows session rows when a `claude`/`codex` is running, updates ~1s, drag works via the title bar, opacity slider changes transparency live, hide button + `Ctrl+Shift+Space` toggle visibility. Stop it.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(frontend): transparent panel rendering live snapshot rows

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 12: HermesCollector (sample-contingent)

**This task has an external data dependency.** Hermes's on-disk format is not yet known. Step 1 gathers a real sample from the user's machine; the implementation in Steps 3–4 is written against that sample's actual JSON schema. The structure (same `Collector` trait, same `IncrementalReader`, same state-accumulator pattern as Claude/Codex) is fixed; only the line-parser keys are sample-derived.

**Files:**
- Modify: `src-tauri/src/hermes.rs` (replace the stub from Task 10)
- Create: `src-tauri/tests/fixtures/hermes_sample.jsonl` (from the real sample)
- Create: `src-tauri/tests/hermes_test.rs`
- Modify: `src-tauri/src/lib.rs` (already has `pub mod hermes;`)

**Interfaces:**
- Produces: `pub struct HermesCollector { data_dir: PathBuf, ... }`, `impl HermesCollector { pub fn new(data_dir: PathBuf) -> Self; }`, `impl Collector for HermesCollector`.
- Consumes: `model`, `transcript`, `collector`.

- [ ] **Step 1: Locate a real Hermes data sample**

With the user, find where Hermes writes session/transcript data. Likely candidates to check: `~/.hermes/`, `%APPDATA%\hermes\`, `%LOCALAPPDATA%\hermes\`, `~/.config/hermes/`, and `~/.local/share/hermes/`. Run a Hermes session briefly, then `find` for files modified in the last few minutes:
```bash
find ~ "/mnt/c/Users/ivan.vykopal/AppData" -type f -newermt "-10 minutes" 2>/dev/null | grep -iE 'hermes|session|transcript|rollout'
```
Record: (a) the data root directory, (b) the file naming pattern (session id? pid? date?), (c) the JSON schema of a transcript line (paste 2–3 representative lines, scrubbed of private content, into `hermes_sample.jsonl`).

If Hermes data cannot be located at all, stop here and report back — do not fabricate a format. Leave the stub collector in place (it returns `[]`, so the panel simply shows no Hermes rows) and move on.

- [ ] **Step 2: Write the failing test against the sample**

`src-tauri/tests/hermes_test.rs` — adapt the field names to the real schema recorded in Step 1. Skeleton (fill the `assert!` values from the sample):
```rust
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use usage_tracker::hermes::HermesCollector;
use usage_tracker::collector::{Collector, ProcessContext};

fn build_fake_hermes_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-hermes-{}", std::process::id()));
    fs::create_dir_all(root.join("sessions")).unwrap(); // adjust subpath to real layout
    fs::write(
        root.join("sessions").join("sample.jsonl"), // adjust to real naming
        include_str!("fixtures/hermes_sample.jsonl"),
    ).unwrap();
    root
}

#[test]
fn collects_hermes_session_with_tokens() {
    let root = build_fake_hermes_root();
    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports };
    let mut c = HermesCollector::new(root);
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.agent_cli, "hermes");
    // assert token values derived from the sample below:
    // assert_eq!(s.total_input_tokens, ???);
    // assert_eq!(s.total_output_tokens, ???);
}
```

- [ ] **Step 3: Write the implementation against the sample**

Model `hermes.rs` on `codex.rs`: discover transcript files under `data_dir` by the real naming pattern from Step 1, filter to recently-modified files, use `IncrementalReader`, accumulate tokens/status in a `HermesState`, and map to `AgentSession` with `agent_cli: "hermes"`. The `apply_hermes_line` parser reads the exact JSON keys found in Step 1's sample.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test hermes_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/hermes.rs src-tauri/tests/hermes_test.rs src-tauri/tests/fixtures/hermes_sample.jsonl
git commit -m "feat(hermes): HermesCollector against real on-disk format

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 13: Manual QA checklist + README

**Files:**
- Create: `README.md`

- [ ] **Step 1: Write the README with the QA checklist**

```markdown
# AI Assistant Usage Overlay

A Windows desktop overlay (like NVIDIA's GPU overlay) showing live usage of
Claude Code, Codex CLI, and Hermes: tokens, context-window %, status, memory.
Transparent, always-on-top, draggable. Read-only — no API keys, no network.

## Run (dev)
\`\`\`bash
npm install
npm run tauri dev
\`\`\`

## Build (release exe)
\`npm run tauri build\` → produces a Windows installer + portable exe under
\`src-tauri/target/release/\`.

## Config
\`%APPDATA%\usage-tracker\config.toml\`:
\`\`\`toml
poll_interval_ms = 1000
opacity = 0.85
hotkey = "Ctrl+Shift+Space"
enabled_agents = ["claude", "codex", "hermes"]
# hermes_data_dir = "C:/Users/you/AppData/Local/hermes"
\`\`\`

## Manual QA checklist
Run with at least one `claude` and one `codex` session active:

- [ ] Panel appears top-left on launch, transparent, above other windows.
- [ ] Stays on top of a fullscreened browser/editor.
- [ ] Dragging the title bar moves the window; works across monitors.
- [ ] Opacity slider updates transparency live.
- [ ] `Ctrl+Shift+Space` toggles show/hide; tray "Show/Hide" does too.
- [ ] Session rows update ~1s; tokens accumulate as the agent works.
- [ ] Status changes (Waiting/Thinking/Executing) reflect agent activity.
- [ ] Closing a `claude` session removes its row within ~1s.
- [ ] Quitting via tray exits cleanly (no orphan tick thread).
- [ ] With no agents running, panel shows "No active AI assistants".
- [ ] Deleting/renaming the config file → app still starts with defaults.
```

- [ ] **Step 2: Run through the QA checklist**

Run `npm run tauri dev` with a `claude` and a `codex` session active. Tick each box. Fix any failure inline before committing.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: README with run/build/config and manual QA checklist

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

## Out of scope (future)

- macOS / Linux support.
- OpenCode support.
- Context-window % (requires per-model window sizes; v1 leaves it 0 unless the transcript reports it).
- MCP servers, subagents, chat messages, tool-call history, file accesses, git stats, open-port/orphan-port detection.
- Incremental delta-merge beyond the simple offset+identity reader (sufficient for v1 perf).
- Persistent window position across restarts (config has the fields; wiring is deferred).
- Codex PID→rollout open-file mapping (v1 uses recent-mtime discovery instead).

## Self-review notes

- **Spec coverage:** transparent topmost draggable panel (Tasks 10–11), tray + hotkey (Task 10), Claude/Codex/Hermes collection (Tasks 7–8, 12), tick loop + Snapshot (Task 9), graceful degradation (panic catch in `app::tick`, missing-file `[]` returns in every collector, config fallback in Task 2), read-only/no-network (Global Constraints), TDD pure logic (Tasks 2–4, 7–9), manual QA for OS-level behavior (Task 13). All spec sections map to tasks.
- **Deviations from spec, flagged:** (1) v1 omits abtop's richer fields (subagents, chat, tool calls, ports, git, multi-profile) — listed in "Out of scope". (2) Context-window % left at 0 in v1 (needs per-model window sizes) — noted in claude.rs and Out of scope. (3) Codex uses recent-mtime discovery instead of PID→fd open-file mapping — noted in Task 8 and Out of scope. These are YAGNI cuts that keep v1 shippable; each is reversible later without rearchitecting.
- **Type consistency:** `AgentSession` fields are identical across model.rs definition and all collectors; `Snapshot` shape matches between `build_snapshot` and the frontend `render.ts` interface; `Collector::collect` signature is uniform. `HermesCollector::new` takes `PathBuf` matching Task 10's `build_collectors` call.
- **Hermes honestly flagged:** Task 12 is sample-contingent because the format is a genuine external unknown, not a placeholder to hand-wave. The structure is fixed; only line-parser keys are deferred to a real sample, with an explicit stop-and-report fallback if no sample exists.
