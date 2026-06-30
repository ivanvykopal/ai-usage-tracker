use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct FileIdentity {
    pub mtime_ms: i64,
    pub size: u64,
}

pub struct IncrementalReader {
    pub offset: u64,
    pub identity: Option<FileIdentity>,
}

impl IncrementalReader {
    pub fn new() -> Self {
        Self { offset: 0, identity: None }
    }

    /// Returns complete JSONL lines appended since the last call.
    ///
    /// Continues from `offset` on a normal append (the file grew at the end).
    /// Re-reads from offset 0 only when the file shrank or was replaced
    /// (truncation/rotation) — i.e. when bytes before `offset` may have
    /// changed. A trailing line without a newline is held back until it
    /// completes (the writer is mid-line).
    pub fn read_new_lines(&mut self, path: &Path) -> Vec<String> {
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let size = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let identity = FileIdentity { mtime_ms, size };

        // Decide where to read from based on how the file changed since last call:
        //   - No prior identity           → first read, start at 0.
        //   - Identity unchanged          → file didn't change, nothing new; bail.
        //   - File strictly grew (append) → continue from offset (new bytes at end).
        //   - File shrank (truncation)    → earlier bytes may differ, re-read from 0.
        // A same-size rewrite within the same millisecond is not distinguishable
        // from "unchanged" via mtime+size, but that does not occur for the
        // append-only transcripts we read.
        let start = match &self.identity {
            None => 0u64,
            Some(prev) if prev == &identity => {
                // Unchanged file: no new data this tick.
                return Vec::new();
            }
            Some(prev) if size > prev.size => self.offset, // normal append
            Some(_) => 0u64, // shrink/truncation → re-read all
        };

        let mut file = match fs::File::open(path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut buf = String::new();
        if file.read_to_string(&mut buf).is_err() {
            return Vec::new();
        }

        // Only return lines terminated by '\n'. A trailing fragment without
        // '\n' is incomplete (the writer is mid-line); hold it back.
        let ends_with_newline = buf.ends_with('\n');
        let mut lines: Vec<String> = buf.split('\n').map(|s| s.to_string()).collect();
        if ends_with_newline {
            // split on a trailing '\n' yields a final empty element; drop it
            if lines.last().map(|s| s.is_empty()).unwrap_or(false) {
                lines.pop();
            }
            self.offset = start + buf.len() as u64;
        } else if !lines.is_empty() {
            // drop the incomplete trailing fragment, rewind offset to before it
            let fragment_len = lines.last().map(|s| s.len() as u64).unwrap_or(0);
            self.offset = start + buf.len() as u64 - fragment_len;
            lines.pop();
        }
        self.identity = Some(identity);
        lines
    }
}

impl Default for IncrementalReader {
    fn default() -> Self {
        Self::new()
    }
}
