//! Разрешения: гранулярные гейты перед выполнением инструментов + подтверждение
//! рискованных действий через диалог в UI. Всё логируется в журнал действий.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

use crate::config::Config;

#[derive(Debug, Clone, Serialize)]
pub struct Confirmation {
    pub id: String,
    /// Что именно собирается сделать инструмент.
    pub title: String,
    /// Например: «Удалить в корзину: D:\Downloads\old.zip».
    pub detail: String,
    pub risk: String, // low | medium | high
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationResult {
    pub id: String,
    pub approved: bool,
}

/// Ожидающие подтверждения: id → канал ответа.
static PENDING: Lazy<Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// Просит подтверждение у пользователя (UI-диалог). false — отклонено/таймаут.
pub async fn ask_confirmation(app: &AppHandle, title: &str, detail: &str, risk: &str) -> bool {
    let id = crate::util::short_id();
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    PENDING.lock().unwrap().insert(id.clone(), tx);
    let conf = Confirmation {
        id: id.clone(),
        title: title.to_string(),
        detail: detail.to_string(),
        risk: risk.to_string(),
    };
    let _ = app.emit("agent://confirm", conf);
    let result = tokio::time::timeout(std::time::Duration::from_secs(120), rx).await;
    let approved = matches!(result, Ok(Ok(true)));
    PENDING.lock().unwrap().remove(&id);
    approved
}

/// Ответ пользователя из UI.
pub fn resolve_confirmation(id: &str, approved: bool) {
    if let Some(tx) = PENDING.lock().unwrap().remove(id) {
        let _ = tx.send(approved);
    }
}

/// Результат проверки разрешения.
pub enum Gate {
    Allowed,
    Denied(String),
}

/// Проверяет разрешение; при отказе логирует и возвращает причину.
pub fn check(app: &AppHandle, cfg: &Config, db: &crate::db::Db, key: &str) -> Gate {
    if !cfg.permission(key) {
        let names = crate::config::ALL_PERMISSIONS.iter().find(|(k, _)| *k == key).map(|(_, n)| *n).unwrap_or(key);
        let reason = format!("нет разрешения: {}", names);
        db.log_action("permission", &format!("{{\"key\":\"{}\"}}", key), "denied", &reason);
        let _ = app.emit("agent://permission-needed", key);
        return Gate::Denied(reason);
    }
    Gate::Allowed
}

/// Эмитит событие «нужно разрешение» (UI открывает настройки на вкладке разрешений).
pub fn emit_permission_needed(app: &AppHandle, key: &str) {
    let _ = app.emit("agent://permission-needed", key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_flow() {
        // Синхронный тест канала без AppHandle.
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        PENDING.lock().unwrap().insert("abc".into(), tx);
        resolve_confirmation("abc", true);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let val = rt.block_on(async { rx.await.unwrap() });
        assert!(val);
        assert!(PENDING.lock().unwrap().get("abc").is_none());
    }
}
