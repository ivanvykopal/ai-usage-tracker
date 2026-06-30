use std::fs;
use std::path::PathBuf;
use usage_tracker::transcript::IncrementalReader;

fn tmp(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("utt-tr-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

#[test]
fn reads_all_lines_on_first_call() {
    let p = tmp("a.jsonl", "{\"i\":1}\n{\"i\":2}\n");
    let mut r = IncrementalReader::new();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":1}".to_string(), "{\"i\":2}".to_string()]);
}

#[test]
fn reads_only_appended_lines_on_subsequent_call() {
    let p = tmp("b.jsonl", "{\"i\":1}\n");
    let mut r = IncrementalReader::new();
    let _ = r.read_new_lines(&p);
    // append a line
    let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    use std::io::Write;
    write!(f, "{{\"i\":2}}\n").unwrap();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":2}".to_string()]);
}

#[test]
fn reparses_from_zero_when_file_truncated() {
    let p = tmp("c.jsonl", "{\"i\":1}\n{\"i\":2}\n");
    let mut r = IncrementalReader::new();
    let first = r.read_new_lines(&p);
    assert_eq!(first.len(), 2);
    // truncate + rewrite with FEWER bytes (size shrinks). This is the realistic
    // re-parse trigger for transcript rotation/truncation. (A same-size rewrite
    // within the same millisecond is not detectable from mtime+size and does not
    // occur for append-only transcripts, so it is intentionally not covered.)
    fs::write(&p, "{\"i\":9}\n").unwrap();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":9}".to_string()]);
}

#[test]
fn skips_malformed_line_keeps_rest() {
    // malformed middle line still yields the good ones around it
    let p = tmp("d.jsonl", "{\"i\":1}\nnot json\n{\"i\":2}\n");
    let mut r = IncrementalReader::new();
    let lines = r.read_new_lines(&p);
    // read_new_lines returns raw lines; parsing is the caller's job.
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1], "not json");
}

#[test]
fn missing_file_returns_empty() {
    let mut r = IncrementalReader::new();
    assert!(r
        .read_new_lines(std::path::Path::new("/no/such/file.jsonl"))
        .is_empty());
}

#[test]
fn trailing_partial_line_is_not_returned() {
    // no trailing newline → last line is incomplete, must not be returned yet
    let p = tmp("e.jsonl", "{\"i\":1}\n{\"i\":2}");
    let mut r = IncrementalReader::new();
    let lines = r.read_new_lines(&p);
    assert_eq!(lines, vec!["{\"i\":1}".to_string()]);
}
