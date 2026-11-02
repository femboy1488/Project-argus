//! Web chat for phones on the same Wi-Fi: http://<pc lan ip>:50080/#pin=NNNNNN
//! A tiny HTTP/1.1 server (tokio, Connection: close) that serves an embedded page and proxies
//! POST /api/chat to the cluster llama-server (or a «Помощник» for simple prompts) with SSE passthrough.
//! Every /api/* call needs header X-Argus-Pin == features.webchat.pin.
use crate::state::AppState;
use std::{sync::{Arc, Mutex, OnceLock}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, task::JoinHandle};

pub const WEBCHAT_PORT: u16 = 50080;
const MAX_BODY: usize = 24 * 1024 * 1024;

const INDEX: &str = include_str!("../webchat/index.html");
const ICON: &[u8] = include_bytes!("../icons/128x128@2x.png");
const MANIFEST: &str = r##"{"name":"Argus Chat","short_name":"Argus","start_url":"/","display":"standalone","background_color":"#0d0f14","theme_color":"#0d0f14","icons":[{"src":"/icon.png","sizes":"256x256","type":"image/png","purpose":"any"}]}"##;

#[derive(Default)]
struct Rt { handle: Option<JoinHandle<()>>, error: String }

fn rt() -> &'static Mutex<Rt> {
    static R: OnceLock<Mutex<Rt>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Rt::default()))
}

/// (running, last error)
pub fn status() -> (bool, String) {
    let r = rt().lock().unwrap_or_else(|e| e.into_inner());
    (r.handle.as_ref().is_some_and(|h| !h.is_finished()), r.error.clone())
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(3)).build().unwrap_or_default())
}

/// Starts / stops the server following features.webchat.enabled (checked every 2 s).
pub async fn supervise(st: Arc<AppState>) {
    loop {
        let want = crate::features::get().webchat.enabled;
        let running = status().0;
        if want && !running {
            match TcpListener::bind(("0.0.0.0", WEBCHAT_PORT)).await {
                Ok(l) => {
                    log::info!("webchat on :{WEBCHAT_PORT}");
                    let st2 = st.clone();
                    let h = tokio::spawn(async move {
                        loop {
                            let Ok((sock, _)) = l.accept().await else { tokio::time::sleep(Duration::from_millis(200)).await; continue };
                            let st3 = st2.clone();
                            tokio::spawn(async move { let _ = handle(sock, st3).await; });
                        }
                    });
                    let mut r = rt().lock().unwrap_or_else(|e| e.into_inner());
                    r.handle = Some(h); r.error.clear();
                }
                Err(e) => {
                    rt().lock().unwrap_or_else(|e| e.into_inner()).error = format!("порт {WEBCHAT_PORT} занят: {e}");
                }
            }
        } else if !want {
            let mut r = rt().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = r.handle.take() { h.abort(); }
            r.error.clear();
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

struct Req { method: String, path: String, headers: Vec<(String, String)>, body: Vec<u8> }

impl Req {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

async fn read_req(sock: &mut TcpStream) -> anyhow::Result<Req> {
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(15), sock.read(&mut tmp)).await??;
        if n == 0 { anyhow::bail!("eof"); }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break i; }
        if buf.len() > 64 * 1024 { anyhow::bail!("headers too large"); }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY { anyhow::bail!("body too large"); }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = tokio::time::timeout(Duration::from_secs(30), sock.read(&mut tmp)).await??;
        if n == 0 { break; }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len);
    Ok(Req { method, path, headers, body })
}

async fn respond(sock: &mut TcpStream, code: u16, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match code { 200 => "OK", 401 => "Unauthorized", 404 => "Not Found", 413 => "Payload Too Large", 502 => "Bad Gateway", _ => "Error" };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await?;
    sock.flush().await
}

async fn json_err(sock: &mut TcpStream, code: u16, msg: &str) -> std::io::Result<()> {
    respond(sock, code, "application/json", serde_json::json!({ "error": msg }).to_string().as_bytes()).await
}

fn pin_ok(req: &Req) -> bool {
    let want = crate::features::get().webchat.pin;
    let got = req.header("x-argus-pin").unwrap_or_default();
    !want.is_empty() && got.len() == want.len() && got.bytes().zip(want.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

async fn handle(mut sock: TcpStream, st: Arc<AppState>) -> anyhow::Result<()> {
    let req = match read_req(&mut sock).await {
        Ok(r) => r,
        Err(e) => { let _ = json_err(&mut sock, 413, &e.to_string()).await; return Ok(()); }
    };
    let path = req.path.split('?').next().unwrap_or("/").to_string();
    match (req.method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => respond(&mut sock, 200, "text/html; charset=utf-8", INDEX.as_bytes()).await?,
        ("GET", "/manifest.json") => respond(&mut sock, 200, "application/manifest+json", MANIFEST.as_bytes()).await?,
        ("GET", "/icon.png") | ("GET", "/favicon.ico") | ("GET", "/apple-touch-icon.png") => respond(&mut sock, 200, "image/png", ICON).await?,
        (_, p) if p.starts_with("/api/") => {
            if !pin_ok(&req) {
                tokio::time::sleep(Duration::from_millis(700)).await; // slow down PIN guessing
                json_err(&mut sock, 401, "Неверный PIN").await?;
                return Ok(());
            }
            match (req.method.as_str(), p) {
                ("GET", "/api/ping") => {
                    let model = crate::features::main_model(&st).await;
                    let up = crate::llama::engine_running(&st).await;
                    respond(&mut sock, 200, "application/json", serde_json::json!({ "ok": true, "model": model, "engine": up }).to_string().as_bytes()).await?
                }
                ("POST", "/api/chat") => chat(&mut sock, &st, &req.body).await?,
                _ => json_err(&mut sock, 404, "not found").await?,
            }
        }
        _ => json_err(&mut sock, 404, "not found").await?,
    }
    Ok(())
}

/// last user message text + whether any message carries an image / non-text part
pub fn inspect(messages: &serde_json::Value) -> (String, bool) {
    let mut attach = false;
    let mut last = String::new();
    if let Some(arr) = messages.as_array() {
        for m in arr {
            let c = &m["content"];
            let text = match c {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(parts) => parts.iter().filter_map(|p| {
                    if p["type"] != "text" { attach = true; }
                    p["text"].as_str().map(|s| s.to_string())
                }).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            if m["role"] == "user" { last = text; }
        }
    }
    (last, attach)
}

async fn chat(sock: &mut TcpStream, st: &AppState, body: &[u8]) -> std::io::Result<()> {
    let Ok(mut req) = serde_json::from_slice::<serde_json::Value>(body) else { return json_err(sock, 400, "неверный JSON").await };
    let (prompt, attach) = inspect(&req["messages"]);
    let target = crate::features::route(st, &prompt, attach).await;
    req["model"] = target.model.clone().into();
    if req.get("stream").is_none() { req["stream"] = true.into(); }
    if target.kind == "helper" { req["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": false }); }
    let resp = http().post(format!("{}/v1/chat/completions", target.base)).json(&req).timeout(Duration::from_secs(1800)).send().await;
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            let msg = if target.kind == "main" { format!("Модель не запущена в Argus ({e})") } else { format!("Помощник недоступен ({e})") };
            return json_err(sock, 502, &msg).await;
        }
    };
    let code = resp.status().as_u16();
    let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("text/event-stream").to_string();
    let reason = if code == 200 { "OK" } else { "Upstream" };
    let head = format!("HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nCache-Control: no-store\r\nX-Accel-Buffering: no\r\nX-Argus-Model: {}\r\nX-Argus-Kind: {}\r\nAccess-Control-Expose-Headers: X-Argus-Model, X-Argus-Kind\r\nConnection: close\r\n\r\n",
                       target.model.replace(['\r', '\n'], ""), target.kind);
    sock.write_all(head.as_bytes()).await?;
    loop {
        match resp.chunk().await {
            Ok(Some(c)) => { if sock.write_all(&c).await.is_err() { break; } let _ = sock.flush().await; } // phone closed ⇒ drop upstream
            _ => break,
        }
    }
    let _ = sock.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn inspect_parts() {
        let m = serde_json::json!([{"role":"user","content":[{"type":"text","text":"hi"},{"type":"image_url","image_url":{"url":"data:"}}]}]);
        assert_eq!(super::inspect(&m), ("hi".to_string(), true));
    }
}
