use crate::collector::{Collector, ProcessContext};
use crate::model::{AgentSession, SessionStatus};
use crate::transcript::IncrementalReader;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A rollout is considered live if its mtime is within this window.
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
        Self {
            sessions_dir,
            readers: HashMap::new(),
            state: HashMap::new(),
        }
    }

    /// Today's session directory: `~/.codex/sessions/YYYY/MM/DD`.
    fn today_dir(&self) -> Option<PathBuf> {
        let now = chrono::Local::now();
        let d = self
            .sessions_dir
            .join(now.format("%Y").to_string())
            .join(now.format("%m").to_string())
            .join(now.format("%d").to_string());
        if d.exists() {
            Some(d)
        } else {
            None
        }
    }
}

impl Collector for CodexCollector {
    fn name(&self) -> &str {
        "codex"
    }

    fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
        let mut out = Vec::new();
        let Some(today) = self.today_dir() else {
            return out;
        };
        let entries = match fs::read_dir(&today) {
            Ok(e) => e,
            Err(_) => return out,
        };
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
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

            let project_name = st
                .cwd
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("?")
                .to_string();
            let status = if st.task_complete {
                SessionStatus::Done
            } else if st.pending_tool {
                SessionStatus::Executing
            } else if st.last_user {
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
                context_percent: 0.0,
                total_input_tokens: st.total_input,
                total_output_tokens: st.total_output,
                total_cache_read: 0,
                turn_count: 0,
                current_task: st.current_task.clone(),
                mem_mb: 0,
            });
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
            st.cwd = v.get("cwd").and_then(|s| s.as_str()).unwrap_or("").to_string();
            st.model = v
                .get("model")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
        }
        "event_msg" => {
            let pty = v
                .get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            match pty {
                "user_message" => {
                    st.last_user = true;
                    st.pending_tool = false;
                }
                "token_count" => {
                    let p = v.get("payload").unwrap();
                    st.total_input += p.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                    st.total_output += p.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                    st.last_user = false;
                }
                "task_complete" => {
                    st.task_complete = true;
                    st.last_user = false;
                }
                "task_started" => {
                    st.task_complete = false;
                }
                _ => {}
            }
        }
        "response_item" => {
            st.last_user = false;
            let has_tool = v
                .get("payload")
                .and_then(|p| p.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("function_call"))
                })
                .unwrap_or(false);
            st.pending_tool = has_tool;
            if has_tool {
                st.current_task = "function_call".to_string();
            }
        }
        _ => {}
    }
}

fn is_recent(path: &Path, max_age_secs: u64) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(m) = meta.modified() else {
        return false;
    };
    let age = SystemTime::now().duration_since(m).unwrap_or(Duration::ZERO);
    age.as_secs() <= max_age_secs
}
