//! JARVIS 3.0 — голосовой ИИ-агент для Windows.
//! Rust-ядро (Tauri v2): OS-автоматизация через WinAPI, агентский цикл с
//! инструментами, SQLite-память с векторным поиском, edge-tts стриминг.

pub mod agent;
pub mod appscan;
pub mod appshell;
pub mod commands;
pub mod config;
pub mod db;
pub mod events;
pub mod llm;
pub mod os;
pub mod permissions;
pub mod safety;
pub mod screen;
pub mod search;
pub mod secrets;
pub mod stt;
pub mod tools;
pub mod tts;
pub mod util;
pub mod voice;

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
#[allow(unused_imports)]
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub is_media: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub body: String,
    pub kind: String,
    pub audio_path: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEntry {
    pub ts: String,
    pub tool: String,
    pub args: String,
    pub status: String,
    pub detail: String,
}

/// Глобальное состояние приложения.
pub struct AppState {
    pub db: db::Db,
    pub config: std::sync::RwLock<config::Config>,
    /// Флаг «реальный выход» (иначе крестик = свернуть в tray).
    pub quit: AtomicBool,
}

pub fn b64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cfg = config::load();
    let db = db::Db::open().expect("не удалось открыть SQLite");
    let state = AppState {
        db,
        config: std::sync::RwLock::new(cfg.clone()),
        quit: AtomicBool::new(false),
    };

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        builder = builder
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                // Второй запуск: показать окно.
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }))
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                None,
            ))
            .plugin(tauri_plugin_global_shortcut::Builder::new().build());
    }

    builder
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::permission_list,
            commands::set_permission,
            commands::ask,
            commands::stop_speaking,
            commands::confirm,
            commands::speak_test,
            commands::transcribe,
            commands::save_voice_note,
            commands::scan_apps,
            commands::find_apps,
            commands::action_log,
            commands::memory_facts,
            commands::memory_add,
            commands::memory_remove,
            commands::chats,
            commands::chat_messages,
            commands::new_chat,
            commands::timers,
            commands::notes,
            commands::save_note,
            commands::toggle_overlay,
            commands::hide_to_tray,
            commands::quit_app,
            commands::dev_ping,
        ])
        .setup(move |app| {
            // Хэндл для трансляции TTS-событий в webview (iOS-озвучка).
            events::set_app(app.handle().clone());
            // TTS-воркер: очередь команд + генерация для barge-in (desktop;
            // на iOS озвучку делает webview через speechSynthesis).
            #[cfg(any(windows, target_os = "macos"))]
            {
                let (tts_tx, tts_rx) = tokio::sync::mpsc::unbounded_channel::<events::TtsCommand>();
                let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
                *events::TTX.lock().unwrap() = Some(tts_tx);
                tts::spawn_worker(tts_rx, generation);
            }

            // Tray, хоткей, таймеры, автозапуск — desktop.
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            {
                appshell::build_tray(app.handle());
                let accel = app.state::<AppState>().config.read().unwrap().overlay_hotkey.clone();
                appshell::rebind_hotkey(app.handle(), &accel);
                if cfg.autostart {
                    use tauri_plugin_autostart::ManagerExt;
                    let _ = app.autolaunch().enable();
                }
            }
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            appshell::restore_timers(app.handle());

            // Приветствие при запуске (по времени суток).
            let hour = chrono::Local::now().format("%H").to_string().parse::<u32>().unwrap_or(12);
            let greeting = match hour {
                4..=11 => "Доброе утро.",
                12..=17 => "Добрый день.",
                18..=22 => "Добрый вечер.",
                _ => "Доброй ночи.",
            };
            #[cfg(any(windows, target_os = "macos"))]
            if cfg.tts_enabled && cfg.llm_configured() {
                // Лёгкое приветствие после старта, не мешает.
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let (v, r, p) = voice::resolve(&cfg, "neutral");
                    events::tts_send(events::TtsCommand::Speak {
                        text: greeting.to_string(),
                        voice: v,
                        rate: r,
                        pitch: p,
                    });
                });
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let _ = (&greeting, &cfg);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Крестик главного окна = свернуть в tray (реальный выход — из tray).
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    let app = window.app_handle();
                    let quitting = app.state::<AppState>().quit.load(Ordering::SeqCst);
                    if !quitting {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("ошибка запуска JARVIS");
}
