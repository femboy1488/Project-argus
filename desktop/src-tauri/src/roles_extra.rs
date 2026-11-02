//! Node roles «Голос» (whisper-server) and «Библиотека» (kiwix-serve), Argus Node 0.3.3+ (docs/ROLES.md «Протокол ролей ноды»).
//!
//! The user's choice per node lives in argus-features.json → `nodeRoles` ({id: {voice, voiceModel, library}}).
//! * Голос: the ggml whisper model is downloaded once to the PC (helpers_dir, same downloader/progress as «Помощник»),
//!   served on :50055 `/helper/<file>`; USB nodes get it through `adb reverse`, Wi-Fi nodes through the beacon reply.
//! * Библиотека: the .zim is chosen on the phone (Argus Node → «Выбрать .zim»); the PC only turns the role on/off.
//! USB: `am start-foreground-service … --es role voice|library …` / `--es role_stop voice|library`, re-sent at most once a
//! minute while the node does not report the role as running. Wi-Fi: `"voice"`/`"library"` keys in the beacon reply.
//! nodesvc.rs forwards the phone's 50061/50063 to the PC once the node reports them; voice.rs then finds whisper itself.
use crate::state::{AppState, Transport};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock}, time::{Duration, Instant}};
use tauri::State;

const NODE_PKG: &str = crate::cluster::NODE_PKG;
const WHISPER_REPO: &str = "ggerganov/whisper.cpp";
const RESEND: Duration = Duration::from_secs(60);
const FEATURE_KEY: &str = "nodeRoles";

/// (id, label, approx. size MB)
pub const VOICE_MODELS: &[(&str, &str, u64)] = &[
    ("tiny", "tiny · 75 МБ · самый быстрый", 75),
    ("base", "base · 142 МБ · баланс", 142),
    ("small", "small · 466 МБ · точнее, медленнее", 466),
];
pub const DEFAULT_VOICE: &str = "base";

pub fn voice_file(model: &str) -> Option<String> {
    VOICE_MODELS.iter().find(|m| m.0 == model).map(|m| format!("ggml-{}.bin", m.0))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeRolePref {
    pub voice: bool,
    /// "tiny" | "base" | "small"
    pub voice_model: String,
    pub library: bool,
}

fn prefs() -> HashMap<String, NodeRolePref> {
    crate::features::get().extra.get(FEATURE_KEY).cloned()
        .and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

fn save_pref(id: &str, op: impl FnOnce(&mut NodeRolePref)) {
    crate::features::update(|f| {
        let mut all: HashMap<String, NodeRolePref> = f.extra.get(FEATURE_KEY).cloned()
            .and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        let p = all.entry(id.to_string()).or_default();
        op(p);
        if p.voice_model.is_empty() { p.voice_model = DEFAULT_VOICE.into(); }
        if let Ok(v) = serde_json::to_value(&all) { f.extra.insert(FEATURE_KEY.into(), v); }
    });
}

/// last command per (node, role): (signature, when)
fn sent() -> &'static Mutex<HashMap<(String, &'static str), (String, Instant)>> {
    static S: OnceLock<Mutex<HashMap<(String, &'static str), (String, Instant)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}
/// what the node said last (USB `argus-status` roles map) + a note from the PC side
fn status() -> &'static Mutex<HashMap<String, (serde_json::Value, String)>> {
    static S: OnceLock<Mutex<HashMap<String, (serde_json::Value, String)>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn due(id: &str, role: &'static str, sig: &str) -> bool {
    match sent().lock().unwrap_or_else(|e| e.into_inner()).get(&(id.to_string(), role)) {
        Some((s, t)) => s != sig || t.elapsed() > RESEND,
        None => true,
    }
}
fn mark(id: &str, role: &'static str, sig: &str) {
    sent().lock().unwrap_or_else(|e| e.into_inner()).insert((id.to_string(), role), (sig.to_string(), Instant::now()));
}

/// `argus-status {json}` → its `roles` object (None: no such line = Argus Node older than 0.3.3)
fn parse_roles(out: &str) -> Option<serde_json::Value> {
    let line = out.lines().map(str::trim).find(|l| l.starts_with("argus-status"))?;
    let j: serde_json::Value = serde_json::from_str(line.trim_start_matches("argus-status").trim()).ok()?;
    Some(j.get("roles").cloned().unwrap_or(serde_json::Value::Null))
}

fn role_state(roles: &serde_json::Value, role: &str) -> (String, String) {
    let r = &roles[role];
    (r["state"].as_str().unwrap_or("idle").to_string(),
     r["model"].as_str().or(r["zim"].as_str()).unwrap_or("").to_string())
}

const ACTIVE: &[&str] = &["downloading", "starting", "loading", "ready"];

async fn am(st: &AppState, serial: &str, extras: &str) -> Result<(), String> {
    let out = st.adb.shell(serial, &format!("am start-foreground-service --user 0 -n {NODE_PKG}/.RpcService {extras}")).await
        .map_err(|e| format!("{e:#}"))?;
    if out.contains("Error") { return Err(out.trim().to_string()); }
    Ok(())
}

async fn tick(st: &Arc<AppState>) {
    let all = prefs();
    if all.is_empty() { return; }
    // 1. whisper models on the PC (USB and Wi-Fi nodes both fetch them from :50055)
    let mut ready_files: HashMap<String, u64> = HashMap::new();
    for p in all.values().filter(|p| p.voice) {
        let Some(file) = voice_file(&p.voice_model) else { continue };
        if ready_files.contains_key(&file) { continue; }
        if let Ok(path) = crate::helper::ensure_pc_model(st, WHISPER_REPO, &file, None).await {
            ready_files.insert(file, std::fs::metadata(path).map(|m| m.len()).unwrap_or(0));
        }
    }
    // 2. USB nodes: send the assignment
    let usb: Vec<String> = st.nodes.lock().await.values()
        .filter(|n| n.transport == Transport::Usb && n.online()).map(|n| n.id.clone()).collect();
    for (id, p) in &all {
        if !usb.contains(id) { continue; }
        let dump = st.adb.shell(id, &format!("dumpsys activity service {NODE_PKG}/.RpcService")).await.unwrap_or_default();
        let Some(roles) = parse_roles(&dump) else {
            let note = if p.voice || p.library { "Для ролей «Голос» и «Библиотека» нужна Argus Node 0.3.3 — обновите ноду" } else { "" };
            status().lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), (serde_json::Value::Null, note.into()));
            continue;
        };
        let mut note = String::new();

        let (vs, vm) = role_state(&roles, "voice");
        if p.voice {
            match voice_file(&p.voice_model) {
                Some(file) => match ready_files.get(&file) {
                    Some(&size) => {
                        let running = vm == file && ACTIVE.contains(&vs.as_str());
                        let sig = format!("{file}|{size}");
                        if !running && due(id, "voice", &sig) {
                            let port = crate::updater::NODE_UPDATE_PORT;
                            let rev = st.adb.run(&["-s", id, "reverse", &format!("tcp:{port}"), &format!("tcp:{port}")]).await;
                            let r = match rev {
                                Err(e) => Err(format!("adb reverse: {e:#}")),
                                Ok(_) => am(st, id, &format!("--es role voice --es voice_model {file} --es voice_url http://127.0.0.1:{port}/helper/{file} --el voice_size {size}")).await,
                            };
                            mark(id, "voice", &sig);
                            if let Err(e) = r { note = format!("Голос: {e}"); }
                        }
                    }
                    None => note = "Голос: модель ещё качается на ПК".into(),
                },
                None => note = "Голос: неизвестная модель".into(),
            }
        } else if ACTIVE.contains(&vs.as_str()) && due(id, "voice", "stop") {
            if let Err(e) = am(st, id, "--es role_stop voice").await { note = format!("Голос: {e}"); }
            mark(id, "voice", "stop");
        }

        let (ls, _) = role_state(&roles, "library");
        if p.library {
            if !ACTIVE.contains(&ls.as_str()) && due(id, "library", "on") {
                if let Err(e) = am(st, id, "--es role library").await { note = format!("Библиотека: {e}"); }
                mark(id, "library", "on");
            }
        } else if ACTIVE.contains(&ls.as_str()) && due(id, "library", "stop") {
            if let Err(e) = am(st, id, "--es role_stop library").await { note = format!("Библиотека: {e}"); }
            mark(id, "library", "stop");
        }
        status().lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone(), (roles, note));
    }
}

/// Background loop (spawned from main).
pub async fn run(st: Arc<AppState>) {
    loop {
        tick(&st).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Adds `"voice"` / `"library"` to a beacon reply for a Wi-Fi node (only for nodes the user configured here).
pub fn extend_lan_reply(msg: Vec<u8>, node_id: &str) -> Vec<u8> {
    let Some(p) = prefs().get(node_id).cloned() else { return msg };
    let Ok(mut j) = serde_json::from_slice::<serde_json::Value>(&msg) else { return msg };
    if p.voice {
        if let Some(file) = voice_file(&p.voice_model) {
            if let Ok(m) = std::fs::metadata(crate::helper::helpers_dir().join(&file)) {
                if m.len() > 0 {
                    j["voice"] = serde_json::json!({ "file": file, "path": format!("/helper/{file}"), "size": m.len(), "threads": 0 });
                }
            } // not on the PC yet: no key = no change
        }
    } else {
        j["voice"] = serde_json::Value::Bool(false);
    }
    j["library"] = if p.library { serde_json::json!({}) } else { serde_json::Value::Bool(false) };
    j.to_string().into_bytes()
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModelView { pub id: String, pub label: String, pub size_mb: u64 }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRolesView {
    pub voice: bool,
    pub voice_model: String,
    pub library: bool,
    pub models: Vec<VoiceModelView>,
    /// PC-side download of the chosen whisper model: None = on disk / not needed
    pub download_pct: Option<u32>,
    pub download_error: Option<String>,
    pub model_on_pc: bool,
    /// node report (USB): {voice:{state,pct,model,error}, library:{state,zim,error,import_pct}}; null if unknown
    pub node: serde_json::Value,
    pub note: String,
}

#[tauri::command]
pub async fn node_roles_get(state: State<'_, Arc<AppState>>, id: String) -> Result<NodeRolesView, String> {
    let p = prefs().get(&id).cloned().unwrap_or_default();
    let model = if p.voice_model.is_empty() { DEFAULT_VOICE.to_string() } else { p.voice_model.clone() };
    let file = voice_file(&model).unwrap_or_default();
    let on_pc = std::fs::metadata(crate::helper::helpers_dir().join(&file)).map(|m| m.len() > 0).unwrap_or(false);
    let (mut pct, mut err) = (None, None);
    if !on_pc {
        if let Some(d) = state.helper_dl.lock().await.get(&file) {
            err = d.error.clone();
            if d.running { pct = Some(if d.total > 0 { (d.done * 100 / d.total) as u32 } else { 0 }); }
        }
    }
    let (node, note) = status().lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().unwrap_or((serde_json::Value::Null, String::new()));
    Ok(NodeRolesView {
        voice: p.voice, voice_model: model, library: p.library,
        models: VOICE_MODELS.iter().map(|m| VoiceModelView { id: m.0.into(), label: m.1.into(), size_mb: m.2 }).collect(),
        download_pct: pct, download_error: err, model_on_pc: on_pc, node, note,
    })
}

#[tauri::command]
pub async fn set_node_voice(state: State<'_, Arc<AppState>>, id: String, enabled: bool, model: Option<String>) -> Result<(), String> {
    let model = model.map(|m| m.trim().to_lowercase()).filter(|m| !m.is_empty());
    if let Some(m) = &model { if voice_file(m).is_none() { return Err("Модель голоса: tiny, base или small".into()); } }
    save_pref(&id, |p| { p.voice = enabled; if let Some(m) = model { p.voice_model = m; } });
    if enabled {
        // voice.rs: an empty URL means «whisper on a phone» (nodesvc::whisper_base)
        crate::features::update(|f| f.voice.url.clear());
        // a failed earlier download should not block a new try
        state.helper_dl.lock().await.retain(|_, d| d.running);
    }
    sent().lock().unwrap_or_else(|e| e.into_inner()).remove(&(id, "voice"));
    Ok(())
}

#[tauri::command]
pub async fn set_node_library(id: String, enabled: bool) -> Result<(), String> {
    save_pref(&id, |p| p.library = enabled);
    sent().lock().unwrap_or_else(|e| e.into_inner()).remove(&(id, "library"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn files_and_status() {
        assert_eq!(voice_file("base").as_deref(), Some("ggml-base.bin"));
        assert!(voice_file("large").is_none());
        assert!(crate::helper::safe_file("ggml-small.bin"));
        let r = parse_roles("x\n argus-status {\"roles\":{\"voice\":{\"state\":\"ready\",\"model\":\"ggml-base.bin\"},\"library\":{\"state\":\"idle\"}}}").unwrap();
        assert_eq!(role_state(&r, "voice"), ("ready".to_string(), "ggml-base.bin".to_string()));
        assert_eq!(role_state(&r, "library").0, "idle");
        assert!(parse_roles("argus-helper state=idle").is_none());
    }
}
