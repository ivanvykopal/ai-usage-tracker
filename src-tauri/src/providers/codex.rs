use crate::collector::{Collector, ProcessContext};
use crate::config::Config;
use crate::home::HomeDir;
use crate::model::{AgentSession, RateLimitInfo, SessionStatus};
use crate::transcript::IncrementalReader;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A rollout is considered live if its mtime is within this window.
const RECENT_AGE_SECS: u64 = 300; // 5 min

pub struct CodexCollector {
    sessions_dirs: Vec<PathBuf>,
    readers: HashMap<PathBuf, IncrementalReader>,
    state: HashMap<PathBuf, CodexState>,
    /// Last-known account-level usage, retained even after the rollout that
    /// produced it is no longer "recent" (session ended) or is evicted from
    /// `state`. Cleared only by a fresh, differing value from a later tick.
    last_usage_limits: Option<RateLimitInfo>,
    /// mtime of each stale rollout the last time it was opportunistically
    /// scanned for usage limits, so an unchanged file isn't re-read and
    /// re-parsed in full on every tick.
    scanned_stale: HashMap<PathBuf, SystemTime>,
}

#[derive(Default, Clone)]
struct CodexState {
    session_id: String,
    cwd: String,
    model: String,
    started_at: i64,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    /// Input tokens actually in context for the most recent turn (from
    /// `last_token_usage`) — this is what Codex's own statusline shows
    /// context usage against, not the session's cumulative totals.
    last_context_tokens: u64,
    /// Model's context window size in tokens, as reported by Codex itself
    /// (`model_context_window`) rather than guessed from the model name.
    context_window: u64,
    /// True once a "user_message" event arrives and stays true until the
    /// model visibly responds (agent_message/function_call/task_complete) —
    /// mirrors abtop's `model_generating` flag, drives the Thinking status.
    model_generating: bool,
    /// call_id -> tool name, for every function_call without a matching
    /// function_call_output yet. Non-empty means the session is Executing.
    pending_calls: HashMap<String, String>,
    current_task: String,
    task_complete: bool,
    rate_limit: Option<RateLimitInfo>,
}

impl CodexCollector {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dirs: vec![sessions_dir],
            readers: HashMap::new(),
            state: HashMap::new(),
            last_usage_limits: None,
            scanned_stale: HashMap::new(),
        }
    }

    /// Create a collector that checks multiple sessions directories.
    pub fn new_multi(sessions_dirs: Vec<PathBuf>) -> Self {
        Self {
            sessions_dirs,
            readers: HashMap::new(),
            state: HashMap::new(),
            last_usage_limits: None,
            scanned_stale: HashMap::new(),
        }
    }

    /// Today's session directory: `~/.codex/sessions/YYYY/MM/DD`.
    fn today_dirs(&self) -> Vec<PathBuf> {
        let now = chrono::Local::now();
        let mut dirs = Vec::new();
        for sessions_dir in &self.sessions_dirs {
            let d = sessions_dir
                .join(now.format("%Y").to_string())
                .join(now.format("%m").to_string())
                .join(now.format("%d").to_string());
            if d.exists() {
                dirs.push(d);
            }
        }
        dirs
    }
}

impl Collector for CodexCollector {
    fn name(&self) -> &str {
        "codex"
    }

    fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
        let mut out = Vec::new();
        let today_dirs = self.today_dirs();
        if today_dirs.is_empty() {
            return out;
        }

        let mut seen: HashSet<PathBuf> = HashSet::new();
        for sessions_dir in &today_dirs {
            let entries = match fs::read_dir(sessions_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                    continue;
                }
                if !is_recent(&path, RECENT_AGE_SECS) {
                    // Not live, but still worth a one-off scan for its last
                    // known account-level usage — covers "app just launched,
                    // no session started yet today." Skip the full re-read if
                    // the file's mtime hasn't changed since the last scan —
                    // a stale rollout isn't being appended to, so a repeat
                    // parse would just waste IO every tick.
                    let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
                    let already_scanned = mtime.is_some() && self.scanned_stale.get(&path) == mtime.as_ref();
                    if !already_scanned {
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
                        if let Some(m) = mtime {
                            self.scanned_stale.insert(path.clone(), m);
                        }
                    }
                    continue;
                }
                seen.insert(path.clone());

                let reader = self.readers.entry(path.clone()).or_default();
                let prev_offset = reader.offset;
                let lines = reader.read_new_lines(&path);
                let rewound = reader.offset < prev_offset && prev_offset > 0;
                let st = self.state.entry(path.clone()).or_default();
                if rewound {
                    *st = CodexState::default();
                }
                for line in lines {
                    apply_codex_line(&line, st);
                }
                let st = self.state.get(&path).cloned().unwrap_or_default();

                let project_name = st.cwd.rsplit(['/', '\\']).next().unwrap_or("?").to_string();
                let status = if st.task_complete {
                    SessionStatus::Done
                } else if !st.pending_calls.is_empty() {
                    SessionStatus::Executing
                } else if st.model_generating {
                    SessionStatus::Thinking
                } else {
                    SessionStatus::Waiting
                };

                out.push(AgentSession {
                    agent_cli: "codex".into(),
                    pid: 0, // v1 doesn't map rollout → pid
                    session_id: st.session_id.clone(),
                    cwd: st.cwd.clone(),
                    project_name,
                    started_at: st.started_at,
                    status,
                    model: st.model.clone(),
                    // Codex reports its own context-window size and per-turn
                    // usage in the transcript, so use those directly instead
                    // of guessing a window from the model name and dividing
                    // by cumulative session totals (which overstates usage
                    // more with every turn and never matches Codex's own
                    // statusline).
                    context_percent: if st.context_window > 0 {
                        ((st.last_context_tokens as f64 / st.context_window as f64) * 100.0)
                            .clamp(0.0, 100.0)
                    } else {
                        crate::collector::context_percent_for(&st.model, "", st.last_context_tokens)
                    },
                    total_input_tokens: st.total_input,
                    total_output_tokens: st.total_output,
                    total_cache_read: st.total_cache_read,
                    total_cache_create: 0, // Codex doesn't report cache-creation tokens
                    turn_count: 0,
                    current_task: st.current_task.clone(),
                    mem_mb: 0,
                    cost_usd: None,
                });
            }
        }

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

    fn usage_limits(&self) -> Option<RateLimitInfo> {
        self.last_usage_limits.clone()
    }
}

fn apply_codex_line(line: &str, st: &mut CodexState) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "session_meta" => {
            // Codex nests session fields under `payload`, matching event_msg/
            // response_item/turn_context — fall back to top-level for safety.
            let payload = v.get("payload").unwrap_or(&v);
            st.session_id = payload
                .get("id")
                .or_else(|| payload.get("session_id"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            st.cwd = payload
                .get("cwd")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            // Real Codex rollouts don't carry `model` on session_meta (it's
            // reported via turn_context below); some fixtures/older formats
            // do, so take it here as a fallback that turn_context overrides.
            if let Some(m) = payload.get("model").and_then(|s| s.as_str()) {
                st.model = m.to_string();
            }
            if let Some(ts) = payload
                .get("timestamp")
                .and_then(|s| s.as_str())
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            {
                st.started_at = ts.timestamp_millis();
            }
        }
        "turn_context" => {
            // The model actually in use is only reported here — session_meta
            // doesn't carry it. Codex lets users switch models/effort mid
            // session, so always take the latest value.
            if let Some(payload) = v.get("payload") {
                if let Some(m) = payload.get("model").and_then(|s| s.as_str()) {
                    st.model = m.to_string();
                }
                if let Some(cw) = payload.get("model_context_window").and_then(|v| v.as_u64()) {
                    st.context_window = cw;
                }
            }
        }
        "event_msg" => {
            let Some(payload) = v.get("payload") else {
                return;
            };
            let pty = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match pty {
                "user_message" => {
                    st.model_generating = true;
                }
                "agent_message" => {
                    st.model_generating = false;
                }
                "token_count" => {
                    // Codex reports cumulative totals, not deltas, so this is
                    // an overwrite — the latest event is authoritative.
                    let usage = &payload["info"]["total_token_usage"];
                    let input = usage["input_tokens"].as_u64().unwrap_or(0);
                    let output = usage["output_tokens"].as_u64().unwrap_or(0);
                    let cache = usage["cached_input_tokens"]
                        .as_u64()
                        .or_else(|| usage["cache_read_input_tokens"].as_u64())
                        .unwrap_or(0);
                    st.total_input = input.saturating_sub(cache);
                    st.total_output = output;
                    st.total_cache_read = cache;

                    // The current-turn context size Codex's own statusline
                    // uses — not the cumulative session totals above.
                    let last = &payload["info"]["last_token_usage"];
                    if let Some(inp) = last["input_tokens"].as_u64() {
                        st.last_context_tokens = inp;
                    }
                    if let Some(cw) = payload["info"]["model_context_window"].as_u64() {
                        st.context_window = cw;
                    }

                    let rl = &payload["rate_limits"];
                    if rl.is_object() && is_account_level_codex_rate_limit(rl) {
                        let updated_at = v
                            .get("timestamp")
                            .and_then(|t| t.as_str())
                            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                            .map(|dt| dt.timestamp() as u64);
                        let mut info = RateLimitInfo {
                            source: "codex".to_string(),
                            updated_at,
                            ..Default::default()
                        };
                        for slot in ["primary", "secondary"] {
                            let w = &rl[slot];
                            if !w.is_object() {
                                continue;
                            }
                            let mins = w["window_minutes"].as_u64().unwrap_or(0);
                            let pct = w["used_percent"].as_f64();
                            let resets = w["resets_at"].as_u64();
                            // Codex doesn't always report a 5h+weekly pair —
                            // the free plan reports only a single ~30-day
                            // ("monthly") window as `primary` with no
                            // `secondary` at all. Bucket by actual duration
                            // rather than assuming primary=5h/secondary=week.
                            if mins <= 6 * 60 {
                                info.five_hour_pct = pct;
                                info.five_hour_resets_at = resets;
                            } else if mins <= 8 * 24 * 60 {
                                info.seven_day_pct = pct;
                                info.seven_day_resets_at = resets;
                            } else {
                                info.monthly_pct = pct;
                                info.monthly_resets_at = resets;
                            }
                        }
                        st.rate_limit = Some(info);
                    }
                }
                "task_complete" => {
                    st.task_complete = true;
                    st.model_generating = false;
                }
                "task_started" => {
                    st.task_complete = false;
                    if let Some(cw) = payload["model_context_window"].as_u64() {
                        st.context_window = cw;
                    }
                }
                _ => {}
            }
        }
        "response_item" => {
            let Some(payload) = v.get("payload") else {
                return;
            };
            match payload.get("type").and_then(|t| t.as_str()) {
                Some("function_call") => {
                    st.model_generating = false;
                    if let Some(call_id) = payload.get("call_id").and_then(|c| c.as_str()) {
                        let name = payload
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let arg = payload
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .map(parse_codex_tool_arg)
                            .unwrap_or_default();
                        st.current_task = if arg.is_empty() {
                            name.clone()
                        } else {
                            format!("{} {}", name, arg)
                        };
                        st.pending_calls.insert(call_id.to_string(), name);
                    }
                }
                Some("function_call_output") => {
                    if let Some(call_id) = payload.get("call_id").and_then(|c| c.as_str()) {
                        st.pending_calls.remove(call_id);
                    }
                    if st.pending_calls.is_empty() {
                        st.current_task.clear();
                    }
                }
                _ => {
                    st.model_generating = false;
                }
            }
        }
        _ => {}
    }
}

/// Pull a short, human-readable argument out of a function_call's JSON
/// arguments string — the file path for read/write-style tools, or the
/// command for shell-style tools — so `current_task` reads like
/// "shell git status" instead of just "shell".
fn parse_codex_tool_arg(arguments: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        return String::new();
    };

    for key in ["file_path", "path"] {
        if let Some(raw) = value.get(key).and_then(|v| v.as_str()) {
            let short = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
            return truncate_arg(short);
        }
    }

    for key in ["cmd", "command"] {
        if let Some(v) = value.get(key) {
            if let Some(s) = v.as_str() {
                return truncate_arg(s);
            }
            if let Some(items) = v.as_array() {
                let parts: Vec<&str> = items.iter().filter_map(|i| i.as_str()).collect();
                if parts.len() >= 3 && parts[0] == "bash" && parts[1] == "-lc" {
                    return truncate_arg(parts[2]);
                }
                if !parts.is_empty() {
                    return truncate_arg(&parts.join(" "));
                }
            }
        }
    }

    String::new()
}

fn truncate_arg(arg: &str) -> String {
    arg.chars().take(120).collect()
}

/// Codex emits per-project as well as account-level rate-limit snapshots;
/// only the account-level one (no `limit_id`, or `limit_id: "codex"`) reflects
/// the user's actual 5h/weekly usage.
fn is_account_level_codex_rate_limit(rate_limits: &Value) -> bool {
    matches!(rate_limits["limit_id"].as_str(), Some("codex") | None)
}

fn is_recent(path: &Path, max_age_secs: u64) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(m) = meta.modified() else {
        return false;
    };
    let age = SystemTime::now()
        .duration_since(m)
        .unwrap_or(Duration::ZERO);
    age.as_secs() <= max_age_secs
}

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
