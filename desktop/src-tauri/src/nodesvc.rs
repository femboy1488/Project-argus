//! Extra services on USB phones (embeddings, whisper, kiwix), reached through `adb forward`
//! (or the phone's USB-tethering IP when «USB-модем» is on, tether.rs).
//!
//! Port block per USB device: PC port = phone port + 100 + 10*slot. Slot 0 keeps the helper at 50162
//! (50062 + 100); a second phone gets 50172 for its helper, 50170/50171/50173 for its services, and so on.
//! The node app reports its services in `dumpsys activity service …/.RpcService` as
//! `argus-status {"roles":{"voice":{"state":"ready","port":50061},"library":{…},"embeddings":{…}}}`.
//!
//! Wi-Fi nodes: no forward. The same `roles` map arrives in the UDP beacon (beacon.rs `Discovered::roles`);
//! the PC talks to the phone's IP directly (http://<ip>:50061 whisper, :50063 kiwix) and checks each service
//! with a short HTTP request, so a role the phone calls "ready" but the PC cannot reach shows up as "unreachable".

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
    /// USB: forwarded port on the PC; Wi-Fi: the port on the phone itself
    pub pc_port: u16,
    pub state: String,
    /// base URL the PC uses for this service (USB: http://127.0.0.1:<pc_port>, Wi-Fi: http://<phone ip>:<port>)
    pub base: String,
    /// "usb" (adb forward) | "tether" (USB-модем: phone IP on the USB network) | "wifi"
    pub transport: String,
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

/// Base URLs of a service ("whisper" | "kiwix" | "embeddings") on all phones, ready ones first, USB before Wi-Fi.
pub fn bases(service: &str) -> Vec<String> {
    let m = services().lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<&NodeService> = m.values().filter(|s| s.service == service).collect();
    v.sort_by_key(|s| (s.state != "ready", s.transport == "wifi", s.pc_port, s.device.clone()));
    v.into_iter().map(|s| s.base.clone()).collect()
}

/// Base URLs of a service that answered its last check.
pub fn ready_bases(service: &str) -> Vec<String> {
    let m = services().lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<&NodeService> = m.values().filter(|s| s.service == service && s.state == "ready").collect();
    v.sort_by_key(|s| (s.transport == "wifi", s.pc_port, s.device.clone()));
    v.into_iter().map(|s| s.base.clone()).collect()
}

/// Base URL of a whisper server on any phone (USB or Wi-Fi), a ready one if there is one.
pub fn whisper_base() -> Option<String> { bases("whisper").into_iter().next() }

/// beacons come every 2 s; older roles info is not trusted
const BEACON_FRESH: Duration = Duration::from_secs(10);

fn http_client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().no_proxy().connect_timeout(Duration::from_millis(1500))
        .timeout(Duration::from_millis(2500)).build().unwrap_or_default())
}

/// any HTTP answer on GET / = the server is listening (same check the node app does on 127.0.0.1)
async fn http_alive(base: &str) -> bool {
    http_client().get(format!("{base}/")).send().await.is_ok()
}

fn inactive(state: &str) -> bool { matches!(state, "idle" | "off" | "disabled" | "stopped") }

/// Wi-Fi nodes: role status from the beacon (or the user's choice when the beacon says nothing), checked over HTTP.
async fn tick_lan(st: &Arc<AppState>) -> Vec<String> {
    let lan: Vec<(String, String, bool)> = st.nodes.lock().await.values()
        .filter(|n| n.transport == Transport::Lan)
        .filter_map(|n| {
            let ep = n.endpoint.as_deref()?;
            let host = ep.rsplit_once(':').map(|(h, _)| h).unwrap_or(ep).trim_matches(|c| c == '[' || c == ']').to_string();
            Some((n.id.clone(), host, n.online()))
        }).collect();
    let disc: HashMap<String, serde_json::Value> = st.discovered.lock().await.iter()
        .filter(|(_, d)| d.last_seen.elapsed() < BEACON_FRESH)
        .filter_map(|(k, d)| d.roles.clone().map(|r| (k.clone(), r))).collect();
    let mut seen = Vec::new();
    for (id, host, online) in lan {
        let roles = disc.get(&id);
        if roles.is_none() && !online && !st.discovered.lock().await.get(&id).map(|d| d.last_seen.elapsed() < BEACON_FRESH).unwrap_or(false) {
            // node gone: forget its services
            services().lock().unwrap_or_else(|e| e.into_inner()).retain(|(d, _), _| d != &id);
            continue;
        }
        seen.push(id.clone());
        let (want_voice, want_library) = crate::roles_extra::wanted(&id);
        for &(role, name, def_port) in SERVICES {
            let key = (id.clone(), name.to_string());
            let r = roles.and_then(|r| r.get(role)).filter(|v| v.is_object());
            let reported = r.and_then(|v| v["state"].as_str()).map(str::to_string);
            let port = r.and_then(|v| v["port"].as_u64()).and_then(|p| u16::try_from(p).ok()).unwrap_or(def_port);
            let wanted = match role { "voice" => want_voice, "library" => want_library, _ => false };
            let active = reported.as_deref().map(|s| !inactive(s)).unwrap_or(false);
            // no beacon roles (older node / beacon lost): probe only what the user turned on here
            if !active && !(reported.is_none() && wanted) {
                services().lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                continue;
            }
            let base = format!("http://{host}:{port}");
            let state = if http_alive(&base).await { "ready".to_string() } else {
                match reported.as_deref() {
                    Some("ready") => "unreachable".to_string(),
                    Some(s) => s.to_string(),
                    None => { services().lock().unwrap_or_else(|e| e.into_inner()).remove(&key); continue; }
                }
            };
            services().lock().unwrap_or_else(|e| e.into_inner()).insert(key, NodeService {
                device: id.clone(), service: name.into(), pc_port: port, state, base, transport: "wifi".into(),
            });
        }
    }
    seen
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
    let lan = tick_lan(st).await;
    let usb: Vec<String> = st.nodes.lock().await.values()
        .filter(|n| n.transport == Transport::Usb && n.online()).map(|n| n.id.clone()).collect();
    // unplugged devices / removed Wi-Fi nodes: drop their services (adb removes forwards itself on disconnect)
    services().lock().unwrap_or_else(|e| e.into_inner()).retain(|(d, _), _| usb.contains(d) || lan.contains(d));
    if usb.is_empty() { return; }
    let fw = st.adb.forward_list().await.unwrap_or_default();
    for serial in usb {
        let out = st.adb.shell(&serial, &format!("dumpsys activity service {NODE_PKG}/.RpcService")).await.unwrap_or_default();
        let roles = parse_status(&out);
        // «USB-модем»: the node app is in its network mode (whisper on 0.0.0.0, kiwix on all IPv4) → phone IP directly
        let tether = crate::tether::service_host(st, &serial).await;
        for &(role, name, def_port) in SERVICES {
            let key = (serial.clone(), name.to_string());
            let local = pc_port(&serial, def_port);
            match roles.get(role) {
                Some((state, port)) if !inactive(state) => {
                    let remote = port.unwrap_or(def_port);
                    if let Some(host) = &tether {
                        let base = format!("http://{host}:{remote}");
                        if http_alive(&base).await {
                            services().lock().unwrap_or_else(|e| e.into_inner()).insert(key, NodeService {
                                device: serial.clone(), service: name.into(), pc_port: remote, state: "ready".into(), base, transport: "tether".into(),
                            });
                            continue;
                        } // not answering over the USB network yet: the adb forward below
                    }
                    let mut state = state.clone();
                    if !fw.iter().any(|(s, l, r)| s == &serial && *l == local && *r == remote) {
                        if let Err(e) = st.adb.forward(&serial, local, remote).await {
                            log::warn!("nodesvc: forward {serial} {local}->{remote}: {e:#}");
                            state = "error".into();
                        }
                    }
                    services().lock().unwrap_or_else(|e| e.into_inner()).insert(key, NodeService {
                        device: serial.clone(), service: name.into(), pc_port: local, state,
                        base: format!("http://127.0.0.1:{local}"), transport: "usb".into(),
                    });
                }
                _ => {
                    let had = services().lock().unwrap_or_else(|e| e.into_inner()).remove(&key).is_some();
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
    let mut v: Vec<NodeService> = services().lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
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
