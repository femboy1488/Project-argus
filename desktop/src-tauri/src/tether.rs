//! «USB-модем»: per-device option to reach a USB phone through Android USB tethering instead of `adb forward`.
//!
//! With USB tethering on, the phone gets a network adapter towards the PC (rndis/ncm). Argus Node 0.3.3 reports
//! that interface's IPv4 as `usb_net_ip` in `dumpsys … RpcService` (`argus-status {json}`, Telemetry.usbNetIp()).
//! In its USB mode the node app binds rpc-server (and whisper/kiwix) to 127.0.0.1 only, so the PC switches the app
//! to its network mode with the extra the app already reads (`--ez wifi true`, RpcService.onStartCommand): that
//! binds 0.0.0.0:50052 (0.0.0.0 still covers the adb forward, which stays as the fallback). Argus remembers it made
//! the switch (settings.usb_tether_switched) and switches back (`--ez wifi false`) when the option is turned off.
//!
//! Every few seconds, per USB node with the option on:
//!  * no `usb_net_ip` → keep the adb forward, hint «Включи на телефоне: …»;
//!  * app in USB mode → switch it (only while the running model doesn't use this node: the switch restarts rpc-server);
//!  * TCP connect to <ip>:50052 works → node endpoint = <ip>:50052 (llama-server `--rpc`), link "tether";
//!  * not reachable (the Windows RNDIS adapter needs a moment after enabling) → forward, retried next tick.
//! A node used by the running model never changes endpoint under it (the engine would lose it) — except when the
//! tether endpoint it uses is dead, then the forward takes over and the supervisor restarts the model.
use crate::cluster::{DEVICE_RPC_PORT, NODE_PKG};
use crate::state::{AppState, NodeStage, Transport};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpStream, time::timeout};

pub const HINT_ENABLE: &str = "Включи на телефоне: Настройки → Подключения → Мобильная точка доступа → USB-модем";
const HINT_OLD_APP: &str = "Для USB-модема нужна Argus Node 0.3.3 или новее — обнови приложение на телефоне";
const HINT_SWITCHING: &str = "Переключаю телефон на USB-модем…";
const HINT_WAIT_ENGINE: &str = "Перейду на USB-модем, когда модель перезапустится или остановится";
const HINT_UNREACHABLE: &str = "USB-модем не отвечает. Включи на телефоне: Настройки → Подключения → Мобильная точка доступа → USB-модем";
const HINT_BACK_WAIT: &str = "Вернусь на обычный USB, когда модель перезапустится или остановится";
/// failed checks in a row before a tether endpoint in use is given up (one lost SYN must not kill a model)
const FAILS_TO_FALLBACK: u32 = 2;

/// What `argus-status {json}` says about the network side of the node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NetStatus {
    pub usb_net_ip: Option<String>,
    pub wifi_ip: Option<String>,
    /// rpc.mode: "usb" (rpc-server on 127.0.0.1) | "wifi" (0.0.0.0)
    pub mode: Option<String>,
}

/// `argus-status {…,"usb_net_ip":"192.168.42.129","wifi_ip":…,"rpc":{"mode":"wifi",…}}` (None: no such line = app older than 0.3.3)
pub fn parse_net_status(dump: &str) -> Option<NetStatus> {
    let line = dump.lines().find_map(|l| l.trim().strip_prefix("argus-status "))?;
    let j: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let ip = |k: &str| j.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|s| s.parse::<std::net::Ipv4Addr>().is_ok()).map(str::to_string);
    Some(NetStatus { usb_net_ip: ip("usb_net_ip"), wifi_ip: ip("wifi_ip"), mode: j["rpc"]["mode"].as_str().map(str::to_string) })
}

/// IPv4 addresses from `ip -4 -o addr show` (`3: wlan0    inet 192.168.1.5/24 brd … scope global wlan0`), loopback skipped.
pub fn parse_ips(out: &str) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    for l in out.lines() {
        let mut it = l.split_whitespace();
        while let Some(t) = it.next() {
            if t != "inet" { continue; }
            if let Some(a) = it.next().and_then(|a| a.split('/').next()) {
                if let Ok(ip) = a.parse::<std::net::Ipv4Addr>() {
                    if !ip.is_loopback() && !v.contains(&a.to_string()) { v.push(a.to_string()); }
                }
            }
        }
    }
    v
}

async fn reachable(ip: &str) -> bool {
    matches!(timeout(Duration::from_millis(900), TcpStream::connect((ip, DEVICE_RPC_PORT))).await, Ok(Ok(_)))
}

/// Host of a `host:port` endpoint.
pub fn ep_host(ep: &str) -> &str { ep.rsplit_once(':').map(|(h, _)| h).unwrap_or(ep) }

/// A Wi-Fi beacon from this address comes from a phone that is connected by USB (it is in the node app's network
/// mode because of «USB-модем»): not a new LAN node, and the PC must not answer it with LAN role assignments.
pub async fn owned_ip(st: &AppState, host: &str) -> bool {
    let nodes = st.nodes.lock().await;
    let rt = st.runtime.lock().await;
    rt.iter().any(|(id, r)| r.own_ips.iter().any(|ip| ip == host)
        && nodes.get(id).map(|n| n.transport == Transport::Usb).unwrap_or(false))
}

/// Endpoint the PC should use for a USB node right now + how it gets there.
async fn set_link(st: &AppState, serial: &str, ep: &str, link: &str, hint: Option<&str>) {
    {
        let mut rt = st.runtime.lock().await;
        let r = rt.entry(serial.to_string()).or_default();
        r.tether_ep = (link == "tether").then(|| ep.to_string());
        if link != "tether" { r.tether_fails = 0; }
    }
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        if n.stage != NodeStage::Connected { return; }
        if n.endpoint.as_deref() != Some(ep) { log::info!("{serial}: rpc endpoint {:?} -> {ep} ({link})", n.endpoint); }
        n.endpoint = Some(ep.to_string());
        n.link = link.to_string();
        n.tether_hint = hint.map(str::to_string);
    }
}

async fn set_hint(st: &AppState, serial: &str, hint: Option<&str>) {
    if let Some(n) = st.nodes.lock().await.get_mut(serial) { n.tether_hint = hint.map(str::to_string); }
}

/// `am start-foreground-service … --ez wifi <on>`: the node app's own mode switch (persisted on the phone).
async fn set_node_mode(st: &AppState, serial: &str, wifi: bool) -> Result<(), String> {
    let out = st.adb.shell(serial, &format!(
        "am start-foreground-service --user 0 -n {NODE_PKG}/.RpcService --ez wifi {wifi} --ei port {DEVICE_RPC_PORT}"
    )).await.map_err(|e| format!("{e:#}"))?;
    if out.contains("Error") { return Err(out.trim().to_string()); }
    log::info!("{serial}: node app switched to {} mode for «USB-модем»", if wifi { "network (0.0.0.0)" } else { "USB (127.0.0.1)" });
    Ok(())
}

/// Background loop body (every 4 s).
pub async fn tick(st: &Arc<AppState>) {
    let (on, switched) = { let s = st.settings.lock().await; (s.usb_tether.clone(), s.usb_tether_switched.clone()) };
    let usb: Vec<(String, Option<String>)> = st.nodes.lock().await.values()
        .filter(|n| n.transport == Transport::Usb && n.online()).map(|n| (n.id.clone(), n.endpoint.clone())).collect();
    let mut set = tokio::task::JoinSet::new();
    for (serial, ep) in usb {
        let (want, sw) = (on.contains(&serial), switched.contains(&serial));
        if !want && !sw && ep.as_deref().map(|e| e.starts_with("127.0.0.1:")).unwrap_or(true) { continue; } // plain USB node
        let st = st.clone();
        set.spawn(async move {
            if timeout(Duration::from_secs(8), tick_one(&st, &serial, want, sw, ep)).await.is_err() {
                log::warn!("{serial}: «USB-модем» check timed out");
            }
        });
    }
    while set.join_next().await.is_some() {}
}

async fn tick_one(st: &Arc<AppState>, serial: &str, want: bool, switched: bool, cur: Option<String>) {
    let Some(port) = st.ports.lock().await.get(serial).copied() else { return };
    let fwd = format!("127.0.0.1:{port}");
    let in_engine = match &cur { Some(e) => crate::llama::engine_uses_endpoint(st, e).await, None => false };

    if !want {
        // option off: back to the adb forward, then the node app back to its USB mode (if Argus switched it)
        if cur.as_deref() != Some(fwd.as_str()) {
            if in_engine { set_hint(st, serial, Some(HINT_BACK_WAIT)).await; return; }
            set_link(st, serial, &fwd, "usb", None).await;
        }
        if switched {
            match set_node_mode(st, serial, false).await {
                Ok(()) => {
                    st.settings.lock().await.usb_tether_switched.remove(serial);
                    st.save_settings().await;
                }
                Err(e) => log::warn!("{serial}: switch node app back to USB mode: {e}"),
            }
        }
        st.runtime.lock().await.entry(serial.to_string()).or_default().own_ips.clear();
        return;
    }

    let dump = st.adb.shell(serial, &format!("dumpsys activity service {NODE_PKG}/.RpcService")).await.unwrap_or_default();
    let status = parse_net_status(&dump);
    // the phone's own addresses, BEFORE the app goes to network mode and starts its Wi-Fi beacon
    let mut ips = parse_ips(&st.adb.shell(serial, "ip -4 -o addr show 2>/dev/null").await.unwrap_or_default());
    if let Some(s) = &status {
        for ip in [&s.usb_net_ip, &s.wifi_ip].into_iter().flatten() { if !ips.contains(ip) { ips.push(ip.clone()); } }
    }
    if !ips.is_empty() { st.runtime.lock().await.entry(serial.to_string()).or_default().own_ips = ips; }

    let Some(status) = status else { fallback(st, serial, &fwd, cur.as_deref(), in_engine, HINT_OLD_APP).await; return };
    let Some(ip) = status.usb_net_ip.clone() else { fallback(st, serial, &fwd, cur.as_deref(), in_engine, HINT_ENABLE).await; return };

    if status.mode.as_deref() != Some("wifi") {
        // USB mode: rpc-server listens on 127.0.0.1 only → the node app's network mode (restarts rpc-server)
        if in_engine { set_hint(st, serial, Some(HINT_WAIT_ENGINE)).await; return; }
        match set_node_mode(st, serial, true).await {
            Ok(()) => {
                st.settings.lock().await.usb_tether_switched.insert(serial.to_string());
                st.save_settings().await;
                set_hint(st, serial, Some(HINT_SWITCHING)).await;
            }
            Err(e) => { log::warn!("{serial}: switch node app to network mode: {e}"); fallback(st, serial, &fwd, cur.as_deref(), in_engine, HINT_ENABLE).await; }
        }
        return;
    }

    let tep = format!("{ip}:{DEVICE_RPC_PORT}");
    if reachable(&ip).await {
        st.runtime.lock().await.entry(serial.to_string()).or_default().tether_fails = 0;
        if cur.as_deref() == Some(tep.as_str()) { set_link(st, serial, &tep, "tether", None).await; }
        else if in_engine { set_hint(st, serial, Some(HINT_WAIT_ENGINE)).await; }
        else { set_link(st, serial, &tep, "tether", None).await; }
    } else {
        log::debug!("{serial}: {tep} not reachable (USB tethering adapter not ready?)");
        fallback(st, serial, &fwd, cur.as_deref(), in_engine, HINT_UNREACHABLE).await;
    }
}

/// Back to the adb forward with a hint. A tether endpoint the running model uses gets FAILS_TO_FALLBACK chances.
async fn fallback(st: &AppState, serial: &str, fwd: &str, cur: Option<&str>, in_engine: bool, hint: &str) {
    let on_tether = cur.map(|c| c != fwd).unwrap_or(false);
    if on_tether && in_engine {
        let fails = { let mut rt = st.runtime.lock().await; let r = rt.entry(serial.to_string()).or_default(); r.tether_fails += 1; r.tether_fails };
        if fails < FAILS_TO_FALLBACK { return; }
        log::warn!("{serial}: «USB-модем» lost while the model uses it — back to adb forward");
    }
    set_link(st, serial, fwd, "usb", Some(hint)).await;
}

/// Base URL of a phone service over «USB-модем» (None: the node isn't on tether right now).
pub async fn service_host(st: &AppState, serial: &str) -> Option<String> {
    let nodes = st.nodes.lock().await;
    let n = nodes.get(serial).filter(|n| n.link == "tether" && n.online())?;
    n.endpoint.as_deref().map(|e| ep_host(e).to_string())
}

/// «USB-модем» switch in the device inspector (USB nodes). Applied by the next tick (≤ 4 s).
#[tauri::command]
pub async fn set_usb_tether(state: tauri::State<'_, Arc<AppState>>, id: String, enabled: bool) -> Result<(), String> {
    if id.starts_with("lan:") { return Err("USB-модем — только для телефонов, подключённых кабелем".into()); }
    {
        let mut s = state.settings.lock().await;
        if enabled { s.usb_tether.insert(id.clone()); } else { s.usb_tether.remove(&id); }
    }
    state.save_settings().await;
    if let Some(n) = state.nodes.lock().await.get_mut(&id) {
        n.tether = enabled;
        n.tether_hint = enabled.then(|| "Проверяю USB-модем…".to_string());
    }
    let st = state.inner().clone();
    tauri::async_runtime::spawn(async move { tick(&st).await });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_and_ips() {
        let s = parse_net_status("x\n  argus-status {\"vn\":\"0.3.3\",\"usb_net_ip\":\"192.168.42.129\",\"rpc\":{\"state\":\"ready\",\"mode\":\"usb\"}}\n").unwrap();
        assert_eq!(s.usb_net_ip.as_deref(), Some("192.168.42.129"));
        assert_eq!(s.mode.as_deref(), Some("usb"));
        assert_eq!(s.wifi_ip, None);
        let s = parse_net_status("argus-status {\"wifi_ip\":\"192.168.1.7\",\"rpc\":{\"mode\":\"wifi\"}}").unwrap();
        assert_eq!((s.usb_net_ip, s.wifi_ip.as_deref(), s.mode.as_deref()), (None, Some("192.168.1.7"), Some("wifi")));
        assert!(parse_net_status("argus-rpc state=READY").is_none());
        let ips = parse_ips("1: lo    inet 127.0.0.1/8 scope host lo\n3: wlan0    inet 192.168.1.5/24 brd 192.168.1.255 scope global wlan0\n9: rndis0    inet 192.168.42.129/24 brd 192.168.42.255 scope global rndis0\n");
        assert_eq!(ips, vec!["192.168.1.5".to_string(), "192.168.42.129".to_string()]);
        assert_eq!(ep_host("192.168.42.129:50052"), "192.168.42.129");
    }
}
