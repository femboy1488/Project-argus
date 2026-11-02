//! Speech-to-text through a whisper.cpp server (`/inference`, multipart).
//! Base URL: the one given by the UI (features → voice url), else a whisper service on a USB phone (nodesvc).

use base64::Engine;
use std::time::Duration;

fn multipart(wav: &[u8], boundary: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(wav.len() + 512);
    let field = |b: &mut Vec<u8>, name: &str, val: &str| {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{val}\r\n").as_bytes());
    };
    field(&mut b, "response_format", "json");
    field(&mut b, "temperature", "0.0");
    field(&mut b, "language", "auto");
    b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes());
    b.extend_from_slice(wav);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    b
}

#[tauri::command]
pub async fn transcribe(wav_b64: String, base: Option<String>) -> Result<String, String> {
    let b64 = wav_b64.split_once(',').map(|(_, x)| x).unwrap_or(&wav_b64);
    let wav = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| format!("bad audio: {e}"))?;
    let base = base.map(|b| b.trim().trim_end_matches('/').to_string()).filter(|b| !b.is_empty())
        .or_else(crate::nodesvc::whisper_base)
        .ok_or("Сервер распознавания речи не найден: включите роль «Голос» на телефоне или укажите адрес whisper-сервера")?;
    let url = if base.ends_with("/inference") { base } else { format!("{base}/inference") };
    let boundary = format!("argus{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let r = reqwest::Client::new().post(&url)
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(multipart(&wav, &boundary))
        .timeout(Duration::from_secs(120))
        .send().await.map_err(|e| format!("whisper: {e}"))?;
    let status = r.status();
    let text = r.text().await.map_err(|e| format!("whisper: {e}"))?;
    if !status.is_success() { return Err(format!("whisper {status}: {}", text.chars().take(200).collect::<String>())); }
    let out = serde_json::from_str::<serde_json::Value>(&text).ok()
        .and_then(|j| j.get("text").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or(text);
    Ok(out.trim().to_string())
}
