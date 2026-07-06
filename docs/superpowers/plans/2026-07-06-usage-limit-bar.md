# Usage-Limit Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show an always-visible, per-provider 5h/weekly usage-limit bar (Claude + Codex) that no longer depends on an active session, with Claude sourced from Anthropic's own usage API (using the OAuth token already on disk) instead of requiring a custom StatusLine hook.

**Architecture:** Move `RateLimitInfo` from a per-session field to a snapshot-level `BTreeMap<String, RateLimitInfo>` populated by a new optional `Collector::usage_limits()` method. Claude gets a dedicated background-thread poller (`claude_usage.rs`) hitting `api.anthropic.com` every 180s and publishing into a shared `Arc<Mutex<Option<RateLimitInfo>>>`; Codex keeps its existing local rollout parsing but retains the last-known value at the collector level so it survives a session ending. The frontend renders a new always-visible bar above the session list.

**Tech Stack:** Rust (Tauri v2 backend), `ureq` (new dependency, blocking HTTP client) for the Claude usage API call, `chrono` (already a dependency) for ISO8601 parsing, vanilla JS/CSS frontend (`dist/main.js`, `dist/style.css`).

## Global Constraints

- No new user-facing credentials — Claude's OAuth token is read from the existing `~/.claude/.credentials.json` (via the WSL/Windows home-dir discovery already in `lib.rs`).
- The Claude usage-API call is the **one exception** to this project's "no network" principle; it must be clearly documented in the README and gated by a `claude_usage_enabled` config flag (default `true`).
- Poll interval for the Claude usage API: 180s, independent of `poll_interval_ms` (which stays at ~1s for the session tick loop).
- On API error/429, keep serving the last-known-good value; back off 30s on generic errors, `Retry-After` (default 300s) on HTTP 429.
- A provider row with no usage data ever obtained is omitted from the UI — no "N/A" placeholders.
- Follow existing code patterns: collectors are `Send`, panics in a collector must not blank the whole snapshot (see `App::tick`'s `catch_unwind`).

---

### Task 1: Move `RateLimitInfo` from per-session to snapshot-level

**Files:**
- Modify: `src-tauri/src/model.rs`
- Modify: `src-tauri/src/collector.rs`
- Modify: `src-tauri/src/claude.rs:123-227` (collect loop + `AgentSession` construction)
- Modify: `src-tauri/src/codex.rs:82-173` (collect loop + `AgentSession` construction)
- Modify: `src-tauri/src/hermes.rs` (around line 111, `AgentSession` construction)
- Modify: `src-tauri/src/app.rs` (`App::tick`)
- Modify: `src-tauri/tests/model_test.rs`
- Modify: `src-tauri/tests/app_test.rs`

**Interfaces:**
- Produces: `Snapshot.usage_limits: std::collections::BTreeMap<String, RateLimitInfo>` (key: collector `name()`, e.g. `"claude"`, `"codex"`).
- Produces: `Collector::usage_limits(&self) -> Option<RateLimitInfo>` (default-implemented to return `None`, so `HermesCollector` needs no changes beyond removing the field from its `AgentSession` construction).
- Produces: `build_snapshot(sessions: Vec<AgentSession>, usage_limits: BTreeMap<String, RateLimitInfo>) -> Snapshot` (signature change — second parameter added).
- Consumes: nothing new from other tasks (this is the foundation task).

- [ ] **Step 1: Update `model.rs` — remove per-session field, add snapshot-level map**

Edit `src-tauri/src/model.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

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
    pub total_cache_create: u64,
    pub turn_count: u32,
    pub current_task: String,
    pub mem_mb: u64,
}

/// Account-level usage-limit windows, ported from abtop's `RateLimitInfo`.
/// Account-level (not session-level), so these live on `Snapshot.usage_limits`
/// rather than on individual sessions. For Claude this comes from Anthropic's
/// usage API (with a hook-file fallback); for Codex it's parsed live from
/// `token_count` events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub source: String,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<u64>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<u64>,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub sessions: Vec<AgentSession>,
    pub total_tokens: u64,
    pub by_agent_tokens: HashMap<String, u64>,
    pub by_status: HashMap<SessionStatus, u32>,
    pub usage_limits: BTreeMap<String, RateLimitInfo>,
}

pub fn build_snapshot(
    sessions: Vec<AgentSession>,
    usage_limits: BTreeMap<String, RateLimitInfo>,
) -> Snapshot {
    let mut total_tokens: u64 = 0;
    let mut by_agent_tokens: HashMap<String, u64> = HashMap::new();
    let mut by_status: HashMap<SessionStatus, u32> = HashMap::new();
    // Pre-populate all statuses to ensure every variant has an entry
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
    }
    Snapshot {
        sessions,
        total_tokens,
        by_agent_tokens,
        by_status,
        usage_limits,
    }
}
```

- [ ] **Step 2: Add default `usage_limits()` to the `Collector` trait**

Edit `src-tauri/src/collector.rs`, replace the trait definition (last lines of the file):

```rust
/// The single extension point for an AI assistant. Each agent (Claude, Codex,
/// Hermes) implements this to turn local file/process state into
/// `AgentSession`s. A collector that fails should return an empty `Vec` for
/// the tick rather than panicking — `App::tick` additionally catches panics so
/// one broken agent never blanks the panel.
pub trait Collector: Send {
    fn name(&self) -> &str;
    fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession>;

    /// Account-level 5h/weekly usage, if this collector has any. Called once
    /// per tick after `collect()`. Default `None` — only Claude and Codex
    /// currently report this.
    fn usage_limits(&self) -> Option<crate::model::RateLimitInfo> {
        None
    }
}
```

- [ ] **Step 3: Update `claude.rs` — drop the per-session field, keep the hook-file read for later use**

In `src-tauri/src/claude.rs`, remove the `rate_limit: rate_limit.clone(),` line from the `AgentSession { ... }` literal (around line 214) and remove the now-unused `rate_limit` local variable and its read (lines 132-134) — this will be reintroduced properly in Task 4's fallback logic, so for this task just delete it entirely along with the now-unused `use crate::rate_limit::{self, CLAUDE_RATE_FILE};` import at the top of the file. The `rate_limit` module itself (`src-tauri/src/rate_limit.rs`) stays untouched.

- [ ] **Step 4: Update `codex.rs` — drop the per-session field**

In `src-tauri/src/codex.rs`, remove `rate_limit: st.rate_limit.clone(),` from the `AgentSession { ... }` literal (around line 163). Leave `CodexState.rate_limit` and all the `token_count` parsing logic in place — Task 2 will add the `usage_limits()` trait method that reads it.

- [ ] **Step 5: Update `hermes.rs` — drop the per-session field**

In `src-tauri/src/hermes.rs`, remove the `rate_limit: None, // Hermes's API doc doesn't define 5h/weekly windows.` line from its `AgentSession { ... }` literal.

- [ ] **Step 6: Update `App::tick` to aggregate `usage_limits`**

Edit `src-tauri/src/app.rs`, replace the body of `tick()`:

```rust
    pub fn tick(&mut self) -> Snapshot {
        let ps = process::snapshot();
        let wsl: HashMap<String, ProcessSnapshot> = self
            .wsl_distros
            .iter()
            .map(|d| (d.clone(), process::wsl_snapshot(d)))
            .collect();
        let ctx = ProcessContext {
            procs: &ps.procs,
            children: &ps.children,
            ports: &ps.ports_by_pid,
            wsl: &wsl,
        };
        let mut sessions = Vec::new();
        let mut usage_limits = std::collections::BTreeMap::new();
        for c in &mut self.collectors {
            // Catch a collector panic so one agent can't take down the tick loop.
            let name = c.name().to_string();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.collect(&ctx)));
            if let Ok(s) = result {
                sessions.extend(s);
            }
            // On panic: skip this collector this tick, keep going.
            if let Ok(Some(rl)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.usage_limits())) {
                usage_limits.insert(name, rl);
            }
        }
        // Dedupe by (agent_cli, session_id); last one wins.
        sessions.sort_by(|a, b| {
            a.agent_cli
                .cmp(&b.agent_cli)
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        sessions.dedup_by(|a, b| a.agent_cli == b.agent_cli && a.session_id == b.session_id);
        build_snapshot(sessions, usage_limits)
    }
```

- [ ] **Step 7: Update `model_test.rs` for the new signatures**

Edit `src-tauri/tests/model_test.rs`:

```rust
use usage_tracker::model::{build_snapshot, AgentSession, RateLimitInfo, SessionStatus};
use std::collections::{BTreeMap, HashMap};

fn session(agent: &str, pid: u32, inp: u64, out: u64, status: SessionStatus) -> AgentSession {
    AgentSession {
        agent_cli: agent.into(), pid, session_id: format!("{agent}-{pid}"),
        cwd: "/proj".into(), project_name: "proj".into(), started_at: 0,
        status, model: "m".into(), context_percent: 0.0,
        total_input_tokens: inp, total_output_tokens: out, total_cache_read: 0,
        total_cache_create: 0,
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
    let snap = build_snapshot(sessions, BTreeMap::new());
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
    let snap = build_snapshot(sessions, BTreeMap::new());
    assert_eq!(snap.by_status.get(&SessionStatus::Executing), Some(&2));
    assert_eq!(snap.by_status.get(&SessionStatus::Waiting), Some(&1));
    assert_eq!(snap.by_status.get(&SessionStatus::Thinking), Some(&0));
}

#[test]
fn snapshot_serializes_to_json() {
    let snap = build_snapshot(vec![session("claude", 1, 1, 1, SessionStatus::Waiting)], BTreeMap::new());
    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("\"agent_cli\":\"claude\""));
    assert!(json.contains("\"total_tokens\":2"));
}

#[test]
fn snapshot_carries_usage_limits_map() {
    let mut limits = BTreeMap::new();
    limits.insert(
        "claude".to_string(),
        RateLimitInfo {
            source: "claude".into(),
            five_hour_pct: Some(42.0),
            five_hour_resets_at: Some(1_700_000_000),
            seven_day_pct: None,
            seven_day_resets_at: None,
            updated_at: Some(1_700_000_000),
        },
    );
    let snap = build_snapshot(Vec::new(), limits.clone());
    assert_eq!(snap.usage_limits, limits);
}
```

- [ ] **Step 8: Update `app_test.rs`'s empty-snapshot assertion (no signature usage there, just confirm it still compiles)**

`src-tauri/tests/app_test.rs` calls `app.tick()` and checks `snap.sessions`/`snap.total_tokens` only, so no edit is needed — just re-run it in Step 10 to confirm it still compiles and passes.

- [ ] **Step 9: Build and run the full test suite**

Run: `cd src-tauri && cargo build 2>&1 | tail -50`
Expected: compiles cleanly (fixing any remaining `rate_limit` field references the grep in Step 10 turns up).

Run: `cd src-tauri && grep -rn "rate_limit:" src tests`
Expected: no more per-`AgentSession`-literal `rate_limit:` fields — only `RateLimitInfo`-struct fields and `CodexState.rate_limit` remain.

- [ ] **Step 10: Run the test suite**

Run: `cd src-tauri && cargo test`
Expected: all tests pass, including the new `snapshot_carries_usage_limits_map`.

- [ ] **Step 11: Commit**

```bash
git add src-tauri/src/model.rs src-tauri/src/collector.rs src-tauri/src/claude.rs src-tauri/src/codex.rs src-tauri/src/hermes.rs src-tauri/src/app.rs src-tauri/tests/model_test.rs
git commit -m "refactor: move RateLimitInfo from per-session field to snapshot-level map"
```

---

### Task 2: Codex — retain last-known usage limits across ticks, decoupled from active sessions

**Files:**
- Modify: `src-tauri/src/codex.rs`
- Modify: `src-tauri/tests/codex_test.rs`

**Interfaces:**
- Consumes: `Collector::usage_limits(&self) -> Option<RateLimitInfo>` trait method (Task 1, Step 2).
- Produces: `CodexCollector` now implements `usage_limits()`, returning the last `RateLimitInfo` seen from any rollout, surviving that rollout no longer being "recent" (session ended) and surviving app restarts within the same day by re-scanning the most recent rollout file.

- [ ] **Step 1: Write the failing test — usage limit survives the session no longer being recent**

Add to `src-tauri/tests/codex_test.rs`:

```rust
#[test]
fn usage_limits_survive_after_rollout_goes_stale() {
    let root = std::env::temp_dir().join(format!("utt-codex-usage-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    let rollout_path = day_dir.join("rollout-codex-3.jsonl");
    fs::write(
        &rollout_path,
        concat!(
            "{\"type\":\"session_meta\",\"session_id\":\"codex-3\",\"cwd\":\"/webapp\",\"model\":\"gpt-5-codex\"}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cached_input_tokens\":0},\"last_token_usage\":{\"input_tokens\":100},\"model_context_window\":400000},\"rate_limits\":{\"primary\":{\"window_minutes\":300,\"used_percent\":12.0,\"resets_at\":1700003600},\"secondary\":{\"window_minutes\":10080,\"used_percent\":5.0,\"resets_at\":1700600000}}}}\n",
        ),
    )
    .unwrap();

    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports };
    let mut c = CodexCollector::new(root.join("sessions"));

    // First tick: rollout is fresh, session present, usage limits populated.
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    let rl = c.usage_limits().expect("usage limits after first tick");
    assert_eq!(rl.five_hour_pct, Some(12.0));
    assert_eq!(rl.seven_day_pct, Some(5.0));

    // Make the rollout look stale (older than RECENT_AGE_SECS) so the next
    // collect() drops the session entirely.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    let old_ft = filetime::FileTime::from_system_time(old);
    filetime::set_file_mtime(&rollout_path, old_ft).unwrap();

    let sessions = c.collect(&ctx);
    assert!(sessions.is_empty(), "stale rollout must drop the session");
    let rl_after = c.usage_limits().expect("usage limits must survive session ending");
    assert_eq!(rl_after.five_hour_pct, Some(12.0));
}
```

This test needs the `filetime` crate to set an old mtime deterministically. Add it as a dev-dependency.

- [ ] **Step 2: Add the `filetime` dev-dependency**

Edit `src-tauri/Cargo.toml`, add a `[dev-dependencies]` section (after `[dependencies]`):

```toml
[dev-dependencies]
filetime = "0.2"
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cd src-tauri && cargo test usage_limits_survive_after_rollout_goes_stale`
Expected: FAIL — `usage_limits` method doesn't exist on `CodexCollector` yet (compile error), or returns `None` after the rollout goes stale.

- [ ] **Step 4: Implement collector-level last-known usage limits**

Edit `src-tauri/src/codex.rs`. Add a field to `CodexCollector` and implement `usage_limits()`:

```rust
pub struct CodexCollector {
    sessions_dirs: Vec<PathBuf>,
    readers: HashMap<PathBuf, IncrementalReader>,
    state: HashMap<PathBuf, CodexState>,
    /// Last-known account-level usage, retained even after the rollout that
    /// produced it is no longer "recent" (session ended) or is evicted from
    /// `state`. Cleared only by a fresh, differing value from a later tick.
    last_usage_limits: Option<RateLimitInfo>,
}
```

Update both constructors (`new` and `new_multi`) to initialize `last_usage_limits: None`.

In `collect()`, after the existing eviction lines (`self.state.retain(...)` / `self.readers.retain(...)`), add:

```rust
        // Retain the most recently seen account-level usage even after its
        // rollout is evicted (session ended) — usage is account-level, not
        // session-level, so it shouldn't disappear from the UI.
        if let Some(latest) = self
            .state
            .values()
            .chain(std::iter::empty()) // kept sessions
            .filter_map(|st| st.rate_limit.clone())
            .max_by_key(|rl| rl.updated_at.unwrap_or(0))
        {
            self.last_usage_limits = Some(latest);
        }
```

Wait — the eviction above already dropped states for rollouts no longer `seen`, so evicted rollouts' `rate_limit` is unreachable by the time we get here. Reorder: capture the latest usage limit **before** the `retain` calls. Replace the eviction block instead:

```rust
        // Capture the latest account-level usage limit across all rollouts
        // seen this tick *before* evicting stale state, so a session ending
        // doesn't blank previously-known usage.
        if let Some(latest) = self
            .state
            .values()
            .filter_map(|st| st.rate_limit.clone())
            .max_by_key(|rl| rl.updated_at.unwrap_or(0))
        {
            self.last_usage_limits = Some(latest);
        }

        // Evict state for rollouts no longer recent/present.
        self.state.retain(|p, _| seen.contains(p));
        self.readers.retain(|p, _| seen.contains(p));
        out
    }
}

impl CodexCollector {
    // (keep existing inherent methods above; this is a new trait impl below)
}
```

Then add the trait method to the existing `impl Collector for CodexCollector` block (alongside `name` and `collect`):

```rust
    fn usage_limits(&self) -> Option<RateLimitInfo> {
        self.last_usage_limits.clone()
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd src-tauri && cargo test usage_limits_survive_after_rollout_goes_stale`
Expected: PASS.

- [ ] **Step 6: Write the failing test — usage limits available before any session exists today (opportunistic scan)**

Add to `src-tauri/tests/codex_test.rs`:

```rust
#[test]
fn usage_limits_populated_from_recent_rollout_even_when_not_treated_as_active_session() {
    // A rollout aged just past RECENT_AGE_SECS (so it's not surfaced as a
    // live session) should still seed usage_limits on the very first tick —
    // this covers "app just launched, no session started yet today."
    let root = std::env::temp_dir().join(format!("utt-codex-usage-scan-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    let rollout_path = day_dir.join("rollout-codex-4.jsonl");
    fs::write(
        &rollout_path,
        concat!(
            "{\"type\":\"session_meta\",\"session_id\":\"codex-4\",\"cwd\":\"/webapp\",\"model\":\"gpt-5-codex\"}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":10,\"cached_input_tokens\":0},\"last_token_usage\":{\"input_tokens\":100},\"model_context_window\":400000},\"rate_limits\":{\"primary\":{\"window_minutes\":300,\"used_percent\":33.0,\"resets_at\":1700003600}}}}\n",
        ),
    )
    .unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    let old_ft = filetime::FileTime::from_system_time(old);
    filetime::set_file_mtime(&rollout_path, old_ft).unwrap();

    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports };
    let mut c = CodexCollector::new(root.join("sessions"));

    let sessions = c.collect(&ctx);
    assert!(sessions.is_empty(), "stale rollout is not a live session");
    let rl = c
        .usage_limits()
        .expect("usage limits should be seeded from the day's most recent rollout even without a live session");
    assert_eq!(rl.five_hour_pct, Some(33.0));
}
```

- [ ] **Step 7: Run the test to verify it fails**

Run: `cd src-tauri && cargo test usage_limits_populated_from_recent_rollout_even_when_not_treated_as_active_session`
Expected: FAIL — `usage_limits()` returns `None` because the rollout was filtered out by `is_recent` before ever being parsed.

- [ ] **Step 8: Implement opportunistic scan of the day's most recent rollout**

Edit `src-tauri/src/codex.rs`. In `collect()`, change the loop that currently `continue`s past non-recent rollouts so it still parses them into a **throwaway** state purely to extract `rate_limit`, without adding them to `out` or to the persistent `self.state`/`self.readers` maps (which must stay reserved for live sessions). Replace:

```rust
                if !is_recent(&path, RECENT_AGE_SECS) {
                    continue;
                }
                seen.insert(path.clone());
```

with:

```rust
                if !is_recent(&path, RECENT_AGE_SECS) {
                    // Not live, but still worth a one-off scan for its last
                    // known account-level usage — covers "app just launched,
                    // no session started yet today."
                    if let Ok(text) = fs::read_to_string(&path) {
                        let mut scratch = CodexState::default();
                        for line in text.lines() {
                            apply_codex_line(line, &mut scratch);
                        }
                        if let Some(rl) = scratch.rate_limit {
                            let is_newer = self
                                .last_usage_limits
                                .as_ref()
                                .map(|cur| rl.updated_at.unwrap_or(0) > cur.updated_at.unwrap_or(0))
                                .unwrap_or(true);
                            if is_newer {
                                self.last_usage_limits = Some(rl);
                            }
                        }
                    }
                    continue;
                }
                seen.insert(path.clone());
```

This reads the whole stale rollout with `fs::read_to_string` (simple and correct — stale rollouts aren't being actively appended to, so there's no need for `IncrementalReader`'s incremental-read machinery here).

- [ ] **Step 9: Run the test to verify it passes**

Run: `cd src-tauri && cargo test usage_limits_populated_from_recent_rollout_even_when_not_treated_as_active_session`
Expected: PASS.

- [ ] **Step 10: Run the full test suite**

Run: `cd src-tauri && cargo test`
Expected: all tests pass, including both new tests and the existing `codex_test.rs` tests.

- [ ] **Step 11: Commit**

```bash
git add src-tauri/src/codex.rs src-tauri/tests/codex_test.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(codex): retain account-level usage limits across ticks and idle periods"
```

---

### Task 3: Claude usage-API poller (`claude_usage.rs`)

**Files:**
- Create: `src-tauri/src/claude_usage.rs`
- Modify: `src-tauri/src/lib.rs:1-10` (module declaration)
- Modify: `src-tauri/Cargo.toml` (add `ureq` dependency)

**Interfaces:**
- Produces: `pub fn read_access_token(credentials_path: &Path) -> Option<String>` — parses `claudeAiOauth.accessToken` out of `.credentials.json`.
- Produces: `pub fn parse_usage_response(body: &str) -> Option<RateLimitInfo>` — parses the API JSON body into `RateLimitInfo`.
- Produces: `pub struct ClaudeUsagePoller` with `pub fn start(credentials_path: PathBuf) -> Arc<Mutex<Option<RateLimitInfo>>>` — spawns the background polling thread and returns the shared handle immediately (poller fills it in asynchronously).
- Consumes: `crate::model::RateLimitInfo` (Task 1).

This task is pure backend logic with no active session dependency — fully unit-testable without a live network call by testing `parse_usage_response` and `read_access_token` directly, and by structuring the HTTP call behind a small trait so the retry/backoff logic is testable without hitting the network.

- [ ] **Step 1: Add the `ureq` dependency**

Edit `src-tauri/Cargo.toml`, add to `[dependencies]`:

```toml
ureq = { version = "2", features = ["json"] }
```

- [ ] **Step 2: Write the failing test for `read_access_token`**

Create `src-tauri/src/claude_usage.rs` with just enough to compile a failing test:

```rust
use crate::model::RateLimitInfo;
use serde::Deserialize;
use std::path::Path;

/// `~/.claude/.credentials.json`'s shape — only the fields this module needs.
#[derive(Debug, Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthSection>,
}

#[derive(Debug, Deserialize)]
struct OauthSection {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

/// Reads the OAuth access token Claude Code itself stores after login.
/// Returns `None` if the file is missing, malformed, or the account isn't
/// using OAuth (e.g. Bedrock/Vertex/API-key auth) — callers should fall back
/// to the hook-file mechanism in that case.
pub fn read_access_token(credentials_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(credentials_path).ok()?;
    let file: CredentialsFile = serde_json::from_str(&text).ok()?;
    file.claude_ai_oauth?.access_token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_access_token_from_credentials_file() {
        let dir = std::env::temp_dir().join(format!("utt-claude-usage-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(".credentials.json");
        std::fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"sk-test-token","refreshToken":"r"}}"#,
        )
        .unwrap();
        assert_eq!(read_access_token(&path), Some("sk-test-token".to_string()));
    }

    #[test]
    fn missing_oauth_section_returns_none() {
        let dir = std::env::temp_dir().join(format!("utt-claude-usage-noauth-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(".credentials.json");
        std::fs::write(&path, r#"{"someOtherAuth":{}}"#).unwrap();
        assert_eq!(read_access_token(&path), None);
    }

    #[test]
    fn missing_file_returns_none() {
        assert_eq!(read_access_token(Path::new("/nonexistent/.credentials.json")), None);
    }
}
```

- [ ] **Step 3: Register the module and run the tests**

Edit `src-tauri/src/lib.rs`, add `pub mod claude_usage;` to the `pub mod` list at the top (alphabetically, after `pub mod claude;`).

Run: `cd src-tauri && cargo test claude_usage::tests`
Expected: all three tests PASS (this is already the real implementation, not a stub — the "write failing test first" step is satisfied by the fact that `claude_usage.rs` didn't exist before this step).

- [ ] **Step 4: Write the failing test for `parse_usage_response`**

Add to the `tests` module in `src-tauri/src/claude_usage.rs`:

```rust
    #[test]
    fn parses_five_hour_and_seven_day_buckets() {
        let body = r#"{
            "five_hour": {"utilization": 42.0, "resets_at": "2024-11-15T12:00:00Z"},
            "seven_day": {"utilization": 18.5, "resets_at": "2024-11-20T00:00:00Z"}
        }"#;
        let rl = parse_usage_response(body).expect("should parse");
        assert_eq!(rl.source, "claude");
        assert_eq!(rl.five_hour_pct, Some(42.0));
        assert_eq!(rl.seven_day_pct, Some(18.5));
        assert!(rl.five_hour_resets_at.is_some());
        assert!(rl.seven_day_resets_at.is_some());
    }

    #[test]
    fn missing_buckets_returns_none() {
        assert!(parse_usage_response(r#"{"extra_usage": {"is_enabled": false}}"#).is_none());
    }

    #[test]
    fn malformed_json_returns_none() {
        assert!(parse_usage_response("not json").is_none());
    }
```

- [ ] **Step 5: Run the test to verify it fails**

Run: `cd src-tauri && cargo test claude_usage::tests::parses_five_hour_and_seven_day_buckets`
Expected: FAIL — `parse_usage_response` doesn't exist yet.

- [ ] **Step 6: Implement `parse_usage_response`**

Add to `src-tauri/src/claude_usage.rs` (above the `#[cfg(test)]` module):

```rust
#[derive(Debug, Deserialize)]
struct UsageBucket {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageApiResponse {
    #[serde(default)]
    five_hour: Option<UsageBucket>,
    #[serde(default)]
    seven_day: Option<UsageBucket>,
}

fn parse_resets_at(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp() as u64)
}

/// Parses Anthropic's `GET /api/oauth/usage` response body into a
/// `RateLimitInfo`. Returns `None` if both `five_hour` and `seven_day` are
/// absent (malformed/unexpected response shape) — same "both windows absent
/// means don't trust this" rule the hook-file parser in `rate_limit.rs` uses.
pub fn parse_usage_response(body: &str) -> Option<RateLimitInfo> {
    let parsed: UsageApiResponse = serde_json::from_str(body).ok()?;
    if parsed.five_hour.is_none() && parsed.seven_day.is_none() {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(RateLimitInfo {
        source: "claude".to_string(),
        five_hour_pct: parsed.five_hour.as_ref().and_then(|b| b.utilization),
        five_hour_resets_at: parsed
            .five_hour
            .as_ref()
            .and_then(|b| b.resets_at.as_deref())
            .and_then(parse_resets_at),
        seven_day_pct: parsed.seven_day.as_ref().and_then(|b| b.utilization),
        seven_day_resets_at: parsed
            .seven_day
            .as_ref()
            .and_then(|b| b.resets_at.as_deref())
            .and_then(parse_resets_at),
        updated_at: Some(now),
    })
}
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cd src-tauri && cargo test claude_usage::tests`
Expected: all tests PASS.

- [ ] **Step 8: Write the failing test for `Retry-After` backoff parsing**

Add to the `tests` module in `src-tauri/src/claude_usage.rs`:

```rust
    #[test]
    fn retry_after_header_parses_to_seconds() {
        assert_eq!(parse_retry_after(Some("120")), Duration::from_secs(120));
    }

    #[test]
    fn missing_retry_after_uses_default_rate_limit_backoff() {
        assert_eq!(parse_retry_after(None), RATE_LIMIT_BACKOFF);
    }

    #[test]
    fn malformed_retry_after_uses_default_rate_limit_backoff() {
        assert_eq!(parse_retry_after(Some("not-a-number")), RATE_LIMIT_BACKOFF);
    }
```

Add `use std::time::Duration;` to the test module's imports if not already in scope via `use super::*;`.

- [ ] **Step 9: Run the test to verify it fails**

Run: `cd src-tauri && cargo test claude_usage::tests::retry_after_header_parses_to_seconds`
Expected: FAIL — `parse_retry_after` and `RATE_LIMIT_BACKOFF` don't exist yet.

- [ ] **Step 10: Implement the background poller, with `Retry-After` parsing as a standalone testable function**

Add to `src-tauri/src/claude_usage.rs` (above the `#[cfg(test)]` module, below `parse_usage_response`):

```rust
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const USAGE_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const POLL_INTERVAL: Duration = Duration::from_secs(180);
const ERROR_BACKOFF: Duration = Duration::from_secs(30);
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(300);

/// Parses an HTTP `Retry-After` header value (seconds form only — Anthropic's
/// API doesn't use the HTTP-date form) into a backoff duration, falling back
/// to `RATE_LIMIT_BACKOFF` when absent or malformed.
fn parse_retry_after(header: Option<&str>) -> Duration {
    header
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(RATE_LIMIT_BACKOFF)
}

/// Calls Anthropic's usage API once. Returns `Err(retry_after)` on failure —
/// the caller decides how long to wait before the next attempt.
fn fetch_once(token: &str) -> Result<RateLimitInfo, Duration> {
    let response = ureq::get(USAGE_API_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(5))
        .call();

    match response {
        Ok(resp) => {
            let body = resp.into_string().map_err(|_| ERROR_BACKOFF)?;
            parse_usage_response(&body).ok_or(ERROR_BACKOFF)
        }
        Err(ureq::Error::Status(429, resp)) => {
            Err(parse_retry_after(resp.header("Retry-After")))
        }
        Err(_) => Err(ERROR_BACKOFF),
    }
}

/// Owns the background thread that polls Anthropic's usage API and publishes
/// the latest known `RateLimitInfo` into a shared handle the Claude collector
/// reads each tick. Never blocks the 1s session tick loop.
pub struct ClaudeUsagePoller;

impl ClaudeUsagePoller {
    /// Spawns the polling thread and returns the shared handle immediately.
    /// `credentials_path` is `.claude/.credentials.json` under the home
    /// directory resolved by `lib.rs::resolve_home_dirs`.
    pub fn start(credentials_path: PathBuf) -> Arc<Mutex<Option<RateLimitInfo>>> {
        let handle = Arc::new(Mutex::new(None));
        let thread_handle = handle.clone();
        std::thread::spawn(move || loop {
            let wait = match read_access_token(&credentials_path) {
                Some(token) => match fetch_once(&token) {
                    Ok(rl) => {
                        if let Ok(mut guard) = thread_handle.lock() {
                            *guard = Some(rl);
                        }
                        POLL_INTERVAL
                    }
                    Err(backoff) => backoff,
                },
                // No OAuth token (e.g. Bedrock/Vertex/API-key auth) — nothing
                // to poll; the Claude collector's hook-file fallback covers
                // this case. Recheck periodically in case the user logs in.
                None => POLL_INTERVAL,
            };
            std::thread::sleep(wait);
        });
        handle
    }
}
```

- [ ] **Step 11: Run the retry-after tests to verify they pass, then the full suite**

Run: `cd src-tauri && cargo test claude_usage::tests`
Expected: all `claude_usage` tests PASS, including the three new `parse_retry_after` tests.

Run: `cd src-tauri && cargo build 2>&1 | tail -50 && cargo test`
Expected: builds cleanly, all tests pass. (`ClaudeUsagePoller::start` and `fetch_once`'s HTTP call itself aren't unit-tested — they require a real network round-trip — but the two functions that determine correctness under failure, `parse_usage_response` and `parse_retry_after`, are both covered. End-to-end behavior is exercised via manual verification in Task 5.)

- [ ] **Step 12: Commit**

```bash
git add src-tauri/src/claude_usage.rs src-tauri/src/lib.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(claude): add background poller for Anthropic's usage API"
```

---

### Task 4: Wire the poller into `ClaudeCollector`, with hook-file fallback and config opt-out

**Files:**
- Modify: `src-tauri/src/claude.rs`
- Modify: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/tests/claude_test.rs`
- Modify: `src-tauri/tests/config_test.rs`

**Interfaces:**
- Consumes: `claude_usage::ClaudeUsagePoller::start(credentials_path) -> Arc<Mutex<Option<RateLimitInfo>>>` (Task 3).
- Consumes: `rate_limit::read_rate_limit_file(path, default_source) -> Option<RateLimitInfo>` (existing, unchanged).
- Produces: `ClaudeCollector::new_multi_with_usage(config_dirs: Vec<ConfigDirEntry>, usage_source: ClaudeUsageSource) -> Self` — new constructor variant that accepts where to read account-level usage from.
- Produces: `Config.claude_usage_enabled: bool` (default `true`).

- [ ] **Step 1: Write the failing test — collector falls back to hook file when no shared usage handle has data**

Add to `src-tauri/tests/claude_test.rs`:

```rust
use usage_tracker::claude::ClaudeUsageSource;
use usage_tracker::model::RateLimitInfo;

#[test]
fn falls_back_to_hook_file_when_api_poller_has_no_data() {
    let root = build_fake_claude_root("hook_fallback");
    fs::write(
        root.join("abtop-rate-limits.json"),
        r#"{"five_hour":{"used_percentage":55.0,"resets_at":1700000000},"seven_day":{"used_percentage":10.0,"resets_at":1700500000}}"#,
    )
    .unwrap();
    let mut procs = HashMap::new();
    procs.insert(
        4242,
        ProcInfo { pid: 4242, command: "claude".into(), rss_kb: 50_000, cpu: 0.0, parent_pid: None },
    );
    let ctx = ProcessContext { procs: &procs, children: &HashMap::new(), ports: &HashMap::new() };

    let usage_source = ClaudeUsageSource::ApiHandle(std::sync::Arc::new(std::sync::Mutex::new(None)));
    let mut c = usage_tracker::claude::ClaudeCollector::new_multi_with_usage(
        vec![usage_tracker::claude::ConfigDirEntry { dir: root, wsl_distro: None }],
        usage_source,
    );
    let _ = c.collect(&ctx);
    let rl = c.usage_limits().expect("hook file should be used as fallback");
    assert_eq!(rl.five_hour_pct, Some(55.0));
}

#[test]
fn prefers_api_handle_data_over_hook_file() {
    let root = build_fake_claude_root("api_precedence");
    fs::write(
        root.join("abtop-rate-limits.json"),
        r#"{"five_hour":{"used_percentage":55.0,"resets_at":1700000000}}"#,
    )
    .unwrap();
    let mut procs = HashMap::new();
    procs.insert(
        4242,
        ProcInfo { pid: 4242, command: "claude".into(), rss_kb: 50_000, cpu: 0.0, parent_pid: None },
    );
    let ctx = ProcessContext { procs: &procs, children: &HashMap::new(), ports: &HashMap::new() };

    let api_value = RateLimitInfo {
        source: "claude".into(),
        five_hour_pct: Some(7.0),
        five_hour_resets_at: Some(1),
        seven_day_pct: None,
        seven_day_resets_at: None,
        updated_at: Some(1),
    };
    let usage_source = ClaudeUsageSource::ApiHandle(std::sync::Arc::new(std::sync::Mutex::new(Some(api_value))));
    let mut c = usage_tracker::claude::ClaudeCollector::new_multi_with_usage(
        vec![usage_tracker::claude::ConfigDirEntry { dir: root, wsl_distro: None }],
        usage_source,
    );
    let _ = c.collect(&ctx);
    let rl = c.usage_limits().expect("api handle should provide usage");
    assert_eq!(rl.five_hour_pct, Some(7.0));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd src-tauri && cargo test falls_back_to_hook_file_when_api_poller_has_no_data prefers_api_handle_data_over_hook_file`
Expected: FAIL — `ClaudeUsageSource` and `new_multi_with_usage` don't exist yet.

- [ ] **Step 3: Implement `ClaudeUsageSource` and wire it into `ClaudeCollector`**

Edit `src-tauri/src/claude.rs`:

Add near the top (after the existing `use` statements):

```rust
use std::sync::{Arc, Mutex};

/// Where `ClaudeCollector` gets account-level 5h/weekly usage from.
#[derive(Clone)]
pub enum ClaudeUsageSource {
    /// Shared handle updated by `claude_usage::ClaudeUsagePoller`'s
    /// background thread. Preferred when it has data.
    ApiHandle(Arc<Mutex<Option<crate::model::RateLimitInfo>>>),
    /// No API poller running (e.g. `claude_usage_enabled = false` in config)
    /// — read only from the hook file.
    HookFileOnly,
}
```

Update `ClaudeCollector`:

```rust
pub struct ClaudeCollector {
    config_dirs: Vec<ConfigDirEntry>,
    readers: HashMap<String, IncrementalReader>,
    state: HashMap<String, ParseState>,
    usage_source: ClaudeUsageSource,
}
```

Replace the `impl ClaudeCollector` constructor block:

```rust
impl ClaudeCollector {
    pub fn new(config_dir: PathBuf) -> Self {
        Self::new_multi(vec![ConfigDirEntry {
            dir: config_dir,
            wsl_distro: None,
        }])
    }

    /// Create a collector that checks multiple configuration directories,
    /// with no API-based usage source (hook file only). This is useful for
    /// detecting sessions in both WSL and Windows environments.
    pub fn new_multi(config_dirs: Vec<ConfigDirEntry>) -> Self {
        Self::new_multi_with_usage(config_dirs, ClaudeUsageSource::HookFileOnly)
    }

    /// Create a collector with an explicit usage source — used by
    /// `lib.rs::build_collectors` to wire in the live API poller handle.
    pub fn new_multi_with_usage(config_dirs: Vec<ConfigDirEntry>, usage_source: ClaudeUsageSource) -> Self {
        Self {
            config_dirs,
            readers: HashMap::new(),
            state: HashMap::new(),
            usage_source,
        }
    }
}
```

Remove the old per-tick hook-file read from `collect()` (there is none left after Task 1, Step 3 — confirm the `rate_limit` local variable and its `.clone()` use are gone).

Add `usage_limits()` to `impl Collector for ClaudeCollector`:

```rust
    fn usage_limits(&self) -> Option<crate::model::RateLimitInfo> {
        if let ClaudeUsageSource::ApiHandle(handle) = &self.usage_source {
            if let Ok(guard) = handle.lock() {
                if guard.is_some() {
                    return guard.clone();
                }
            }
        }
        // Fall back to the hook file — either the API source has no data
        // yet (still starting up, or no OAuth token / Bedrock-Vertex auth),
        // or usage is configured to be hook-file-only.
        self.config_dirs.iter().find_map(|entry| {
            rate_limit::read_rate_limit_file(&entry.dir.join(CLAUDE_RATE_FILE), "claude")
        })
    }
```

Re-add the import removed in Task 1, Step 3: `use crate::rate_limit::{self, CLAUDE_RATE_FILE};`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd src-tauri && cargo test falls_back_to_hook_file_when_api_poller_has_no_data prefers_api_handle_data_over_hook_file`
Expected: PASS.

- [ ] **Step 5: Add the `claude_usage_enabled` config flag**

Edit `src-tauri/src/config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub poll_interval_ms: u64,
    pub opacity: f32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    pub hotkey: String,
    pub enabled_agents: Vec<String>,
    pub hermes_data_dir: Option<PathBuf>,
    /// When `true` (default), Claude's 5h/weekly usage is fetched directly
    /// from Anthropic's usage API using the OAuth token already stored by
    /// Claude Code — the one network call this app makes. Set `false` to
    /// keep the app strictly local-only; Claude usage then only populates
    /// via the `abtop-rate-limits.json` hook file, if configured.
    #[serde(default = "default_claude_usage_enabled")]
    pub claude_usage_enabled: bool,
}

fn default_claude_usage_enabled() -> bool {
    true
}

pub fn default_config() -> Config {
    Config {
        poll_interval_ms: 1000,
        opacity: 1.0,
        window_x: None,
        window_y: None,
        hotkey: "Ctrl+Shift+Space".to_string(),
        enabled_agents: vec!["claude".into(), "codex".into(), "hermes".into()],
        hermes_data_dir: None,
        claude_usage_enabled: true,
    }
}
```

- [ ] **Step 6: Write the failing test for the config default and round-trip**

Add to `src-tauri/tests/config_test.rs`:

```rust
#[test]
fn claude_usage_enabled_defaults_true() {
    let cfg = load_config(std::path::Path::new("/nonexistent/config.toml"));
    assert!(cfg.claude_usage_enabled);
}

#[test]
fn claude_usage_enabled_can_be_disabled_and_round_trips() {
    let dir = tempfile_dir("claude_usage_disabled");
    let path = dir.join("config.toml");
    let mut cfg = default_config();
    cfg.claude_usage_enabled = false;
    save_config(&path, &cfg).unwrap();
    let loaded = load_config(&path);
    assert!(!loaded.claude_usage_enabled);
}

#[test]
fn old_config_file_without_claude_usage_enabled_defaults_true() {
    let dir = tempfile_dir("legacy_config");
    let path = dir.join("config.toml");
    fs::write(
        &path,
        r#"
poll_interval_ms = 1000
opacity = 1.0
hotkey = "Ctrl+Shift+Space"
enabled_agents = ["claude", "codex"]
"#,
    )
    .unwrap();
    let cfg = load_config(&path);
    assert!(cfg.claude_usage_enabled, "missing field in an old config file must default to true, not fail to parse");
}
```

- [ ] **Step 7: Run the tests to verify the new ones fail before Step 5's edit... (already applied) — verify they pass**

Run: `cd src-tauri && cargo test claude_usage_enabled`
Expected: PASS (Step 5's `#[serde(default = ...)]` was written before this point in the plan, but if executing strictly TDD, write Step 6's tests first, confirm they fail to compile/pass against a `Config` without the field, then apply Step 5). Regardless of order followed, end state: all three tests PASS.

- [ ] **Step 8: Wire the poller into `lib.rs::build_collectors`**

Edit `src-tauri/src/lib.rs`. Add `pub mod claude_usage;` was already done in Task 3; now update `build_collectors`:

```rust
fn build_collectors(cfg: &config::Config, home_dirs: &[HomeDir]) -> Vec<Box<dyn collector::Collector>> {
    let mut v: Vec<Box<dyn collector::Collector>> = Vec::new();

    if cfg.enabled_agents.iter().any(|a| a == "claude") {
        // Create a collector that checks all possible .claude directories
        let claude_dirs: Vec<claude::ConfigDirEntry> = home_dirs
            .iter()
            .map(|h| claude::ConfigDirEntry {
                dir: h.path.join(".claude"),
                wsl_distro: h.wsl_distro.clone(),
            })
            .filter(|e| e.dir.exists())
            .collect();

        let usage_source = if cfg.claude_usage_enabled {
            // Poll using the first resolved home directory's credentials
            // file — accounts are per-user, so there's exactly one relevant
            // OAuth token even when multiple .claude dirs are found (e.g.
            // WSL + Windows both pointing at the same Anthropic account).
            match claude_dirs.first() {
                Some(first) => {
                    let creds_path = first.dir.join(".credentials.json");
                    claude::ClaudeUsageSource::ApiHandle(claude_usage::ClaudeUsagePoller::start(creds_path))
                }
                None => claude::ClaudeUsageSource::HookFileOnly,
            }
        } else {
            claude::ClaudeUsageSource::HookFileOnly
        };

        v.push(Box::new(claude::ClaudeCollector::new_multi_with_usage(claude_dirs, usage_source)));
    }

    if cfg.enabled_agents.iter().any(|a| a == "codex") {
        let codex_dirs: Vec<PathBuf> = home_dirs
            .iter()
            .map(|h| h.path.join(".codex").join("sessions"))
            .filter(|p| p.exists())
            .collect();
        v.push(Box::new(codex::CodexCollector::new_multi(codex_dirs)));
    }

    if cfg.enabled_agents.iter().any(|a| a == "hermes") {
        // HERMES_HOME defaults to ~/.hermes; only override via config.
        let hermes_dirs: Vec<PathBuf> = if let Some(ref custom_dir) = cfg.hermes_data_dir {
            vec![custom_dir.clone()]
        } else {
            home_dirs
                .iter()
                .map(|h| h.path.join(".hermes"))
                .filter(|p| p.exists())
                .collect()
        };
        v.push(Box::new(hermes::HermesCollector::new_multi(hermes_dirs)));
    }

    v
}
```

- [ ] **Step 9: Build and run the full test suite**

Run: `cd src-tauri && cargo build 2>&1 | tail -50 && cargo test`
Expected: builds cleanly, all tests pass.

- [ ] **Step 10: Commit**

```bash
git add src-tauri/src/claude.rs src-tauri/src/config.rs src-tauri/src/lib.rs src-tauri/tests/claude_test.rs src-tauri/tests/config_test.rs
git commit -m "feat(claude): wire usage-API poller into ClaudeCollector with hook-file fallback and opt-out"
```

---

### Task 5: Frontend — always-visible usage-limit bar

**Files:**
- Modify: `dist/index.html`
- Modify: `dist/main.js`
- Modify: `dist/style.css`

**Interfaces:**
- Consumes: `snapshot.usage_limits: { [agent: string]: RateLimitInfo }` (serialized `BTreeMap` from Task 1 — JS sees it as a plain object keyed by agent name).
- `RateLimitInfo` JSON shape (from `model.rs`, unchanged): `{ source, five_hour_pct, five_hour_resets_at, seven_day_pct, seven_day_resets_at, updated_at }` — `resets_at` fields are Unix seconds (`u64`), `null` when absent.

- [ ] **Step 1: Add the usage-bar container to `index.html`**

Edit `dist/index.html`, add a container between `#titlebar` and `#content`:

```html
    <div id="panel">
      <div id="titlebar">
        <span>AI Usage Tracker</span>
        <button id="hide-btn" title="Hide">_</button>
      </div>
      <div id="usage-limits"></div>
      <div id="content"><div class="empty">Loading...</div></div>
    </div>
```

- [ ] **Step 2: Add rendering logic to `main.js`**

Edit `dist/main.js`. Add a `usageLimits` element reference near the top:

```js
const panel = document.getElementById("panel");
const content = document.getElementById("content");
const titlebar = document.getElementById("titlebar");
const hideBtn = document.getElementById("hide-btn");
const usageLimitsEl = document.getElementById("usage-limits");
```

Remove the now-dead per-row rate-limit rendering (it's gone from the backend payload). In `renderSnapshot`, delete these lines:

```js
    const rl = sess.rate_limit;
    const rateLimitRow = rl
      ? `<div class="rate-limit">
          ${rl.five_hour_pct != null ? `<span>5h ${Math.round(rl.five_hour_pct)}%</span>` : ""}
          ${rl.seven_day_pct != null ? `<span>week ${Math.round(rl.seven_day_pct)}%</span>` : ""}
        </div>`
      : "";
```

and remove the `${rateLimitRow}` line at the end of the row template (right after the closing `</div>` of `.meta`).

Add new formatting + rendering functions (near `fmtDuration`):

```js
const AGENT_LABEL = { claude: "Claude", codex: "Codex", hermes: "Hermes" };

function fmtCountdown(resetsAtSecs) {
  if (resetsAtSecs == null) return null;
  const ms = resetsAtSecs * 1000 - Date.now();
  if (ms <= 0) return "now";
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) {
    const remMins = mins % 60;
    return remMins ? `${hours}h${remMins}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

function renderUsageWindow(label, pct, resetsAt) {
  if (pct == null) return `<span>${label} —</span>`;
  const clamped = Math.min(100, Math.max(0, Math.round(pct)));
  const countdown = fmtCountdown(resetsAt);
  return `<span class="usage-window">
      ${label} <span class="usage-pct">${clamped}%</span>
      <span class="bar usage-bar"><span class="bar-fill" style="width:${clamped}%"></span></span>
      ${countdown ? `<span class="usage-reset">resets ${countdown}</span>` : ""}
    </span>`;
}

function renderUsageLimits(usageLimits) {
  const agents = Object.keys(usageLimits || {});
  if (agents.length === 0) return "";
  const rows = agents.map(agent => {
    const rl = usageLimits[agent];
    const label = AGENT_LABEL[agent] || agent;
    return `<div class="usage-row">
        <span class="usage-agent">${escapeHtml(label)}</span>
        ${renderUsageWindow("5h", rl.five_hour_pct, rl.five_hour_resets_at)}
        ${renderUsageWindow("week", rl.seven_day_pct, rl.seven_day_resets_at)}
      </div>`;
  }).join("");
  return rows;
}
```

Update the `snapshot://update` listener to render both:

```js
window.__TAURI__.event.listen("snapshot://update", (e) => {
  usageLimitsEl.innerHTML = renderUsageLimits(e.payload.usage_limits);
  content.innerHTML = renderSnapshot(e.payload);
});
```

- [ ] **Step 3: Add CSS for the usage bar**

Edit `dist/style.css`, add after the existing `.bar`/`.bar-fill`/`.rate-limit` rules (the `.rate-limit` class becomes unused after Step 2 and can stay or be deleted — delete it, since it was per-row and no per-row rate-limit markup remains):

Remove:
```css
.rate-limit { display: flex; gap: 10px; font-size: 10px; opacity: 0.55; margin-top: 2px; }
```

Add:
```css
#usage-limits {
  padding: 4px 10px;
  border-bottom: 1px solid rgba(255,255,255,0.08);
  display: flex;
  flex-direction: column;
  gap: 3px;
}
#usage-limits:empty { display: none; }
.usage-row { display: flex; align-items: center; gap: 10px; font-size: 10px; opacity: 0.85; }
.usage-agent { font-weight: 600; flex-shrink: 0; width: 42px; }
.usage-window { display: flex; align-items: center; gap: 4px; }
.usage-pct { opacity: 0.8; }
.usage-bar { width: 40px; margin: 0; display: inline-block; }
.usage-reset { opacity: 0.55; }
```

- [ ] **Step 4: Manual verification**

Run: `cd src-tauri && cargo tauri dev`
Expected: the overlay launches; with a real `~/.claude/.credentials.json` present (as confirmed earlier in this project's investigation), within ~180s the usage bar shows a "Claude" row with 5h/week percentages and reset countdowns, even with no active Claude session running. If Codex has ever produced a rollout with `rate_limits` today, a "Codex" row appears too. This can't be fully automated (it depends on live account state and real credentials) — confirm visually per the project's UI-testing convention.

- [ ] **Step 5: Commit**

```bash
git add dist/index.html dist/main.js dist/style.css
git commit -m "feat(ui): always-visible usage-limit bar, decoupled from active sessions"
```

---

### Task 6: Documentation

**Files:**
- Modify: `README.md`

**Interfaces:** none (docs only).

- [ ] **Step 1: Update the README's rate-limit section and config example**

Edit `README.md`. Replace the existing "5-hour / weekly usage limits" section:

```markdown
### 5-hour / weekly usage limits

A usage-limit bar at the top of the panel always shows each provider's
account-level 5h/weekly quota, independent of whether a session is currently
running.

- **Codex** reports this in its own transcript, so it appears automatically
  once you've used Codex at least once that day.
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
```

Update the config example block to include the new field:

```toml
poll_interval_ms = 1000
opacity = 0.85
hotkey = "Ctrl+Shift+Space"
enabled_agents = ["claude", "codex", "hermes"]
claude_usage_enabled = true
# hermes_data_dir = "C:/Users/you/AppData/Local/hermes"
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: document the usage-limit bar and claude_usage_enabled opt-out"
```
