use crate::collector::{Collector, ProcessContext};
use crate::model::{AgentSession, SessionStatus};
use crate::transcript::IncrementalReader;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// Hermes persists session/token data to `<HERMES_HOME>/state.db` (SQLite)
/// and live activity to `<HERMES_HOME>/logs/agent.log`, per
/// docs/EXTERNAL_MONITORING_API.md. We poll the database for the active
/// session's tokens (1-5s latency, per the doc) and tail the log for status
/// (thinking/tool-execution/idle), scoped to that session's `[session_id]`
/// tag so unrelated log lines from other tools don't bleed in.
pub struct HermesCollector {
    data_dirs: Vec<PathBuf>,
    log_reader: IncrementalReader,
    status: SessionStatus,
    current_task: String,
    last_session_id: String,
}

struct ActiveSession {
    id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    message_count: u32,
    cwd: String,
    started_at: f64,
}

impl HermesCollector {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dirs: vec![data_dir],
            log_reader: IncrementalReader::new(),
            status: SessionStatus::Waiting,
            current_task: String::new(),
            last_session_id: String::new(),
        }
    }

    /// Create a collector that checks multiple data directories.
    pub fn new_multi(data_dirs: Vec<PathBuf>) -> Self {
        Self {
            data_dirs,
            log_reader: IncrementalReader::new(),
            status: SessionStatus::Waiting,
            current_task: String::new(),
            last_session_id: String::new(),
        }
    }
}

impl Collector for HermesCollector {
    fn name(&self) -> &str {
        "hermes"
    }

    fn collect(&mut self, _ctx: &ProcessContext) -> Vec<AgentSession> {
        let mut all_sessions = Vec::new();

        for data_dir in &self.data_dirs {
            let db_path = data_dir.join("state.db");
            let Some(active) = query_active_session(&db_path) else {
                continue;
            };

            if active.id != self.last_session_id {
                // A different (or first) session became active; drop any
                // status/task carried over from whatever was active before.
                self.last_session_id = active.id.clone();
                self.status = SessionStatus::Waiting;
                self.current_task.clear();
            }

            let log_path = data_dir.join("logs").join("agent.log");
            for line in self.log_reader.read_new_lines(&log_path) {
                apply_log_line(&line, &active.id, &mut self.status, &mut self.current_task);
            }

            let project_name = active
                .cwd
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("?")
                .to_string();

            all_sessions.push(AgentSession {
                agent_cli: "hermes".into(),
                pid: 0, // Hermes's SQLite schema doesn't expose a process id.
                session_id: active.id,
                cwd: active.cwd,
                project_name,
                started_at: (active.started_at * 1000.0) as i64,
                status: self.status,
                context_percent: crate::collector::context_percent_for(
                    &active.model,
                    "",
                    active.input_tokens + active.cache_read_tokens,
                ),
                model: active.model,
                total_input_tokens: active.input_tokens,
                total_output_tokens: active.output_tokens,
                total_cache_read: active.cache_read_tokens,
                total_cache_create: active.cache_write_tokens,
                turn_count: active.message_count,
                current_task: self.current_task.clone(),
                mem_mb: 0,
            });
        }

        all_sessions
    }
}

/// Mirrors the doc's "Active Session Query": the most recently started
/// session with `ended_at IS NULL`. Read-only connection so we never create
/// or lock the agent's own database; any failure (missing file, locked,
/// schema mismatch) degrades to "no active session" rather than erroring.
///
/// `state.db` is WAL-mode and actively written by Hermes, so we must NOT
/// use the `immutable=1` URI hint here: it makes SQLite skip the WAL file
/// entirely, which means missing not just recent rows but the schema
/// itself whenever Hermes hasn't checkpointed yet (observed in practice:
/// a freshly-(re)created `state.db` with an empty main file and all data,
/// including the `sessions` table, sitting in the as-yet-unchecked `-wal`).
///
/// When Hermes runs inside WSL and this app runs natively on Windows, the
/// db is reached over `\\wsl$\<distro>\...`. WAL's shared-memory (`-shm`)
/// coordination relies on `mmap`, which doesn't work correctly over network
/// filesystems (this is a documented SQLite limitation, not a timing
/// race) — every read collides with Hermes's own writer and fails with
/// `SQLITE_BUSY` ("database is locked"), even with a busy timeout, since
/// there's nothing to wait out. Work around it the standard way: copy the
/// db plus its `-wal`/`-shm` sidecars to a local temp path each poll and
/// query that copy instead, so SQLite's locking never has to cross the
/// network boundary.
fn query_active_session(db_path: &Path) -> Option<ActiveSession> {
    if !db_path.exists() {
        return None;
    }
    let local_db_path = copy_db_locally(db_path)?;
    let conn =
        Connection::open_with_flags(&local_db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.query_row(
        "SELECT id, model, input_tokens, output_tokens, cache_read_tokens, \
                cache_write_tokens, message_count, cwd, started_at \
         FROM sessions WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1",
        [],
        |row| {
            Ok(ActiveSession {
                id: row.get(0)?,
                model: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                input_tokens: row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u64,
                output_tokens: row.get::<_, Option<i64>>(3)?.unwrap_or(0) as u64,
                cache_read_tokens: row.get::<_, Option<i64>>(4)?.unwrap_or(0) as u64,
                cache_write_tokens: row.get::<_, Option<i64>>(5)?.unwrap_or(0) as u64,
                message_count: row.get::<_, Option<i64>>(6)?.unwrap_or(0) as u32,
                cwd: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                started_at: row.get(8)?,
            })
        },
    )
    .ok()
}

/// Copies `state.db` and its `-wal`/`-shm` sidecars (if present) into a
/// local temp directory keyed by a hash of the source path, so distinct
/// `data_dirs` (e.g. a Windows home and a WSL home) don't collide. Returns
/// the path to the local copy of the main db file.
fn copy_db_locally(db_path: &Path) -> Option<PathBuf> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    db_path.hash(&mut hasher);
    let tmp_dir = std::env::temp_dir()
        .join("ai-usage-overlay-hermes")
        .join(format!("{:x}", hasher.finish()));
    std::fs::create_dir_all(&tmp_dir).ok()?;

    let local_db = tmp_dir.join("state.db");
    std::fs::copy(db_path, &local_db).ok()?;

    for ext in ["-wal", "-shm"] {
        let src = PathBuf::from(format!("{}{ext}", db_path.display()));
        let dst = PathBuf::from(format!("{}{ext}", local_db.display()));
        if src.exists() {
            let _ = std::fs::copy(&src, &dst);
        } else {
            // Avoid a stale sidecar from a previous poll confusing SQLite
            // about the copy's WAL state.
            let _ = std::fs::remove_file(&dst);
        }
    }

    Some(local_db)
}

/// Applies one `agent.log` line to the running status/task, scoped to
/// `session_id` via its `[session_id]` tag. Pattern priority (thinking,
/// then tool-execution, then complete) matches the doc's own
/// `STATUS_PATTERNS` dict order, where the first matching pattern wins.
fn apply_log_line(
    line: &str,
    session_id: &str,
    status: &mut SessionStatus,
    current_task: &mut String,
) {
    let tag = format!("[{session_id}]");
    let Some((_, after_tag)) = line.split_once(tag.as_str()) else {
        return;
    };
    let message = match after_tag.split_once(": ") {
        Some((_, msg)) => msg,
        None => after_tag,
    };
    let lower = message.to_ascii_lowercase();

    if contains_any(&lower, &["thinking", "reasoning", "processing"]) {
        *status = SessionStatus::Thinking;
    } else if contains_any(
        &lower,
        &[
            "tool call",
            "tool starting",
            "tool complete",
            "executing tool",
            "running tool",
        ],
    ) {
        // Matched as adjacent phrases, not "tool" and "call" appearing
        // independently anywhere in the line — a line like "Turn ended:
        // ... tool_turns=0 ... api_calls=1/150" contains both words purely
        // from unrelated field names and previously false-matched this as
        // Executing, getting the status stuck since "ended" never matched
        // the completion branch below either.
        *status = SessionStatus::Executing;
        if let Some(name) = extract_tool_name(message) {
            *current_task = name;
        }
    } else if contains_any(&lower, &["complete", "finished", "done", "turn ended"]) {
        // The current turn wrapped up; the session itself stays active
        // (it would have dropped out of the SQL query otherwise) so this
        // reads as idle/waiting-for-input, not a terminal state.
        *status = SessionStatus::Waiting;
        current_task.clear();
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn extract_tool_name(message: &str) -> Option<String> {
    for prefix in ["Tool call complete: ", "Tool starting: ", "Tool complete: "] {
        if let Some(rest) = message.strip_prefix(prefix) {
            return Some(rest.split_whitespace().next().unwrap_or(rest).to_string());
        }
    }
    None
}
