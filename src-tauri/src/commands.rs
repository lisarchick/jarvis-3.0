//! IPC-команды Tauri: конфигурация (секреты маскируются при отдаче в UI),
//! распознавание речи, диктофон, скан приложений, журнал, память, таймеры.

use base64::Engine;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::{Config, ALL_PERMISSIONS};
use crate::events::{self, TtsCommand};
use crate::permissions::resolve_confirmation;
use crate::AppState;

// ------------------------------------------------------------------ маскировка секретов

fn mask(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else if s.chars().count() > 8 {
        format!("••••{}", s.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>())
    } else {
        "••••".into()
    }
}

fn mask_config(cfg: &mut Config) {
    cfg.llm_key = mask(&cfg.llm_key);
    for (_k, v) in cfg.keys.iter_mut() {
        *v = mask(v);
    }
    for m in cfg.connected_models.iter_mut() {
        m.api_key = mask(&m.api_key);
    }
}

/// Возвращает старое значение, если новое — маска (не менял пользователь).
fn keep_secret(new: &str, old: &str) -> String {
    if new.starts_with("••") { old.to_string() } else { new.to_string() }
}

// ------------------------------------------------------------------ конфигурация

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    let mut cfg = state.config.read().unwrap().clone();
    mask_config(&mut cfg);
    Ok(cfg)
}

#[tauri::command]
pub fn save_config(app: AppHandle, state: State<'_, AppState>, mut cfg: Config) -> Result<(), String> {
    let old = state.config.read().unwrap().clone();
    // Пользователь не менял ключи (маски) — сохраняем прежние значения.
    cfg.llm_key = keep_secret(&cfg.llm_key, &old.llm_key);
    for (k, v) in cfg.keys.iter_mut() {
        let old_v = old.keys.get(k).cloned().unwrap_or_default();
        *v = keep_secret(v, &old_v);
    }
    for m in cfg.connected_models.iter_mut() {
        if let Some(old_m) = old.connected_models.iter().find(|o| o.id == m.id) {
            m.api_key = keep_secret(&m.api_key, &old_m.api_key);
        }
    }
    crate::config::save(&cfg)?;
    *state.config.write().unwrap() = cfg.clone();
    // Горячую клавишу пересматриваем на месте (desktop).
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    crate::appshell::rebind_hotkey(&app, &cfg.overlay_hotkey);
    crate::search::cache_clear();
    Ok(())
}

#[tauri::command]
pub fn permission_list() -> Vec<(String, String)> {
    ALL_PERMISSIONS.iter().map(|(k, n)| (k.to_string(), n.to_string())).collect()
}

#[tauri::command]
pub fn set_permission(state: State<'_, AppState>, key: String, value: bool) -> Result<(), String> {
    let mut cfg = state.config.write().unwrap();
    cfg.set_permission(&key, value);
    crate::config::save(&cfg)
}

// ------------------------------------------------------------------ агент

#[tauri::command]
pub async fn ask(app: AppHandle, text: String) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    crate::agent::ask(&app, &text).await;
    Ok(())
}

#[tauri::command]
pub fn stop_speaking() {
    // Barge-in: сброс TTS + остановка звука.
    events::tts_send(TtsCommand::Stop);
}

#[tauri::command]
pub fn confirm(id: String, approved: bool) {
    resolve_confirmation(&id, approved);
}

#[tauri::command]
pub fn speak_test(text: String, state: State<'_, AppState>) {
    let cfg = state.config.read().unwrap().clone();
    if cfg.tts_enabled {
        events::tts_send(TtsCommand::Speak { text, voice: cfg.voice });
    }
}

// ------------------------------------------------------------------ голос: STT и диктофон

#[tauri::command]
pub async fn transcribe(app: AppHandle, wav_b64: String) -> Result<Value, String> {
    let cfg = app.state::<AppState>().config.read().unwrap().clone();
    let wav = app_engine_decode(&wav_b64)?;
    let lang = if cfg.language == "en" { "en-US" } else { "ru-RU" };
    let api_key = cfg.keys.get("openai").cloned().unwrap_or_default();
    let t = crate::stt::transcribe(&wav, &cfg.stt_engine, &api_key, "", lang).await?;
    let best = crate::stt::prefer_alternatives(&t);
    Ok(Value::String(best))
}

fn app_engine_decode(b64: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| format!("битое аудио: {}", e))
}

/// Диктофон: сохранить аудио + расшифровать + создать заметку.
#[tauri::command]
pub async fn save_voice_note(app: AppHandle, wav_b64: String) -> Result<Value, String> {
    let state = app.state::<AppState>();
    // Разрешение микрофона.
    {
        let cfg = state.config.read().unwrap();
        if !cfg.permission("microphone") {
            return Err("нет разрешения на микрофон".into());
        }
    }
    let cfg = state.config.read().unwrap().clone();
    let wav = app_engine_decode(&wav_b64)?;
    let lang = if cfg.language == "en" { "en-US" } else { "ru-RU" };
    let api_key = cfg.keys.get("openai").cloned().unwrap_or_default();
    let transcript = crate::stt::transcribe(&wav, &cfg.stt_engine, &api_key, "", lang)
        .await
        .map(|t| crate::stt::prefer_alternatives(&t))
        .unwrap_or_default();

    // Аудио — в папку заметок.
    let notes_dir = cfg.effective_notes_dir();
    let audio_dir = notes_dir.join("voice");
    let _ = std::fs::create_dir_all(&audio_dir);
    let fname = format!("{}.wav", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    let audio_path = audio_dir.join(&fname);
    std::fs::write(&audio_path, &wav).map_err(|e| e.to_string())?;

    let title: String = transcript.chars().take(40).collect();
    let id = state
        .db
        .note_add(&if title.is_empty() { format!("Голосовая {}", fname) } else { title }, &transcript, "voice", &audio_path.to_string_lossy())
        .map_err(|e| e.to_string())?;
    if !transcript.is_empty() {
        let _ = state.db.fact_add(&format!("Голосовая заметка: {}", transcript));
    }
    state.db.log_action("save_voice_note", &audio_path.to_string_lossy(), "ok", "");
    Ok(serde_json::json!({"id": id, "transcript": transcript, "audio": audio_path.to_string_lossy()}))
}

// ------------------------------------------------------------------ скан приложений

#[tauri::command]
pub async fn scan_apps(app: AppHandle) -> Result<Value, String> {
    let state = app.state::<AppState>();
    {
        let cfg = state.config.read().unwrap();
        if !cfg.permission("app_scan") {
            state.db.log_action("scan_apps", "", "denied", "нет разрешения app_scan");
            let _ = app.emit("agent://permission-needed", "app_scan");
            return Err("нет разрешения: сканирование программ".into());
        }
    }
    let apps = tokio::task::spawn_blocking(crate::appscan::scan_all)
        .await
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String, bool)> = apps
        .into_iter()
        .map(|a| (a.name, a.path, a.kind, a.is_media))
        .collect();
    let n = state.db.apps_replace(&rows)?;
    state.db.log_action("scan_apps", "", "ok", &format!("{} программ", n));
    Ok(serde_json::json!({"count": n}))
}

#[tauri::command]
pub fn find_apps(state: State<'_, AppState>, query: String) -> Result<Vec<crate::AppEntry>, String> {
    state.db.apps_search(&query)
}

// ------------------------------------------------------------------ журнал, память, чаты, таймеры

#[tauri::command]
pub fn action_log(state: State<'_, AppState>, n: Option<usize>) -> Result<Vec<crate::ActionEntry>, String> {
    state.db.action_log_recent(n.unwrap_or(50))
}

#[tauri::command]
pub fn memory_facts(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.db.facts_all()?.into_iter().map(|(_id, t)| t).collect())
}

#[tauri::command]
pub fn memory_add(state: State<'_, AppState>, fact: String) -> Result<bool, String> {
    if crate::safety::contains_secrets(&fact) {
        return Err("не сохраняю секреты".into());
    }
    state.db.fact_add(&fact)
}

#[tauri::command]
pub fn memory_remove(state: State<'_, AppState>, query: String) -> Result<Vec<String>, String> {
    state.db.fact_remove_matching(&query)
}

#[tauri::command]
pub fn chats(state: State<'_, AppState>) -> Result<Value, String> {
    let list = state.db.chats_list()?;
    Ok(serde_json::to_value(list).unwrap_or_default())
}

#[tauri::command]
pub fn chat_messages(state: State<'_, AppState>, chat_id: i64) -> Result<Value, String> {
    let msgs = state.db.messages_recent(chat_id, 200)?;
    Ok(serde_json::to_value(msgs).unwrap_or_default())
}

#[tauri::command]
pub fn new_chat(state: State<'_, AppState>) -> Result<i64, String> {
    state.db.chat_create("Новый диалог")
}

#[tauri::command]
pub fn timers(state: State<'_, AppState>) -> Result<Value, String> {
    let pending = state.db.timers_pending()?;
    let now = chrono::Utc::now();
    let items: Vec<Value> = pending
        .iter()
        .map(|(id, name, ends)| {
            let ends = chrono::DateTime::parse_from_rfc3339(ends)
                .map(|d| d.with_timezone(&chrono::Utc))
                .unwrap_or(now);
            serde_json::json!({"id": id, "name": name, "left_seconds": (ends - now).num_seconds().max(0)})
        })
        .collect();
    Ok(Value::Array(items))
}

#[tauri::command]
pub fn notes(state: State<'_, AppState>) -> Result<Vec<crate::Note>, String> {
    state.db.notes_recent(50)
}

#[tauri::command]
pub fn save_note(state: State<'_, AppState>, title: String, body: String) -> Result<i64, String> {
    state.db.note_add(&title, &body, "note", "")
}

// ------------------------------------------------------------------ окна и tray

#[tauri::command]
pub fn toggle_overlay(app: AppHandle) {
    // Оверлей — desktop-режим; на iOS команда безвредна.
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    crate::appshell::toggle_overlay(&app);
    #[cfg(any(target_os = "ios", target_os = "android"))]
    let _ = &app;
}

#[tauri::command]
pub fn hide_to_tray(app: AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.state::<AppState>().quit.store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
}

#[tauri::command]
pub fn dev_ping() -> String {
    "pong".into()
}

// для unit-теста маскировки
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_and_keep() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("short"), "••••");
        assert_eq!(mask("sk-1234567890abcdef"), "••••cdef");
        assert_eq!(keep_secret("••••cdef", "sk-1234567890abcdef"), "sk-1234567890abcdef");
        assert_eq!(keep_secret("new-key", "old-key"), "new-key");
    }
}
