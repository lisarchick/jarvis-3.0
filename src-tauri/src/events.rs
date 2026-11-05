//! Центральная шина событий агента: статусы конвейера (Idle/Listening/Thinking/
//! Searching/Executing/Speaking) и эмоции персонажа. Все модули публикуют события
//! сюда, UI и 2.5D-риг подписываются через Tauri events.

use once_cell::sync::Lazy;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Idle,
    Listening,
    Thinking,
    Searching,
    Executing,
    Speaking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Emotion {
    Idle,
    Listening,
    Thinking,
    Searching,
    Speaking,
    Executing,
    Confused,
    Happy,
    Sleep,
    Permission,
    Sorry,
}

impl Emotion {
    pub fn from_status(s: Status) -> Self {
        match s {
            Status::Idle => Emotion::Idle,
            Status::Listening => Emotion::Listening,
            Status::Thinking => Emotion::Thinking,
            Status::Searching => Emotion::Searching,
            Status::Executing => Emotion::Executing,
            Status::Speaking => Emotion::Speaking,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentEvent {
    /// Статус конвейера (для статусной строки).
    pub status: Status,
    /// Эмоция персонажа.
    pub emotion: Emotion,
    /// Человекочитаемая деталь, напр. «Ищу: кто изобрёл радио».
    pub detail: String,
}

/// Глобальный publish: статус + эмоция одним событием `agent://state`.
pub fn emit(app: &AppHandle, status: Status, emotion: Emotion, detail: &str) {
    let _ = app.emit(
        "agent://state",
        AgentEvent { status, emotion, detail: detail.to_string() },
    );
}

pub fn status(app: &AppHandle, s: Status, detail: &str) {
    emit(app, s, Emotion::from_status(s), detail);
}

pub fn emotion(app: &AppHandle, e: Emotion, detail: &str) {
    let st = match e {
        Emotion::Listening => Status::Listening,
        Emotion::Searching => Status::Searching,
        Emotion::Executing => Status::Executing,
        Emotion::Speaking => Status::Speaking,
        Emotion::Thinking => Status::Thinking,
        _ => Status::Idle,
    };
    emit(app, st, e, detail);
}

// ------------------------------------------------------------------ токен стриминга

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StreamChunk {
    Delta { text: String },
    Done { message_id: i64 },
    Error { message: String },
}

/// Кусок текста от LLM — в UI немедленно.
pub fn emit_delta(app: &AppHandle, text: &str) {
    let _ = app.emit("agent://delta", StreamChunk::Delta { text: text.to_string() });
}

pub fn emit_done(app: &AppHandle, message_id: i64) {
    let _ = app.emit("agent://delta", StreamChunk::Done { message_id });
}

pub fn emit_error(app: &AppHandle, message: &str) {
    let _ = app.emit("agent://delta", StreamChunk::Error { message: message.to_string() });
}

// ------------------------------------------------------------------ карточки инструментов

#[derive(Debug, Clone, Serialize)]
pub struct ToolEvent {
    pub id: String,
    pub tool: String,
    pub title: String,
    pub args: serde_json::Value,
    #[serde(rename = "type")]
    pub kind: String, // start | ok | error | need-confirm
    pub detail: String,
}

pub fn emit_tool(app: &AppHandle, ev: ToolEvent) {
    let _ = app.emit("agent://tool", ev);
}

// ------------------------------------------------------------------ озвучка и звук

/// Команда TTS-очереди (стрим по предложениям).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TtsCommand {
    /// Синтезировать и поставить в очередь (не ждать).
    Speak { text: String, voice: String },
    /// Сбросить очередь и остановить воспроизведение (barge-in / стоп).
    Stop,
}

pub static TTX: Lazy<Mutex<Option<tokio::sync::mpsc::UnboundedSender<TtsCommand>>>> = Lazy::new(|| Mutex::new(None));

pub fn tts_send(cmd: TtsCommand) {
    if let Some(tx) = TTX.lock().unwrap().as_ref() {
        let _ = tx.send(cmd);
    }
}
