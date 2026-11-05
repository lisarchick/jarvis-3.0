//! Синтез речи: Microsoft edge-tts через WebSocket (голоса ru-RU-SvetlanaNeural /
//! ru-RU-DmitryNeural), формат riff-24khz-16bit-mono-pcm (WAV) — rodio играет нативно.
//! Стриминг по предложениям: следующее синтезируется, пока звучит текущее.
//! Barge-in: Stop сбрасывает очередь и глушит воспроизведение немедленно.

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::events::TtsCommand;

const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
const WSS_HOST: &str = "speech.platform.bing.com";
const CHROMIUM_VERSION: &str = "130.0.2849.68";

pub fn voice_id(voice: &str) -> &'static str {
    match voice {
        "male" => "ru-RU-DmitryNeural",
        _ => "ru-RU-SvetlanaNeural",
    }
}

/// DRM-токен edge-tts: SHA256(winTicks(300s) + TRUSTED_CLIENT_TOKEN), hex uppercase.
fn sec_ms_gec() -> String {
    let win_epoch: u64 = 11644473600;
    let ticks = (chrono::Utc::now().timestamp() as u64 + win_epoch) / 300 * 300;
    let mut hasher = Sha256::new();
    hasher.update(ticks.to_string());
    hasher.update(TRUSTED_CLIENT_TOKEN);
    let hash = hasher.finalize();
    hash.iter().map(|b| format!("{:02X}", b)).collect::<String>()
}

fn connection_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..32).map(|_| format!("{:x}", rng.gen::<u16>() & 0xF)).collect()
}

/// Синтезирует текст → WAV-байты (24 кГц, моно, 16 бит).
pub async fn synthesize(text: &str, voice: &str) -> Result<Vec<u8>, String> {
    let voice = voice_id(voice);
    let cid = connection_id();
    let url = format!(
        "wss://{}/consumer/speech/synthesize/readaloud/edge/v1?TrustedClientToken={}&Sec-MS-GEC={}&Sec-MS-GEC-Version=1-{}&ConnectionId={}",
        WSS_HOST, TRUSTED_CLIENT_TOKEN, sec_ms_gec(), CHROMIUM_VERSION, cid
    );
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    let headers = request.headers_mut();
    headers.insert("Origin", "chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold".parse().unwrap());
    headers.insert("User-Agent", format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{} Safari/537.36 Edg/{}", CHROMIUM_VERSION, CHROMIUM_VERSION).parse().unwrap());

    let (mut ws, _resp) = tokio_tungstenite::connect_async(request).await.map_err(|e| format!("edge-tts недоступен: {}", e))?;

    // 1. Конфигурация
    let ts = chrono::Utc::now().timestamp_subsec_millis();
    let config = format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"riff-24khz-16bit-mono-pcm\"}}}}}}}}",
        ts
    );
    ws.send(Message::Text(config.into())).await.map_err(|e| e.to_string())?;

    // 2. SSML
    let ssml = format!(
        "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='ru-RU'><voice name='{}'><prosody pitch='+0Hz' rate='+0%' volume='+0%'>{}</prosody></voice></speak>",
        cid,
        chrono::Utc::now().format("%a %b %d %Y %H:%M:%S GMT+0000"),
        voice,
        xml_escape(text)
    );
    ws.send(Message::Text(ssml.into())).await.map_err(|e| e.to_string())?;

    // 3. Читаем бинарные кадры, пока не придёт turn.end
    let mut audio: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while let Some(msg) = tokio::time::timeout_at(deadline, ws.next()).await.map_err(|_| "таймаут синтеза".to_string())? {
        match msg.map_err(|e| e.to_string())? {
            Message::Binary(frame) => {
                // Кадр: 2 байка big-endian header length + header + payload
                if frame.len() > 2 {
                    let header_len = u16::from_be_bytes([frame[0], frame[1]]) as usize;
                    if frame.len() > 2 + header_len {
                        audio.extend_from_slice(&frame[2 + header_len..]);
                    }
                }
            }
            Message::Text(txt) => {
                if txt.contains("Path:turn.end") {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    let _ = ws.close(None).await;
    if audio.is_empty() {
        return Err("пустой ответ синтеза речи (проверь интернет)".into());
    }
    Ok(audio)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ------------------------------------------------------------------ воркер очереди

/// Запускает обработчик команд TTS: синтез + воспроизведение через rodio,
/// с поддержкой мгновенной остановки (barge-in). `generation` растёт на каждый Stop:
/// фразы, синтез которых начался до Stop, не воспроизводятся.
pub fn spawn_worker(rx: tokio::sync::mpsc::UnboundedReceiver<TtsCommand>, generation: Arc<AtomicU64>) {
    std::thread::spawn(move || {
        let mut rx = rx;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        while let Some(cmd) = rx.blocking_recv() {
            match cmd {
                TtsCommand::Stop => {
                    generation.fetch_add(1, Ordering::SeqCst);
                    crate::os::audio_stop();
                }
                TtsCommand::Speak { text, voice } => {
                    let gen_before = generation.load(Ordering::SeqCst);
                    match rt.block_on(async { synthesize(&text, &voice).await }) {
                        Ok(wav) => {
                            if generation.load(Ordering::SeqCst) == gen_before {
                                crate::os::audio_play(&wav); // блокируется до конца звучания
                            }
                        }
                        Err(e) => eprintln!("[tts] {}", e),
                    }
                }
            }
        }
    });
}

#[allow(dead_code)]
pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}
