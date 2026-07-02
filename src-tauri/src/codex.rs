use crate::collector::{Collector, ProcessContext};
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
}

#[derive(Default, Clone)]
struct CodexState {
    session_id: String,
    cwd: String,
    model: String,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
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
        }
    }

    /// Create a collector that checks multiple sessions directories.
    pub fn new_multi(sessions_dirs: Vec<PathBuf>) -> Self {
        Self {
            sessions_dirs,
            readers: HashMap::new(),
            state: HashMap::new(),
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
                    started_at: 0,
                    status,
                    model: st.model.clone(),
                    context_percent: crate::collector::context_percent_for(
                        &st.model,
                        "",
                        st.total_input + st.total_cache_read,
                    ),
                    total_input_tokens: st.total_input,
                    total_output_tokens: st.total_output,
                    total_cache_read: st.total_cache_read,
                    total_cache_create: 0, // Codex doesn't report cache-creation tokens
                    turn_count: 0,
                    current_task: st.current_task.clone(),
                    mem_mb: 0,
                    rate_limit: st.rate_limit.clone(),
                });
            }
        }

        // Evict state for rollouts no longer recent/present.
        self.state.retain(|p, _| seen.contains(p));
        self.readers.retain(|p, _| seen.contains(p));
        out
    }
}

fn apply_codex_line(line: &str, st: &mut CodexState) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "session_meta" => {
            st.session_id = v
                .get("session_id")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            st.cwd = v
                .get("cwd")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            st.model = v
                .get("model")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
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
                            if mins <= 300 {
                                info.five_hour_pct = pct;
                                info.five_hour_resets_at = resets;
                            } else {
                                info.seven_day_pct = pct;
                                info.seven_day_resets_at = resets;
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
                        st.current_task = name.clone();
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
