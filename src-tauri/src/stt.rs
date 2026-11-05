//! Распознавание речи: Google Speech (бесплатный эндпоинт, без ключа) и
//! OpenAI-совместимый /audio/transcriptions (whisper). Аудио: 16 кГц mono 16-bit WAV.
//! Фронтенд делает VAD и присылает готовые фразы.

use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

/// Результат STT: основной текст + альтернативы (для выбора wake-word варианта).
pub struct Transcript {
    pub text: String,
    pub alternatives: Vec<String>,
}

/// Google Speech API v2 — бесплатный публичный ключ Chrome voice search,
/// принимает 16 кГц mono 16-bit PCM (как в v1 через SpeechRecognition).
pub async fn google_stt(wav: &[u8], lang: &str) -> Result<Transcript, String> {
    let client = Client::builder().timeout(Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    let url = format!(
        "https://www.google.com/speech-api/v2/recognize?output=json&lang={}&key={}",
        lang, "AIzaSyBOti4mM-6x9WDnZIjIeyEU21OpBXqWBgw"
    );
    let resp = client
        .post(&url)
        .header("Content-Type", "audio/l16; rate=16000")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .body(wav.to_vec())
        .send()
        .await
        .map_err(|e| format!("распознавание недоступно: {}", e))?;
    let text = resp.text().await.map_err(|e| e.to_string())?;
    parse_google(&text)
}

fn parse_google(body: &str) -> Result<Transcript, String> {
    // Эндпоинт возвращает несколько JSON-строк (по одной на результат).
    let mut alts: Vec<String> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(arr) = v["result"].as_array() {
                for r in arr {
                    if let Some(list) = r["alternative"].as_array() {
                        for a in list {
                            if let Some(t) = a["transcript"].as_str() {
                                alts.push(t.trim().to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    if alts.is_empty() {
        return Err("речь не распознана".into());
    }
    Ok(Transcript { text: alts[0].clone(), alternatives: alts })
}

/// OpenAI-совместимый whisper (нужен ключ; base_url настраивается).
pub async fn openai_stt(wav: &[u8], api_key: &str, base_url: &str, model: &str, lang: &str) -> Result<Transcript, String> {
    let client = Client::builder().timeout(Duration::from_secs(60)).build().map_err(|e| e.to_string())?;
    let base = if base_url.is_empty() { "https://api.openai.com/v1".to_string() } else { base_url.to_string() };
    let model = if model.is_empty() { "whisper-1".to_string() } else { model.to_string() };

    let part = reqwest::multipart::Part::bytes(wav.to_vec())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;
    let form = reqwest::multipart::Form::new()
        .text("model", model)
        .text("language", lang.to_string())
        .text("response_format", "json")
        .part("file", part);

    let resp = client
        .post(format!("{}/audio/transcriptions", base))
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("распознавание недоступно: {}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("ошибка STT ({}): {}", status.as_u16(), &text[..text.len().min(120)]));
    }
    let v: Value = serde_json::from_str(&text).map_err(|_| "пустой ответ STT".to_string())?;
    let out = v["text"].as_str().unwrap_or("").trim().to_string();
    if out.is_empty() {
        return Err("речь не распознана".into());
    }
    Ok(Transcript { text: out.clone(), alternatives: vec![out] })
}

/// Основная точка входа: движок выбирается по конфигурации.
pub async fn transcribe(wav: &[u8], engine: &str, api_key: &str, base_url: &str, lang: &str) -> Result<Transcript, String> {
    match engine {
        "openai" if !api_key.is_empty() => openai_stt(wav, api_key, base_url, "", lang).await,
        _ => google_stt(wav, lang).await,
    }
}

/// Выбирает лучший вариант транскрипта: содержащий wake word или «стоп».
pub fn prefer_alternatives(t: &Transcript) -> String {
    for alt in t.alternatives.iter() {
        let low = alt.to_lowercase().replace('ё', "е");
        if crate::util::WAKE_RE.is_match(&low) || crate::util::STOP_RE.is_match(&low) {
            return alt.clone();
        }
    }
    t.text.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_parse() {
        let body = "{\"result\":[{\"alternative\":[{\"transcript\":\"джарвис открой калькулятор\",\"confidence\":0.9},{\"transcript\":\"джервис открой калькулятор\"}]}]}\n{\"result\":[{\"alternative\":[{\"transcript\":\"конец\"}]}]}";
        let t = parse_google(body).unwrap();
        assert_eq!(t.text, "джарвис открой калькулятор");
        assert_eq!(t.alternatives.len(), 3);
        // предпочитаем вариант с wake word, даже если он не первый
        let plain = Transcript { text: "сервисный центр".into(), alternatives: vec!["сервисный центр".into(), "джарвис привет".into()] };
        assert_eq!(prefer_alternatives(&plain), "джарвис привет");
    }
}
