use crate::collector::{Collector, ProcessContext};
use crate::model::{AgentSession, SessionStatus};
use crate::process::has_active_descendant;
use crate::rate_limit::{self, CLAUDE_RATE_FILE};
use crate::transcript::IncrementalReader;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// On-disk `~/.claude/sessions/{pid}.json` header.
#[derive(Debug, Deserialize)]
struct SessionFile {
    #[serde(default)]
    pid: u32,
    #[serde(default)]
    cwd: String,
    #[serde(default, rename = "sessionId")]
    session_id: String,
    #[serde(default, rename = "startedAt")]
    started_at: i64,
}

/// Encode a cwd into the directory name Claude Code uses under
/// `~/.claude/projects/`: slashes, backslashes, and colons become `-`.
pub fn encode_cwd_path(cwd: &str) -> String {
    cwd.chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            _ => c,
        })
        .collect()
}

pub struct ClaudeCollector {
    config_dirs: Vec<PathBuf>,
    readers: HashMap<String, IncrementalReader>,
    state: HashMap<String, ParseState>,
}

/// Resolve the project directory that holds a session's transcripts.
/// Handles worktree sessions where the directory doesn't match encode_cwd_path(cwd).
fn resolve_project_dir(config_dir: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    let projects_dir = config_dir.join("projects");
    let encoded = encode_cwd_path(cwd);
    let primary = projects_dir.join(&encoded);
    let jsonl_name = format!("{}.jsonl", session_id);

    // First, check the primary (encoded cwd) location
    let primary_path = primary.join(&jsonl_name);
    if primary_path.exists() {
        return Some(primary);
    }

    // Fallback: search for the session ID in other subdirectories (worktree sessions)
    if let Ok(entries) = fs::read_dir(&projects_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let candidate = path.join(&jsonl_name);
            if candidate.exists() {
                return Some(path);
            }
        }
    }

    // Final fallback: return primary dir if it exists (will create transcript there)
    if primary.is_dir() {
        return Some(primary);
    }

    None
}

#[derive(Default, Clone)]
struct ParseState {
    model: String,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_create: u64,
    last_user_ts_ms: i64,
    pending_tool: bool,
    current_task: String,
    last_context_tokens: u64,
}

impl ClaudeCollector {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_dirs: vec![config_dir],
            readers: HashMap::new(),
            state: HashMap::new(),
        }
    }

    /// Create a collector that checks multiple configuration directories.
    /// This is useful for detecting sessions in both WSL and Windows environments.
    pub fn new_multi(config_dirs: Vec<PathBuf>) -> Self {
        Self {
            config_dirs,
            readers: HashMap::new(),
            state: HashMap::new(),
        }
    }
}

impl Collector for ClaudeCollector {
    fn name(&self) -> &str {
        "claude"
    }

    fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession> {
        let mut out = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Collect sessions from all configured directories (e.g., WSL + Windows)
        for config_dir in &self.config_dirs {
            let sessions_dir = config_dir.join("sessions");
            // Account-level, so read once and share across every Claude session this tick.
            let rate_limit =
                rate_limit::read_rate_limit_file(&config_dir.join(CLAUDE_RATE_FILE), "claude");

            let entries = match fs::read_dir(&sessions_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(sf) = serde_json::from_str::<SessionFile>(&text) else {
                    continue;
                };
                if !seen_ids.insert(sf.session_id.clone()) {
                    continue;
                }

                // Only keep sessions whose pid is alive and looks like claude.
                let alive = ctx
                    .procs
                    .get(&sf.pid)
                    .map(|p| p.command.contains("claude"))
                    .unwrap_or(false);
                if !alive {
                    continue;
                }

                // Resolve project dir, handling worktree sessions and post-/clear renames
                let project_dir = match resolve_project_dir(config_dir, &sf.cwd, &sf.session_id) {
                    Some(dir) => dir,
                    None => continue,
                };
                let transcript = project_dir.join(format!("{}.jsonl", sf.session_id));
                if transcript.exists() {
                    let reader = self.readers.entry(sf.session_id.clone()).or_default();
                    let prev_offset = reader.offset;
                    let lines = reader.read_new_lines(&transcript);
                    let rewound = reader.offset < prev_offset && prev_offset > 0;
                    let st = self.state.entry(sf.session_id.clone()).or_default();
                    if rewound {
                        // The transcript was truncated/rotated since last tick;
                        // drop accumulated counters so we don't double-count.
                        *st = ParseState::default();
                    }
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
                    context_percent: 0.0, // needs per-model window sizes; deferred
                    total_input_tokens: st.total_input,
                    total_output_tokens: st.total_output,
                    total_cache_read: st.total_cache_read,
                    total_cache_create: st.total_cache_create,
                    turn_count: 0,
                    current_task: st.current_task.clone(),
                    mem_mb,
                    rate_limit: rate_limit.clone(),
                });
            }
        }

        // Evict accumulated state for sessions no longer present (pid died /
        // file gone) so counters don't persist forever.
        self.state.retain(|sid, _| seen_ids.contains(sid));
        self.readers.retain(|sid, _| seen_ids.contains(sid));

        out.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        out
    }
}

fn derive_status(st: &ParseState, pid: u32, ctx: &ProcessContext) -> SessionStatus {
    let active_child = has_active_descendant(pid, ctx.procs, ctx.children);
    if active_child || st.pending_tool {
        SessionStatus::Executing
    } else if st.last_user_ts_ms > 0 {
        SessionStatus::Thinking
    } else {
        SessionStatus::Waiting
    }
}

/// Mutates accumulated parse state from one transcript JSON line.
fn apply_claude_line(line: &str, st: &mut ParseState) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "user" => {
            // A tool-result wrapper or local-command echo is a "user" line
            // Claude Code writes for bookkeeping, not a real prompt the
            // model owes a reply to. Treating it as one would pin the
            // session in "Thinking" forever.
            if !is_synthetic_user_msg(&v) {
                st.last_user_ts_ms = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(parse_iso_to_ms)
                    .unwrap_or(st.last_user_ts_ms);
            }
            st.pending_tool = false;
        }
        "assistant" => {
            if let Some(msg) = v.get("message") {
                // Extract usage data - try both "message.usage" and top-level "usage"
                let usage = msg.get("usage").or_else(|| v.get("usage"));

                if let Some(u) = usage {
                    let inp = u.get("input_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                    let out = u.get("output_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
                    let cr = u
                        .get("cache_read_input_tokens")
                        .and_then(|n| n.as_u64())
                        .unwrap_or(0);
                    let cc = u
                        .get("cache_creation_input_tokens")
                        .and_then(|n| n.as_u64())
                        .unwrap_or(0);

                    st.total_input += inp;
                    st.total_output += out;
                    st.total_cache_read += cr;
                    st.total_cache_create += cc;
                    st.last_context_tokens = st.total_input + st.total_cache_read;
                }

                if let Some(model) = msg.get("model").and_then(|m| m.as_str()) {
                    if !model.is_empty() {
                        st.model = model.to_string();
                    }
                }
                // pending_tool if this assistant turn contained a tool_use
                let has_tool_use = msg
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                    })
                    .unwrap_or(false);
                st.pending_tool = has_tool_use;
                if has_tool_use {
                    st.current_task = msg
                        .get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| {
                            arr.iter().find_map(|b| {
                                if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                    b.get("name").and_then(|n| n.as_str()).map(String::from)
                                } else {
                                    None
                                }
                            })
                        })
                        .unwrap_or_default();
                }
            }
            // The assistant replied, so the model is no longer generating.
            st.last_user_ts_ms = 0;
        }
        _ => {}
    }
}

/// True iff a `user`-role transcript entry is *synthetic* — i.e. not a real
/// human prompt that the model still owes a reply for. Three forms,
/// ported from abtop: `isMeta: true` markers, a content array that's
/// entirely `tool_result` blocks (Claude Code's wrapper for feeding tool
/// output back to the model), or a string opening with a known
/// local-command tag (`/plugin`, `!bash`, etc., which never invoke the
/// model).
fn is_synthetic_user_msg(entry: &Value) -> bool {
    if entry
        .get("isMeta")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    let Some(message) = entry.get("message") else {
        return false;
    };
    match message.get("content") {
        Some(Value::Array(arr)) => {
            !arr.is_empty()
                && arr
                    .iter()
                    .all(|block| block.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
        }
        Some(Value::String(s)) => {
            let t = s.trim_start();
            t.starts_with("<local-command-stdout>")
                || t.starts_with("<local-command-stderr>")
                || t.starts_with("<local-command-caveat>")
                || t.starts_with("<command-name>")
                || t.starts_with("<bash-input>")
                || t.starts_with("<bash-stdout>")
                || t.starts_with("<bash-stderr>")
        }
        _ => false,
    }
}

fn parse_iso_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}
