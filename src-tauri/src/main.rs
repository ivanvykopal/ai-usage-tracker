#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod claude;
mod codex;
mod collector;
mod config;
mod model;
mod process;
mod transcript;

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

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

#[tauri::command]
fn set_opacity(window: tauri::Window, opacity: f32) {
    let _ = window.set_opacity(opacity.clamp(0.1, 1.0));
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
        if let Some(dir) = &cfg.hermes_data_dir {
            v.push(Box::new(hermes::HermesCollector::new(dir.clone())));
        }
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
        .setup(move |app_handle| {
            // Initial opacity
            if let Some(w) = app_handle.get_webview_window("overlay") {
                let _ = w.set_opacity(cfg.opacity);
            }

            // Global hotkey: Ctrl+Shift+Space toggles visibility
            let shortcut: Shortcut = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
            let app_handle_clone = app_handle.clone();
            let _ = app_handle.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    if let Some(w) = app_handle_clone.get_webview_window("overlay") {
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
            std::thread::spawn(move || {
                loop {
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
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            toggle_visibility, set_opacity, set_poll_interval, quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
