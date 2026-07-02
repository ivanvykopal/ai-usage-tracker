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

fn build_collectors(cfg: &config::Config) -> Vec<Box<dyn collector::Collector>> {
    let home = dirs::home_dir().unwrap_or_default();
    let mut v: Vec<Box<dyn collector::Collector>> = Vec::new();
    if cfg.enabled_agents.iter().any(|a| a == "claude") {
        v.push(Box::new(claude::ClaudeCollector::new(home.join(".claude"))));
    }
    if cfg.enabled_agents.iter().any(|a| a == "codex") {
        v.push(Box::new(codex::CodexCollector::new(home.join(".codex").join("sessions"))));
    }
    if cfg.enabled_agents.iter().any(|a| a == "hermes") {
        // HERMES_HOME defaults to ~/.hermes; only override via config.
        let dir = cfg.hermes_data_dir.clone().unwrap_or_else(|| home.join(".hermes"));
        v.push(Box::new(hermes::HermesCollector::new(dir)));
    }
    v
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config_path = dirs::config_dir()
        .unwrap_or_default()
        .join("ai-usage-overlay")
        .join("config.toml");
    let cfg = config::load_config(&config_path);
    let collectors = build_collectors(&cfg);
    let app_state = AppState {
        app: Mutex::new(app::App::new(collectors)),
        config: Mutex::new(cfg.clone()),
        config_path: config_path.clone(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(app_state)
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // Create tray icon; left-click toggles the panel.
            let _ = TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))?)
                .tooltip("AI Assistant Usage")
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
            let shortcut: Shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            let hotkey_handle = app_handle.clone();
            let _ = app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
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
                    state.config.lock().map(|c| c.poll_interval_ms).unwrap_or(1000)
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
