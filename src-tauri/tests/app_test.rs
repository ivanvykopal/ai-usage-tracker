use std::fs;
use std::path::PathBuf;
use usage_tracker::app::App;
use usage_tracker::claude::ClaudeCollector;

fn build_fake_claude_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("utt-app-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let _ = fs::create_dir_all(root.join("sessions"));
    let _ = fs::create_dir_all(root.join("projects").join("-proj"));
    fs::write(
        root.join("sessions").join("4242.json"),
        r#"{ "pid": 4242, "cwd": "/proj", "session_id": "abc", "startedAt": 1700000000000 }"#,
    )
    .unwrap();
    fs::write(
        root.join("projects").join("-proj").join("abc.jsonl"),
        r#"{"type":"assistant","message":{"role":"assistant","model":"m","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":0}}}"#,
    )
    .unwrap();
    root
}

#[test]
fn tick_builds_snapshot_from_collectors() {
    let root = build_fake_claude_root();
    // pid 4242 is not actually running on the real system, so the Claude
    // collector drops it. This test pins the wiring + the empty-snapshot
    // contract: a well-formed Snapshot with no sessions.
    let mut app = App::new(vec![Box::new(ClaudeCollector::new(root))]);
    let snap = app.tick();
    assert!(snap.sessions.is_empty());
    assert_eq!(snap.total_tokens, 0);
}
