//! Telegram bot (off by default). Long-polls getUpdates, answers allowed ids with the cluster model
//! (or a «Помощник» for simple prompts when routing is on). Others get their id back so the owner can allow them.
//! /new (or /start) resets the per-chat history. Replies are split at 4096 characters.
use crate::state::AppState;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock}, time::Duration};

const TG_LIMIT: usize = 4096;
const HISTORY: usize = 20;

fn status_cell() -> &'static Mutex<String> {
    static S: OnceLock<Mutex<String>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(String::new()))
}
fn set_status(s: impl Into<String>) { *status_cell().lock().unwrap_or_else(|e| e.into_inner()) = s.into(); }
/// "" (off) | "работает (@bot)" | "ошибка: …"
pub fn status() -> String { status_cell().lock().unwrap_or_else(|e| e.into_inner()).clone() }

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).build().unwrap_or_default())
}

async fn api(token: &str, method: &str, body: Value, timeout_s: u64) -> anyhow::Result<Value> {
    let r: Value = http().post(format!("https://api.telegram.org/bot{token}/{method}")).json(&body)
        .timeout(Duration::from_secs(timeout_s)).send().await?.json().await?;
    if r["ok"].as_bool() != Some(true) {
        anyhow::bail!("{}", r["description"].as_str().unwrap_or("Telegram API error"));
    }
    Ok(r["result"].clone())
}

/// split on char boundaries, preferring a newline near the limit
pub fn split_msg(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        let mut end = (i + TG_LIMIT).min(chars.len());
        if end < chars.len() {
            if let Some(nl) = chars[i..end].iter().rposition(|c| *c == '\n') { if nl > TG_LIMIT / 2 { end = i + nl + 1; } }
        }
        out.push(chars[i..end].iter().collect());
        i = end;
    }
    if out.is_empty() { out.push("(пустой ответ)".into()); }
    out
}

async fn send(token: &str, chat: i64, text: &str) {
    for part in split_msg(text) {
        if let Err(e) = api(token, "sendMessage", json!({ "chat_id": chat, "text": part }), 30).await { log::warn!("telegram send: {e}"); }
    }
}

pub async fn supervise(st: Arc<AppState>) {
    let mut offset: i64 = 0;
    let mut cur_token = String::new();
    let mut hist_token = String::new();
    let mut histories: HashMap<i64, Vec<Value>> = HashMap::new();
    loop {
        let tg = crate::features::get().telegram;
        if !tg.enabled || tg.token.is_empty() {
            set_status(if tg.enabled { "нет токена" } else { "" });
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        if tg.token != cur_token {
            cur_token = tg.token.clone();
            if hist_token != cur_token { hist_token = cur_token.clone(); offset = 0; histories.clear(); }
            match api(&cur_token, "getMe", json!({}), 15).await {
                Ok(me) => set_status(format!("работает (@{})", me["username"].as_str().unwrap_or("bot"))),
                Err(e) => { set_status(format!("ошибка: {e}")); cur_token.clear(); tokio::time::sleep(Duration::from_secs(10)).await; continue; }
            }
        }
        let updates = match api(&cur_token, "getUpdates", json!({ "offset": offset, "timeout": 25, "allowed_updates": ["message"] }), 40).await {
            Ok(u) => u,
            Err(e) => { set_status(format!("ошибка: {e}")); tokio::time::sleep(Duration::from_secs(5)).await; cur_token.clear(); continue; }
        };
        for u in updates.as_array().cloned().unwrap_or_default() {
            offset = offset.max(u["update_id"].as_i64().unwrap_or(0) + 1);
            let msg = &u["message"];
            let Some(chat) = msg["chat"]["id"].as_i64() else { continue };
            let from = msg["from"]["id"].as_i64().unwrap_or(chat);
            // re-read: ids may have been edited while we were polling
            let allowed = crate::features::get().telegram.allowed_ids;
            if !allowed.contains(&from) && !allowed.contains(&chat) {
                send(&cur_token, chat, &format!("Нет доступа. Ваш id: {from}\nДобавьте его в Argus → «Возможности» → Telegram.")).await;
                continue;
            }
            let text = msg["text"].as_str().or(msg["caption"].as_str()).unwrap_or("").trim().to_string();
            if text.is_empty() { send(&cur_token, chat, "Пока понимаю только текст.").await; continue; }
            if text.starts_with("/new") || text.starts_with("/start") {
                histories.remove(&chat);
                send(&cur_token, chat, "Новый диалог. Спрашивайте.").await;
                continue;
            }
            let h = histories.entry(chat).or_default();
            h.push(json!({ "role": "user", "content": text }));
            if h.len() > HISTORY { let cut = h.len() - HISTORY; h.drain(..cut); }
            let _ = api(&cur_token, "sendChatAction", json!({ "chat_id": chat, "action": "typing" }), 10).await;
            let reply = match ask(&st, h.clone(), &text).await {
                Ok(r) => { h.push(json!({ "role": "assistant", "content": r })); r }
                Err(e) => { h.pop(); format!("Ошибка: {e}") }
            };
            send(&cur_token, chat, &reply).await;
        }
    }
}

async fn ask(st: &AppState, messages: Vec<Value>, prompt: &str) -> anyhow::Result<String> {
    let t = crate::features::route(st, prompt, false).await;
    let mut body = json!({ "model": t.model, "messages": crate::features::normalize_messages(&Value::Array(messages)), "stream": false });
    if t.kind == "helper" { body["chat_template_kwargs"] = json!({ "enable_thinking": false }); }
    let r = http().post(format!("{}/v1/chat/completions", t.base)).json(&body).timeout(Duration::from_secs(1800)).send().await
        .map_err(|e| anyhow::anyhow!(if t.kind == "main" { format!("модель не запущена в Argus ({e})") } else { format!("помощник недоступен ({e})") }))?;
    let code = r.status();
    let v: Value = r.json().await?;
    if !code.is_success() { anyhow::bail!("HTTP {code}: {}", v["error"]["message"].as_str().unwrap_or("")); }
    let content = v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    Ok(crate::helper::strip_think(&content))
}

#[cfg(test)]
mod tests {
    #[test]
    fn split() {
        let s = "я".repeat(9000);
        let p = super::split_msg(&s);
        assert_eq!(p.len(), 3);
        assert!(p.iter().all(|x| x.chars().count() <= 4096));
    }
}
