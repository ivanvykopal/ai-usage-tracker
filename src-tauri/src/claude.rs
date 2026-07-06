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

/// A `.claude` config dir plus which WSL distro it was found under, if any
/// (`None` for the native host). Lets `collect()` check pid liveness against
/// the right process list — the Windows host can't see WSL's PIDs.
pub struct ConfigDirEntry {
    pub dir: PathBuf,
    pub wsl_distro: Option<String>,
}

pub struct ClaudeCollector {
    config_dirs: Vec<ConfigDirEntry>,
    readers: HashMap<String, IncrementalReader>,
    state: HashMap<String, ParseState>,
    usage_source: ClaudeUsageSource,
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

impl Collector for ClaudeCollector {
    fn name(&self) -> &str {
        "claude"
    }

    fn collect(&mut self, ctx: &ProcessContext) -> Vec<AgentSession> {
        let mut out = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Collect sessions from all configured directories (e.g., WSL + Windows)
        for entry in &self.config_dirs {
            let config_dir = &entry.dir;
            let (procs, children) = ctx.procs_for(entry.wsl_distro.as_deref());
            let sessions_dir = config_dir.join("sessions");

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
                let alive = procs
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
                let proc = procs.get(&sf.pid);
                let mem_mb = proc.map(|p| p.rss_kb / 1024).unwrap_or(0);
                let status = derive_status(&st, sf.pid, procs, children);
                let project_name = sf.cwd.rsplit(['/', '\\']).next().unwrap_or("?").to_string();
                let configured_model = read_configured_model(&sf.cwd);
                let context_percent = crate::collector::context_percent_for(
                    &st.model,
                    &configured_model,
                    st.last_context_tokens,
                );

                out.push(AgentSession {
                    agent_cli: "claude".into(),
                    pid: sf.pid,
                    session_id: sf.session_id.clone(),
                    cwd: sf.cwd.clone(),
                    project_name,
                    started_at: sf.started_at,
                    status,
                    model: st.model.clone(),
                    context_percent,
                    total_input_tokens: st.total_input,
                    total_output_tokens: st.total_output,
                    total_cache_read: st.total_cache_read,
                    total_cache_create: st.total_cache_create,
                    turn_count: 0,
                    current_task: st.current_task.clone(),
                    mem_mb,
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
}

fn derive_status(
    st: &ParseState,
    pid: u32,
    procs: &HashMap<u32, crate::process::ProcInfo>,
    children: &HashMap<u32, Vec<u32>>,
) -> SessionStatus {
    let active_child = has_active_descendant(pid, procs, children);
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
                    // Context usage reflects what's in the window for THIS
                    // turn, not the running session total.
                    st.last_context_tokens = inp + cr + cc;
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

/// Returns the ordered list of Claude Code settings files to check, from
/// highest to lowest priority, matching Claude Code's own resolution order:
/// 1. `{cwd}/.claude/settings.local.json`
/// 2. `{cwd}/.claude/settings.json`
/// 3. `~/.claude/settings.local.json`
/// 4. `~/.claude/settings.json`
fn settings_candidate_paths(cwd: &str) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let cwd_path = PathBuf::from(cwd);
    candidates.push(cwd_path.join(".claude").join("settings.local.json"));
    candidates.push(cwd_path.join(".claude").join("settings.json"));
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".claude").join("settings.local.json"));
        candidates.push(home.join(".claude").join("settings.json"));
    }
    candidates
}

/// Read the configured model from Claude Code's settings files.
///
/// Precedence (highest wins), matching Claude Code's own resolution order:
/// 1. `CLAUDE_CODE_MODEL` env var
/// 2. `{cwd}/.claude/settings.local.json`
/// 3. `{cwd}/.claude/settings.json`
/// 4. `~/.claude/settings.local.json`
/// 5. `~/.claude/settings.json`
///
/// Returns an empty string when no model is configured. The value may include
/// the `[1m]` suffix (e.g. `"sonnet[1m]"`) which is used to detect 1M context.
fn read_configured_model(cwd: &str) -> String {
    if let Ok(v) = std::env::var("CLAUDE_CODE_MODEL") {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    for path in settings_candidate_paths(cwd) {
        if let Some(model) = read_model_from_settings(&path) {
            return model;
        }
    }
    String::new()
}

fn read_model_from_settings(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let val: Value = serde_json::from_str(&content).ok()?;
    let model = val.get("model")?.as_str()?.trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

fn parse_iso_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}
