# Usage-Limit Bar — Design Spec

**Date:** 2026-07-06
**Status:** Approved for planning

## Problem

Account-level 5h/weekly quota usage exists in the app today but is broken and
inconsistent:

- It's attached to `AgentSession.rate_limit`, so it only ever appears on an
  active session row — it disappears the moment no session is running.
- Claude's rate limit is read from `~/.claude/abtop-rate-limits.json`, a file
  that only exists if the user has separately configured a custom StatusLine
  hook to write it. Without that hook (the common case), Claude never shows
  usage data at all.
- Codex's rate limit is parsed live from `token_count` events inside an active
  rollout, so it also vanishes as soon as the session ends.

Investigation of `ccstatusline` (which *does* show real Claude usage without a
custom hook) found that it fetches usage from Anthropic's own API using the
OAuth token already present in `~/.claude/.credentials.json` — there is no
purely local file with real Claude quota percentages. This spec adopts the
same approach: reuse the existing on-disk OAuth credential to call Anthropic's
usage endpoint directly. This is a deliberate, scoped exception to the
project's "no network" principle — the only outbound call in the app, and it
only ever talks to `api.anthropic.com` with a credential the user already has.

## Goals

- Show each provider's account-level 5h/weekly quota usage in a bar that is
  **always visible**, independent of whether a session is currently active.
- Claude: real usage percentages without requiring a custom hook.
- Codex: usage persists across a session ending, using data this app already
  parses.
- No new credentials, no third-party network calls, and an explicit opt-out
  for anyone who wants to keep the app 100% network-free.

## Non-goals

- Historical/spend charts (tracked separately in `docs/PROPOSED_FEATURES.md`
  §3.1/3.3).
- Hermes account-level usage (no such concept exists for it today).
- Any provider-usage data beyond 5h/weekly windows (e.g. per-model breakdowns,
  extra-usage credits) — the API returns more fields than this spec surfaces;
  future work can extend `RateLimitInfo` if needed.

## Design

### 1. Model changes

Move rate-limit data from a per-session field to a snapshot-level map, since
it is account-level, not session-level:

```rust
// model.rs
pub struct Snapshot {
    ...
    pub usage_limits: BTreeMap<String, RateLimitInfo>, // key: "claude" | "codex"
}
```

`AgentSession.rate_limit` is removed. `RateLimitInfo` (already defined in
`model.rs`) is unchanged.

### 2. Claude: direct usage-API polling

New module `src-tauri/src/claude_usage.rs`:

- Reads `claudeAiOauth.accessToken` from `.credentials.json`, located via the
  existing WSL/Windows home discovery logic (`lib.rs`).
- On a background thread (not the 1s tick loop), polls
  `GET https://api.anthropic.com/api/oauth/usage` with headers:
  - `Authorization: Bearer <token>`
  - `anthropic-beta: oauth-2025-04-20`
- Poll interval: 180s, matching `ccstatusline`'s own cache window — far slower
  than the UI tick, so this is a dedicated thread, not inline in `App::tick`.
- Parses the response's `five_hour` / `seven_day` buckets
  (`{utilization, resets_at}`, `resets_at` as ISO8601 parsed via `chrono`,
  already a dependency) into `RateLimitInfo`.
- Publishes results to a shared `Arc<Mutex<Option<RateLimitInfo>>>` that the
  Claude collector reads each tick without blocking.
- **Error handling / backoff:** on request failure, back off 30s before
  retrying; on HTTP 429, back off using `Retry-After` (default 300s). The
  last-known-good value keeps being served during backoff — never blank the
  bar because of a transient failure.
- **Fallback:** if no OAuth token is found in `.credentials.json` (e.g.
  Bedrock/Vertex/API-key auth, which don't have this token), fall back to the
  existing `rate_limit::read_rate_limit_file` hook-file mechanism if present.
  If neither source has data, the Claude row is omitted (same graceful
  omission as today).
- **New dependency:** `ureq` (with a TLS feature) — a lightweight blocking
  HTTP client, matching the synchronous nature of the tick loop. No need to
  pull in `tokio`/`reqwest` for one polling thread.
- **Config:** new `claude_usage_enabled` flag in `config.toml`, default
  `true`. When `false`, the API poller never starts and Claude usage is
  sourced only from the hook file (strict local-only mode).

### 3. Codex: decouple from active sessions

No network changes — Codex already exposes 5h/weekly usage locally via
`token_count` events in its rollout files. Changes:

- The Codex collector's persistent per-session state already tracks
  `rate_limit: Option<RateLimitInfo>`; hoist the last-known value to
  collector-level state (not tied to a single session's lifetime) so it
  survives the session ending.
- On ticks with no active rollout, opportunistically scan the most recent
  rollout file (today's, falling back to the latest prior day) for its last
  `rate_limits` event, so usage data is available on first launch of the day
  before any new session starts.

### 4. Frontend

- A slim bar, always shown above/below the session list (not per-row),
  reusing the existing `.bar` / `.bar-fill` CSS classes.
- One row per enabled agent that has ever produced usage data for this run;
  an agent with no data yet is simply omitted (no "N/A" placeholder row).
- Example:
  ```
  Claude   5h ▓▓▓▓▓▓░░░░ 42%  resets 2h14m   week ▓▓░░░░░░░░ 18%  resets 3d
  Codex    5h ▓░░░░░░░░░ 12%  resets 4h50m   week —
  ```
- Reset countdown (`Xh Ym` / `Xd`) is computed live in the frontend from
  `resets_at`, not precomputed on the backend.

### 5. Security / trust notes

- The OAuth token is read from a file already on disk with OS-level
  permissions (`.credentials.json`); it is never logged, never written
  anywhere else, and is only ever sent over HTTPS to `api.anthropic.com`.
- This is the one exception to the project's "no network" principle, and is
  called out explicitly in the README alongside the `claude_usage_enabled`
  opt-out.

## Testing

- Unit tests for the usage-API response parser (`claude_usage.rs`), covering:
  missing fields, malformed `resets_at`, 429 backoff timing.
- Unit test for the fallback path (no token found → hook file used).
- Unit test for Codex's last-known-value retention across a session ending.
- Manual verification: confirm the bar appears after a fresh app launch with
  no active sessions, using existing local Claude/Codex credentials.
