//! Конфигурация JARVIS 3.0 — JSON в %APPDATA%\Jarvis3\config.json.
//! Ключи API шифруются Windows DPAPI (secrets.rs), в репозиторий не попадают.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::secrets;

/// Ключи конфигурации, которые хранятся только в зашифрованном виде.
pub const SECRET_KEYS: &[&str] = &[
    "llm_key",
    "keys.spotify",
    "keys.openai",
    "keys.anthropic",
    "keys.google",
    "keys.custom",
];

/// Гранулярные разрешения (granular, revocable).
pub const ALL_PERMISSIONS: &[(&str, &str)] = &[
    ("app_scan", "Сканировать установленные программы и рабочий стол"),
    ("open_close_apps", "Открывать и закрывать программы и папки"),
    ("file_ops", "Операции с файлами (в т.ч. удаление в корзину)"),
    ("microphone", "Микрофон и диктофон"),
    ("screen_capture", "Съёмка экрана для «посмотри на экран»"),
    ("clipboard", "Чтение и запись буфера обмена"),
    ("external_apis", "Подключённые внешние ИИ-модели"),
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectedModel {
    pub id: String,
    pub label: String,
    /// openai | anthropic | gemini | local (OpenAI-совместимый)
    pub provider: String,
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    /// Теги возможностей: text | vision | presentation | image | code
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Config {
    // ---- Разговор
    /// Имя, которым пользователь просил себя называть.
    pub user_name: String,
    /// short | normal | detailed
    pub reply_style: String,
    /// auto | ru | en
    pub language: String,
    /// Свободный текст характера (persona) — в системный промпт.
    pub persona: String,
    /// Wake word (транскрипция, напр. «джарвис»).
    pub wake_word: String,
    /// Непрерывный диалог: окно тишины, сек.
    pub dialogue_timeout_sec: u32,

    // ---- Основная модель (диалог)
    /// gemini | anthropic | openai | custom | local
    pub llm_provider: String,
    pub llm_key: String,
    pub llm_model: String,
    pub llm_base_url: String,

    // ---- Подключённые модели (эскалация)
    pub connected_models: Vec<ConnectedModel>,
    /// Ключи по сервисам для быстрых подключений.
    pub keys: BTreeMap<String, String>,

    // ---- Голос
    pub tts_enabled: bool,
    /// female (Светлана) | male (Дмитрий) — фолбэк без персонажа
    pub voice: String,
    /// Персонаж: aria|sumi|ren|kaito|mika|shizuku|core
    pub character_id: String,
    /// google (без ключа) | openai (нужен ключ openai)
    pub stt_engine: String,
    /// Устройства: имя из перечисления.
    pub mic_device: String,

    // ---- Инструменты
    pub commands_enabled: bool,
    pub use_web_search: bool,
    pub timers_enabled: bool,
    pub music_service: String, // youtube | yandex | spotify | vk
    /// Папка по умолчанию для заметок/документов.
    pub notes_dir: String,
    /// Дополнительные папки, где разрешено удалять файлы.
    pub extra_allowed_dirs: Vec<String>,

    // ---- Разрешения
    pub permissions: BTreeMap<String, bool>,

    // ---- Приложение
    pub overlay_hotkey: String,
    pub autostart: bool,
    pub character_enabled: bool,
    /// idle | minimal — набор состояний персонажа.
    pub character_style: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            user_name: String::new(),
            reply_style: "short".into(),
            language: "auto".into(),
            persona: String::new(),
            wake_word: "джарвис".into(),
            dialogue_timeout_sec: 8,
            llm_provider: "gemini".into(),
            llm_key: String::new(),
            llm_model: String::new(),
            llm_base_url: String::new(),
            connected_models: Vec::new(),
            keys: BTreeMap::new(),
            tts_enabled: true,
            voice: "female".into(),
            character_id: "aria".into(),
            stt_engine: "google".into(),
            mic_device: String::new(),
            commands_enabled: true,
            use_web_search: true,
            timers_enabled: true,
            music_service: "youtube".into(),
            notes_dir: String::new(),
            extra_allowed_dirs: Vec::new(),
            permissions: BTreeMap::new(),
            overlay_hotkey: "Alt+Space".into(),
            autostart: false,
            character_enabled: true,
            character_style: "idle".into(),
        }
    }
}

impl Config {
    pub fn permission(&self, key: &str) -> bool {
        self.permissions.get(key).copied().unwrap_or(false)
    }

    pub fn set_permission(&mut self, key: &str, value: bool) {
        self.permissions.insert(key.to_string(), value);
    }

    /// Первичный ли это запуск (разрешения ещё не выдавались).
    pub fn permissions_initialized(&self) -> bool {
        !self.permissions.is_empty()
    }

    pub fn effective_notes_dir(&self) -> PathBuf {
        if !self.notes_dir.is_empty() {
            let p = PathBuf::from(&self.notes_dir);
            if p.is_dir() {
                return p;
            }
        }
        dirs_documents().unwrap_or_else(|| app_dir().join("notes"))
    }

    /// Основная модель настроена?
    pub fn llm_configured(&self) -> bool {
        match self.llm_provider.as_str() {
            "local" => !self.llm_model.is_empty(),
            "custom" => !self.llm_key.is_empty() && !self.llm_base_url.is_empty() && !self.llm_model.is_empty(),
            "" => false,
            _ => !self.llm_key.is_empty(),
        }
    }

    /// Первая подключённая модель с указанной возможностью.
    pub fn model_with_capability(&self, cap: &str) -> Option<ConnectedModel> {
        self.connected_models
            .iter()
            .find(|m| m.capabilities.iter().any(|c| c == cap))
            .cloned()
    }
}

fn dirs_documents() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .map(|h| PathBuf::from(h).join("Documents"))
        .filter(|p| p.is_dir())
}

/// Каталог данных приложения: %APPDATA%\Jarvis3.
pub fn app_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from).unwrap_or(PathBuf::from(".")));
    let dir = base.join("Jarvis3");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

/// Шифрует секретные поля JSON: llm_key, keys.*, connected_models[].api_key.
fn encrypt_value(value: &mut serde_json::Value) {
    let serde_json::Value::Object(map) = value else { return };
    for (k, v) in map.iter_mut() {
        match k.as_str() {
            "api_key" | "llm_key" => {
                if let serde_json::Value::String(s) = v {
                    if !s.is_empty() {
                        *s = secrets::encrypt(s);
                    }
                }
            }
            "keys" => {
                if let serde_json::Value::Object(keys) = v {
                    for (_kk, vv) in keys.iter_mut() {
                        if let serde_json::Value::String(s) = vv {
                            if !s.is_empty() {
                                *s = secrets::encrypt(s);
                            }
                        }
                    }
                }
            }
            "connected_models" => {
                if let serde_json::Value::Array(items) = v {
                    for m in items.iter_mut() {
                        if let serde_json::Value::Object(mo) = m {
                            if let Some(serde_json::Value::String(s)) = mo.get_mut("api_key") {
                                if !s.is_empty() {
                                    *s = secrets::encrypt(s);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Расшифровывает секретные поля (зеркально encrypt_value).
fn decrypt_value(value: &mut serde_json::Value) {
    let serde_json::Value::Object(map) = value else { return };
    for (k, v) in map.iter_mut() {
        match k.as_str() {
            "api_key" | "llm_key" => {
                if let serde_json::Value::String(s) = v {
                    *s = secrets::decrypt(s);
                }
            }
            "keys" => {
                if let serde_json::Value::Object(keys) = v {
                    for (_kk, vv) in keys.iter_mut() {
                        if let serde_json::Value::String(s) = vv {
                            *s = secrets::decrypt(s);
                        }
                    }
                }
            }
            "connected_models" => {
                if let serde_json::Value::Array(items) = v {
                    for m in items.iter_mut() {
                        if let serde_json::Value::Object(mo) = m {
                            if let Some(serde_json::Value::String(s)) = mo.get_mut("api_key") {
                                *s = secrets::decrypt(s);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Загружает конфиг, расшифровывая секреты; при любой ошибке — дефолт.
pub fn load() -> Config {
    let Ok(raw) = std::fs::read_to_string(config_path()) else {
        return Config::default();
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Config::default();
    };
    decrypt_value(&mut value);
    serde_json::from_value(value).unwrap_or_default()
}

/// Сохраняет конфиг, шифруя секреты через DPAPI (атомарно, через tmp-файл).
pub fn save(cfg: &Config) -> Result<(), String> {
    let mut value = serde_json::to_value(cfg).map_err(|e| e.to_string())?;
    encrypt_value(&mut value);
    let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    let path = config_path();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}
