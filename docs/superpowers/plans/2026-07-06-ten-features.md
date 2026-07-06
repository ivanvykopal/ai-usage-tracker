# Ten Features Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cost estimation, burn-rate prediction, stall alerts, per-project grouping, a compact view toggle, finished Hermes support, Linux support, a `providers/` directory for pluggable CLI collectors, in-app config, theming, and historical usage tracking to the `ai-usage-overlay` Tauri app.

**Architecture:** Phase 0 refactors the three existing collectors (`claude.rs`, `codex.rs`, `hermes.rs`) into a `src-tauri/src/providers/` directory behind a common `build(cfg, home_dirs) -> Option<Box<dyn Collector>>` function per module, registered in a static `providers::ALL` table. Every later phase builds on top of this: new Tauri commands read/write `config::Config` and emit events; `dist/main.js` listens for those events and re-renders. History and burn-rate share one SQLite-backed module. All backend additions extend existing structs (`AgentSession`, `RateLimitInfo`, `Snapshot`, `Config`) rather than introducing parallel data paths.

**Tech Stack:** Rust 2021, Tauri 2, rusqlite (bundled SQLite), serde/serde_json, toml, chrono, ureq. Frontend: vanilla JS/CSS, no build step (`dist/` served directly).

## Global Constraints

- Rust edition 2021, existing crate name `usage_tracker` — do not rename.
- No new frontend build tooling (no bundler/framework) — keep `dist/main.js`/`style.css`/`index.html` as plain files.
- Every existing public constructor (`ClaudeCollector::new`, `CodexCollector::new`, `HermesCollector::new`, etc.) must keep working — other code/tests may call them directly.
- `Collector::collect` must never panic; `App::tick` already isolates panics per collector — new code inside `collect`/`usage_limits` must preserve that (no `.unwrap()` on external data).
- New Cargo dependencies require a one-line justification in the task that adds them.
- All new Tauri commands must be added to `invoke_handler!` in `lib.rs` and (if they touch fs paths outside the existing ones) covered by `capabilities/default.json` permissions.
- Config fields must have `#[serde(default = "...")]` so existing `config.toml` files on disk keep loading after upgrade (see `claude_usage_enabled` for the existing pattern).

---

## Phase 0 — `providers/` directory (foundation for pluggable CLIs)

**Why first:** every later backend phase (cost, burn-rate, stall alerts, Hermes fixes, in-app config's provider list) touches collector code. Doing the reshuffle first means later phases edit `providers/claude.rs` etc. directly instead of touching soon-to-move files twice.

### Task 0.1: Extract `HomeDir`/`resolve_home_dirs`/`wsl_distros` into `src-tauri/src/home.rs`

**Files:**
- Create: `src-tauri/src/home.rs`
- Modify: `src-tauri/src/lib.rs:1-11` (mod list), `lib.rs:61-223` (remove moved code), `lib.rs:226-239` (call sites)

**Interfaces:**
- Produces: `pub struct HomeDir { pub path: PathBuf, pub wsl_distro: Option<String> }`, `pub fn resolve_home_dirs() -> Vec<HomeDir>`, `pub fn wsl_distros(home_dirs: &[HomeDir]) -> Vec<String>`

- [ ] **Step 1: Create `src-tauri/src/home.rs`** with the exact content of `lib.rs` lines 61–223 (the `HomeDir` struct doc comment, struct, `resolve_home_dirs`, and `wsl_distros` functions), changing only visibility (`struct HomeDir` → `pub struct HomeDir`, fields → `pub`, `fn resolve_home_dirs` → `pub fn resolve_home_dirs`, `fn wsl_distros` → `pub fn wsl_distros`) and adding at the top:
```rust
use crate::process;
use std::path::PathBuf;
```

- [ ] **Step 2: Remove the moved code from `lib.rs`** — delete lines 61–223 (the `HomeDir` struct through the end of `wsl_distros`), and delete `use std::path::PathBuf;` from line 13 if nothing else in `lib.rs` still needs it (check: `AppState.config_path: PathBuf` still does, so keep it).

- [ ] **Step 3: Add `pub mod home;` to the top of `lib.rs`** (alphabetically among the existing `pub mod` lines, after `pub mod hermes;` — this line moves again in Task 0.5, that's fine).

- [ ] **Step 4: Update `lib.rs` call sites** to use `home::HomeDir`, `home::resolve_home_dirs()`, `home::wsl_distros(...)` instead of the bare names, e.g. in `run()`:
```rust
let home_dirs = home::resolve_home_dirs();
let collectors = build_collectors(&cfg, &home_dirs);
let distros = home::wsl_distros(&home_dirs);
```
and in `build_collectors`'s signature, change `home_dirs: &[HomeDir]` to `home_dirs: &[home::HomeDir]`.

- [ ] **Step 5: Compile check**

Run: `cd src-tauri && cargo build --lib`
Expected: builds with no errors (warnings about now-unused imports are fine to leave for Task 0.6, which deletes `build_collectors` entirely).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/home.rs src-tauri/src/lib.rs
git commit -m "refactor: extract HomeDir/resolve_home_dirs into home.rs"
```

### Task 0.2: Create `src-tauri/src/providers/mod.rs` with the common provider interface

**Files:**
- Create: `src-tauri/src/providers/mod.rs`
- Modify: `src-tauri/src/lib.rs` (mod list)

**Interfaces:**
- Consumes: `crate::home::HomeDir` (Task 0.1), `crate::config::Config`, `crate::collector::Collector`
- Produces: `pub struct ProviderEntry { pub key: &'static str, pub label: &'static str, pub build: fn(&Config, &[HomeDir]) -> Option<Box<dyn Collector>> }`, `pub static ALL: &[ProviderEntry]`, `pub fn build_collectors(cfg: &Config, home_dirs: &[HomeDir]) -> Vec<Box<dyn Collector>>`

- [ ] **Step 1: Write `src-tauri/src/providers/mod.rs`**

```rust
//! One module per supported AI-assistant CLI. Each module exposes a
//! `build(cfg, home_dirs) -> Option<Box<dyn Collector>>` function with the
//! same signature; adding a new CLI means adding a module here and one more
//! entry in `ALL` — nothing else in the app needs to change.
pub mod claude;
pub mod codex;
pub mod hermes;

use crate::collector::Collector;
use crate::config::Config;
use crate::home::HomeDir;

pub struct ProviderEntry {
    pub key: &'static str,
    pub label: &'static str,
    pub build: fn(&Config, &[HomeDir]) -> Option<Box<dyn Collector>>,
}

pub static ALL: &[ProviderEntry] = &[
    ProviderEntry {
        key: "claude",
        label: "Claude Code",
        build: claude::build,
    },
    ProviderEntry {
        key: "codex",
        label: "Codex CLI",
        build: codex::build,
    },
    ProviderEntry {
        key: "hermes",
        label: "Hermes",
        build: hermes::build,
    },
];

/// Build one collector per enabled, detected provider. A provider whose
/// `build` returns `None` (not enabled in config, or no matching directory
/// found on disk) contributes nothing — matches the current behavior of
/// `lib.rs::build_collectors` before this refactor.
pub fn build_collectors(cfg: &Config, home_dirs: &[HomeDir]) -> Vec<Box<dyn Collector>> {
    ALL.iter()
        .filter(|p| cfg.enabled_agents.iter().any(|a| a == p.key))
        .filter_map(|p| (p.build)(cfg, home_dirs))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_keys_are_unique() {
        let mut keys: Vec<&str> = ALL.iter().map(|p| p.key).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate provider key in ALL");
    }

    #[test]
    fn disabled_provider_yields_no_collector() {
        let cfg = Config {
            enabled_agents: vec![],
            ..crate::config::default_config()
        };
        let collectors = build_collectors(&cfg, &[]);
        assert!(collectors.is_empty());
    }
}
```

- [ ] **Step 2: Add `pub mod providers;` to `lib.rs`'s mod list.** Leave `providers::claude`/`codex`/`hermes` unresolved for now — Tasks 0.3–0.5 create them; this file won't compile stand-alone until then, which is expected mid-refactor.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/providers/mod.rs src-tauri/src/lib.rs
git commit -m "feat: add providers module skeleton with common build() interface"
```

### Task 0.3: Move `claude.rs` into `providers/claude.rs` and add `build()`

**Files:**
- Modify: `src-tauri/src/claude.rs` → moved to `src-tauri/src/providers/claude.rs`
- Modify: `src-tauri/src/lib.rs` (remove `pub mod claude;`, remove Claude branch of old `build_collectors`)

**Interfaces:**
- Consumes: `crate::home::HomeDir`, `crate::config::Config`, `crate::claude_usage::ClaudeUsagePoller` (unchanged, stays top-level — it's a shared HTTP polling utility, not provider-specific parsing)
- Produces: `pub fn build(cfg: &Config, home_dirs: &[HomeDir]) -> Option<Box<dyn Collector>>`

- [ ] **Step 1: Move the file**

Run: `git mv src-tauri/src/claude.rs src-tauri/src/providers/claude.rs`

- [ ] **Step 2: Fix the now-relative imports** at the top of `providers/claude.rs` — `crate::collector`, `crate::model`, `crate::process`, `crate::rate_limit`, `crate::transcript` all still resolve correctly from the new location since they're absolute `crate::` paths (no change needed). Add:
```rust
use crate::config::Config;
use crate::home::HomeDir;
```

- [ ] **Step 3: Append `build()` to the bottom of `providers/claude.rs`**, extracted verbatim from the current `lib.rs::build_collectors` Claude branch (`lib.rs:156-184`):

```rust
/// Constructs the Claude collector if the `claude` provider is enabled in
/// config and at least one `.claude` directory exists under a resolved home
/// directory. Matches the interface every `providers::*` module implements —
/// see `providers::ProviderEntry`.
pub fn build(cfg: &Config, home_dirs: &[HomeDir]) -> Option<Box<dyn Collector>> {
    let claude_dirs: Vec<ConfigDirEntry> = home_dirs
        .iter()
        .map(|h| ConfigDirEntry {
            dir: h.path.join(".claude"),
            wsl_distro: h.wsl_distro.clone(),
        })
        .filter(|e| e.dir.exists())
        .collect();
    if claude_dirs.is_empty() {
        return None;
    }

    let usage_source = if cfg.claude_usage_enabled {
        match claude_dirs.first() {
            Some(first) => {
                let creds_path = first.dir.join(".credentials.json");
                ClaudeUsageSource::ApiHandle(crate::claude_usage::ClaudeUsagePoller::start(creds_path))
            }
            None => ClaudeUsageSource::HookFileOnly,
        }
    } else {
        ClaudeUsageSource::HookFileOnly
    };

    Some(Box::new(ClaudeCollector::new_multi_with_usage(
        claude_dirs,
        usage_source,
    )))
}
```

- [ ] **Step 4: Update `lib.rs`** — remove `pub mod claude;` from the mod list, remove the `if cfg.enabled_agents.iter().any(|a| a == "claude") { ... }` block from `build_collectors` (lines 156-184).

- [ ] **Step 5: Compile check**

Run: `cd src-tauri && cargo build --lib 2>&1 | head -50`
Expected: errors only about `codex`/`hermes` still being unmoved (Tasks 0.4/0.5) — no errors mentioning `claude`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: move claude collector into providers/claude.rs"
```

### Task 0.4: Move `codex.rs` into `providers/codex.rs` and add `build()`

**Files:**
- Modify: `src-tauri/src/codex.rs` → moved to `src-tauri/src/providers/codex.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `pub fn build(cfg: &Config, home_dirs: &[HomeDir]) -> Option<Box<dyn Collector>>`

- [ ] **Step 1: Move the file**

Run: `git mv src-tauri/src/codex.rs src-tauri/src/providers/codex.rs`

- [ ] **Step 2: Add imports** at the top of `providers/codex.rs`:
```rust
use crate::config::Config;
use crate::home::HomeDir;
```

- [ ] **Step 3: Append `build()`**, extracted from `lib.rs:186-193`:

```rust
pub fn build(cfg: &Config, home_dirs: &[HomeDir]) -> Option<Box<dyn Collector>> {
    let codex_dirs: Vec<std::path::PathBuf> = home_dirs
        .iter()
        .map(|h| h.path.join(".codex").join("sessions"))
        .filter(|p| p.exists())
        .collect();
    if codex_dirs.is_empty() {
        return None;
    }
    Some(Box::new(CodexCollector::new_multi(codex_dirs)))
}
```

- [ ] **Step 4: Update `lib.rs`** — remove `pub mod codex;` and the Codex branch of `build_collectors`.

- [ ] **Step 5: Compile check**

Run: `cd src-tauri && cargo build --lib 2>&1 | head -50`
Expected: errors only mention `hermes` (Task 0.5 not done yet).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: move codex collector into providers/codex.rs"
```

### Task 0.5: Move `hermes.rs` into `providers/hermes.rs`, add `build()`, and finish the refactor

**Files:**
- Modify: `src-tauri/src/hermes.rs` → moved to `src-tauri/src/providers/hermes.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `pub fn build(cfg: &Config, home_dirs: &[HomeDir]) -> Option<Box<dyn Collector>>`

- [ ] **Step 1: Move the file**

Run: `git mv src-tauri/src/hermes.rs src-tauri/src/providers/hermes.rs`

- [ ] **Step 2: Add imports** at the top of `providers/hermes.rs`:
```rust
use crate::config::Config;
use crate::home::HomeDir;
```

- [ ] **Step 3: Append `build()`**, extracted from `lib.rs:195-207`:

```rust
pub fn build(cfg: &Config, home_dirs: &[HomeDir]) -> Option<Box<dyn Collector>> {
    let hermes_dirs: Vec<std::path::PathBuf> = if let Some(ref custom_dir) = cfg.hermes_data_dir {
        vec![custom_dir.clone()]
    } else {
        home_dirs
            .iter()
            .map(|h| h.path.join(".hermes"))
            .filter(|p| p.exists())
            .collect()
    };
    if hermes_dirs.is_empty() {
        return None;
    }
    Some(Box::new(HermesCollector::new_multi(hermes_dirs)))
}
```

- [ ] **Step 4: Delete `build_collectors` and its `HomeDir` usage from `lib.rs` entirely** (it's now `providers::build_collectors`), and remove `pub mod hermes;`, `pub mod codex;`, `pub mod claude;` if any remain. `lib.rs`'s `run()` should now read:
```rust
let collectors = providers::build_collectors(&cfg, &home_dirs);
```
and its mod list at the top should be:
```rust
pub mod app;
pub mod claude_usage;
pub mod collector;
pub mod config;
pub mod home;
pub mod model;
pub mod process;
pub mod providers;
pub mod rate_limit;
pub mod transcript;
```

- [ ] **Step 5: Full build + test run**

Run: `cd src-tauri && cargo build && cargo test`
Expected: builds clean; all pre-existing tests (in `claude_usage.rs`, `transcript.rs` if any, and the new `providers::tests`) pass. If any test module referenced the old `crate::claude::`/`crate::codex::`/`crate::hermes::` paths directly, update them to `crate::providers::claude::` etc.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: move hermes collector into providers/hermes.rs; finish providers/ extraction"
```

---

## Phase 1 — Historical usage tracking

### Task 1.1: `src-tauri/src/history.rs` — SQLite-backed sample store

**Files:**
- Create: `src-tauri/src/history.rs`
- Modify: `src-tauri/src/lib.rs` (mod list: add `pub mod history;`)

**Interfaces:**
- Consumes: `crate::model::Snapshot`
- Produces: `pub fn open(path: &Path) -> rusqlite::Result<Connection>`, `pub fn record_snapshot(conn: &Connection, ts_ms: i64, snapshot: &Snapshot) -> rusqlite::Result<()>`, `pub struct TokenPoint { pub ts_ms: i64, pub tokens: u64 }`, `pub fn token_history(conn: &Connection, agent: &str, since_ms: i64) -> rusqlite::Result<Vec<TokenPoint>>`, `pub fn prune_older_than(conn: &Connection, cutoff_ms: i64) -> rusqlite::Result<()>`

- [ ] **Step 1: Write the failing test** (create the file with just the test module first, using an in-memory db so no filesystem cleanup is needed):

```rust
use crate::model::Snapshot;
use rusqlite::{Connection, Result};
use std::path::Path;

pub struct TokenPoint {
    pub ts_ms: i64,
    pub tokens: u64,
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS token_samples (
            ts_ms INTEGER NOT NULL,
            agent TEXT NOT NULL,
            tokens INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_token_samples_agent_ts ON token_samples(agent, ts_ms);
        CREATE TABLE IF NOT EXISTS rate_limit_samples (
            ts_ms INTEGER NOT NULL,
            agent TEXT NOT NULL,
            window TEXT NOT NULL,
            pct REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_rate_limit_samples_agent_window_ts
            ON rate_limit_samples(agent, window, ts_ms);",
    )?;
    Ok(conn)
}

pub fn record_snapshot(conn: &Connection, ts_ms: i64, snapshot: &Snapshot) -> Result<()> {
    for (agent, tokens) in &snapshot.by_agent_tokens {
        conn.execute(
            "INSERT INTO token_samples (ts_ms, agent, tokens) VALUES (?1, ?2, ?3)",
            (ts_ms, agent, *tokens as i64),
        )?;
    }
    for (agent, rl) in &snapshot.usage_limits {
        for (window, pct) in [
            ("five_hour", rl.five_hour_pct),
            ("seven_day", rl.seven_day_pct),
            ("monthly", rl.monthly_pct),
        ] {
            if let Some(pct) = pct {
                conn.execute(
                    "INSERT INTO rate_limit_samples (ts_ms, agent, window, pct) VALUES (?1, ?2, ?3, ?4)",
                    (ts_ms, agent, window, pct),
                )?;
            }
        }
    }
    Ok(())
}

pub fn token_history(conn: &Connection, agent: &str, since_ms: i64) -> Result<Vec<TokenPoint>> {
    let mut stmt = conn.prepare(
        "SELECT ts_ms, tokens FROM token_samples WHERE agent = ?1 AND ts_ms >= ?2 ORDER BY ts_ms ASC",
    )?;
    let rows = stmt.query_map((agent, since_ms), |row| {
        Ok(TokenPoint {
            ts_ms: row.get(0)?,
            tokens: row.get::<_, i64>(1)? as u64,
        })
    })?;
    rows.collect()
}

/// Rate-limit percentage samples for one agent/window, ordered oldest-first —
/// the shape `burn_rate::project_time_to_limit` (Phase 3) consumes directly.
pub fn rate_limit_history(
    conn: &Connection,
    agent: &str,
    window: &str,
    since_ms: i64,
) -> Result<Vec<(i64, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT ts_ms, pct FROM rate_limit_samples WHERE agent = ?1 AND window = ?2 AND ts_ms >= ?3 ORDER BY ts_ms ASC",
    )?;
    let rows = stmt.query_map((agent, window, since_ms), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
    })?;
    rows.collect()
}

pub fn prune_older_than(conn: &Connection, cutoff_ms: i64) -> Result<()> {
    conn.execute("DELETE FROM token_samples WHERE ts_ms < ?1", [cutoff_ms])?;
    conn.execute(
        "DELETE FROM rate_limit_samples WHERE ts_ms < ?1",
        [cutoff_ms],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{build_snapshot, RateLimitInfo};
    use std::collections::BTreeMap;

    fn open_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE token_samples (ts_ms INTEGER, agent TEXT, tokens INTEGER);
             CREATE TABLE rate_limit_samples (ts_ms INTEGER, agent TEXT, window TEXT, pct REAL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn records_and_reads_back_token_totals() {
        let conn = open_memory();
        let snapshot = build_snapshot(vec![], BTreeMap::new());
        // build_snapshot with no sessions has an empty by_agent_tokens map;
        // insert directly to exercise record/query without a full AgentSession.
        conn.execute(
            "INSERT INTO token_samples (ts_ms, agent, tokens) VALUES (1000, 'claude', 500)",
            [],
        )
        .unwrap();
        let points = token_history(&conn, "claude", 0).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].tokens, 500);
        let _ = snapshot; // keep import used
    }

    #[test]
    fn record_snapshot_writes_rate_limit_samples() {
        let conn = open_memory();
        let mut limits = BTreeMap::new();
        limits.insert(
            "codex".to_string(),
            RateLimitInfo {
                source: "codex".into(),
                five_hour_pct: Some(42.0),
                ..Default::default()
            },
        );
        let snapshot = build_snapshot(vec![], limits);
        record_snapshot(&conn, 2000, &snapshot).unwrap();
        let hist = rate_limit_history(&conn, "codex", "five_hour", 0).unwrap();
        assert_eq!(hist, vec![(2000, 42.0)]);
    }

    #[test]
    fn prune_removes_old_rows_only() {
        let conn = open_memory();
        conn.execute(
            "INSERT INTO token_samples (ts_ms, agent, tokens) VALUES (100, 'claude', 1), (5000, 'claude', 2)",
            [],
        )
        .unwrap();
        prune_older_than(&conn, 1000).unwrap();
        let points = token_history(&conn, "claude", 0).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].ts_ms, 5000);
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cd src-tauri && cargo test history::`
Expected: 3 tests pass (`records_and_reads_back_token_totals`, `record_snapshot_writes_rate_limit_samples`, `prune_removes_old_rows_only`).

- [ ] **Step 3: Add `pub mod history;` to `lib.rs`'s mod list.**

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/history.rs src-tauri/src/lib.rs
git commit -m "feat: add SQLite-backed usage history store"
```

### Task 1.2: Config fields + wiring the recorder into the tick loop

**Files:**
- Modify: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/lib.rs` (`AppState`, `run()`)

**Interfaces:**
- Consumes: `history::open`, `history::record_snapshot`, `history::prune_older_than` (Task 1.1)
- Produces: `Config.history_enabled: bool`, `Config.history_retention_days: u32`

- [ ] **Step 1: Add fields to `Config`** in `config.rs`:
```rust
#[serde(default = "default_history_enabled")]
pub history_enabled: bool,
#[serde(default = "default_history_retention_days")]
pub history_retention_days: u32,
```
and defaults:
```rust
fn default_history_enabled() -> bool {
    true
}
fn default_history_retention_days() -> u32 {
    30
}
```
Add `history_enabled: true, history_retention_days: 30,` to `default_config()`.

- [ ] **Step 2: Open the history DB and prune once at startup, in `lib.rs::run()`**, right after `let cfg = config::load_config(&config_path);`:
```rust
let history_path = dirs::config_dir()
    .unwrap_or_default()
    .join("ai-usage-overlay")
    .join("history.db");
let history_conn: Option<std::sync::Mutex<rusqlite::Connection>> = if cfg.history_enabled {
    history::open(&history_path).ok().map(std::sync::Mutex::new)
} else {
    None
};
if let Some(conn) = &history_conn {
    if let Ok(guard) = conn.lock() {
        let cutoff_ms = chrono::Utc::now().timestamp_millis()
            - (cfg.history_retention_days as i64) * 86_400_000;
        let _ = history::prune_older_than(&guard, cutoff_ms);
    }
}
```

- [ ] **Step 3: Add `history_conn` to `AppState`** and record a sample once per minute in the tick thread (not every 1s tick — bounds DB growth):
```rust
struct AppState {
    app: Mutex<app::App>,
    config: Mutex<config::Config>,
    config_path: PathBuf,
    history_conn: Option<Mutex<rusqlite::Connection>>,
}
```
Update the `AppState { ... }` construction in `run()` to include `history_conn,`.

In the tick-thread closure, after `let _ = app_handle.emit("snapshot://update", &snapshot);`, add:
```rust
{
    let state: tauri::State<AppState> = app_handle.state();
    if let Some(hconn) = &state.history_conn {
        if let Ok(guard) = hconn.lock() {
            let ts_ms = chrono::Utc::now().timestamp_millis();
            // Sample at most once per 60s regardless of poll_interval_ms —
            // a 1s poll interval would otherwise write 60x more rows than needed.
            static LAST_SAMPLE_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
            let last = LAST_SAMPLE_MS.load(std::sync::atomic::Ordering::Relaxed);
            if ts_ms - last >= 60_000 {
                let _ = history::record_snapshot(&guard, ts_ms, &snapshot);
                LAST_SAMPLE_MS.store(ts_ms, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}
```

- [ ] **Step 4: Compile check**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: sample snapshots into history.db once per minute"
```

### Task 1.3: `get_usage_history` Tauri command

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `history::token_history`
- Produces: `#[tauri::command] fn get_usage_history(state: tauri::State<AppState>, agent: String, hours: u32) -> Vec<(i64, u64)>`

- [ ] **Step 1: Add the command** near the other commands (after `quit`):
```rust
#[tauri::command]
fn get_usage_history(state: tauri::State<AppState>, agent: String, hours: u32) -> Vec<(i64, u64)> {
    let Some(hconn) = &state.history_conn else {
        return Vec::new();
    };
    let Ok(guard) = hconn.lock() else {
        return Vec::new();
    };
    let since_ms = chrono::Utc::now().timestamp_millis() - (hours as i64) * 3_600_000;
    history::token_history(&guard, &agent, since_ms)
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.ts_ms, p.tokens))
        .collect()
}
```

- [ ] **Step 2: Register it in `invoke_handler!`**:
```rust
.invoke_handler(tauri::generate_handler![
    toggle_visibility,
    set_opacity,
    set_poll_interval,
    quit,
    get_usage_history
])
```

- [ ] **Step 3: Compile check**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat: expose get_usage_history Tauri command"
```

### Task 1.4: Frontend sparkline

**Files:**
- Modify: `dist/index.html`
- Modify: `dist/main.js`
- Modify: `dist/style.css`

**Interfaces:**
- Consumes: `invoke("get_usage_history", { agent, hours })` → `Array<[number, number]>`

- [ ] **Step 1: Add a canvas container to `index.html`**, inside `#usage-limits`'s sibling position (after it, before `#content`):
```html
<canvas id="history-chart" height="28"></canvas>
```

- [ ] **Step 2: Add rendering logic to `main.js`**, at the bottom:
```js
const historyCanvas = document.getElementById("history-chart");
const historyCtx = historyCanvas.getContext("2d");

function drawSparkline(points) {
  const w = historyCanvas.width = historyCanvas.clientWidth;
  const h = historyCanvas.height;
  historyCtx.clearRect(0, 0, w, h);
  if (points.length < 2) return;
  const values = points.map(p => p[1]);
  const min = Math.min(...values);
  const max = Math.max(...values, min + 1);
  historyCtx.strokeStyle = "#6aa0ff";
  historyCtx.lineWidth = 1.5;
  historyCtx.beginPath();
  points.forEach((p, i) => {
    const x = (i / (points.length - 1)) * w;
    const y = h - ((p[1] - min) / (max - min)) * h;
    if (i === 0) historyCtx.moveTo(x, y);
    else historyCtx.lineTo(x, y);
  });
  historyCtx.stroke();
}

async function refreshHistory() {
  try {
    const points = await window.__TAURI__.core.invoke("get_usage_history", { agent: "claude", hours: 6 });
    drawSparkline(points);
  } catch (e) {
    // history disabled or db unavailable — leave the canvas blank.
  }
}
refreshHistory();
setInterval(refreshHistory, 60000);
```

- [ ] **Step 3: Add CSS**, in `style.css`:
```css
#history-chart { width: 100%; display: block; padding: 2px 10px; opacity: 0.8; }
```

- [ ] **Step 4: Manual verification** — use the `run` skill to launch the dev build and confirm the sparkline renders after a minute of uptime (it needs at least 2 samples 60s apart to draw a line).

- [ ] **Step 5: Commit**

```bash
git add dist/index.html dist/main.js dist/style.css
git commit -m "feat: render a token-usage sparkline in the overlay"
```

---

## Phase 2 — Cost estimation

### Task 2.1: `src-tauri/src/pricing.rs`

**Files:**
- Create: `src-tauri/src/pricing.rs`
- Modify: `src-tauri/src/lib.rs` (mod list)

**Interfaces:**
- Produces: `pub struct Rates { pub input_per_mtok: f64, pub output_per_mtok: f64, pub cache_read_per_mtok: f64, pub cache_write_per_mtok: f64 }`, `pub fn rates_for_model(model: &str) -> Option<Rates>`, `pub fn estimate_cost_usd(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Option<f64>`

- [ ] **Step 1: Write the failing test**

```rust
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_read_per_mtok: f64,
    pub cache_write_per_mtok: f64,
}

/// Published per-million-token USD prices as of the model's release pricing
/// page. Matched by substring against the reported model string (which may
/// carry suffixes like "[1m]" or date stamps), longest/most-specific prefix
/// first so "claude-3-5-haiku" doesn't match the generic "claude-3-5" rule
/// before its own.
const TABLE: &[(&str, Rates)] = &[
    ("claude-opus-4", Rates { input_per_mtok: 15.0, output_per_mtok: 75.0, cache_read_per_mtok: 1.5, cache_write_per_mtok: 18.75 }),
    ("claude-sonnet-4", Rates { input_per_mtok: 3.0, output_per_mtok: 15.0, cache_read_per_mtok: 0.3, cache_write_per_mtok: 3.75 }),
    ("claude-3-5-haiku", Rates { input_per_mtok: 0.8, output_per_mtok: 4.0, cache_read_per_mtok: 0.08, cache_write_per_mtok: 1.0 }),
    ("claude-3-5-sonnet", Rates { input_per_mtok: 3.0, output_per_mtok: 15.0, cache_read_per_mtok: 0.3, cache_write_per_mtok: 3.75 }),
    ("gpt-5-codex", Rates { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.125, cache_write_per_mtok: 1.25 }),
    ("gpt-5", Rates { input_per_mtok: 1.25, output_per_mtok: 10.0, cache_read_per_mtok: 0.125, cache_write_per_mtok: 1.25 }),
    ("gpt-4o", Rates { input_per_mtok: 2.5, output_per_mtok: 10.0, cache_read_per_mtok: 1.25, cache_write_per_mtok: 2.5 }),
];

pub fn rates_for_model(model: &str) -> Option<Rates> {
    let m = model.to_ascii_lowercase();
    TABLE
        .iter()
        .filter(|(prefix, _)| m.contains(prefix))
        .max_by_key(|(prefix, _)| prefix.len()) // most specific match wins
        .map(|(_, rates)| *rates)
}

pub fn estimate_cost_usd(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> Option<f64> {
    let rates = rates_for_model(model)?;
    let mtok = 1_000_000.0;
    Some(
        (input as f64 / mtok) * rates.input_per_mtok
            + (output as f64 / mtok) * rates.output_per_mtok
            + (cache_read as f64 / mtok) * rates.cache_read_per_mtok
            + (cache_write as f64 / mtok) * rates.cache_write_per_mtok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_returns_rates() {
        assert!(rates_for_model("claude-sonnet-4-20250514").is_some());
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(rates_for_model("some-future-model-9000").is_none());
        assert!(estimate_cost_usd("some-future-model-9000", 1000, 1000, 0, 0).is_none());
    }

    #[test]
    fn cost_scales_linearly_with_tokens() {
        let cost = estimate_cost_usd("claude-sonnet-4-20250514", 1_000_000, 0, 0, 0).unwrap();
        assert!((cost - 3.0).abs() < 1e-9);
    }

    #[test]
    fn most_specific_prefix_wins_over_generic_one() {
        // "claude-3-5-haiku" must not be shadowed by a hypothetical shorter
        // "claude-3-5" rule — this test pins that behavior even though the
        // current table has no overlapping short prefix, so a future entry
        // can't silently break haiku pricing.
        let haiku = rates_for_model("claude-3-5-haiku-20241022").unwrap();
        assert!((haiku.input_per_mtok - 0.8).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cd src-tauri && cargo test pricing::`
Expected: 4 tests pass.

- [ ] **Step 3: Add `pub mod pricing;` to `lib.rs`.**

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/pricing.rs src-tauri/src/lib.rs
git commit -m "feat: add per-model USD pricing table and cost estimator"
```

### Task 2.2: Add cost fields to `model.rs`

**Files:**
- Modify: `src-tauri/src/model.rs`

**Interfaces:**
- Produces: `AgentSession.cost_usd: Option<f64>`, `Snapshot.total_cost_usd: f64`, `Snapshot.by_agent_cost_usd: HashMap<String, f64>`

- [ ] **Step 1: Add the field to `AgentSession`** (after `mem_mb`):
```rust
/// USD cost estimate for this session's tokens, or `None` if the model
/// isn't in `pricing::TABLE`. Populated by `App::tick` after `collect()`,
/// not by individual collectors — see `pricing::estimate_cost_usd`.
pub cost_usd: Option<f64>,
```

- [ ] **Step 2: Add fields to `Snapshot`**:
```rust
pub total_cost_usd: f64,
pub by_agent_cost_usd: HashMap<String, f64>,
```

- [ ] **Step 3: Update `build_snapshot`** to accumulate them:
```rust
pub fn build_snapshot(
    sessions: Vec<AgentSession>,
    usage_limits: BTreeMap<String, RateLimitInfo>,
) -> Snapshot {
    let mut total_tokens: u64 = 0;
    let mut by_agent_tokens: HashMap<String, u64> = HashMap::new();
    let mut by_status: HashMap<SessionStatus, u32> = HashMap::new();
    let mut total_cost_usd: f64 = 0.0;
    let mut by_agent_cost_usd: HashMap<String, f64> = HashMap::new();
    by_status.insert(SessionStatus::Waiting, 0);
    by_status.insert(SessionStatus::Thinking, 0);
    by_status.insert(SessionStatus::Executing, 0);
    by_status.insert(SessionStatus::Done, 0);
    by_status.insert(SessionStatus::Unknown, 0);
    for s in &sessions {
        let t = s.total_input_tokens + s.total_output_tokens;
        total_tokens += t;
        *by_agent_tokens.entry(s.agent_cli.clone()).or_insert(0) += t;
        *by_status.entry(s.status).or_insert(0) += 1;
        if let Some(cost) = s.cost_usd {
            total_cost_usd += cost;
            *by_agent_cost_usd.entry(s.agent_cli.clone()).or_insert(0.0) += cost;
        }
    }
    Snapshot {
        sessions,
        total_tokens,
        by_agent_tokens,
        by_status,
        usage_limits,
        total_cost_usd,
        by_agent_cost_usd,
    }
}
```

- [ ] **Step 4: Fix every existing `AgentSession { ... }` literal** to add `cost_usd: None,` — three call sites: `providers/claude.rs` (`out.push(AgentSession { ... })`), `providers/codex.rs` (`out.push(AgentSession { ... })`), `providers/hermes.rs` (`all_sessions.push(AgentSession { ... })`). `App::tick` (Task 2.3) overwrites this immediately after `collect()`, so `None` here is just a valid, harmless placeholder value, not a stub.

- [ ] **Step 5: Compile check**

Run: `cd src-tauri && cargo build 2>&1 | head -80`
Expected: errors listing exactly the three `AgentSession { ... }` literals missing `cost_usd` until Step 4 is applied to each; after that, a clean build.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add cost_usd fields to AgentSession and Snapshot"
```

### Task 2.3: Compute cost per session in `App::tick`

**Files:**
- Modify: `src-tauri/src/app.rs`

**Interfaces:**
- Consumes: `pricing::estimate_cost_usd` (Task 2.1)

- [ ] **Step 1: Write the failing test** — add to `app.rs`'s existing test module (create one if none exists) a test constructing a fake `Collector` that returns one session with a known model/token counts, then asserting `tick()` populates `cost_usd`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentSession, SessionStatus};

    struct FakeCollector;
    impl Collector for FakeCollector {
        fn name(&self) -> &str {
            "fake"
        }
        fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
            vec![AgentSession {
                agent_cli: "claude".into(),
                pid: 0,
                session_id: "s1".into(),
                cwd: String::new(),
                project_name: String::new(),
                started_at: 0,
                status: SessionStatus::Waiting,
                model: "claude-sonnet-4-20250514".into(),
                context_percent: 0.0,
                total_input_tokens: 1_000_000,
                total_output_tokens: 0,
                total_cache_read: 0,
                total_cache_create: 0,
                turn_count: 0,
                current_task: String::new(),
                mem_mb: 0,
                cost_usd: None,
            }]
        }
    }

    #[test]
    fn tick_populates_cost_usd_from_pricing_table() {
        let mut app = App::new(vec![Box::new(FakeCollector)]);
        let snapshot = app.tick();
        assert_eq!(snapshot.sessions.len(), 1);
        let cost = snapshot.sessions[0].cost_usd.expect("cost should be Some for a known model");
        assert!((cost - 3.0).abs() < 1e-9);
    }
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cd src-tauri && cargo test app::tests::tick_populates_cost_usd_from_pricing_table`
Expected: FAIL — `cost_usd` is `None`.

- [ ] **Step 3: Implement** — in `App::tick`, right after the `for c in &mut self.collectors { ... }` loop and before the `sort_by`/`dedup_by` calls, add:
```rust
for s in &mut sessions {
    s.cost_usd = crate::pricing::estimate_cost_usd(
        &s.model,
        s.total_input_tokens,
        s.total_output_tokens,
        s.total_cache_read,
        s.total_cache_create,
    );
}
```

- [ ] **Step 4: Run the test again**

Run: `cd src-tauri && cargo test app::tests::tick_populates_cost_usd_from_pricing_table`
Expected: PASS.

- [ ] **Step 5: Run the full suite**

Run: `cd src-tauri && cargo test`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/app.rs
git commit -m "feat: compute per-session USD cost estimate during tick"
```

### Task 2.4: Show cost in the overlay

**Files:**
- Modify: `dist/main.js`
- Modify: `dist/style.css`

- [ ] **Step 1: Add a cost span to each session row**, in `main.js::renderSnapshot`, inside the `.meta` div (after the `ctx` span):
```js
${sess.cost_usd != null ? `<span class="cost">$${sess.cost_usd.toFixed(3)}</span>` : ""}
```

- [ ] **Step 2: Add total cost to the footer**:
```js
const totalCost = s.total_cost_usd || 0;
return `<div class="rows">${rows}</div>
        <div class="footer">total ${fmt(totalTokens)} tok &#183; $${totalCost.toFixed(2)} &#183; ${liveCount} live</div>`;
```

- [ ] **Step 3: Add CSS**, in `style.css`:
```css
.cost { color: #8bd17c; }
```

- [ ] **Step 4: Manual verification** via the `run` skill — confirm the cost figures render and update per tick.

- [ ] **Step 5: Commit**

```bash
git add dist/main.js dist/style.css
git commit -m "feat: show per-session and total USD cost in the overlay"
```

---

## Phase 3 — Burn-rate / time-to-limit prediction

### Task 3.1: `src-tauri/src/burn_rate.rs` — linear projection

**Files:**
- Create: `src-tauri/src/burn_rate.rs`
- Modify: `src-tauri/src/lib.rs` (mod list)

**Interfaces:**
- Produces: `pub fn project_time_to_limit(points: &[(i64, f64)], now_ms: i64) -> Option<i64>`

- [ ] **Step 1: Write the failing test**

```rust
/// Projects milliseconds-until-100% from a series of `(ts_ms, pct)` samples
/// using ordinary least-squares slope. Returns `None` when there are fewer
/// than 2 points, the pct isn't increasing (slope <= 0 — already trending
/// down or flat, e.g. right after a window reset), or the projection would
/// land more than 30 days out (not a meaningful "burn rate" at that point).
pub fn project_time_to_limit(points: &[(i64, f64)], now_ms: i64) -> Option<i64> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let mean_t: f64 = points.iter().map(|(t, _)| *t as f64).sum::<f64>() / n;
    let mean_p: f64 = points.iter().map(|(_, p)| *p).sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (t, p) in points {
        let dt = *t as f64 - mean_t;
        num += dt * (*p - mean_p);
        den += dt * dt;
    }
    if den == 0.0 {
        return None;
    }
    let slope_pct_per_ms = num / den; // pct change per millisecond
    if slope_pct_per_ms <= 0.0 {
        return None;
    }
    let latest_pct = points.last().unwrap().1;
    if latest_pct >= 100.0 {
        return Some(0);
    }
    let ms_to_100 = ((100.0 - latest_pct) / slope_pct_per_ms) as i64;
    let latest_ts = points.last().unwrap().0;
    let eta_ms = latest_ts - now_ms + ms_to_100;
    if eta_ms < 0 || ms_to_100 > 30 * 86_400_000 {
        None
    } else {
        Some(eta_ms.max(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fewer_than_two_points_returns_none() {
        assert_eq!(project_time_to_limit(&[(0, 10.0)], 0), None);
        assert_eq!(project_time_to_limit(&[], 0), None);
    }

    #[test]
    fn flat_usage_returns_none() {
        let points = vec![(0, 50.0), (60_000, 50.0), (120_000, 50.0)];
        assert_eq!(project_time_to_limit(&points, 120_000), None);
    }

    #[test]
    fn decreasing_usage_returns_none() {
        // A window reset mid-sample-window would show pct dropping; that's
        // not a "burn rate" — don't project a nonsensical ETA off it.
        let points = vec![(0, 80.0), (60_000, 40.0), (120_000, 10.0)];
        assert_eq!(project_time_to_limit(&points, 120_000), None);
    }

    #[test]
    fn steady_linear_increase_projects_a_sane_eta() {
        // +10% per minute starting at 50% -> 5 more minutes to 100%.
        let points = vec![(0, 50.0), (60_000, 60.0), (120_000, 70.0)];
        let eta = project_time_to_limit(&points, 120_000).unwrap();
        assert!((eta - 300_000).abs() < 5_000, "eta was {eta}ms, expected ~300000ms");
    }

    #[test]
    fn already_at_limit_returns_zero() {
        let points = vec![(0, 90.0), (60_000, 100.0)];
        assert_eq!(project_time_to_limit(&points, 60_000), Some(0));
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cd src-tauri && cargo test burn_rate::`
Expected: 5 tests pass.

- [ ] **Step 3: Add `pub mod burn_rate;` to `lib.rs`.**

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/burn_rate.rs src-tauri/src/lib.rs
git commit -m "feat: add linear time-to-limit projection"
```

### Task 3.2: Add ETA fields to `RateLimitInfo` and compute them in the tick thread

**Files:**
- Modify: `src-tauri/src/model.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `history::rate_limit_history` (Task 1.1), `burn_rate::project_time_to_limit` (Task 3.1)
- Produces: `RateLimitInfo.five_hour_eta_ms: Option<i64>`, `.seven_day_eta_ms: Option<i64>`, `.monthly_eta_ms: Option<i64>`

- [ ] **Step 1: Add the fields to `RateLimitInfo`** (after `monthly_resets_at`):
```rust
/// Estimated milliseconds until this window hits 100%, from
/// `burn_rate::project_time_to_limit` over the last hour of samples in
/// `history.db`. `None` when history is disabled, there's not enough data
/// yet, or usage isn't trending upward.
pub five_hour_eta_ms: Option<i64>,
pub seven_day_eta_ms: Option<i64>,
pub monthly_eta_ms: Option<i64>,
```
These default to `None` via `#[derive(Default)]`, already on `RateLimitInfo` — no existing construction site needs updating for this struct (unlike `AgentSession`, everywhere `RateLimitInfo` is built uses `..Default::default()` or explicit `Some`/`None` per field already covering the new fields via struct update syntax; verify `rate_limit.rs`, `claude_usage.rs`, and `providers/codex.rs` all use `..Default::default()` — they do, per the code read in this plan's prep).

- [ ] **Step 2: Compute ETAs in `lib.rs`'s tick thread**, right after the history-sampling block added in Task 1.2, and only when `history_conn` is `Some`:
```rust
if let Some(hconn) = &state.history_conn {
    if let Ok(guard) = hconn.lock() {
        let since_ms = ts_ms - 3_600_000; // last hour of samples
        for (agent, rl) in snapshot.usage_limits.iter_mut() {
            for (window, setter): (&str, fn(&mut model::RateLimitInfo, Option<i64>)) in [
                ("five_hour", (|rl, v| rl.five_hour_eta_ms = v) as fn(&mut model::RateLimitInfo, Option<i64>)),
                ("seven_day", |rl, v| rl.seven_day_eta_ms = v),
                ("monthly", |rl, v| rl.monthly_eta_ms = v),
            ] {
                let points = history::rate_limit_history(&guard, agent, window, since_ms).unwrap_or_default();
                let eta = burn_rate::project_time_to_limit(&points, ts_ms);
                setter(rl, eta);
            }
        }
    }
}
```
Note: this block must run **before** `let _ = app_handle.emit("snapshot://update", &snapshot);` so the ETA is present in the emitted payload, and it needs `snapshot` to be `mut` — change `let snapshot = { ... };` to `let mut snapshot = { ... };` in the tick closure.

- [ ] **Step 3: Compile check**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/model.rs src-tauri/src/lib.rs
git commit -m "feat: compute five_hour/seven_day/monthly ETAs from usage history"
```

### Task 3.3: Show ETA in the overlay

**Files:**
- Modify: `dist/main.js`

- [ ] **Step 1: Extend `renderUsageWindow`** to take an optional eta and render it next to the reset countdown:
```js
function renderUsageWindow(label, pct, resetsAt, etaMs) {
  if (pct == null) return "";
  const clamped = Math.min(100, Math.max(0, Math.round(pct)));
  const countdown = fmtCountdown(resetsAt);
  const eta = etaMs != null ? fmtDuration(etaMs) : null;
  return `<span class="usage-window">
      ${label} <span class="usage-pct">${clamped}%</span>
      <span class="bar usage-bar"><span class="bar-fill" style="width:${clamped}%"></span></span>
      ${countdown ? `<span class="usage-reset">resets ${countdown}</span>` : ""}
      ${eta ? `<span class="usage-eta">&#8776;${eta} to limit</span>` : ""}
    </span>`;
}
```

- [ ] **Step 2: Pass the eta fields through in `renderUsageLimits`**:
```js
const windows = [
  renderUsageWindow("5h", rl.five_hour_pct, rl.five_hour_resets_at, rl.five_hour_eta_ms),
  renderUsageWindow("week", rl.seven_day_pct, rl.seven_day_resets_at, rl.seven_day_eta_ms),
  renderUsageWindow("month", rl.monthly_pct, rl.monthly_resets_at, rl.monthly_eta_ms),
].filter(Boolean).join("");
```

- [ ] **Step 3: Add CSS**, in `style.css`:
```css
.usage-eta { opacity: 0.6; color: #ffb454; }
```

- [ ] **Step 4: Manual verification** via the `run` skill.

- [ ] **Step 5: Commit**

```bash
git add dist/main.js dist/style.css
git commit -m "feat: show time-to-limit estimate next to each usage window"
```

---

## Phase 4 — Session idle/stall detection alert

### Task 4.1: Config field

**Files:**
- Modify: `src-tauri/src/config.rs`

- [ ] **Step 1: Add the field**:
```rust
/// Seconds a session may stay in `Thinking` or `Executing` before it's
/// flagged `stalled` and (once) notified about. `0` disables the feature.
#[serde(default = "default_stall_alert_secs")]
pub stall_alert_secs: u64,
```
```rust
fn default_stall_alert_secs() -> u64 {
    180
}
```
Add `stall_alert_secs: 180,` to `default_config()`.

- [ ] **Step 2: Commit**

```bash
git add src-tauri/src/config.rs
git commit -m "feat: add stall_alert_secs config field"
```

### Task 4.2: `stalled` field on `AgentSession` + tracking logic in `App`

**Files:**
- Modify: `src-tauri/src/model.rs`
- Modify: `src-tauri/src/app.rs`
- Modify: `src-tauri/src/providers/claude.rs`, `providers/codex.rs`, `providers/hermes.rs` (add `stalled: false` to the three `AgentSession` literals)

**Interfaces:**
- Produces: `AgentSession.stalled: bool`, `App::tick_with_threshold(&mut self, stall_alert_secs: u64, now_ms: i64) -> Snapshot` (new; `tick()` becomes a thin wrapper calling it with the real clock and a default threshold, so tests can inject both)

- [ ] **Step 1: Add the field to `AgentSession`**:
```rust
/// `true` once this session has stayed in `Thinking`/`Executing` longer
/// than `Config.stall_alert_secs`. Recomputed every tick by `App`, not by
/// individual collectors — a collector has no cross-tick memory of "how
/// long has this session's status been unchanged."
pub stalled: bool,
```
Add `stalled: false,` to the three provider `AgentSession { ... }` literals (this is immediately overwritten by `App::tick`, same pattern as `cost_usd` in Phase 2).

- [ ] **Step 2: Write the failing test** in `app.rs`'s test module — a session whose status stays `Thinking` across two `tick_with_threshold` calls more than the threshold apart should end up `stalled: true`; one that changes status should not:
```rust
#[test]
fn session_stuck_in_thinking_past_threshold_is_flagged_stalled() {
    struct StuckCollector;
    impl Collector for StuckCollector {
        fn name(&self) -> &str { "stuck" }
        fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
            vec![AgentSession { status: SessionStatus::Thinking, session_id: "s1".into(), agent_cli: "claude".into(), ..blank_session() }]
        }
    }
    let mut app = App::new(vec![Box::new(StuckCollector)]);
    let first = app.tick_with_threshold(60, 0);
    assert!(!first.sessions[0].stalled, "should not be stalled on first sighting");
    let second = app.tick_with_threshold(60, 61_000);
    assert!(second.sessions[0].stalled, "should be stalled after 61s in the same status");
}

#[test]
fn session_that_changes_status_resets_the_stall_timer() {
    struct FlippingCollector { thinking: bool }
    impl Collector for FlippingCollector {
        fn name(&self) -> &str { "flip" }
        fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
            let status = if self.thinking { SessionStatus::Thinking } else { SessionStatus::Executing };
            self.thinking = !self.thinking;
            vec![AgentSession { status, session_id: "s1".into(), agent_cli: "claude".into(), ..blank_session() }]
        }
    }
    let mut app = App::new(vec![Box::new(FlippingCollector { thinking: true })]);
    app.tick_with_threshold(60, 0);
    let second = app.tick_with_threshold(60, 61_000);
    assert!(!second.sessions[0].stalled, "status changed, so the timer should have reset");
}

fn blank_session() -> AgentSession {
    AgentSession {
        agent_cli: String::new(), pid: 0, session_id: String::new(), cwd: String::new(),
        project_name: String::new(), started_at: 0, status: SessionStatus::Waiting,
        model: String::new(), context_percent: 0.0, total_input_tokens: 0, total_output_tokens: 0,
        total_cache_read: 0, total_cache_create: 0, turn_count: 0, current_task: String::new(),
        mem_mb: 0, cost_usd: None, stalled: false,
    }
}
```

- [ ] **Step 3: Run to confirm failure**

Run: `cd src-tauri && cargo test app::tests::session_stuck`
Expected: FAIL — `tick_with_threshold` doesn't exist yet.

- [ ] **Step 4: Implement.** Add a tracker map to `App`:
```rust
pub struct App {
    collectors: Vec<Box<dyn Collector>>,
    wsl_distros: Vec<String>,
    /// (agent_cli, session_id) -> (status as of last tick, ms timestamp that
    /// status was first observed). Drives the `stalled` flag.
    stall_tracker: HashMap<(String, String), (crate::model::SessionStatus, i64)>,
}
```
Update both constructors to initialize `stall_tracker: HashMap::new()`.

Change `tick()` to delegate:
```rust
pub fn tick(&mut self) -> Snapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();
    // Read at call time so a live config change (Phase 9) takes effect
    // without restarting the app; App itself has no Config dependency, so
    // lib.rs passes the current value in.
    self.tick_with_threshold(180, now_ms)
}

pub fn tick_with_threshold(&mut self, stall_alert_secs: u64, now_ms: i64) -> Snapshot {
    // ... existing body up through the cost_usd loop from Task 2.3 ...
    if stall_alert_secs > 0 {
        let threshold_ms = (stall_alert_secs as i64) * 1000;
        let mut still_present: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
        for s in &mut sessions {
            let key = (s.agent_cli.clone(), s.session_id.clone());
            still_present.insert(key.clone());
            let is_active_status = matches!(
                s.status,
                crate::model::SessionStatus::Thinking | crate::model::SessionStatus::Executing
            );
            let entry = self.stall_tracker.entry(key).or_insert((s.status, now_ms));
            if entry.0 != s.status {
                *entry = (s.status, now_ms);
            }
            s.stalled = is_active_status && (now_ms - entry.1) >= threshold_ms;
        }
        self.stall_tracker.retain(|k, _| still_present.contains(k));
    }
    build_snapshot(sessions, usage_limits)
}
```
(The `for s in &mut sessions { s.cost_usd = ... }` block from Task 2.3 stays where it was, just before this new block.)

Note real caller (`lib.rs`) still calls `app.tick()` today; Task 4.4 changes that call site to pass the live config value via `tick_with_threshold`.

- [ ] **Step 5: Run the tests**

Run: `cd src-tauri && cargo test app::`
Expected: all pass, including the two new stall tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: track and flag stalled sessions in App::tick_with_threshold"
```

### Task 4.3: OS notification on stall (new dependency)

**Files:**
- Modify: `src-tauri/Cargo.toml` — add `tauri-plugin-notification = "2"` (justification: native OS toast is the standard Tauri-recommended way to surface a background alert; the app has no other notification mechanism and an in-panel-only flag would be missed while the overlay is hidden, which is the app's default idle state for many users)
- Modify: `src-tauri/capabilities/default.json` — add `"notification:default"` permission
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the dependency**

Run: `cd src-tauri && cargo add tauri-plugin-notification@2`

- [ ] **Step 2: Register the plugin** in `run()`, alongside the existing `.plugin(...)` call:
```rust
.plugin(tauri_plugin_global_shortcut::Builder::new().build())
.plugin(tauri_plugin_notification::init())
```

- [ ] **Step 3: Add the permission** to `capabilities/default.json`'s `permissions` array:
```json
"notification:default"
```

- [ ] **Step 4: Fire a one-shot notification per newly-stalled session** in the tick thread. Track already-notified keys alongside the tick call:
```rust
use tauri_plugin_notification::NotificationExt;
```
and, in `run()`'s tick-thread closure, replace the direct `a.tick()` call with:
```rust
let (interval, stall_alert_secs) = {
    let state: tauri::State<AppState> = app_handle.state();
    let cfg = state.config.lock().unwrap();
    (cfg.poll_interval_ms, cfg.stall_alert_secs)
};
let mut snapshot = {
    let state: tauri::State<AppState> = app_handle.state();
    let mut a = state.app.lock().unwrap();
    a.tick_with_threshold(stall_alert_secs, chrono::Utc::now().timestamp_millis())
};
```
(this replaces the earlier `let interval = { ... }` and `let snapshot = { ... }` blocks — merge rather than duplicate them). Then, before the history-sampling block, add:
```rust
static NOTIFIED: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
let notified = NOTIFIED.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
if let Ok(mut notified) = notified.lock() {
    let stalled_keys: std::collections::HashSet<String> = snapshot
        .sessions
        .iter()
        .filter(|s| s.stalled)
        .map(|s| format!("{}:{}", s.agent_cli, s.session_id))
        .collect();
    for key in &stalled_keys {
        if notified.insert(key.clone()) {
            let _ = app_handle
                .notification()
                .builder()
                .title("AI assistant stalled")
                .body(format!("{key} has been thinking/executing longer than expected."))
                .show();
        }
    }
    notified.retain(|k| stalled_keys.contains(k));
}
```

- [ ] **Step 5: Compile check**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: send an OS notification the first time a session stalls"
```

### Task 4.4: Highlight stalled sessions in the overlay

**Files:**
- Modify: `dist/main.js`
- Modify: `dist/style.css`

- [ ] **Step 1: Add the class** in `renderSnapshot`'s row template:
```js
<div class="row${sess.stalled ? " stalled" : ""}">
```

- [ ] **Step 2: Add CSS**:
```css
.row.stalled { background: rgba(255,80,80,0.12); border-left: 2px solid #ff5050; padding-left: 6px; }
```

- [ ] **Step 3: Manual verification** via the `run` skill.

- [ ] **Step 4: Commit**

```bash
git add dist/main.js dist/style.css
git commit -m "feat: highlight stalled sessions in the panel"
```

---

## Phase 5 — Per-project grouping

Pure frontend — `AgentSession.project_name` already exists on every session from every provider.

### Task 5.1: Group rows by project in `renderSnapshot`

**Files:**
- Modify: `dist/main.js`

- [ ] **Step 1: Replace the flat `rows` construction** in `renderSnapshot` with a grouped version. Replace:
```js
const rows = s.sessions.map(sess => { ... }).join("");
```
with:
```js
function renderSessionRow(sess) {
  const bar = Math.min(100, Math.round(sess.context_percent || 0));
  const usage = (sess.total_input_tokens || 0)
    + (sess.total_output_tokens || 0)
    + (sess.total_cache_read || 0)
    + (sess.total_cache_create || 0);
  const elapsedMs = sess.started_at > 0 ? Date.now() - sess.started_at : 0;
  const rate = elapsedMs > 1000 ? usage / (elapsedMs / 60000) : 0;
  const usageRow = elapsedMs > 1000
    ? `<span>&#931;${fmt(usage)} / ${fmtDuration(elapsedMs)}</span><span>${fmt(rate)} tok/min</span>`
    : `<span>&#931;${fmt(usage)}</span>`;
  return `
    <div class="row${sess.stalled ? " stalled" : ""}">
      <div class="head">
        <span class="dot dot-${sess.agent_cli}"></span>
        <span class="agent">${sess.agent_cli}</span>
        ${sess.model ? `<span class="model">${escapeHtml(sess.model)}</span>` : ""}
        <span class="status status-${sess.status}">${STATUS_LABEL[sess.status] || sess.status}</span>
      </div>
      <div class="bar"><div class="bar-fill" style="width:${bar}%"></div></div>
      <div class="meta">
        <span>&#8595;${fmt(sess.total_input_tokens || 0)}</span>
        <span>&#8593;${fmt(sess.total_output_tokens || 0)}</span>
        ${usageRow}
        <span>ctx ${bar}%</span>
        ${sess.cost_usd != null ? `<span class="cost">$${sess.cost_usd.toFixed(3)}</span>` : ""}
        <span>${sess.mem_mb || 0}MB</span>
        <span class="task">${escapeHtml(sess.current_task || "")}</span>
      </div>
    </div>`;
}

function groupByProject(sessions) {
  const groups = new Map();
  for (const sess of sessions) {
    const key = sess.project_name || "(unknown)";
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(sess);
  }
  return groups;
}

const groups = groupByProject(s.sessions);
const rows = Array.from(groups.entries()).map(([project, sessions]) => `
  <div class="project-group">
    <div class="project-header">${escapeHtml(project)} <span class="project-count">${sessions.length}</span></div>
    ${sessions.map(renderSessionRow).join("")}
  </div>
`).join("");
```
Note `sess.project` was previously rendered inline in `.head` via a `.proj` span — that's now redundant with the group header, so remove the `<span class="proj">...</span>` line from `renderSessionRow` (already omitted above).

- [ ] **Step 2: Manual verification** via the `run` skill — with two mock sessions from different `project_name`s (or two real Claude sessions in different directories), confirm two group headers render.

- [ ] **Step 3: Commit**

```bash
git add dist/main.js
git commit -m "feat: group session rows by project"
```

### Task 5.2: Styling for group headers

**Files:**
- Modify: `dist/style.css`

- [ ] **Step 1: Add rules**:
```css
.project-group { margin-bottom: 4px; }
.project-header {
  font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em;
  opacity: 0.55; padding: 4px 0 2px; display: flex; align-items: center; gap: 6px;
}
.project-count { font-weight: 400; opacity: 0.7; background: rgba(255,255,255,0.08); border-radius: 8px; padding: 0 5px; }
```

- [ ] **Step 2: Commit**

```bash
git add dist/style.css
git commit -m "style: add project group header styling"
```

---

## Phase 6 — Compact/expanded view toggle

### Task 6.1: Config field + `set_compact_view` command

**Files:**
- Modify: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the config field**:
```rust
#[serde(default)]
pub compact_view: bool,
```
(`bool`'s `Default` is `false`, matching current always-expanded behavior — no `default_config()` change needed since `false` is also `bool::default()`, but add it explicitly for clarity: `compact_view: false,`.)

- [ ] **Step 2: Add the command**, alongside `set_opacity`:
```rust
#[tauri::command]
fn set_compact_view(window: tauri::Window, state: tauri::State<AppState>, compact: bool) {
    if let Ok(mut cfg) = state.config.lock() {
        cfg.compact_view = compact;
        let _ = config::save_config(&state.config_path, &cfg);
    }
    let _ = window.emit("compact://update", compact);
}
```

- [ ] **Step 3: Register it in `invoke_handler!`** and emit the initial state at startup alongside the existing opacity emit:
```rust
if let Some(w) = app_handle.get_webview_window("overlay") {
    let _ = w.emit("opacity://update", cfg.opacity);
    let _ = w.emit("compact://update", cfg.compact_view);
}
```

- [ ] **Step 4: Compile check**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config.rs src-tauri/src/lib.rs
git commit -m "feat: add compact_view config field and set_compact_view command"
```

### Task 6.2: Titlebar toggle button and frontend wiring

**Files:**
- Modify: `dist/index.html`
- Modify: `dist/main.js`
- Modify: `dist/style.css`

- [ ] **Step 1: Add a button to `index.html`'s titlebar**, before `#hide-btn`:
```html
<button id="compact-btn" title="Toggle compact view">&#8942;</button>
```

- [ ] **Step 2: Wire it in `main.js`**:
```js
const compactBtn = document.getElementById("compact-btn");
let compactView = false;

compactBtn.addEventListener("click", () => {
  compactView = !compactView;
  window.__TAURI__.core.invoke("set_compact_view", { compact: compactView });
});

window.__TAURI__.event.listen("compact://update", (e) => {
  compactView = e.payload;
  panel.classList.toggle("compact", compactView);
});
```

- [ ] **Step 3: Add CSS**:
```css
.compact .meta > span:not(.task) { display: none; }
.compact .meta .task { font-size: 9px; }
.compact .row { padding: 3px 0; }
.compact .project-header { padding: 2px 0; }
```

- [ ] **Step 4: Manual verification** via the `run` skill — click the button, confirm the panel shrinks per-row content and the preference survives an app restart (reads back from `config.toml`).

- [ ] **Step 5: Commit**

```bash
git add dist/index.html dist/main.js dist/style.css
git commit -m "feat: add compact/expanded view toggle"
```

---

## Phase 7 — Finish Hermes integration

The collector logic in `providers/hermes.rs` already implements session/status/token polling per the external monitoring contract. The gaps are test coverage and account-level quota parity with Claude/Codex.

### Task 7.1: Unit tests for `apply_log_line` status transitions

**Files:**
- Modify: `src-tauri/src/providers/hermes.rs`

- [ ] **Step 1: Add a test module** at the bottom of the file:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionStatus;

    #[test]
    fn thinking_keyword_sets_thinking_status() {
        let mut status = SessionStatus::Waiting;
        let mut task = String::new();
        apply_log_line("[abc] Agent is thinking about the next step", "abc", &mut status, &mut task);
        assert_eq!(status, SessionStatus::Thinking);
    }

    #[test]
    fn tool_call_sets_executing_and_extracts_name() {
        let mut status = SessionStatus::Waiting;
        let mut task = String::new();
        apply_log_line("[abc] Tool starting: read_file some/path", "abc", &mut status, &mut task);
        assert_eq!(status, SessionStatus::Executing);
        assert_eq!(task, "read_file");
    }

    #[test]
    fn turn_ended_with_unrelated_tool_words_does_not_falsely_match_executing() {
        // Regression: a summary line naming both "tool" and "call" as field
        // labels (not an actual tool invocation) must not be read as an
        // executing-tool event.
        let mut status = SessionStatus::Thinking;
        let mut task = String::new();
        apply_log_line("[abc] Turn ended: tool_turns=0 api_calls=1/150", "abc", &mut status, &mut task);
        assert_eq!(status, SessionStatus::Waiting);
    }

    #[test]
    fn lines_for_a_different_session_id_are_ignored() {
        let mut status = SessionStatus::Waiting;
        let mut task = String::new();
        apply_log_line("[other-session] Tool starting: read_file x", "abc", &mut status, &mut task);
        assert_eq!(status, SessionStatus::Waiting);
        assert!(task.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cd src-tauri && cargo test providers::hermes::`
Expected: 4 tests pass (they exercise already-correct existing behavior — this task is about locking it in, not changing it).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/providers/hermes.rs
git commit -m "test: cover Hermes log-line status transitions"
```

### Task 7.2: Unit test for `query_active_session` against a real temp SQLite db

**Files:**
- Modify: `src-tauri/src/providers/hermes.rs`

- [ ] **Step 1: Add to the same test module**:
```rust
#[test]
fn query_active_session_reads_the_most_recent_open_session() {
    let dir = std::env::temp_dir().join(format!("utt-hermes-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let db_path = dir.join("state.db");
    let _ = std::fs::remove_file(&db_path);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER,
            cache_read_tokens INTEGER, cache_write_tokens INTEGER, message_count INTEGER,
            cwd TEXT, started_at REAL, ended_at REAL
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions VALUES ('old', 'm1', 1, 1, 0, 0, 1, '/tmp/a', 100.0, 200.0)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO sessions VALUES ('new', 'm1', 5, 5, 0, 0, 2, '/tmp/b', 300.0, NULL)",
        [],
    ).unwrap();
    drop(conn);

    let active = query_active_session(&db_path).expect("should find the open session");
    assert_eq!(active.id, "new");
    assert_eq!(active.cwd, "/tmp/b");

    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run it**

Run: `cd src-tauri && cargo test providers::hermes::query_active_session_reads_the_most_recent_open_session`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/providers/hermes.rs
git commit -m "test: cover Hermes active-session SQL query against a real db"
```

### Task 7.3: Account-level quota support via a Hermes hook file, matching Claude's pattern

**Files:**
- Modify: `src-tauri/src/providers/hermes.rs`

**Interfaces:**
- Consumes: `crate::rate_limit::read_rate_limit_file` (existing, already generic over `default_source`)

- [ ] **Step 1: Implement `usage_limits()`** on `HermesCollector`, reusing the same hook-file schema Claude's fallback path uses (`abtop-rate-limits.json`, already parsed by `rate_limit::read_rate_limit_file`) so a Hermes deployment that wants to report quota can drop the same file shape into its data dir — no new parser needed:
```rust
impl Collector for HermesCollector {
    // ... existing name/collect unchanged ...

    fn usage_limits(&self) -> Option<crate::model::RateLimitInfo> {
        self.data_dirs.iter().find_map(|dir| {
            crate::rate_limit::read_rate_limit_file(
                &dir.join(crate::rate_limit::CLAUDE_RATE_FILE),
                "hermes",
            )
        })
    }
}
```
Rename `rate_limit::CLAUDE_RATE_FILE` usages don't need to change — the constant's *name* still says "Claude" even though it's now shared by two providers; that's an accurate description of its origin, not a bug, so leave the constant named as-is per YAGNI (renaming it is a larger, unrelated cleanup).

- [ ] **Step 2: Add a test** to the same module:
```rust
#[test]
fn usage_limits_reads_hook_file_from_data_dir() {
    let dir = std::env::temp_dir().join(format!("utt-hermes-quota-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(
        dir.join("abtop-rate-limits.json"),
        r#"{"five_hour": {"used_percentage": 33.0, "resets_at": 123}}"#,
    ).unwrap();
    let collector = HermesCollector::new(dir.clone());
    let rl = collector.usage_limits().expect("should read the hook file");
    assert_eq!(rl.five_hour_pct, Some(33.0));
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 3: Run it**

Run: `cd src-tauri && cargo test providers::hermes::usage_limits_reads_hook_file_from_data_dir`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/providers/hermes.rs
git commit -m "feat: support account-level quota hook file for Hermes"
```

---

## Phase 8 — Linux support

### Task 8.1: Fix `main.rs`'s Windows-only attribute so it doesn't break non-Windows release builds

**Files:**
- Modify: `src-tauri/src/main.rs`

- [ ] **Step 1: Fix the cfg gate.** `windows_subsystem` is a Windows-only attribute; today's `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` applies it in every *release* build regardless of target OS, which fails to compile on Linux/macOS release builds. Change to:
```rust
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

fn main() {
    usage_tracker::run();
}
```

- [ ] **Step 2: Verify a release build compiles on this machine** (WSL/Linux, per this repo's dev environment)

Run: `cd src-tauri && cargo build --release 2>&1 | tail -30`
Expected: no `windows_subsystem` error; build proceeds (may still fail later on missing Linux system libs — that's Task 8.2/8.3's concern, not this attribute).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/main.rs
git commit -m "fix: gate windows_subsystem attribute on the windows target, not just release mode"
```

### Task 8.2: Skip WSL-probing on native Linux

**Files:**
- Modify: `src-tauri/src/home.rs`

- [ ] **Step 1: Guard the `wsl.exe`-discovery branch** in `resolve_home_dirs` with a target check, since `wsl` as a command only exists on Windows and this branch is otherwise dead code (returning early via `.ok()` swallowing the spawn failure) on Linux — making the intent explicit avoids a needless process-spawn attempt on every native-Linux startup:
```rust
} else if cfg!(target_os = "windows") {
    // We're on native Windows - check for WSL home directories
    // Try to discover WSL distributions and their users
    if let Ok(output) = process::silent_command("wsl")
        .args(["-l", "-q"])
        .output()
    {
        // ... unchanged body ...
    }
}
```
(The `if is_wsl { ... }` branch above it is unchanged — it already only applies to running *inside* WSL, which is Linux-based and stays relevant.)

- [ ] **Step 2: Compile check**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/home.rs
git commit -m "refactor: skip Windows-only WSL discovery on native Linux"
```

### Task 8.3: Verify the full build and test suite on Linux

**Files:** none (verification-only task)

- [ ] **Step 1: Install Tauri's Linux build dependencies** if not already present (WebKitGTK + AppIndicator, required for the tray icon and webview on Linux):

Run: `pkg-config --exists webkit2gtk-4.1 && pkg-config --exists ayatana-appindicator3-0.1 && echo OK || echo MISSING`
Expected: if `MISSING`, install via the distro's package manager before continuing (e.g. `sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf` on Debian/Ubuntu) — this is a one-time environment setup step, not a code change.

- [ ] **Step 2: Full build**

Run: `cd src-tauri && cargo build --release`
Expected: succeeds, producing a native Linux binary at `src-tauri/target/release/ai-usage-overlay`.

- [ ] **Step 3: Full test suite**

Run: `cd src-tauri && cargo test`
Expected: all tests pass on Linux (the `wsl_snapshot`/`silent_command` Windows-only code paths are behind `#[cfg(windows)]` already and simply don't compile in on Linux, per `process.rs:5-6`).

- [ ] **Step 4: Manual smoke test** — use the `run` skill to launch the built binary and confirm the overlay window appears, is transparent/draggable, and the tray icon shows up (`tauri.conf.json`'s `bundle.targets: "all"` already produces `.deb`/`.AppImage` on Linux with no config change needed — confirmed by reading `tauri.conf.json` during planning).

- [ ] **Step 5: No commit** (verification task; if Step 2 or 3 uncovers a real bug, fix it in a follow-up task with its own commit rather than folding a surprise fix into this checklist item).

---

## Phase 9 — In-app config UI

### Task 9.1: Backend commands (`get_config`, `set_enabled_agents`, `list_providers`)

**Files:**
- Modify: `src-tauri/src/app.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `providers::ALL` (Phase 0)
- Produces: `App::set_collectors(&mut self, collectors: Vec<Box<dyn Collector>>)`, `#[tauri::command] fn get_config(state) -> Config`, `fn set_enabled_agents(state, app_handle, agents: Vec<String>)`, `fn list_providers() -> Vec<(String, String)>`

- [ ] **Step 1: Add `App::set_collectors`**, in `app.rs`:
```rust
/// Replaces the running set of collectors — used when the user toggles
/// which providers are enabled from the in-app settings panel, so a change
/// takes effect on the very next tick without an app restart.
pub fn set_collectors(&mut self, collectors: Vec<Box<dyn Collector>>) {
    self.collectors = collectors;
}
```

- [ ] **Step 2: Add `get_config`**, in `lib.rs`:
```rust
#[tauri::command]
fn get_config(state: tauri::State<AppState>) -> config::Config {
    state.config.lock().unwrap().clone()
}
```

- [ ] **Step 3: Add `list_providers`**:
```rust
#[tauri::command]
fn list_providers() -> Vec<(String, String)> {
    providers::ALL.iter().map(|p| (p.key.to_string(), p.label.to_string())).collect()
}
```

- [ ] **Step 4: Add `set_enabled_agents`**, rebuilding collectors against the current home dirs — this requires `home_dirs` to be reachable from the command, so store it in `AppState`:
```rust
struct AppState {
    app: Mutex<app::App>,
    config: Mutex<config::Config>,
    config_path: PathBuf,
    history_conn: Option<Mutex<rusqlite::Connection>>,
    home_dirs: Vec<home::HomeDir>,
}
```
Update the `AppState { ... }` literal in `run()` to add `home_dirs: home_dirs.clone(),` — this requires `#[derive(Clone)]` on `home::HomeDir` (add it in Task 9.1's edit to `home.rs`: `#[derive(Clone)] pub struct HomeDir { ... }`).

```rust
#[tauri::command]
fn set_enabled_agents(state: tauri::State<AppState>, agents: Vec<String>) {
    let mut cfg = match state.config.lock() {
        Ok(c) => c,
        Err(_) => return,
    };
    cfg.enabled_agents = agents;
    let _ = config::save_config(&state.config_path, &cfg);
    let new_collectors = providers::build_collectors(&cfg, &state.home_dirs);
    if let Ok(mut app) = state.app.lock() {
        app.set_collectors(new_collectors);
    }
}
```

- [ ] **Step 5: Register all three in `invoke_handler!`**:
```rust
.invoke_handler(tauri::generate_handler![
    toggle_visibility,
    set_opacity,
    set_poll_interval,
    quit,
    get_usage_history,
    get_config,
    list_providers,
    set_enabled_agents,
    set_compact_view
])
```

- [ ] **Step 6: Compile check**

Run: `cd src-tauri && cargo build`
Expected: builds clean.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: add get_config/list_providers/set_enabled_agents commands"
```

### Task 9.2: Settings panel UI

**Files:**
- Modify: `dist/index.html`
- Modify: `dist/main.js`
- Modify: `dist/style.css`

- [ ] **Step 1: Add a gear button and a hidden settings panel** to `index.html`, inside `#panel` after `#titlebar`:
```html
<button id="settings-btn" title="Settings">&#9881;</button>
<div id="settings" class="hidden">
  <div class="settings-row">
    <label for="opacity-slider">Opacity</label>
    <input type="range" id="opacity-slider" min="0.1" max="1" step="0.05" />
  </div>
  <div class="settings-row">
    <label for="poll-interval-input">Poll interval (ms)</label>
    <input type="number" id="poll-interval-input" min="200" step="100" />
  </div>
  <div id="provider-toggles"></div>
</div>
```
Add the gear button next to `hide-btn` in `#titlebar`:
```html
<span id="settings-btn" title="Settings">&#9881;</span>
```
(remove the duplicate `#settings-btn` declaration above — it belongs only in the titlebar; `#settings` itself is the panel body, positioned as a sibling of `#content`.)

- [ ] **Step 2: Wire it up in `main.js`**:
```js
const settingsBtn = document.getElementById("settings-btn");
const settingsPanel = document.getElementById("settings");
const opacitySlider = document.getElementById("opacity-slider");
const pollIntervalInput = document.getElementById("poll-interval-input");
const providerToggles = document.getElementById("provider-toggles");

settingsBtn.addEventListener("click", async () => {
  const willOpen = settingsPanel.classList.contains("hidden");
  settingsPanel.classList.toggle("hidden");
  if (willOpen) await loadSettings();
});

async function loadSettings() {
  const [cfg, providers] = await Promise.all([
    window.__TAURI__.core.invoke("get_config"),
    window.__TAURI__.core.invoke("list_providers"),
  ]);
  opacitySlider.value = cfg.opacity;
  pollIntervalInput.value = cfg.poll_interval_ms;
  providerToggles.innerHTML = providers.map(([key, label]) => `
    <label class="settings-row">
      <input type="checkbox" data-provider="${key}" ${cfg.enabled_agents.includes(key) ? "checked" : ""} />
      ${escapeHtml(label)}
    </label>
  `).join("");
  providerToggles.querySelectorAll("input[type=checkbox]").forEach(cb => {
    cb.addEventListener("change", onProviderToggleChange);
  });
}

function onProviderToggleChange() {
  const checked = Array.from(providerToggles.querySelectorAll("input[type=checkbox]:checked"))
    .map(cb => cb.dataset.provider);
  window.__TAURI__.core.invoke("set_enabled_agents", { agents: checked });
}

opacitySlider.addEventListener("input", () => {
  window.__TAURI__.core.invoke("set_opacity", { opacity: parseFloat(opacitySlider.value) });
});
pollIntervalInput.addEventListener("change", () => {
  window.__TAURI__.core.invoke("set_poll_interval", { ms: parseInt(pollIntervalInput.value, 10) });
});

window.__TAURI__.event.listen("opacity://update", (e) => {
  panel.style.opacity = e.payload;
  opacitySlider.value = e.payload;
});
```
Note: `opacity://update` was previously emitted but never consumed (per Phase-0 prep notes) — this task is what finally wires it up, applying the opacity as CSS on `#panel` as the code comment in `lib.rs::set_opacity` already documents as the intended mechanism.

- [ ] **Step 3: Add CSS**:
```css
#settings-btn { background: transparent; border: none; color: #aaa; cursor: pointer; font-size: 13px; }
#settings { padding: 6px 10px; border-bottom: 1px solid rgba(255,255,255,0.08); display: flex; flex-direction: column; gap: 6px; font-size: 11px; }
#settings.hidden { display: none; }
.settings-row { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.settings-row input[type="range"], .settings-row input[type="number"] { width: 110px; }
```

- [ ] **Step 4: Manual verification** via the `run` skill — open settings, toggle a provider off, confirm its sessions disappear from the panel within one tick; toggle it back on.

- [ ] **Step 5: Commit**

```bash
git add dist/index.html dist/main.js dist/style.css
git commit -m "feat: add in-app settings panel for opacity/poll-interval/enabled providers"
```

---

## Phase 10 — Theming

### Task 10.1: CSS custom properties + light theme

**Files:**
- Modify: `dist/style.css`

- [ ] **Step 1: Introduce variables at the top of the file**, replacing hardcoded colors used more than once:
```css
:root {
  color-scheme: dark;
  --bg: rgba(20, 22, 28, 0.82);
  --border: rgba(255,255,255,0.12);
  --border-soft: rgba(255,255,255,0.06);
  --fg: #e6e6e6;
  --fg-muted: rgba(255,255,255,0.55);
  --surface: rgba(255,255,255,0.08);
  --accent: #6aa0ff;
}
:root[data-theme="light"] {
  color-scheme: light;
  --bg: rgba(245, 246, 248, 0.92);
  --border: rgba(0,0,0,0.12);
  --border-soft: rgba(0,0,0,0.08);
  --fg: #1c1e22;
  --fg-muted: rgba(0,0,0,0.55);
  --surface: rgba(0,0,0,0.06);
  --accent: #3b6fd6;
}
```

- [ ] **Step 2: Replace hardcoded usages** — `#panel { background: var(--bg); border: 1px solid var(--border); color: var(--fg); }`, `.model { background: var(--surface); }`, `.status { background: var(--surface); }`, `.bar { background: var(--surface); }`, `.bar-fill { background: var(--accent); }`, `.row { border-bottom: 1px solid var(--border-soft); }`, `#titlebar { background: var(--surface); }`, `#usage-limits { border-bottom: 1px solid var(--border); }`. (Status-specific greens/blues like `.status-executing`'s `rgba(80,200,120,0.25)` stay as-is — those are semantic status colors, not theme colors, and should read the same in both themes for consistency.)

- [ ] **Step 3: Manual verification** — temporarily set `document.documentElement.dataset.theme = "light"` in the browser devtools console during a `run`-skill session and confirm every panel element re-colors sensibly with no unreadable-contrast text.

- [ ] **Step 4: Commit**

```bash
git add dist/style.css
git commit -m "refactor: theme all overlay colors through CSS custom properties"
```

### Task 10.2: Config fields, `set_theme` command, and settings UI

**Files:**
- Modify: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `dist/index.html`
- Modify: `dist/main.js`

- [ ] **Step 1: Add config fields**:
```rust
#[serde(default = "default_theme")]
pub theme: String,
#[serde(default = "default_accent_color")]
pub accent_color: String,
```
```rust
fn default_theme() -> String {
    "dark".to_string()
}
fn default_accent_color() -> String {
    "#6aa0ff".to_string()
}
```
Add both to `default_config()`.

- [ ] **Step 2: Add the command**, in `lib.rs`:
```rust
#[tauri::command]
fn set_theme(window: tauri::Window, state: tauri::State<AppState>, theme: String, accent_color: String) {
    if let Ok(mut cfg) = state.config.lock() {
        cfg.theme = theme.clone();
        cfg.accent_color = accent_color.clone();
        let _ = config::save_config(&state.config_path, &cfg);
    }
    let _ = window.emit("theme://update", (theme, accent_color));
}
```
Register it in `invoke_handler!`, and emit the initial state at startup next to the existing `opacity://update`/`compact://update` emits:
```rust
let _ = w.emit("theme://update", (cfg.theme.clone(), cfg.accent_color.clone()));
```

- [ ] **Step 3: Apply it in `main.js`**:
```js
window.__TAURI__.event.listen("theme://update", (e) => {
  const [theme, accent] = e.payload;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.setProperty("--accent", accent);
});
```

- [ ] **Step 4: Add theme controls to the settings panel** in `index.html`, inside `#settings`:
```html
<div class="settings-row">
  <label for="theme-select">Theme</label>
  <select id="theme-select">
    <option value="dark">Dark</option>
    <option value="light">Light</option>
  </select>
</div>
<div class="settings-row">
  <label for="accent-input">Accent color</label>
  <input type="color" id="accent-input" />
</div>
```
Wire in `main.js`'s `loadSettings()`:
```js
const themeSelect = document.getElementById("theme-select");
const accentInput = document.getElementById("accent-input");
themeSelect.value = cfg.theme;
accentInput.value = cfg.accent_color;
```
and outside `loadSettings`, add change handlers:
```js
function pushTheme() {
  window.__TAURI__.core.invoke("set_theme", { theme: themeSelect.value, accentColor: accentInput.value });
}
themeSelect.addEventListener("change", pushTheme);
accentInput.addEventListener("input", pushTheme);
```

- [ ] **Step 5: Compile + manual verification**

Run: `cd src-tauri && cargo build`
Expected: builds clean. Then via the `run` skill: open settings, switch to Light, confirm the panel restyles immediately and the choice persists across a restart.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: add theme and accent-color settings"
```

---

## Final integration check

- [ ] **Full workspace build**: `cd src-tauri && cargo build --release`
- [ ] **Full test suite**: `cd src-tauri && cargo test`
- [ ] **Manual smoke test** via the `run` skill: launch the app, confirm sessions appear grouped by project, costs and history sparkline render, a usage-limit ETA shows once enough samples exist, compact-view toggle and settings panel (opacity/poll-interval/providers/theme) all work, and no regressions in the existing tray icon / global hotkey / drag behavior.
