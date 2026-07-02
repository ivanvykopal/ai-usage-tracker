use std::collections::HashMap;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub command: String,
    pub rss_kb: u64,
    pub cpu: f32,
    pub parent_pid: Option<u32>,
}

pub struct ProcessSnapshot {
    pub procs: HashMap<u32, ProcInfo>,
    pub children: HashMap<u32, Vec<u32>>,
    pub ports_by_pid: HashMap<u32, Vec<u16>>,
}

/// Decode `wsl.exe` stdout. When wsl.exe's output isn't attached to a real
/// console (always true for a piped `Command` capture), it switches to
/// UTF-16LE instead of the console codepage/UTF-8 — decoding that as UTF-8
/// (or lossy UTF-8) turns every character into garbage interleaved with NUL
/// bytes, which silently breaks every downstream string match (distro names,
/// paths, ps output). Detect that case by checking whether the bytes at odd
/// offsets are NUL, as they are for ASCII text stored as UTF-16LE.
pub fn decode_wsl_output(bytes: &[u8]) -> String {
    let looks_utf16le = bytes.len() >= 4
        && bytes.len() % 2 == 0
        && bytes.iter().skip(1).step_by(2).take(16).all(|&b| b == 0);
    if looks_utf16le {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if let Ok(s) = String::from_utf16(&units) {
            return s.trim_start_matches('\u{feff}').to_string();
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Builds a snapshot of running processes (pid → cmd/rss/cpu/parent), the
/// parent→children map, and listening TCP ports by pid.
pub fn snapshot() -> ProcessSnapshot {
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut procs: HashMap<u32, ProcInfo> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, p) in sys.processes() {
        let pid_u = pid.as_u32();
        // sysinfo 0.32 returns resident set size in BYTES; convert to KB.
        let rss_kb = p.memory() / 1024;
        procs.insert(
            pid_u,
            ProcInfo {
                pid: pid_u,
                command: p.name().to_string_lossy().into_owned(),
                rss_kb,
                cpu: p.cpu_usage(),
                parent_pid: p.parent().map(|pp| pp.as_u32()),
            },
        );
        if let Some(ppid) = p.parent().map(|pp| pp.as_u32()) {
            children.entry(ppid).or_default().push(pid_u);
        }
    }
    let ports_by_pid = listening_ports();
    ProcessSnapshot {
        procs,
        children,
        ports_by_pid,
    }
}

/// True if any descendant of `pid` has CPU > 5%. Takes the process and
/// children maps directly (both held by `ProcessContext` and
/// `ProcessSnapshot`) so it can be called from either.
pub fn has_active_descendant(
    pid: u32,
    procs: &HashMap<u32, ProcInfo>,
    children: &HashMap<u32, Vec<u32>>,
) -> bool {
    let mut stack: Vec<u32> = children.get(&pid).cloned().unwrap_or_default();
    while let Some(c) = stack.pop() {
        if let Some(info) = procs.get(&c) {
            if info.cpu > 5.0 {
                return true;
            }
        }
        if let Some(grandkids) = children.get(&c) {
            stack.extend(grandkids);
        }
    }
    false
}

/// Builds a process snapshot for a WSL distribution by shelling out to
/// `wsl.exe -d <distro> ps`, since the Windows host's own process list
/// (via `sysinfo`) can't see PIDs inside the WSL VM. Used on native Windows
/// only, and only for distros that a WSL-sourced config dir was found in.
pub fn wsl_snapshot(distro: &str) -> ProcessSnapshot {
    let mut procs: HashMap<u32, ProcInfo> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();

    if let Ok(out) = std::process::Command::new("wsl.exe")
        .args(["-d", distro, "-e", "ps", "-eo", "pid,ppid,pcpu,rss,comm", "--no-headers"])
        .output()
    {
        if out.status.success() {
            let text = decode_wsl_output(&out.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 5 {
                    continue;
                }
                let Ok(pid) = parts[0].parse::<u32>() else {
                    continue;
                };
                let ppid = parts[1].parse::<u32>().ok();
                let cpu: f32 = parts[2].parse().unwrap_or(0.0);
                let rss_kb: u64 = parts[3].parse().unwrap_or(0);
                let command = parts[4..].join(" ");
                procs.insert(
                    pid,
                    ProcInfo {
                        pid,
                        command,
                        rss_kb,
                        cpu,
                        parent_pid: ppid,
                    },
                );
                if let Some(ppid) = ppid {
                    children.entry(ppid).or_default().push(pid);
                }
            }
        }
    }

    ProcessSnapshot {
        procs,
        children,
        ports_by_pid: HashMap::new(),
    }
}

/// Maps pid → listening TCP ports. Tries `netstat -ano` (Windows) first, then
/// `ss -tlnpH` (Linux). Returns an empty map on any failure or if neither tool
/// is available — the panel simply shows no ports in that case.
fn listening_ports() -> HashMap<u32, Vec<u16>> {
    if let Some(map) = parse_netstat() {
        return map;
    }
    if let Some(map) = parse_ss() {
        return map;
    }
    HashMap::new()
}

/// `netstat -ano -p TCP` → pid → listening ports. Output lines look like:
///   TCP    0.0.0.0:8080   0.0.0.0:0   LISTENING   1234
fn parse_netstat() -> Option<HashMap<u32, Vec<u16>>> {
    let out = std::process::Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map: HashMap<u32, Vec<u16>> = HashMap::new();
    for line in text.lines() {
        if !line.contains("LISTENING") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 5 {
            continue;
        }
        let local = parts[1];
        let port = match local.rsplit_once(':').map(|(_, p)| p.parse::<u16>()) {
            Some(Ok(p)) => p,
            _ => continue,
        };
        let pid: u32 = match parts[parts.len() - 1].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        map.entry(pid).or_default().push(port);
    }
    Some(map)
}

/// `ss -tlnH` → pid → listening ports. Output lines look like:
///   LISTEN 0 4096 0.0.0.0:8080 0.0.0.0:* users:(("app",pid=1234,fd=3))
/// The pid is embedded in the `users:(("name",pid=N,...))` field. Falls back
/// to pid 0 (unknown) when the pid can't be parsed — but we skip those, since a
/// port with no owner is useless for attributing to a session.
fn parse_ss() -> Option<HashMap<u32, Vec<u16>>> {
    let out = std::process::Command::new("ss")
        .args(["-tlnH"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map: HashMap<u32, Vec<u16>> = HashMap::new();
    for line in text.lines() {
        // Only listening TCP sockets.
        if !line.starts_with("LISTEN") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let local = parts[3];
        let port = match local.rsplit_once(':').map(|(_, p)| p.parse::<u16>()) {
            Some(Ok(p)) => p,
            _ => continue,
        };
        // Find pid=N anywhere on the line.
        let pid = parts
            .iter()
            .find_map(|p| p.split("pid=").nth(1).and_then(|s| s.split(',').next()))
            .and_then(|s| s.parse::<u32>().ok());
        let Some(pid) = pid else { continue };
        map.entry(pid).or_default().push(port);
    }
    Some(map)
}
