use crate::process;
use std::path::PathBuf;

/// A resolved home directory plus which WSL distro it came from, if any.
/// `wsl_distro` is `None` for the native host's own home directory; it's
/// `Some(name)` for a home directory reached through `\\wsl$\<name>\...` on
/// native Windows. Collectors use this to know when a session's pid must be
/// checked against that distro's own process list instead of the host's.
pub struct HomeDir {
    pub path: PathBuf,
    pub wsl_distro: Option<String>,
}

/// Resolve all possible home directories for agent data.
/// In WSL, this includes both the WSL home and the Windows home (via /mnt/c).
/// On Windows, this includes both the Windows home and WSL homes (via wsl.exe).
/// This allows detecting Claude/Codex/Hermes sessions running in either environment.
pub fn resolve_home_dirs() -> Vec<HomeDir> {
    let mut dirs = Vec::new();

    // Always add the primary home directory (platform-specific)
    if let Some(home) = dirs::home_dir() {
        dirs.push(HomeDir {
            path: home,
            wsl_distro: None,
        });
    }

    // Check if we're in WSL
    let is_wsl = std::fs::read_to_string("/proc/version")
        .map(|v| v.to_lowercase().contains("microsoft"))
        .unwrap_or(false);

    if is_wsl {
        // We're in WSL - also check the Windows home directory
        // WSL typically mounts Windows drives under /mnt/[drive-letter]
        if let Some(username) = std::env::var("USER")
            .ok()
            .or_else(|| std::env::var("LOGNAME").ok())
        {
            let windows_home = PathBuf::from("/mnt/c").join("Users").join(&username);
            if windows_home.exists() && !dirs.iter().any(|h: &HomeDir| h.path == windows_home) {
                dirs.push(HomeDir {
                    path: windows_home,
                    wsl_distro: None,
                });
            }
        }
    } else {
        // We're on native Windows - check for WSL home directories
        // Try to discover WSL distributions and their users
        if let Ok(output) = process::silent_command("wsl")
            .args(["-l", "-q"])
            .output()
        {
            let wsl_distributions = process::decode_wsl_output(&output.stdout);
            for dist in wsl_distributions.lines() {
                // wsl -l -q can emit a UTF-16 BOM as the first character of
                // the first line even after decoding; strip it defensively.
                let dist = dist.trim().trim_start_matches('\u{feff}');
                if dist.is_empty() {
                    continue;
                }

                // Get the default user for this distribution
                if let Ok(user_output) = process::silent_command("wsl")
                    .args(["-d", dist, "sh", "-c", "echo $HOME"])
                    .output()
                {
                    let home_path = process::decode_wsl_output(&user_output.stdout)
                        .trim()
                        .to_string();
                    // Convert WSL path like /home/username to Windows path
                    if let Some(username) = home_path.strip_prefix("/home/") {
                        let windows_wsl_home = PathBuf::from("\\\\wsl$")
                            .join(dist)
                            .join("home")
                            .join(username);
                        if windows_wsl_home.exists()
                            && !dirs.iter().any(|h: &HomeDir| h.path == windows_wsl_home)
                        {
                            dirs.push(HomeDir {
                                path: windows_wsl_home,
                                wsl_distro: Some(dist.to_string()),
                            });
                        }
                    }
                }
            }
        }
    }

    dirs
}

/// Distinct WSL distros discovered across the resolved home directories —
/// the set `App::tick` needs to poll each tick for WSL-sourced sessions'
/// process liveness.
pub fn wsl_distros(home_dirs: &[HomeDir]) -> Vec<String> {
    let mut distros: Vec<String> = home_dirs
        .iter()
        .filter_map(|h| h.wsl_distro.clone())
        .collect();
    distros.sort();
    distros.dedup();
    distros
}
