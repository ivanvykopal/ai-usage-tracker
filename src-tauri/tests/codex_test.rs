use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use chrono::Local;
use usage_tracker::codex::CodexCollector;
use usage_tracker::collector::{Collector, ProcessContext};

fn build_fake_codex_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-codex-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    fs::write(
        day_dir.join("rollout-codex-1.jsonl"),
        include_str!("fixtures/codex_rollout.jsonl"),
    )
    .unwrap();
    root
}

#[test]
fn collects_recent_codex_rollout_with_tokens_and_project() {
    let root = build_fake_codex_root();
    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext {
        procs: &procs,
        children: &kids,
        ports: &ports,
    };

    let mut c = CodexCollector::new(root.join("sessions"));
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1, "exactly one recent rollout expected");
    let s = &sessions[0];
    assert_eq!(s.agent_cli, "codex");
    assert_eq!(s.session_id, "codex-1");
    assert_eq!(s.project_name, "webapp");
    assert_eq!(s.model, "gpt-5-codex");
    // Codex reports cumulative totals per token_count event (not deltas), so
    // the last event wins: input 3000 - cached 500 = 2500, output 120.
    assert_eq!(s.total_input_tokens, 2500);
    assert_eq!(s.total_output_tokens, 120);
    assert_eq!(s.total_cache_read, 500);
    // task_complete fired last, and the one function_call/-_output pair closed.
    assert_eq!(s.status, usage_tracker::model::SessionStatus::Done);
}

#[test]
fn pending_function_call_marks_session_executing() {
    let root = std::env::temp_dir().join(format!("utt-codex-pending-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let now = Local::now();
    let day_dir = root
        .join("sessions")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string());
    fs::create_dir_all(&day_dir).unwrap();
    fs::write(
        day_dir.join("rollout-codex-2.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"session_id\":\"codex-2\",\"cwd\":\"/webapp\",\"model\":\"gpt-5-codex\"}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"build it\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"call_id\":\"call-1\",\"name\":\"shell\",\"arguments\":\"{}\"}}\n",
        ),
    )
    .unwrap();

    let procs = HashMap::new();
    let kids = HashMap::new();
    let ports = HashMap::new();
    let ctx = ProcessContext { procs: &procs, children: &kids, ports: &ports };
    let mut c = CodexCollector::new(root.join("sessions"));
    let sessions = c.collect(&ctx);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].status, usage_tracker::model::SessionStatus::Executing);
    assert_eq!(sessions[0].current_task, "shell");
}
