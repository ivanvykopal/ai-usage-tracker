pub mod app;
pub mod burn_rate;
pub mod claude_usage;
pub mod collector;
pub mod config;
pub mod history;
pub mod home;
pub mod model;
pub mod pricing;
pub mod process;
pub mod providers;
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
    history_conn: Option<Mutex<rusqlite::Connection>>,
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

#[tauri::command]
fn get_usage_history(state: tauri::State<AppState>, agent: String, hours: u32) -> Vec<(i64, u64)> {
    let Some(hconn) = &state.history_conn else {
        return Vec::new();
    };
    let Ok(guard) = hconn.lock() else {
        return Vec::new();
    };
    let since_ms = chrono::Utc::now().timestamp_millis() - (hours as i64) * 3_600_000;
    history::token_history(&guard, &agent, since_ms)
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.ts_ms, p.tokens))
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config_path = dirs::config_dir()
        .unwrap_or_default()
        .join("ai-usage-overlay")
        .join("config.toml");
    let cfg = config::load_config(&config_path);

    let history_path = dirs::config_dir()
        .unwrap_or_default()
        .join("ai-usage-overlay")
        .join("history.db");
    let history_conn: Option<Mutex<rusqlite::Connection>> = if cfg.history_enabled {
        history::open(&history_path).ok().map(Mutex::new)
    } else {
        None
    };
    if let Some(conn) = &history_conn {
        if let Ok(guard) = conn.lock() {
            let cutoff_ms = chrono::Utc::now().timestamp_millis()
                - (cfg.history_retention_days as i64) * 86_400_000;
            let _ = history::prune_older_than(&guard, cutoff_ms);
        }
    }

    let home_dirs = home::resolve_home_dirs();
    let collectors = providers::build_collectors(&cfg, &home_dirs);
    let distros = home::wsl_distros(&home_dirs);
    let app_state = AppState {
        app: Mutex::new(app::App::new_with_wsl_distros(collectors, distros)),
        config: Mutex::new(cfg.clone()),
        config_path: config_path.clone(),
        history_conn,
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
                {
                    let state: tauri::State<AppState> = app_handle.state();
                    if let Some(hconn) = &state.history_conn {
                        if let Ok(guard) = hconn.lock() {
                            let ts_ms = chrono::Utc::now().timestamp_millis();
                            // Sample at most once per 60s regardless of poll_interval_ms —
                            // a 1s poll interval would otherwise write 60x more rows than needed.
                            static LAST_SAMPLE_MS: std::sync::atomic::AtomicI64 =
                                std::sync::atomic::AtomicI64::new(0);
                            let last = LAST_SAMPLE_MS.load(std::sync::atomic::Ordering::Relaxed);
                            if ts_ms - last >= 60_000 {
                                let _ = history::record_snapshot(&guard, ts_ms, &snapshot);
                                LAST_SAMPLE_MS.store(ts_ms, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(interval.max(200)));
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            toggle_visibility,
            set_opacity,
            set_poll_interval,
            quit,
            get_usage_history
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
