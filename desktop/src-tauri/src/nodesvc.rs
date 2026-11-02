//! Extra services on USB phones (embeddings, whisper, kiwix), reached through `adb forward`.
//!
//! Port block per USB device: PC port = phone port + 100 + 10*slot. Slot 0 keeps the helper at 50162
//! (50062 + 100); a second phone gets 50172 for its helper, 50170/50171/50173 for its services, and so on.
//! The node app reports its services in `dumpsys activity service …/.RpcService` as
//! `argus-status {"roles":{"voice":{"state":"ready","port":50061},"library":{…},"embeddings":{…}}}`.

use crate::state::{AppState, Transport};
use serde::Serialize;
use std::{collections::HashMap, sync::{Arc, Mutex, OnceLock}, time::Duration};

pub const EMBED_PORT: u16 = 50060;
pub const WHISPER_PORT: u16 = 50061;
pub const KIWIX_PORT: u16 = 50063;
const NODE_PKG: &str = crate::cluster::NODE_PKG;
const MAX_SLOTS: u16 = 20;

/// (role in argus-status, service name in the UI, default phone port)
const SERVICES: &[(&str, &str, u16)] = &[
    ("embeddings", "embeddings", EMBED_PORT),
    ("voice", "whisper", WHISPER_PORT),
    ("library", "kiwix", KIWIX_PORT),
];

#[derive(Serialize, Clone, Debug)]
pub struct NodeService {
    pub device: String,
    pub service: String,
    pub pc_port: u16,
    pub state: String,
}

fn slots() -> &'static Mutex<Vec<String>> { static S: OnceLock<Mutex<Vec<String>>> = OnceLock::new(); S.get_or_init(Default::default) }
fn services() -> &'static Mutex<HashMap<(String, String), NodeService>> {
    static S: OnceLock<Mutex<HashMap<(String, String), NodeService>>> = OnceLock::new(); S.get_or_init(Default::default)
}

/// Stable slot of a USB device for this session (first seen = 0).
pub fn slot_of(serial: &str) -> u16 {
    let mut v = slots().lock().unwrap();
    if let Some(i) = v.iter().position(|s| s == serial) { return i as u16; }
    v.push(serial.to_string());
    ((v.len() - 1) as u16).min(MAX_SLOTS - 1)
}

/// PC port for a phone port of this device.
pub fn pc_port(serial: &str, phone_port: u16) -> u16 { phone_port + 100 + 10 * slot_of(serial) }

/// Base URL of a ready whisper server on any USB phone.
pub fn whisper_base() -> Option<String> {
    let m = services().lock().unwrap();
    let mut v: Vec<&NodeService> = m.values().filter(|s| s.service == "whisper").collect();
    v.sort_by_key(|s| (s.state != "ready", s.pc_port));
    v.first().map(|s| format!("http://127.0.0.1:{}", s.pc_port))
}

/// `argus-status {json}` → role → (state, port)
pub fn parse_status(out: &str) -> HashMap<String, (String, Option<u16>)> {
    let mut r = HashMap::new();
    let Some(line) = out.lines().map(str::trim).find(|l| l.starts_with("argus-status")) else { return r };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(line.trim_start_matches("argus-status").trim()) else { return r };
    if let Some(roles) = j.get("roles").and_then(|x| x.as_object()) {
        for (k, v) in roles {
            let state = v.get("state").and_then(|x| x.as_str()).unwrap_or("unknown").to_string();
            let port = v.get("port").and_then(|x| x.as_u64()).and_then(|p| u16::try_from(p).ok());
            r.insert(k.clone(), (state, port));
        }
    }
    r
}

async fn tick(st: &Arc<AppState>) {
    let usb: Vec<String> = st.nodes.lock().await.values()
        .filter(|n| n.transport == Transport::Usb && n.online()).map(|n| n.id.clone()).collect();
    // unplugged devices: drop their services (adb removes forwards itself on disconnect)
    services().lock().unwrap().retain(|(d, _), _| usb.contains(d));
    if usb.is_empty() { return; }
    let fw = st.adb.forward_list().await.unwrap_or_default();
    for serial in usb {
        let out = st.adb.shell(&serial, &format!("dumpsys activity service {NODE_PKG}/.RpcService")).await.unwrap_or_default();
        let roles = parse_status(&out);
        for &(role, name, def_port) in SERVICES {
            let key = (serial.clone(), name.to_string());
            let local = pc_port(&serial, def_port);
            match roles.get(role) {
                Some((state, port)) if !matches!(state.as_str(), "idle" | "off" | "disabled" | "stopped") => {
                    let remote = port.unwrap_or(def_port);
                    let mut state = state.clone();
                    if !fw.iter().any(|(s, l, r)| s == &serial && *l == local && *r == remote) {
                        if let Err(e) = st.adb.forward(&serial, local, remote).await {
                            log::warn!("nodesvc: forward {serial} {local}->{remote}: {e:#}");
                            state = "error".into();
                        }
                    }
                    services().lock().unwrap().insert(key, NodeService { device: serial.clone(), service: name.into(), pc_port: local, state });
                }
                _ => {
                    let had = services().lock().unwrap().remove(&key).is_some();
                    if had { let _ = st.adb.remove_forward(local).await; }
                }
            }
        }
    }
}

/// Background loop (spawned from main).
pub async fn run(st: Arc<AppState>) {
    loop {
        tick(&st).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tauri::command]
pub async fn nodesvc_list() -> Result<Vec<NodeService>, String> {
    let mut v: Vec<NodeService> = services().lock().unwrap().values().cloned().collect();
    v.sort_by(|a, b| (a.pc_port, &a.device).cmp(&(b.pc_port, &b.device)));
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_and_ports() {
        let r = parse_status("foo\n  argus-status {\"roles\":{\"voice\":{\"state\":\"ready\",\"port\":50061},\"library\":{\"state\":\"off\"}}}\n");
        assert_eq!(r.get("voice"), Some(&("ready".to_string(), Some(50061))));
        assert_eq!(r.get("library").map(|x| x.0.as_str()), Some("off"));
        assert_eq!(pc_port("A", 50062), 50162);
        assert_eq!(pc_port("B", 50061), 50171);
        assert_eq!(pc_port("A", 50063), 50163);
    }
}
