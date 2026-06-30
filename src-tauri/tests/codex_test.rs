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
    // deltas summed: 900+3000 input, 40+120 output
    assert_eq!(s.total_input_tokens, 3900);
    assert_eq!(s.total_output_tokens, 160);
}
