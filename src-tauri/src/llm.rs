//! LLM-слой: OpenAI-совместимые API (openai/custom/local Ollama), Anthropic, Gemini.
//! Единый интерфейс: стриминг текста + function calling (tool calls), SSE.
//! Плюс vision-запросы (скриншоты) и одноразовые completion для делегирования.

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

pub const OPENAI_BASE: &str = "https://api.openai.com/v1";
pub const ANTHROPIC_BASE: &str = "https://api.anthropic.com/v1";
pub const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const OLLAMA_BASE: &str = "http://localhost:11434/v1";

pub fn default_model(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude-haiku-5-5",
        "openai" => "gpt-4.1-mini",
        "local" => "qwen2.5:3b",
        _ => "gemini-2.5-flash-lite",
    }
}

pub fn provider_base(provider: &str, custom: &str) -> String {
    match provider {
        "anthropic" => ANTHROPIC_BASE.into(),
        "gemini" => GEMINI_BASE.into(),
        "openai" => OPENAI_BASE.into(),
        "local" => {
            if custom.is_empty() { OLLAMA_BASE.into() } else { custom.to_string() }
        }
        _ => custom.to_string(), // custom
    }
}

pub fn provider_label(provider: &str) -> &'static str {
    match provider {
        "gemini" => "Gemini",
        "anthropic" => "Claude",
        "openai" => "GPT",
        "local" => "локальная модель",
        "custom" => "своя модель",
        _ => "ИИ",
    }
}

// ------------------------------------------------------------------ сообщения

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String, // JSON-объект строкой
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Message {
    pub role: String, // system | user | assistant | tool
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str, // "function"
    pub parameters: Value,
}

impl ToolSpec {
    pub fn new(name: &'static str, desc: &'static str, params: Value) -> Self {
        Self { name, description: desc, kind: "function", parameters: params }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatOutcome {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug)]
pub struct LlmError(pub String);

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Параметры запроса к модели.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
}

// ------------------------------------------------------------------ основной стриминг

/// Стримит ответ модели. `on_delta` получает куски текста в реальном времени.
/// Возвращает финальный текст и накопленные вызовы инструментов.
pub async fn stream_chat(
    req: &LlmRequest,
    messages: &[Message],
    tools: &[ToolSpec],
    mut on_delta: impl FnMut(&str),
) -> Result<ChatOutcome, LlmError> {
    match stream_once(req, &req.model, messages, tools, &mut on_delta).await {
        Err(e) if e.0.contains("__model_not_found__") => {
            // Автовыбор модели по списку /models (как в v1).
            if let Some(pick) = auto_pick_model(req).await {
                let req2 = LlmRequest { model: pick.clone(), ..req.clone() };
                return stream_once(&req2, &pick, messages, tools, &mut on_delta).await;
            }
            Err(LlmError("модель недоступна — укажи её в настройках".into()))
        }
        other => other,
    }
}

async fn stream_once(
    req: &LlmRequest,
    model: &str,
    messages: &[Message],
    tools: &[ToolSpec],
    mut on_delta: impl FnMut(&str),
) -> Result<ChatOutcome, LlmError> {
    let client = Client::builder().timeout(Duration::from_secs(120)).build().map_err(|e| LlmError(e.to_string()))?;
    let (url, headers, body) = build_request(req, model, messages, tools, true);

    let mut request = client.post(&url);
    for (k, v) in headers {
        request = request.header(k, v);
    }
    let resp = request.json(&body).send().await.map_err(|e| LlmError(network_error(req, &e)))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(LlmError(explain(status.as_u16(), &text)));
    }

    // SSE
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut outcome = ChatOutcome::default();
    // Аккумуляторы tool calls (openai: по index; anthropic: по id).
    let mut oa_tools: Vec<(usize, String, String, String)> = Vec::new(); // index, id, name, args
    let mut an_tools: Vec<(String, String, String)> = Vec::new(); // id, name, json
    let mut gemini_calls: Vec<ToolCall> = Vec::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| LlmError(format!("соединение прервано: {}", e)))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);
            let line = line.trim();
            if !line.starts_with("data:") {
                continue;
            }
            let data = line[5..].trim();
            if data == "[DONE]" {
                break;
            }
            let Ok(ev) = serde_json::from_str::<Value>(data) else { continue };
            match req.provider.as_str() {
                "anthropic" => parse_anthropic_event(&ev, &mut outcome, &mut an_tools, &mut on_delta),
                "gemini" => parse_gemini_event(&ev, &mut outcome, &mut gemini_calls, &mut on_delta),
                _ => parse_openai_event(&ev, &mut outcome, &mut oa_tools, &mut on_delta),
            }
        }
    }

    // Финализация tool calls.
    if !oa_tools.is_empty() {
        oa_tools.sort_by_key(|(i, _, _, _)| *i);
        outcome.tool_calls = oa_tools
            .iter()
            .map(|(i, id, name, args)| ToolCall {
                id: if id.is_empty() { format!("call_{}", i) } else { id.clone() },
                name: name.clone(),
                arguments: if args.is_empty() { "{}".into() } else { args.clone() },
            })
            .collect();
    }
    if !an_tools.is_empty() {
        outcome.tool_calls = an_tools
            .iter()
            .map(|(id, name, args)| ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: if args.is_empty() { "{}".into() } else { args.clone() },
            })
            .collect();
    }
    if !gemini_calls.is_empty() {
        outcome.tool_calls = gemini_calls;
    }
    Ok(outcome)
}

fn network_error(req: &LlmRequest, e: &reqwest::Error) -> String {
    if req.provider == "local" {
        format!("локальная модель не отвечает ({}). Проверь, что Ollama запущена", e)
    } else {
        format!("нет связи с сервисом ИИ — проверь интернет ({})", e)
    }
}

fn explain(status: u16, body: &str) -> String {
    let lower = body.to_lowercase();
    match status {
        401 | 403 => "неверный или неактивный API-ключ".into(),
        404 => "__model_not_found__".into(),
        429 => "превышен лимит запросов или нет оплаты".into(),
        s if s >= 500 => "сервис ИИ временно недоступен".into(),
        _ => {
            if lower.contains("<html") {
                "вместо ответа пришла веб-страница — проверь адрес API".into()
            } else {
                format!("ошибка API ({}): {}", status, truncate(body, 200))
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}…", &s[..max]) }
}

// ------------------------------------------------------------------ сборка запросов

fn build_request(req: &LlmRequest, model: &str, messages: &[Message], tools: &[ToolSpec], stream: bool) -> (String, Vec<(&'static str, String)>, Value) {
    match req.provider.as_str() {
        "anthropic" => {
            let system: String = messages
                .iter()
                .filter(|m| m.role == "system")
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let msgs: Vec<Value> = messages
                .iter()
                .filter(|m| m.role != "system")
                .map(|m| {
                    if m.role == "assistant" && !m.tool_calls.is_empty() {
                        json!({
                            "role": "assistant",
                            "content": m.content,
                            "tool_use": m.tool_calls.iter().map(|tc| json!({
                                "id": tc.id,
                                "name": tc.name,
                                "input": parse_json(&tc.arguments),
                            })).collect::<Vec<_>>(),
                        })
                    } else if m.role == "tool" {
                        json!({
                            "role": "user",
                            "content": [{ "type": "tool_result", "tool_use_id": m.tool_call_id, "content": m.content }],
                        })
                    } else {
                        json!({ "role": m.role, "content": m.content })
                    }
                })
                .collect();
            let mut body = json!({
                "model": model,
                "max_tokens": req.max_tokens,
                "system": system,
                "messages": msgs,
                "stream": stream,
            });
            if !tools.is_empty() {
                body["tools"] = Value::Array(
                    tools.iter()
                        .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters }))
                        .collect(),
                );
            }
            (
                format!("{}/messages", req.base_url),
                vec![
                    ("x-api-key", req.api_key.clone()),
                    ("anthropic-version", "2023-06-01".into()),
                    ("content-type", "application/json".into()),
                ],
                body,
            )
        }
        "gemini" => {
            let system: String = messages
                .iter()
                .filter(|m| m.role == "system")
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let contents: Vec<Value> = messages
                .iter()
                .filter(|m| m.role != "system")
                .map(|m| {
                    let role = if m.role == "assistant" { "model" } else { "user" };
                    if m.role == "assistant" && !m.tool_calls.is_empty() {
                        json!({
                            "role": role,
                            "parts": m.tool_calls.iter().map(|tc| json!({
                                "functionCall": { "name": tc.name, "args": parse_json(&tc.arguments) }
                            })).collect::<Vec<_>>(),
                        })
                    } else if m.role == "tool" {
                        json!({
                            "role": "user",
                            "parts": [{ "functionResponse": {
                                "name": m.tool_call_id,
                                "response": { "result": parse_json(&m.content) }
                            }}],
                        })
                    } else {
                        json!({ "role": role, "parts": [{ "text": m.content }] })
                    }
                })
                .collect();
            let mut body = json!({
                "contents": contents,
                "generationConfig": { "maxOutputTokens": req.max_tokens.saturating_mul(4) },
            });
            if !system.is_empty() {
                body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
            }
            if !tools.is_empty() {
                body["tools"] = json!([{ "functionDeclarations":
                    tools.iter().map(|t| json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })).collect::<Vec<_>>()
                }]);
            }
            let method = if stream { "streamGenerateContent?alt=sse" } else { "generateContent" };
            (
                format!("{}/models/{}:{}", req.base_url, model, method),
                vec![("x-goog-api-key", req.api_key.clone()), ("content-type", "application/json".into())],
                body,
            )
        }
        _ => {
            // OpenAI-совместимый (openai | custom | local)
            let msgs: Vec<Value> = messages
                .iter()
                .map(|m| {
                    if m.role == "assistant" && !m.tool_calls.is_empty() {
                        json!({
                            "role": "assistant",
                            "content": value_or_empty(&m.content),
                            "tool_calls": m.tool_calls.iter().map(|tc| json!({
                                "id": tc.id,
                                "type": "function",
                                "function": { "name": tc.name, "arguments": tc.arguments },
                            })).collect::<Vec<_>>(),
                        })
                    } else if m.role == "tool" {
                        json!({ "role": "tool", "tool_call_id": m.tool_call_id, "content": m.content })
                    } else {
                        json!({ "role": m.role, "content": m.content })
                    }
                })
                .collect();
            let token_key = if req.provider == "openai" { "max_completion_tokens" } else { "max_tokens" };
            let mut body = json!({
                "model": model,
                "messages": msgs,
                "stream": stream,
                token_key: req.max_tokens,
            });
            if !tools.is_empty() {
                body["tools"] = Value::Array(
                    tools.iter()
                        .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.parameters } }))
                        .collect(),
                );
            }
            let key = if req.api_key.is_empty() { "ollama".to_string() } else { req.api_key.clone() };
            (
                format!("{}/chat/completions", req.base_url),
                vec![("authorization", format!("Bearer {}", key)), ("content-type", "application/json".into())],
                body,
            )
        }
    }
}

fn value_or_empty(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { Value::String(s.to_string()) }
}

fn parse_json(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

// ------------------------------------------------------------------ парсеры SSE-событий

fn parse_openai_event(ev: &Value, outcome: &mut ChatOutcome, acc: &mut Vec<(usize, String, String, String)>, on_delta: &mut impl FnMut(&str)) {
    let delta = ev.pointer("/choices/0/delta");
    if let Some(d) = delta {
        if let Some(text) = d.get("content").and_then(|v| v.as_str()) {
            if !text.is_empty() {
                outcome.content.push_str(text);
                on_delta(text);
            }
        }
        if let Some(calls) = d.get("tool_calls").and_then(|v| v.as_array()) {
            for c in calls {
                let idx = c.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let id = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = c.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = c.get("function").and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                if let Some(slot) = acc.iter_mut().find(|(i, _, _, _)| *i == idx) {
                    if !id.is_empty() { slot.1 = id.clone(); }
                    if !name.is_empty() { slot.2.push_str(&name); }
                    slot.3.push_str(&args);
                } else {
                    acc.push((idx, id, name, args));
                }
            }
        }
    }
    if let Some(text) = ev.pointer("/choices/0/message/content").and_then(|v| v.as_str()) {
        if !text.is_empty() && outcome.content.is_empty() {
            outcome.content.push_str(text);
            on_delta(text);
        }
    }
}

fn parse_anthropic_event(ev: &Value, outcome: &mut ChatOutcome, acc: &mut Vec<(String, String, String)>, on_delta: &mut impl FnMut(&str)) {
    let ty = ev.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match ty {
        "content_block_start" => {
            let cb = &ev["content_block"];
            if cb.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                acc.push((
                    cb.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    cb.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    String::new(),
                ));
            }
        }
        "content_block_delta" => {
            let d = &ev["delta"];
            match d.get("type").and_then(|v| v.as_str()) {
                Some("text_delta") => {
                    if let Some(t) = d.get("text").and_then(|v| v.as_str()) {
                        outcome.content.push_str(t);
                        on_delta(t);
                    }
                }
                Some("input_json_delta") => {
                    if let Some(t) = d.get("partial_json").and_then(|v| v.as_str()) {
                        if let Some(slot) = acc.last_mut() {
                            slot.2.push_str(t);
                        }
                    }
                }
                _ => {}
            }
        }
        "error" => {
            let msg = ev.pointer("/error/message").and_then(|v| v.as_str()).unwrap_or("ошибка Anthropic");
            outcome.content.push_str(&format!(" [ошибка: {}]", msg));
        }
        _ => {}
    }
}

fn parse_gemini_event(ev: &Value, outcome: &mut ChatOutcome, calls: &mut Vec<ToolCall>, on_delta: &mut impl FnMut(&str)) {
    if let Some(parts) = ev.pointer("/candidates/0/content/parts").and_then(|v| v.as_array()) {
        for p in parts {
            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                outcome.content.push_str(t);
                on_delta(t);
            }
            if let Some(fc) = p.get("functionCall") {
                calls.push(ToolCall {
                    id: format!("call_{}", calls.len()),
                    name: fc.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    arguments: fc.get("args").map(|a| a.to_string()).unwrap_or_else(|| "{}".into()),
                });
            }
        }
    }
    if let Some(reason) = ev.pointer("/promptFeedback/blockReason").and_then(|v| v.as_str()) {
        outcome.content.push_str(&format!(" [запрос заблокирован фильтром: {}]", reason));
    }
}

// ------------------------------------------------------------------ автоподбор модели

/// При 404: список моделей → исключаем «не-чат» → предпочителные пулы (как в v1).
async fn auto_pick_model(req: &LlmRequest) -> Option<String> {
    let client = Client::builder().timeout(Duration::from_secs(12)).build().ok()?;
    let (url, headers) = match req.provider.as_str() {
        "gemini" => (format!("{}/models?key={}", req.base_url, req.api_key), vec![]),
        "anthropic" => (format!("{}/models", req.base_url), vec![
            ("x-api-key", req.api_key.clone()),
            ("anthropic-version", "2023-06-01".into()),
        ]),
        _ => (format!("{}/models", req.base_url), vec![("authorization", format!("Bearer {}", req.api_key))]),
    };
    let mut r = client.get(&url);
    for (k, v) in headers {
        r = r.header(k, v);
    }
    let body = r.send().await.ok()?.text().await.ok()?;
    let v: Value = serde_json::from_str(&body).ok()?;
    let ids: Vec<String> = match req.provider.as_str() {
        "gemini" => v["models"]
            .as_array()?
            .iter()
            .filter_map(|m| m.get("name")?.as_str()?.strip_prefix("models/")?.to_string().into())
            .collect(),
        "anthropic" => v["data"].as_array()?.iter().filter_map(|m| m.get("id")?.as_str()?.to_string().into()).collect(),
        _ => v["data"].as_array()?.iter().filter_map(|m| m.get("id")?.as_str()?.to_string().into()).collect(),
    };
    pick_model(&req.provider, &ids)
}

pub fn pick_model(provider: &str, ids: &[String]) -> Option<String> {
    let bad = [
        "tts", "image", "embed", "live", "audio", "vision", "robotics", "aqa", "imagen", "veo",
        "transcribe", "realtime", "moderation", "whisper", "dall", "search", "computer",
        "thinking", "deep-research", "customtools", "instruct", "preview", "exp",
    ];
    let chat: Vec<&String> = ids.iter().filter(|id| !bad.iter().any(|b| id.to_lowercase().contains(b))).collect();
    let pools: &[&[&str]] = match provider {
        "gemini" => &[&["flash-lite"], &["flash"]],
        "anthropic" => &[&["haiku"]],
        "openai" => &[&["mini"]],
        _ => &[&["qwen", "llama", "gemma", "mistral"]],
    };
    for pool in pools {
        let mut hits: Vec<&String> = chat.iter().copied().filter(|id| pool.iter().any(|p| id.contains(p))).collect();
        if !hits.is_empty() {
            hits.sort();
            return Some(hits.last()?.to_string());
        }
    }
    chat.last().map(|s| s.to_string())
}

// ------------------------------------------------------------------ vision

/// Отправляет PNG-скриншот vision-модели (подключённой) и возвращает описание.
pub async fn vision_describe(
    m: &crate::config::ConnectedModel,
    prompt: &str,
    png: &[u8],
) -> Result<String, LlmError> {
    let b64 = crate::b64_encode(png);
    let client = Client::builder().timeout(Duration::from_secs(90)).build().map_err(|e| LlmError(e.to_string()))?;
    let base = if m.base_url.is_empty() { provider_base(&m.provider, "") } else { m.base_url.clone() };
    let model = if m.model.is_empty() { default_model(&m.provider) } else { m.model.as_str() };

    let resp = match m.provider.as_str() {
        "anthropic" => {
            let body = json!({
                "model": model,
                "max_tokens": 900,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": b64 } },
                        { "type": "text", "text": prompt },
                    ],
                }],
            });
            client
                .post(format!("{}/messages", base))
                .header("x-api-key", &m.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
        }
        "gemini" => {
            let body = json!({
                "contents": [{
                    "parts": [
                        { "inline_data": { "mime_type": "image/png", "data": b64 } },
                        { "text": prompt },
                    ],
                }],
                "generationConfig": { "maxOutputTokens": 1200 },
            });
            client
                .post(format!("{}/models/{}:generateContent", base, model))
                .header("x-goog-api-key", &m.api_key)
                .json(&body)
                .send()
                .await
        }
        _ => {
            let body = json!({
                "model": model,
                "max_tokens": 900,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": prompt },
                        { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{}", b64) } },
                    ],
                }],
            });
            client
                .post(format!("{}/chat/completions", base))
                .header("authorization", format!("Bearer {}", m.api_key))
                .json(&body)
                .send()
                .await
        }
    }
    .map_err(|e| LlmError(format!("нет связи с vision-моделью: {}", e)))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(LlmError(explain(status.as_u16(), &text)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| LlmError(e.to_string()))?;
    let out = match m.provider.as_str() {
        "anthropic" => v["content"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|c| c.get("text").and_then(|t| t.as_str())).collect::<String>())
            .unwrap_or_default(),
        "gemini" => v["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<String>())
            .unwrap_or_default(),
        _ => v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string(),
    };
    if out.trim().is_empty() {
        return Err(LlmError("vision-модель вернула пустой ответ".into()));
    }
    Ok(out)
}

// ------------------------------------------------------------------ одноразовый completion (делегирование)

pub async fn completion_plain(
    m: &crate::config::ConnectedModel,
    system: &str,
    prompt: &str,
    max_tokens: u32,
) -> Result<String, LlmError> {
    let msgs = vec![
        Message { role: "system".into(), content: system.to_string(), ..Default::default() },
        Message { role: "user".into(), content: prompt.to_string(), ..Default::default() },
    ];
    let req = LlmRequest {
        provider: m.provider.clone(),
        base_url: if m.base_url.is_empty() { provider_base(&m.provider, "") } else { m.base_url.clone() },
        api_key: m.api_key.clone(),
        model: if m.model.is_empty() { default_model(&m.provider).to_string() } else { m.model.clone() },
        max_tokens,
    };
    let out = stream_chat(&req, &msgs, &[], &mut |_: &str| {}).await?;
    Ok(out.content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_event_parse() {
        let mut outcome = ChatOutcome::default();
        let mut acc = Vec::new();
        let mut text = String::new();
        let ev: Value = serde_json::from_str(r#"{"choices":[{"delta":{"content":"Привет"}}]}"#).unwrap();
        parse_openai_event(&ev, &mut outcome, &mut acc, &mut |t| text.push_str(t));
        assert_eq!(text, "Привет");

        let ev: Value = serde_json::from_str(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"set_timer","arguments":"{\"sec"}}]}}]}"#,
        )
        .unwrap();
        parse_openai_event(&ev, &mut outcome, &mut acc, &mut |_| {});
        let ev: Value = serde_json::from_str(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"onds\":300}"}}]}}]}"#,
        )
        .unwrap();
        parse_openai_event(&ev, &mut outcome, &mut acc, &mut |_| {});
        assert_eq!(acc.len(), 1);
        assert_eq!(acc[0].2, "set_timer");
        assert_eq!(acc[0].3, "{\"seconds\":300}");
    }

    #[test]
    fn anthropic_event_parse() {
        let mut outcome = ChatOutcome::default();
        let mut acc = Vec::new();
        parse_anthropic_event(
            &serde_json::from_str(r#"{"type":"content_block_start","content_block":{"type":"tool_use","id":"t1","name":"open_app"}}"#).unwrap(),
            &mut outcome, &mut acc, &mut |_| {},
        );
        parse_anthropic_event(
            &serde_json::from_str(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"name\":"}}"#).unwrap(),
            &mut outcome, &mut acc, &mut |_| {},
        );
        parse_anthropic_event(
            &serde_json::from_str(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"\"calc\"}"}}"#).unwrap(),
            &mut outcome, &mut acc, &mut |_| {},
        );
        assert_eq!(acc.len(), 1);
        assert_eq!(acc[0].1, "open_app");
        assert_eq!(acc[0].2, "{\"name\":\"calc\"}");
    }

    #[test]
    fn gemini_event_parse() {
        let mut outcome = ChatOutcome::default();
        let mut calls = Vec::new();
        let ev: Value = serde_json::from_str(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"web_search","args":{"query":"радио"}}}]}}]}"#,
        )
        .unwrap();
        parse_gemini_event(&ev, &mut outcome, &mut calls, &mut |_| {});
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web_search");
        assert!(calls[0].arguments.contains("радио"));
    }

    #[test]
    fn model_pick_pools() {
        let ids = vec![
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-flash-lite".to_string(),
            "imagen-3".to_string(),
            "text-embedding".to_string(),
        ];
        assert_eq!(pick_model("gemini", &ids).as_deref(), Some("gemini-2.5-flash-lite"));
        let ids2 = vec!["qwen2.5:3b".to_string(), "llama3".to_string()];
        assert_eq!(pick_model("local", &ids2).as_deref(), Some("qwen2.5:3b"));
    }

    #[test]
    fn build_request_shapes() {
        let req = LlmRequest {
            provider: "anthropic".into(),
            base_url: ANTHROPIC_BASE.into(),
            api_key: "k".into(),
            model: "claude-haiku-5-5".into(),
            max_tokens: 300,
        };
        let msgs = vec![Message { role: "user".into(), content: "привет".into(), ..Default::default() }];
        let (url, headers, body) = build_request(&req, &req.model, &msgs, &[], true);
        assert!(url.ends_with("/messages"));
        assert!(headers.iter().any(|(k, _)| *k == "x-api-key"));
        assert_eq!(body["system"].as_str().unwrap_or(""), "");
    }
}
