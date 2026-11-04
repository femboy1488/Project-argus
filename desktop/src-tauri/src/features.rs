//! «Возможности»: optional features stored in argus-features.json (next to argus-settings.json).
//! Web chat for phones (webchat.rs, :50080), Telegram bot (telegram.rs), simple-prompt routing to a «Помощник»,
//! UI language. Other modules ADD fields here — every field is #[serde(default)] so old files keep loading.
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::{Arc, Mutex, OnceLock}};
use tauri::State;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WebChat {
    pub enabled: bool,
    /// required in X-Argus-Pin (and #pin= in the URL); empty = generated on first enable
    pub pin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Telegram {
    pub enabled: bool,
    /// bot token from @BotFather — stored in plain text in the features file (UI warns)
    pub token: String,
    /// Telegram user / chat ids allowed to talk to the bot
    pub allowed_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Routing {
    /// short prompts without code / attachments go to a ready «Помощник» instead of the cluster model
    pub simple_to_helper: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Rag {
    pub enabled: bool,
    pub folders: Vec<String>,
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct UrlFeature {
    pub enabled: bool,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Features {
    pub webchat: WebChat,
    pub telegram: Telegram,
    pub routing: Routing,
    pub rag: Rag,
    pub library: UrlFeature,
    pub voice: UrlFeature,
    /// "ru" | "en"
    pub lang: String,
    /// fields added by other modules / newer versions survive a round-trip through an older build
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Features {
    fn default() -> Self {
        Self { webchat: Default::default(), telegram: Default::default(), routing: Default::default(), rag: Default::default(),
               library: Default::default(), voice: Default::default(), lang: "ru".into(), extra: Default::default() }
    }
}

struct Store { path: Option<PathBuf>, f: Features }

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store { path: None, f: Features::default() }))
}

fn lock() -> std::sync::MutexGuard<'static, Store> { store().lock().unwrap_or_else(|e| e.into_inner()) }

/// argus-features.json in the same dir as argus-settings.json
pub fn path_for(settings_path: &Option<PathBuf>) -> Option<PathBuf> {
    settings_path.as_ref().map(|p| p.with_file_name("argus-features.json"))
}

fn load(path: &Option<PathBuf>) -> Features {
    path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(path: &Option<PathBuf>, f: &Features) {
    let Some(p) = path else { return };
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    if let Ok(s) = serde_json::to_string_pretty(f) {
        let tmp = p.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, s).and_then(|_| std::fs::rename(&tmp, p)) { log::warn!("features save {}: {e}", p.display()); }
    }
}

/// Current features (cheap clone).
pub fn get() -> Features { lock().f.clone() }

/// Modify + persist (for other modules: `features::update(|f| f.extra.insert(...))`).
pub fn update<R>(op: impl FnOnce(&mut Features) -> R) -> R {
    let mut s = lock();
    let r = op(&mut s.f);
    let path = s.path.clone();
    save(&path, &s.f);
    r
}

fn gen_pin() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let x = (n ^ (n >> 17) ^ (std::process::id() as u128) << 7) % 1_000_000;
    format!("{x:06}")
}

/// Load the file and start the background services (web chat, Telegram). Call once from setup.
pub fn start(st: Arc<AppState>) {
    {
        let mut s = lock();
        s.path = path_for(&st.config_path);
        s.f = load(&s.path);
        if s.f.webchat.pin.trim().is_empty() { s.f.webchat.pin = gen_pin(); let p = s.path.clone(); save(&p, &s.f); }
    }
    tauri::async_runtime::spawn(crate::webchat::supervise(st.clone()));
    tauri::async_runtime::spawn(crate::telegram::supervise(st));
}

// ---------------------------------------------------------------- helpers used by webchat / telegram / ChatView

pub fn main_api_base() -> String { format!("http://127.0.0.1:{}", crate::llama::LLAMA_PORT) }

/// model id of the cluster engine (the --alias it was started with)
pub async fn main_model(st: &AppState) -> String {
    st.settings.lock().await.last_launch.as_ref().and_then(|a| a.alias.clone()).filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "argus".into())
}

/// (base url, model) of the least busy ready «Помощник», if any
pub async fn helper_api_base(st: &AppState) -> Option<(String, String)> {
    let rt = st.helpers.lock().await;
    rt.values().filter(|r| r.state == "ready" && r.url.is_some()).min_by_key(|r| r.busy)
        .map(|r| (r.url.clone().unwrap_or_default(), r.model.clone()))
}

/// short, no code, no attachments
pub fn is_simple(prompt: &str, has_attachments: bool) -> bool {
    !has_attachments && prompt.chars().count() < 200 && !prompt.contains("```")
}

/// LAN IPv4 of this PC (the interface with the default route); no packets are sent
pub fn lan_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    let ip = s.local_addr().ok()?.ip();
    if ip.is_unspecified() || ip.is_loopback() { None } else { Some(ip.to_string()) }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteTarget {
    /// OpenAI-compatible base (no /v1)
    pub base: String,
    pub model: String,
    /// "main" | "helper"
    pub kind: String,
}

pub async fn route(st: &AppState, prompt: &str, has_attachments: bool) -> RouteTarget {
    if get().routing.simple_to_helper && is_simple(prompt, has_attachments) {
        if let Some((base, model)) = helper_api_base(st).await {
            return RouteTarget { base, model, kind: "helper".into() };
        }
    }
    RouteTarget { base: main_api_base(), model: main_model(st).await, kind: "main".into() }
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeaturesView {
    #[serde(flatten)]
    pub features: Features,
    pub lan_ip: Option<String>,
    pub webchat_port: u16,
    /// http://<lan ip>:50080/#pin=…
    pub webchat_url: Option<String>,
    pub webchat_running: bool,
    pub webchat_error: String,
    pub telegram_status: String,
}

fn view() -> FeaturesView {
    let f = get();
    let ip = lan_ip();
    let url = ip.as_ref().map(|ip| format!("http://{ip}:{}/#pin={}", crate::webchat::WEBCHAT_PORT, f.webchat.pin));
    let (running, err) = crate::webchat::status();
    FeaturesView { features: f, lan_ip: ip, webchat_port: crate::webchat::WEBCHAT_PORT, webchat_url: url,
                   webchat_running: running, webchat_error: err, telegram_status: crate::telegram::status() }
}

#[tauri::command]
pub async fn features_get() -> Result<FeaturesView, String> { Ok(view()) }

/// the view's read-only fields (they come back flattened when the UI sends a whole view)
const VIEW_ONLY: &[&str] = &["lanIp", "webchatPort", "webchatUrl", "webchatRunning", "webchatError", "telegramStatus"];
/// keys only the backend writes (through their own commands): a UI copy of them is always stale
const BACKEND_OWNED: &[&str] = &["nodeRoles"];

/// Deep merge: objects key by key, everything else (arrays, strings, null) replaced.
fn merge(dst: &mut serde_json::Value, src: serde_json::Value) {
    match (dst, src) {
        (serde_json::Value::Object(d), serde_json::Value::Object(s)) => {
            for (k, v) in s {
                match d.get_mut(&k) {
                    Some(x) if x.is_object() && v.is_object() => merge(x, v),
                    _ => { d.insert(k, v); }
                }
            }
        }
        (d, s) => *d = s,
    }
}

/// Read-modify-write under the store lock: `patch` (whole or partial features object) is merged into the
/// current file, so two saves at once (several screens, roles_extra, the debounce) cannot wipe each other.
fn apply_patch(patch: serde_json::Value) -> Result<(), String> {
    let serde_json::Value::Object(mut obj) = patch else { return Err("неверные настройки: нужен объект".into()) };
    for k in VIEW_ONLY.iter().chain(BACKEND_OWNED) { obj.remove(*k); }
    let mut s = lock();
    let mut cur = serde_json::to_value(&s.f).map_err(|e| e.to_string())?;
    merge(&mut cur, serde_json::Value::Object(obj));
    let mut f: Features = serde_json::from_value(cur).map_err(|e| format!("неверные настройки: {e}"))?;
    f.webchat.pin = f.webchat.pin.trim().to_string();
    if f.webchat.pin.is_empty() { f.webchat.pin = gen_pin(); }
    f.telegram.token = f.telegram.token.trim().to_string();
    if f.lang != "en" { f.lang = "ru".into(); }
    s.f = f;
    let path = s.path.clone();
    save(&path, &s.f);
    Ok(())
}

/// Merges a whole or partial features object into the saved one (only the keys sent are changed).
#[tauri::command]
pub async fn features_set(features: serde_json::Value) -> Result<FeaturesView, String> {
    apply_patch(features)?;
    Ok(view())
}

/// Which backend should answer this prompt (ChatView / web chat): cluster model or a «Помощник».
#[tauri::command]
pub async fn route_target(state: State<'_, Arc<AppState>>, prompt: String, has_attachments: Option<bool>) -> Result<RouteTarget, String> {
    Ok(route(state.inner(), &prompt, has_attachments.unwrap_or(false)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simple() {
        assert!(is_simple("привет", false));
        assert!(!is_simple("привет", true));
        assert!(!is_simple("```rust\nfn main(){}\n```", false));
        assert!(!is_simple(&"a".repeat(250), false));
    }
    #[test]
    fn merge_partial() {
        let mut cur = serde_json::json!({"webchat":{"enabled":false,"pin":"111111"},"telegram":{"allowedIds":[1,2]},"nodeRoles":{"a":{"voice":true}}});
        merge(&mut cur, serde_json::json!({"webchat":{"enabled":true},"telegram":{"allowedIds":[3]}}));
        assert_eq!(cur["webchat"], serde_json::json!({"enabled":true,"pin":"111111"}));
        assert_eq!(cur["telegram"]["allowedIds"], serde_json::json!([3]));
        assert_eq!(cur["nodeRoles"]["a"]["voice"], true);
    }
    #[test]
    fn old_file_loads() {
        let f: Features = serde_json::from_str(r#"{"webchat":{"enabled":true},"queue":{"x":1}}"#).unwrap();
        assert!(f.webchat.enabled);
        assert_eq!(f.lang, "ru");
        assert!(f.extra.contains_key("queue"));
    }
}

/// Many chat templates (Mistral, Gemma, Llama 2…) throw "Conversation roles must alternate user/assistant".
/// Same rules as normalizeMessages() in the UI: one system message first, no empty turns, consecutive
/// user/assistant turns merged, first turn after system is a user turn. Tool turns are left as they are.
pub fn normalize_messages(v: &serde_json::Value) -> serde_json::Value { normalize_messages_ex(v, false) }

pub fn normalize_messages_ex(v: &serde_json::Value, fold_system: bool) -> serde_json::Value {
    use serde_json::{json, Value};
    let Some(arr) = v.as_array() else { return v.clone() };
    let empty = |c: &Value| c.is_null() || c.as_str().map_or(false, |s| s.trim().is_empty()) || c.as_array().map_or(false, |a| a.is_empty());
    let parts = |c: &Value| -> Vec<Value> { match c { Value::Array(a) => a.clone(), Value::String(s) => vec![json!({"type":"text","text":s})], o => vec![json!({"type":"text","text":o.to_string()})] } };
    let mut sys: Vec<String> = vec![];
    let mut out: Vec<Value> = vec![];
    for m in arr {
        let role = m["role"].as_str().unwrap_or("");
        if role == "system" {
            let t = m["content"].as_str().map(str::to_string).unwrap_or_else(|| m["content"].to_string());
            if !t.trim().is_empty() { sys.push(t); }
            continue;
        }
        let plain = (role == "user" || role == "assistant") && m.get("tool_calls").is_none() && m.get("tool_call_id").is_none();
        if plain && empty(&m["content"]) { continue; }
        if let Some(prev) = out.last_mut() {
            let prev_plain = prev.get("tool_calls").is_none() && prev.get("tool_call_id").is_none();
            if plain && prev_plain && prev["role"].as_str() == Some(role) {
                let merged = match (prev["content"].as_str(), m["content"].as_str()) {
                    (Some(a), Some(b)) => Value::String(format!("{a}\n\n{b}")),
                    _ => { let mut p = parts(&prev["content"]); p.extend(parts(&m["content"])); Value::Array(p) }
                };
                prev["content"] = merged;
                continue;
            }
        }
        out.push(m.clone());
    }
    while out.first().map_or(false, |m| m["role"].as_str() != Some("user")) { out.remove(0); }
    if !sys.is_empty() {
        let st = sys.join("\n\n");
        if fold_system {
            if let Some(first) = out.first_mut() {
                first["content"] = match first["content"].as_str() {
                    Some(b) => Value::String(format!("{st}\n\n{b}")),
                    None => { let mut p = vec![json!({"type":"text","text":st})]; p.extend(parts(&first["content"])); Value::Array(p) }
                };
            } else { out.push(json!({"role":"user","content": st})); }
        } else { out.insert(0, json!({"role":"system","content": st})); }
    }
    Value::Array(out)
}

/// Gemma-style templates have no system role. Ask the server which template it runs.
pub async fn template_lacks_system(base: &str) -> bool {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    let c = C.get_or_init(|| reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(2)).build().unwrap_or_default());
    let Ok(r) = c.get(format!("{}/props", base.trim_end_matches('/'))).send().await else { return false };
    let Ok(j) = r.json::<serde_json::Value>().await else { return false };
    let t = j["chat_template"].as_str().unwrap_or("");
    !t.is_empty() && (!t.contains("system") || t.contains("System role not supported") || t.contains("start_of_turn"))
}

/// normalize + fold system into the first user turn when the server's template needs it.
pub async fn prepare_messages(base: &str, v: &serde_json::Value) -> serde_json::Value {
    normalize_messages_ex(v, template_lacks_system(base).await)
}

#[cfg(test)]
mod normalize_tests {
    use serde_json::json;
    #[test]
    fn alternates() {
        let v = json!([{"role":"system","content":"a"},{"role":"assistant","content":"hi"},{"role":"user","content":"1"},
            {"role":"user","content":"2"},{"role":"assistant","content":""},{"role":"system","content":"b"},{"role":"user","content":"3"}]);
        let r = super::normalize_messages(&v);
        assert_eq!(r, json!([{"role":"system","content":"a\n\nb"},{"role":"user","content":"1\n\n2\n\n3"}]));
        let f = super::normalize_messages_ex(&v, true);
        assert_eq!(f, json!([{"role":"user","content":"a\n\nb\n\n1\n\n2\n\n3"}]));
    }
}
