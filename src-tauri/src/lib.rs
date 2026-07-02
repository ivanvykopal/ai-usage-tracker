pub mod app;
pub mod claude;
pub mod codex;
pub mod collector;
pub mod config;
pub mod hermes;
pub mod model;
pub mod process;
pub mod rate_limit;
pub mod transcript;

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

struct AppState {
    app: Mutex<app::App>,
    config: Mutex<config::Config>,
    config_path: PathBuf,
}

#[tauri::command]
fn toggle_visibility(window: tauri::Window) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.show();
    }
}

// Tauri has no cross-platform native window-opacity API, so opacity is
// applied as CSS on the (already-transparent) webview content instead;
// this command just persists the value and tells the frontend to apply it.
#[tauri::command]
fn set_opacity(window: tauri::Window, state: tauri::State<AppState>, opacity: f32) {
    let clamped = opacity.clamp(0.1, 1.0);
    if let Ok(mut cfg) = state.config.lock() {
        cfg.opacity = clamped;
        let _ = config::save_config(&state.config_path, &cfg);
    }
    let _ = window.emit("opacity://update", clamped);
}

#[tauri::command]
fn set_poll_interval(state: tauri::State<AppState>, ms: u64) {
    if let Ok(mut cfg) = state.config.lock() {
        cfg.poll_interval_ms = ms;
        let _ = config::save_config(&state.config_path, &cfg);
    }
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// A resolved home directory plus which WSL distro it came from, if any.
/// `wsl_distro` is `None` for the native host's own home directory; it's
/// `Some(name)` for a home directory reached through `\\wsl$\<name>\...` on
/// native Windows. Collectors use this to know when a session's pid must be
/// checked against that distro's own process list instead of the host's.
struct HomeDir {
    path: PathBuf,
    wsl_distro: Option<String>,
}

/// Resolve all possible home directories for agent data.
/// In WSL, this includes both the WSL home and the Windows home (via /mnt/c).
/// On Windows, this includes both the Windows home and WSL homes (via wsl.exe).
/// This allows detecting Claude/Codex/Hermes sessions running in either environment.
fn resolve_home_dirs() -> Vec<HomeDir> {
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

fn build_collectors(cfg: &config::Config, home_dirs: &[HomeDir]) -> Vec<Box<dyn collector::Collector>> {
    let mut v: Vec<Box<dyn collector::Collector>> = Vec::new();

    if cfg.enabled_agents.iter().any(|a| a == "claude") {
        // Create a collector that checks all possible .claude directories
        let claude_dirs: Vec<claude::ConfigDirEntry> = home_dirs
            .iter()
            .map(|h| claude::ConfigDirEntry {
                dir: h.path.join(".claude"),
                wsl_distro: h.wsl_distro.clone(),
            })
            .filter(|e| e.dir.exists())
            .collect();
        v.push(Box::new(claude::ClaudeCollector::new_multi(claude_dirs)));
    }

    if cfg.enabled_agents.iter().any(|a| a == "codex") {
        let codex_dirs: Vec<PathBuf> = home_dirs
            .iter()
            .map(|h| h.path.join(".codex").join("sessions"))
            .filter(|p| p.exists())
            .collect();
        v.push(Box::new(codex::CodexCollector::new_multi(codex_dirs)));
    }

    if cfg.enabled_agents.iter().any(|a| a == "hermes") {
        // HERMES_HOME defaults to ~/.hermes; only override via config.
        let hermes_dirs: Vec<PathBuf> = if let Some(ref custom_dir) = cfg.hermes_data_dir {
            vec![custom_dir.clone()]
        } else {
            home_dirs
                .iter()
                .map(|h| h.path.join(".hermes"))
                .filter(|p| p.exists())
                .collect()
        };
        v.push(Box::new(hermes::HermesCollector::new_multi(hermes_dirs)));
    }

    v
}

/// Distinct WSL distros discovered across the resolved home directories —
/// the set `App::tick` needs to poll each tick for WSL-sourced sessions'
/// process liveness.
fn wsl_distros(home_dirs: &[HomeDir]) -> Vec<String> {
    let mut distros: Vec<String> = home_dirs
        .iter()
        .filter_map(|h| h.wsl_distro.clone())
        .collect();
    distros.sort();
    distros.dedup();
    distros
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config_path = dirs::config_dir()
        .unwrap_or_default()
        .join("ai-usage-overlay")
        .join("config.toml");
    let cfg = config::load_config(&config_path);
    let home_dirs = resolve_home_dirs();
    let collectors = build_collectors(&cfg, &home_dirs);
    let distros = wsl_distros(&home_dirs);
    let app_state = AppState {
        app: Mutex::new(app::App::new_with_wsl_distros(collectors, distros)),
        config: Mutex::new(cfg.clone()),
        config_path: config_path.clone(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(app_state)
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // Create tray icon; left-click toggles the panel, right-click shows a menu.
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&quit_item])?;
            let _ = TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/128x128.png"
                ))?)
                .tooltip("AI Usage Tracker")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(w) = tray.app_handle().get_webview_window("overlay") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                            }
                        }
                    }
                })
                .build(app)?;

            // Initial opacity: applied as CSS by the frontend, not a native window API.
            if let Some(w) = app_handle.get_webview_window("overlay") {
                let _ = w.emit("opacity://update", cfg.opacity);
            }

            // Global hotkey: Ctrl+Shift+Space toggles visibility
            let shortcut: Shortcut =
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            let hotkey_handle = app_handle.clone();
            let _ = app
                .global_shortcut()
                .on_shortcut(shortcut, move |_app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        if let Some(w) = hotkey_handle.get_webview_window("overlay") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                            }
                        }
                    }
                });

            // Tick thread
            let app_handle = app_handle.clone();
            std::thread::spawn(move || loop {
                let interval = {
                    let state: tauri::State<AppState> = app_handle.state();
                    state
                        .config
                        .lock()
                        .map(|c| c.poll_interval_ms)
                        .unwrap_or(1000)
                };
                let snapshot = {
                    let state: tauri::State<AppState> = app_handle.state();
                    let mut a = state.app.lock().unwrap();
                    a.tick()
                };
                let _ = app_handle.emit("snapshot://update", &snapshot);
                std::thread::sleep(std::time::Duration::from_millis(interval.max(200)));
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            toggle_visibility,
            set_opacity,
            set_poll_interval,
            quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
