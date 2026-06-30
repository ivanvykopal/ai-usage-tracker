use usage_tracker::process::{has_active_descendant, snapshot};

#[test]
fn snapshot_includes_self_and_has_no_panics() {
    let snap = snapshot();
    let me = std::process::id();
    assert!(
        snap.procs.contains_key(&me),
        "self process must appear in the snapshot"
    );
    // The children map is well-formed (iterating it must not panic).
    for (_pid, kids) in &snap.children {
        for _k in kids {
            // touch each entry
        }
    }
}

#[test]
fn has_active_descendant_returns_bool_without_panic() {
    let snap = snapshot();
    let _ = has_active_descendant(std::process::id(), &snap);
}
