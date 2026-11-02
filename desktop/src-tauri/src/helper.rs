//! Experiment «Роли» (0.3.2): the «Помощник» role.
//!
//! A free node runs its OWN llama-server (static aarch64 build shipped in the node APK as libllama_server.so)
//! with a small model, next to the main model. Argus uses it in parallel: chat titles, «Сводка», «Параллельный тест».
//!
//! Flow (one `tick` every 3 s, only while the experiment is on):
//!  1. the helper model is downloaded ONCE on the PC (Hugging Face → <models>/helpers/<file>), with progress;
//!  2. the PC serves it on :50055 (`/helper/<file>`, updater.rs) — USB nodes reach it through `adb reverse tcp:50055`,
//!     Wi-Fi nodes through the PC's LAN address;
//!  3. USB: `am start-foreground-service … --es helper_model <file> --es helper_url http://127.0.0.1:50055/helper/<file>`;
//!     the node app downloads the file into its private storage, starts llama-server on 127.0.0.1:50062, and reports
//!     progress in `dumpsys activity service …/.RpcService` (line `argus-helper …`). PC side: `adb forward tcp:5016N tcp:50062`.
//!     Wi-Fi: the assignment rides in the unicast reply to the node's beacon (lan.rs), status comes back in the beacon;
//!  4. ready = GET /health → 200. Role removed / experiment off ⇒ the helper is stopped (USB: `--ez helper_stop true`,
//!     Wi-Fi: `"helper": false` in the reply).
//! Nothing here installs or updates an app: the user picked the role and the model, the node keeps asking about app updates.
use crate::split::Role;
use crate::state::{AppState, Transport};
use serde::Serialize;
use std::{collections::{HashMap, HashSet}, path::PathBuf, sync::{Arc, OnceLock}, time::{Duration, Instant}};
use tauri::{AppHandle, Emitter, State};

/// port llama-server listens on, on the node
pub const HELPER_PORT: u16 = 50062;
/// first PC port for `adb forward` to a USB helper
const FIRST_FWD_PORT: u16 = 50162;
const NODE_PKG: &str = crate::cluster::NODE_PKG;

pub struct HelperModel { pub id: &'static str, pub label: &'static str, pub repo: &'static str, pub file: &'static str, pub size: u64 }

/// Q4_0: llama.cpp repacks it for arm64 dotprod/i8mm at load time — the fastest 4-bit format on phone CPUs.
pub const HELPER_MODELS: &[HelperModel] = &[
    HelperModel { id: "qwen3-0.6b", label: "Qwen3 0.6B · 0.4 ГБ · быстрый", repo: "unsloth/Qwen3-0.6B-GGUF", file: "Qwen3-0.6B-Q4_0.gguf", size: 382_156_480 },
    HelperModel { id: "qwen3-1.7b", label: "Qwen3 1.7B · 1.1 ГБ · умнее", repo: "unsloth/Qwen3-1.7B-GGUF", file: "Qwen3-1.7B-Q4_0.gguf", size: 1_056_782_912 },
    HelperModel { id: "qwen3-4b-2507", label: "Qwen3 4B Instruct 2507 · 2.4 ГБ · для 8+ ГБ ОЗУ", repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF", file: "Qwen3-4B-Instruct-2507-Q4_0.gguf", size: 2_375_773_280 },
];
pub const DEFAULT_MODEL: &str = "qwen3-0.6b";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperModelView { pub id: String, pub label: String, pub size_mb: u64 }

pub fn model_views() -> Vec<HelperModelView> {
    HELPER_MODELS.iter().map(|m| HelperModelView { id: m.id.into(), label: m.label.into(), size_mb: m.size >> 20 }).collect()
}

/// settings value → id shown in the UI ("" ⇒ default; a custom "owner/repo/file.gguf" stays as is)
pub fn model_id(s: &str) -> &str { if s.trim().is_empty() { DEFAULT_MODEL } else { s.trim() } }

/// (repo, file, label, expected size) of the chosen model. Custom: "owner/repo/File-Q4_0.gguf".
pub fn resolve(s: &str) -> Option<(String, String, String, Option<u64>)> {
    let id = model_id(s);
    if let Some(m) = HELPER_MODELS.iter().find(|m| m.id == id) {
        return Some((m.repo.into(), m.file.into(), m.label.split(" · ").next().unwrap_or(m.id).into(), Some(m.size)));
    }
    let parts: Vec<&str> = id.trim_start_matches("https://huggingface.co/").split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 && parts.last()?.to_lowercase().ends_with(".gguf") {
        let file = parts.last()?.to_string();
        if !safe_file(&file) { return None; }
        return Some((format!("{}/{}", parts[0], parts[1]), file.clone(), file.trim_end_matches(".gguf").into(), None));
    }
    None
}

/// only plain file names are served / pushed (.gguf helper models, .bin whisper models — roles_extra.rs)
pub fn safe_file(f: &str) -> bool {
    !f.is_empty() && f.len() < 160 && (f.to_lowercase().ends_with(".gguf") || f.to_lowercase().ends_with(".bin"))
        && f.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && !f.contains("..")
}

pub fn helpers_dir() -> PathBuf { crate::llama::model_cache_dir().join("helpers") }

#[derive(Default, Clone)]
pub struct Download { pub done: u64, pub total: u64, pub error: Option<String>, pub running: bool }

#[derive(Default)]
pub struct HelperRt {
    /// "prepare" | "pc-download" | "node-download" | "loading" | "ready" | "error"
    pub state: String,
    pub detail: String,
    pub pct: Option<u32>,
    pub model: String,
    /// base URL the PC talks to (http://127.0.0.1:5016N or http://<wifi ip>:50062)
    pub url: Option<String>,
    pub local_port: Option<u16>,
    /// what we last asked the node to run and when
    pub cmd_sig: String,
    pub last_cmd: Option<Instant>,
    /// requests in flight (pick the least busy helper)
    pub busy: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperView {
    pub id: String,
    pub name: String,
    pub model: String,
    pub state: String,
    pub detail: String,
    pub pct: Option<u32>,
    pub ready: bool,
}

impl HelperRt {
    pub fn view(&self, id: &str, name: &str) -> HelperView {
        HelperView { id: id.into(), name: name.into(), model: self.model.clone(), state: self.state.clone(),
                     detail: self.detail.clone(), pct: self.pct, ready: self.state == "ready" }
    }
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_secs(3))
        .user_agent(concat!("Argus-Cluster/", env!("CARGO_PKG_VERSION"))).build().unwrap_or_default())
}

/// 200 ⇒ Some(true) (ready), 503 ⇒ Some(false) (loading), no answer ⇒ None
async fn health(base: &str) -> Option<bool> {
    let r = http().get(format!("{base}/health")).timeout(Duration::from_millis(1500)).send().await.ok()?;
    Some(r.status().is_success())
}

// ---------------------------------------------------------------- PC-side model download

/// Starts (or reports) the download of the helper model to the PC. Returns the path once complete.
pub(crate) async fn ensure_pc_model(st: &Arc<AppState>, repo: &str, file: &str, size: Option<u64>) -> Result<PathBuf, Download> {
    let path = helpers_dir().join(file);
    if let Ok(m) = std::fs::metadata(&path) {
        if m.len() > 0 && size.map(|s| s == m.len()).unwrap_or(true) { return Ok(path); }
    }
    let mut dl = st.helper_dl.lock().await;
    let d = dl.entry(file.to_string()).or_default();
    if d.running || d.error.is_some() { return Err(d.clone()); }
    d.running = true; d.done = 0; d.total = size.unwrap_or(0);
    let snapshot = d.clone();
    drop(dl);
    let (st2, repo, file2) = (st.clone(), repo.to_string(), file.to_string());
    tokio::spawn(async move {
        let r = download(&st2, &repo, &file2).await;
        let mut dl = st2.helper_dl.lock().await;
        let d = dl.entry(file2.clone()).or_default();
        d.running = false;
        if let Err(e) = r { log::warn!("helper model {file2}: {e:#}"); d.error = Some(format!("{e:#}")); }
    });
    Err(snapshot)
}

async fn download(st: &Arc<AppState>, repo: &str, file: &str) -> anyhow::Result<()> {
    let dir = helpers_dir();
    std::fs::create_dir_all(&dir)?;
    let url = format!("https://huggingface.co/{repo}/resolve/main/{file}?download=true");
    let mut r = http().get(&url).send().await?;
    if !r.status().is_success() { anyhow::bail!("Hugging Face ответил {} для {repo}/{file}", r.status()); }
    let total = r.content_length().unwrap_or(0);
    let part = dir.join(format!("{file}.part"));
    let mut f = tokio::fs::File::create(&part).await?;
    let mut done = 0u64;
    let mut last = Instant::now();
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = r.chunk().await? {
        f.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if last.elapsed() > Duration::from_millis(500) {
            last = Instant::now();
            if let Some(d) = st.helper_dl.lock().await.get_mut(file) { d.done = done; d.total = total; }
        }
    }
    f.flush().await?;
    drop(f);
    if total > 0 && done != total { anyhow::bail!("загрузка оборвалась ({done} из {total} байт)"); }
    std::fs::rename(&part, dir.join(file))?;
    Ok(())
}

// ---------------------------------------------------------------- node side

/// `dumpsys activity service com.harkcluster.node/.RpcService` → our `argus-helper k=v …` line
fn parse_dump(out: &str) -> Option<HashMap<String, String>> {
    let line = out.lines().find(|l| l.trim_start().starts_with("argus-helper"))?;
    let mut m = HashMap::new();
    for kv in line.trim().trim_start_matches("argus-helper").split_whitespace() {
        if let Some((k, v)) = kv.split_once('=') { m.insert(k.to_string(), v.replace("%20", " ")); }
    }
    Some(m)
}

async fn usb_stop(st: &AppState, serial: &str, port: Option<u16>) {
    let _ = st.adb.shell(serial, &format!("am start-foreground-service --user 0 -n {NODE_PKG}/.RpcService --ez helper_stop true")).await;
    if let Some(p) = port { let _ = st.adb.remove_forward(p).await; }
}

fn alloc_port(rt: &HashMap<String, HelperRt>) -> u16 {
    let used: HashSet<u16> = rt.values().filter_map(|r| r.local_port).collect();
    (FIRST_FWD_PORT..FIRST_FWD_PORT + 200).find(|p| !used.contains(p)).unwrap_or(FIRST_FWD_PORT)
}

/// What a Wi-Fi node should run (sent in the beacon reply): `{"file","path","size"}` or `false` (stop).
pub async fn lan_assignment(st: &AppState, node_id: &str) -> serde_json::Value {
    let s = st.settings.lock().await;
    let is_helper = s.roles_enabled && s.roles.get(node_id).map(|r| r == "helper").unwrap_or(false);
    let model = s.helper_model.clone();
    drop(s);
    if !is_helper { return serde_json::Value::Bool(false); }
    let Some((_, file, _, _)) = resolve(&model) else { return serde_json::Value::Bool(false) };
    match std::fs::metadata(helpers_dir().join(&file)) {
        Ok(m) if m.len() > 0 => serde_json::json!({ "file": file, "path": format!("/helper/{file}"), "size": m.len(), "port": HELPER_PORT }),
        _ => serde_json::Value::Null, // not on the PC yet: keep whatever runs, nothing new
    }
}

pub async fn tick(app: &AppHandle, st: &Arc<AppState>) {
    let s = st.settings.lock().await.clone();
    let nodes: Vec<crate::state::Node> = st.nodes.lock().await.values().cloned().collect();
    let wanted: Vec<&crate::state::Node> = if s.roles_enabled {
        nodes.iter().filter(|n| s.roles.get(&n.id).map(|r| r == "helper").unwrap_or(false)).collect()
    } else { vec![] };

    // role removed / experiment off / node gone → stop
    let stale: Vec<(String, Option<u16>)> = {
        let rt = st.helpers.lock().await;
        rt.iter().filter(|(id, _)| !wanted.iter().any(|n| &n.id == *id)).map(|(id, r)| (id.clone(), r.local_port)).collect()
    };
    for (id, port) in stale {
        if !id.starts_with("lan:") && nodes.iter().any(|n| n.id == id && n.online()) { usb_stop(st, &id, port).await; }
        else if let Some(p) = port { let _ = st.adb.remove_forward(p).await; }
        st.helpers.lock().await.remove(&id);
    }
    if wanted.is_empty() { return; }

    let Some((repo, file, label, size)) = resolve(&s.helper_model) else {
        for n in &wanted { set(st, &n.id, "error", "Неверное имя модели Помощника (нужно owner/repo/файл.gguf)", None, "").await; }
        return;
    };
    // 1. the model on the PC
    let pc_file = match ensure_pc_model(st, &repo, &file, size).await {
        Ok(p) => p,
        Err(d) => {
            let (state, detail, pct) = match &d.error {
                Some(e) => ("error", format!("Не удалось скачать модель на ПК: {e}"), None),
                None => ("pc-download", "Качаю модель на ПК".to_string(), (d.total > 0).then(|| (d.done * 100 / d.total) as u32)),
            };
            for n in &wanted { set(st, &n.id, state, &detail, pct, &label).await; }
            return;
        }
    };
    let pc_size = std::fs::metadata(&pc_file).map(|m| m.len()).unwrap_or(0);

    for n in wanted {
        if !n.online() {
            set(st, &n.id, "prepare", "Нода не в сети", None, &label).await;
            continue;
        }
        if n.transport == Transport::Lan { lan_status(st, n, &label).await; continue; }
        if n.transport != Transport::Usb { continue; }
        usb_tick(app, st, &n.id, &file, pc_size, &label).await;
    }
}

async fn set(st: &AppState, id: &str, state: &str, detail: &str, pct: Option<u32>, model: &str) {
    let mut rt = st.helpers.lock().await;
    let r = rt.entry(id.to_string()).or_default();
    r.state = state.into(); r.detail = detail.into(); r.pct = pct;
    if !model.is_empty() { r.model = model.into(); }
}

async fn usb_tick(app: &AppHandle, st: &Arc<AppState>, serial: &str, file: &str, size: u64, label: &str) {
    let port = {
        let mut rt = st.helpers.lock().await;
        let p = rt.get(serial).and_then(|r| r.local_port).unwrap_or_else(|| {
            // per-device port block (nodesvc): slot 0 → 50162, slot 1 → 50172 …; fall back if taken
            let want = crate::nodesvc::pc_port(serial, HELPER_PORT);
            if rt.values().any(|r| r.local_port == Some(want)) { alloc_port(&rt) } else { want }
        });
        let r = rt.entry(serial.to_string()).or_default();
        r.local_port = Some(p);
        r.model = label.into();
        p
    };
    let base = format!("http://127.0.0.1:{port}");
    // forward PC:<port> → node:50062 (cheap to repeat; adb drops forwards on a USB reset)
    let fw = st.adb.forward_list().await.unwrap_or_default();
    if !fw.iter().any(|(s, l, r)| s == serial && *l == port && *r == HELPER_PORT) {
        if let Err(e) = st.adb.forward(serial, port, HELPER_PORT).await {
            set(st, serial, "error", &format!("adb forward: {e:#}"), None, label).await;
            return;
        }
    }
    match health(&base).await {
        Some(true) => {
            let was = st.helpers.lock().await.get(serial).map(|r| r.state.clone()).unwrap_or_default();
            { let mut rt = st.helpers.lock().await; if let Some(r) = rt.get_mut(serial) { r.url = Some(base.clone()); } }
            set(st, serial, "ready", "Готов", None, label).await;
            if was != "ready" {
                let name = st.nodes.lock().await.get(serial).map(|n| n.name.clone()).unwrap_or_default();
                crate::cluster::toast(app, "ok", &format!("Помощник на «{name}» готов ({label})"));
            }
            return;
        }
        Some(false) => {
            { let mut rt = st.helpers.lock().await; if let Some(r) = rt.get_mut(serial) { r.url = Some(base.clone()); } }
            set(st, serial, "loading", "Загружает модель в память", None, label).await;
            return;
        }
        None => {}
    }
    // not serving: what does the node say?
    let dump = st.adb.shell(serial, &format!("dumpsys activity service {NODE_PKG}/.RpcService")).await.unwrap_or_default();
    let info = parse_dump(&dump);
    let sig = format!("{file}|{size}");
    let (last_sig, last_cmd) = { let rt = st.helpers.lock().await; rt.get(serial).map(|r| (r.cmd_sig.clone(), r.last_cmd)).unwrap_or_default() };
    let node_state = info.as_ref().and_then(|m| m.get("state").cloned()).unwrap_or_default();
    let node_model = info.as_ref().and_then(|m| m.get("model").cloned()).unwrap_or_default();
    if info.is_none() {
        // node app older than 0.3.2: no helper engine inside
        set(st, serial, "error", "На ноде старая версия Argus Node — для Помощника нужна 0.3.2 (обновите ноду)", None, label).await;
        return;
    }
    match node_state.as_str() {
        "downloading" if node_model == file => {
            let pct = info.as_ref().and_then(|m| m.get("pct")).and_then(|p| p.parse().ok());
            set(st, serial, "node-download", "Передаю модель на ноду", pct, label).await;
            return;
        }
        "starting" | "loading" if node_model == file => {
            set(st, serial, "loading", "Запускаю llama-server на ноде", None, label).await;
            return;
        }
        "error" if node_model == file && last_sig == sig && last_cmd.map(|t| t.elapsed() < Duration::from_secs(60)).unwrap_or(false) => {
            let err = info.as_ref().and_then(|m| m.get("error").cloned()).unwrap_or_else(|| "ошибка на ноде".into());
            set(st, serial, "error", &format!("Нода: {err}"), None, label).await;
            return;
        }
        _ => {}
    }
    // (re)send the command: new model, or nothing happened for 60 s
    if last_sig == sig && last_cmd.map(|t| t.elapsed() < Duration::from_secs(60)).unwrap_or(false) {
        set(st, serial, "prepare", "Жду ответа ноды", None, label).await;
        return;
    }
    // the node fetches the file from the PC over the cable: device 127.0.0.1:50055 → PC :50055
    let port_s = crate::updater::NODE_UPDATE_PORT.to_string();
    if let Err(e) = st.adb.run(&["-s", serial, "reverse", &format!("tcp:{port_s}"), &format!("tcp:{port_s}")]).await {
        set(st, serial, "error", &format!("adb reverse: {e:#}"), None, label).await;
        return;
    }
    let url = format!("http://127.0.0.1:{port_s}/helper/{file}");
    let mut cmd = format!("am start-foreground-service --user 0 -n {NODE_PKG}/.RpcService --es helper_model {file} --es helper_url {url} --el helper_size {size}");
    // node also holds model layers: the helper gets fewer threads
    if let Some(t) = st.helper_threads(serial).await { cmd.push_str(&format!(" --ei helper_threads {t}")); }
    match st.adb.shell(serial, &cmd).await {
        Ok(out) if !out.contains("Error") => {
            let mut rt = st.helpers.lock().await;
            let r = rt.entry(serial.to_string()).or_default();
            r.cmd_sig = sig; r.last_cmd = Some(Instant::now());
            r.state = "prepare".into(); r.detail = "Команда отправлена".into();
        }
        Ok(out) => set(st, serial, "error", out.trim(), None, label).await,
        Err(e) => set(st, serial, "error", &format!("{e:#}"), None, label).await,
    }
}

/// Wi-Fi helper: assignment goes out with the beacon reply; here we only read the beacon status + /health.
async fn lan_status(st: &AppState, n: &crate::state::Node, label: &str) {
    let d = st.discovered.lock().await.get(&n.id).cloned();
    let Some(d) = d else {
        set(st, &n.id, "error", "Помощник по сети работает только с нодами в «Wi-Fi режиме» Argus Node 0.3.2", None, label).await;
        return;
    };
    let base = format!("http://{}:{}", d.host, HELPER_PORT);
    if let Some(ok) = health(&base).await {
        { let mut rt = st.helpers.lock().await; rt.entry(n.id.clone()).or_default().url = Some(base); }
        if ok { set(st, &n.id, "ready", "Готов", None, label).await; } else { set(st, &n.id, "loading", "Загружает модель в память", None, label).await; }
        return;
    }
    match d.helper.as_ref() {
        Some(h) if h.state == "downloading" => set(st, &n.id, "node-download", "Передаю модель на ноду", h.pct, label).await,
        Some(h) if h.state == "error" => set(st, &n.id, "error", &format!("Нода: {}", h.error.clone().unwrap_or_default()), None, label).await,
        Some(h) if h.state == "starting" || h.state == "loading" => set(st, &n.id, "loading", "Запускаю llama-server на ноде", None, label).await,
        Some(_) => set(st, &n.id, "prepare", "Жду ответа ноды (до 10 с)", None, label).await,
        None => set(st, &n.id, "error", "На ноде старая версия Argus Node — для Помощника нужна 0.3.2", None, label).await,
    }
}

// ---------------------------------------------------------------- chat through a helper

fn cancelled() -> &'static std::sync::Mutex<HashSet<String>> {
    static C: OnceLock<std::sync::Mutex<HashSet<String>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelperReply {
    pub content: String,
    pub node: String,
    pub node_name: String,
    pub model: String,
    pub tps: Option<f64>,
    pub tokens: Option<u64>,
    pub ms: u64,
}

/// Removes `<think>…</think>` (Qwen3) from the text.
pub fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(a) = out.find("<think>") {
        match out[a..].find("</think>") {
            Some(b) => out.replace_range(a..a + b + "</think>".len(), ""),
            None => { out.truncate(a); break; }
        }
    }
    out.trim().to_string()
}

/// Chat with a «Помощник». `node` = a specific helper (Параллельный тест) or None = the least busy ready one.
/// With `stream_id`, pieces of the answer are emitted as `helper-stream` events {streamId, delta}.
#[tauri::command]
pub async fn helper_chat(app: AppHandle, state: State<'_, Arc<AppState>>, node: Option<String>, messages: serde_json::Value,
                         max_tokens: Option<u32>, stream_id: Option<String>, temperature: Option<f32>) -> Result<HelperReply, String> {
    let st = state.inner().clone();
    if !st.settings.lock().await.roles_enabled { return Err("Эксперимент «Роли» выключен".into()); }
    let (id, base, model) = {
        let mut rt = st.helpers.lock().await;
        let pick = match &node {
            Some(id) => rt.get(id).filter(|r| r.state == "ready" && r.url.is_some()).map(|_| id.clone()),
            None => rt.iter().filter(|(_, r)| r.state == "ready" && r.url.is_some()).min_by_key(|(_, r)| r.busy).map(|(id, _)| id.clone()),
        };
        let Some(id) = pick else { return Err("Нет готового Помощника".into()) };
        let r = rt.get_mut(&id).unwrap();
        r.busy += 1;
        (id, r.url.clone().unwrap_or_default(), r.model.clone())
    };
    let node_name = st.nodes.lock().await.get(&id).map(|n| n.name.clone()).unwrap_or_else(|| id.clone());
    let r = chat(&app, &base, messages, max_tokens, stream_id.as_deref(), temperature).await;
    if let Some(r) = st.helpers.lock().await.get_mut(&id) { r.busy = r.busy.saturating_sub(1); }
    if let Some(s) = &stream_id { cancelled().lock().unwrap_or_else(|e| e.into_inner()).remove(s); }
    let (content, tps, tokens, ms) = r.map_err(|e| format!("Помощник «{node_name}»: {e:#}"))?;
    Ok(HelperReply { content: strip_think(&content), node: id, node_name, model, tps, tokens, ms })
}

/// Stops a streaming helper request (Параллельный тест «Стоп»).
#[tauri::command]
pub async fn helper_cancel(stream_id: String) -> Result<(), String> {
    cancelled().lock().unwrap_or_else(|e| e.into_inner()).insert(stream_id);
    Ok(())
}

async fn chat(app: &AppHandle, base: &str, messages: serde_json::Value, max_tokens: Option<u32>, stream_id: Option<&str>,
              temperature: Option<f32>) -> anyhow::Result<(String, Option<f64>, Option<u64>, u64)> {
    let t0 = Instant::now();
    let body = serde_json::json!({
        "messages": crate::features::normalize_messages(&messages), "stream": true, "max_tokens": max_tokens.unwrap_or(512),
        "temperature": temperature.unwrap_or(0.4),
        // Qwen3 small models: no thinking, answer straight away
        "chat_template_kwargs": { "enable_thinking": false },
    });
    let mut r = http().post(format!("{base}/v1/chat/completions")).json(&body).timeout(Duration::from_secs(600)).send().await?;
    if !r.status().is_success() {
        let code = r.status();
        anyhow::bail!("HTTP {code}: {}", r.text().await.unwrap_or_default().chars().take(300).collect::<String>());
    }
    let mut buf = String::new();
    let mut content = String::new();
    let (mut tps, mut tokens) = (None, None);
    while let Some(chunk) = r.chunk().await? {
        if let Some(s) = stream_id {
            if cancelled().lock().unwrap_or_else(|e| e.into_inner()).contains(s) { break; }
        }
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(i) = buf.find('\n') {
            let line: String = buf.drain(..=i).collect();
            let l = line.trim();
            let Some(data) = l.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" { break; }
            let Ok(j) = serde_json::from_str::<serde_json::Value>(data) else { continue };
            if let Some(d) = j["choices"][0]["delta"]["content"].as_str() {
                content.push_str(d);
                if let Some(s) = stream_id { let _ = app.emit("helper-stream", serde_json::json!({ "streamId": s, "delta": d })); }
            }
            if let Some(t) = j.get("timings") {
                tps = t["predicted_per_second"].as_f64().or(tps);
                tokens = t["predicted_n"].as_u64().or(tokens);
            }
        }
    }
    Ok((content, tps, tokens, t0.elapsed().as_millis() as u64))
}

// ---------------------------------------------------------------- settings commands

#[tauri::command]
pub async fn set_node_role(state: State<'_, Arc<AppState>>, id: String, role: String) -> Result<(), String> {
    let r = Role::parse(Some(role.as_str()));
    {
        let mut s = state.settings.lock().await;
        if r == Role::Helper && !s.roles_enabled { return Err("Роль «Помощник» доступна, когда включён эксперимент «Роли»".into()); }
        if r == Role::Auto { s.roles.remove(&id); } else { s.roles.insert(id, r.as_str().into()); }
    }
    state.save_settings().await;
    Ok(())
}

#[tauri::command]
pub async fn set_roles_enabled(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.settings.lock().await.roles_enabled = enabled;
    state.save_settings().await;
    Ok(())
}

#[tauri::command]
pub async fn set_helper_model(state: State<'_, Arc<AppState>>, model: String) -> Result<(), String> {
    let m = model.trim().to_string();
    if resolve(&m).is_none() { return Err("Укажите модель из списка или путь вида owner/repo/Файл-Q4_0.gguf".into()); }
    {
        let mut s = state.settings.lock().await;
        s.helper_model = if m == DEFAULT_MODEL { String::new() } else { m };
    }
    // a failed download of the previous choice should not block a retry
    state.helper_dl.lock().await.retain(|_, d| d.running);
    state.save_settings().await;
    Ok(())
}

/// «Повторить» after a failed helper download/start.
#[tauri::command]
pub async fn retry_helper(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.helper_dl.lock().await.retain(|_, d| d.running);
    if let Some(r) = state.helpers.lock().await.get_mut(&id) { r.cmd_sig.clear(); r.last_cmd = None; r.state = "prepare".into(); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn models() {
        assert_eq!(resolve("").unwrap().1, "Qwen3-0.6B-Q4_0.gguf");
        assert_eq!(resolve("qwen3-1.7b").unwrap().0, "unsloth/Qwen3-1.7B-GGUF");
        let c = resolve("bartowski/gemma-2-2b-it-GGUF/gemma-2-2b-it-Q4_0.gguf").unwrap();
        assert_eq!((c.0.as_str(), c.1.as_str(), c.3), ("bartowski/gemma-2-2b-it-GGUF", "gemma-2-2b-it-Q4_0.gguf", None));
        assert!(resolve("nonsense").is_none());
        assert!(!safe_file("../x.gguf"));
        assert!(!safe_file("a b.gguf"));
        assert!(safe_file("Qwen3-0.6B-Q4_0.gguf"));
    }
    #[test]
    fn dump_and_think() {
        let d = "SERVICE com.harkcluster.node/.RpcService 1234 pid=99\n  argus-helper state=downloading pct=35 model=Qwen3-0.6B-Q4_0.gguf error=-\n";
        let m = parse_dump(d).unwrap();
        assert_eq!((m["state"].as_str(), m["pct"].as_str()), ("downloading", "35"));
        assert_eq!(strip_think("<think>hm</think>\n\nЗаголовок"), "Заголовок");
        assert_eq!(strip_think("ответ <think>незакрыт"), "ответ");
    }
}
