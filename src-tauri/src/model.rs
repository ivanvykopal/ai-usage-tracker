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
    Snapshot { sessions, total_tokens, by_agent_tokens, by_status }
}
