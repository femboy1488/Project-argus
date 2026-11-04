//! Network (LAN) nodes: any machine running `rpc-server -H 0.0.0.0 -p 50052` from llama.cpp b12008
//! (Raspberry Pi, old laptop, second PC). Health + memory come from a minimal ggml-rpc client:
//! HELLO -> DEVICE_COUNT -> GET_DEVICE_MEMORY, then disconnect.
//! rpc-server serves ONE client at a time, so while llama-server is attached we only check that
//! the port accepts TCP and keep the last known numbers.
//!
//! 0.3: Wi-Fi nodes announce themselves with a UDP broadcast beacon on :50053 every 2 s
//! (Argus Node "Wi-Fi режим", scripts/argus-node-linux.sh for Linux PCs / Raspberry Pi).
//! They show up under "Найдено в сети"; added beacon nodes go OFFLINE 10 s after the last beacon,
//! and the engine supervisor treats that exactly like a USB unplug.
//!
//! 0.3.1: every beacon source gets a unicast reply (≤ 1 per 10 s per node)
//! `{"argus":"host","v":1,"update_port":50055,"node_vc":N}` so the node app learns the PC address and can
//! offer (ASK) an update from http://<pc>:50055/node.apk. Beacons carry "vn"/"vc" ⇒ «Нода устарела».
use crate::settings::{parse_host_port, LanNode};
use crate::beacon::{host_reply, parse_beacon, Discovered};
use crate::state::{AppState, Node, NodeStage};
use anyhow::{bail, Result};
use std::{collections::HashMap, sync::{Arc, OnceLock}, time::Duration};
use tauri::{AppHandle, State};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpStream, UdpSocket}, time::{sleep, timeout}};

const RPC_PROTO_MAJOR: u8 = 8; // ggml/include/ggml-rpc.h @ b12008
const CMD_GET_DEVICE_MEMORY: u8 = 11;
const CMD_HELLO: u8 = 14;
const CMD_DEVICE_COUNT: u8 = 15;
const CONN_CAPS: usize = 24;
/// Pi/laptop is usually dedicated to the job, but leave room for the OS.
fn usable_lan_mb(free_mb: u64) -> u64 { free_mb * 80 / 100 }

#[derive(Debug, Clone)]
pub struct RpcInfo { pub version: (u8, u8, u8), pub mem: Vec<(u64, u64)> } // (free, total) bytes

async fn send(s: &mut TcpStream, cmd: u8, payload: &[u8]) -> Result<()> {
    let mut b = Vec::with_capacity(9 + payload.len());
    b.push(cmd);
    b.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    b.extend_from_slice(payload);
    s.write_all(&b).await?;
    Ok(())
}

async fn recv(s: &mut TcpStream, expect: usize) -> Result<Vec<u8>> {
    let mut sz = [0u8; 8];
    s.read_exact(&mut sz).await?;
    let n = u64::from_le_bytes(sz) as usize;
    if n != expect { bail!("unexpected RPC reply size {n} (expected {expect})"); }
    let mut v = vec![0u8; n];
    s.read_exact(&mut v).await?;
    Ok(v)
}

pub async fn rpc_query(endpoint: &str) -> Result<RpcInfo> {
    let mut s = timeout(Duration::from_millis(1500), TcpStream::connect(endpoint)).await
        .map_err(|_| anyhow::anyhow!("таймаут подключения"))??;
    s.set_nodelay(true).ok();
    let io = async {
        send(&mut s, CMD_HELLO, &[0u8; CONN_CAPS]).await?;
        let h = recv(&mut s, 4 + CONN_CAPS).await?;
        let version = (h[0], h[1], h[2]);
        if version.0 != RPC_PROTO_MAJOR {
            bail!("rpc-server протокол v{}.{}.{} — нужен v{RPC_PROTO_MAJOR}.x (llama.cpp b12008)", h[0], h[1], h[2]);
        }
        send(&mut s, CMD_DEVICE_COUNT, &[]).await?;
        let c = recv(&mut s, 4).await?;
        let count = u32::from_le_bytes([c[0], c[1], c[2], c[3]]).min(16);
        let mut mem = vec![];
        for d in 0..count {
            send(&mut s, CMD_GET_DEVICE_MEMORY, &d.to_le_bytes()).await?;
            let m = recv(&mut s, 16).await?;
            mem.push((u64::from_le_bytes(m[0..8].try_into().unwrap()), u64::from_le_bytes(m[8..16].try_into().unwrap())));
        }
        Ok::<_, anyhow::Error>(RpcInfo { version, mem })
    };
    // a busy server (another client attached) never answers HELLO
    timeout(Duration::from_millis(2500), io).await.map_err(|_| anyhow::anyhow!("rpc-server занят другим клиентом или не отвечает"))?
}

async fn tcp_alive(endpoint: &str) -> bool {
    matches!(timeout(Duration::from_millis(1500), TcpStream::connect(endpoint)).await, Ok(Ok(_)))
}

// ---------------------------------------------------------------- Wi-Fi beacons

/// UDP port the nodes broadcast to (Argus Node "Wi-Fi режим", scripts/argus-node-linux.sh).
pub const BEACON_PORT: u16 = 50053;
/// no beacon for this long => node OFFLINE
const BEACON_LOST: Duration = Duration::from_secs(10);
/// "Найдено в сети" list forgets a silent node after this
const DISCOVERED_TTL: Duration = Duration::from_secs(60);
/// consecutive failed checks before a LAN node without beacon goes OFFLINE (Wi-Fi hiccups)
const FAILS_TO_OFFLINE: u32 = 2;

/// Listens on 0.0.0.0:50053 forever; re-binds if the socket dies or the port is taken.
pub fn start_beacon_listener(app: AppHandle, st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match UdpSocket::bind(("0.0.0.0", BEACON_PORT)).await {
                Ok(sock) => {
                    st.beacon_error.lock().await.clear();
                    let mut buf = [0u8; 8192]; // 0.3.3 beacons carry helper + roles + telemetry
                    loop {
                        let (n, src) = match sock.recv_from(&mut buf).await {
                            Ok(x) => x,
                            Err(e) => {
                                // Windows reports ICMP "port unreachable" as WSAECONNRESET on UDP: not fatal
                                if e.kind() == std::io::ErrorKind::ConnectionReset { continue; }
                                log::warn!("beacon recv: {e}"); break;
                            }
                        };
                        let Some(mut d) = parse_beacon(&buf[..n], src.ip()) else { continue };
                        d.src_port = src.port();
                        // «USB-модем» puts a cabled phone into the app's network mode, so it beacons: that is the
                        // USB node itself — no second (LAN) node and no LAN reply (its helper:false would stop USB roles)
                        if crate::tether::owned_ip(&st, &d.host).await { continue; }
                        if reply_due(&d.host) {
                            let helper = crate::helper::lan_assignment(&st, &d.id()).await;
                            let msg = host_reply(crate::updater::NODE_UPDATE_PORT, crate::cluster::bundled_code(&st.bundled_node_version), false, Some(helper));
                            let msg = crate::roles_extra::extend_lan_reply(msg, &d.id());
                            if let Err(e) = sock.send_to(&msg, src).await { log::debug!("beacon reply {src}: {e}"); }
                        }
                        on_beacon(&app, &st, d).await;
                    }
                }
                Err(e) => {
                    *st.beacon_error.lock().await = format!("UDP-порт {BEACON_PORT} занят другой программой ({e}) — поиск нод в сети выключен");
                    log::warn!("beacon bind: {e}");
                }
            }
            sleep(Duration::from_secs(10)).await;
        }
    });
}

/// at most one host reply per node address every 10 s
fn reply_due(host: &str) -> bool {
    static LAST: OnceLock<std::sync::Mutex<HashMap<String, std::time::Instant>>> = OnceLock::new();
    let mut m = LAST.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if m.get(host).map(|t| now.duration_since(*t) < Duration::from_secs(10)).unwrap_or(false) { return false; }
    m.insert(host.to_string(), now);
    true
}

/// beacon "vn"/"vc" → node row (version shown, «Нода устарела» when older than the bundled APK)
fn apply_app_version(n: &mut Node, d: &Discovered, bundled: &str) {
    n.app_vc = d.vc;
    n.app_version = d.vn.clone();
    let bvc = crate::cluster::bundled_code(bundled);
    n.update_available = match (d.vc, bvc) { (Some(a), Some(b)) if a < b => Some(bundled.to_string()), _ => None };
}

async fn on_beacon(app: &AppHandle, st: &Arc<AppState>, d: Discovered) {
    let id = d.id();
    let is_new = st.discovered.lock().await.insert(id.clone(), d.clone()).is_none();
    // keep the node row's memory fresh even while the engine holds the rpc-server
    if let Some(n) = st.nodes.lock().await.get_mut(&id) {
        n.beacon = true;
        n.variant = d.variant.clone();
        if d.bench.is_some() { n.bench_gbps = d.bench; }
        apply_app_version(n, &d, &st.bundled_node_version);
        if n.stage == NodeStage::Connected && d.free_mb > 0 { n.available_mb = d.free_mb; }
    }
    if !is_new { return; }
    let auto = st.settings.lock().await.auto_add_discovered;
    let known = st.settings.lock().await.lan_nodes.iter().any(|l| l.id() == id);
    if auto && !known {
        if add_node(st, LanNode { host: d.host.clone(), port: d.port, name: d.name.clone(), auto: true }).await.is_ok() {
            crate::cluster::toast(app, "info", &format!("Добавлена Wi-Fi нода «{}» ({})", d.name, d.endpoint()));
        }
    } else if !known {
        crate::cluster::toast(app, "info", &format!("Найдена нода в сети: «{}» ({}) — Cluster → «Найдено в сети»", d.name, d.endpoint()));
    }
}

fn fail_counts() -> &'static std::sync::Mutex<HashMap<String, u32>> {
    static F: OnceLock<std::sync::Mutex<HashMap<String, u32>>> = OnceLock::new();
    F.get_or_init(Default::default)
}

enum Check { Online { info: Option<RpcInfo>, rtt: Option<f32> }, Down(String), Fatal(String) }

/// Health loop body (every 2 s): all LAN nodes concurrently.
/// * a node the running engine is attached to only gets a TCP connect (rpc-server serves ONE client);
/// * a beacon node goes OFFLINE 10 s after its last beacon;
/// * other failures need 2 checks in a row (Wi-Fi hiccups must not kill a running model).
pub async fn refresh(st: &Arc<AppState>) {
    let list = st.settings.lock().await.lan_nodes.clone();
    {
        let mut nodes = st.nodes.lock().await;
        let ids: Vec<String> = list.iter().map(|l| l.id()).collect();
        nodes.retain(|k, _| !k.starts_with("lan:") || ids.contains(k));
        for l in &list {
            nodes.entry(l.id()).or_insert_with(|| Node::lan(&l.id(), &display_name(l), &l.endpoint()));
        }
    }
    let disc: HashMap<String, Discovered> = {
        let mut d = st.discovered.lock().await;
        d.retain(|_, x| x.last_seen.elapsed() < DISCOVERED_TTL);
        d.clone()
    };
    let attached = crate::llama::engine_endpoints(st).await;
    let mut set = tokio::task::JoinSet::new();
    for l in list {
        let id = l.id();
        let ep = l.endpoint();
        let beacon = disc.get(&id).cloned();
        // beacon-tracked only while we actually hear it (seen in the last 60 s): a firewall that drops
        // UDP or a busy :50053 must not leave an auto-added node OFFLINE forever — TCP/RPC checks take over
        let tracked = beacon.is_some();
        let is_attached = attached.contains(&ep);
        set.spawn(async move {
            if tracked && beacon.as_ref().map(|b| b.last_seen.elapsed() > BEACON_LOST).unwrap_or(false) {
                return (id, beacon, Check::Fatal("Wi-Fi маяк пропал (> 10 с): нода выключена или вне сети".into()));
            }
            let t0 = std::time::Instant::now();
            let r = if is_attached {
                if tcp_alive(&ep).await { Check::Online { info: None, rtt: None } } else { Check::Down("нет TCP-ответа".into()) }
            } else {
                match rpc_query(&ep).await {
                    // HELLO + DEVICE_COUNT + GET_DEVICE_MEMORY (+ connect) ≈ 4 round trips
                    Ok(info) if !info.mem.is_empty() => Check::Online { rtt: Some(t0.elapsed().as_secs_f32() * 1000.0 / (3 + info.mem.len()) as f32), info: Some(info) },
                    Ok(_) => Check::Fatal("rpc-server не сообщил ни одного устройства".into()),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg.contains("протокол") { Check::Fatal(msg) } else { Check::Down(msg) }
                    }
                }
            };
            (id, beacon, r)
        });
    }
    while let Some(r) = set.join_next().await {
        let Ok((id, beacon, check)) = r else { continue };
        let fails = {
            let mut f = fail_counts().lock().unwrap_or_else(|e| e.into_inner());
            let c = f.entry(id.clone()).or_insert(0);
            if matches!(check, Check::Online { .. }) { *c = 0 } else { *c += 1 }
            *c
        };
        let mut nodes = st.nodes.lock().await;
        let Some(n) = nodes.get_mut(&id) else { continue };
        n.beacon = beacon.is_some();
        if let Some(b) = &beacon {
            n.variant = b.variant.clone();
            if b.bench.is_some() { n.bench_gbps = b.bench; }
            apply_app_version(n, b, &st.bundled_node_version);
        }
        match check {
            Check::Online { info: Some(info), rtt } => {
                if let Some(r) = rtt { n.rtt_ms = Some(match n.rtt_ms { Some(old) => old * 0.7 + r * 0.3, None => r }); }
                let free: u64 = info.mem.iter().map(|m| m.0).sum::<u64>() / 1024 / 1024;
                let total: u64 = info.mem.iter().map(|m| m.1).sum::<u64>() / 1024 / 1024;
                n.total_mb = total;
                n.available_mb = free;
                n.usable_mb = usable_lan_mb(free);
                n.rpc_devices = info.mem.len() as u32;
                n.version = Some(format!("rpc v{}.{}.{}", info.version.0, info.version.1, info.version.2));
                n.stage = NodeStage::Connected;
                n.error = None;
            }
            Check::Online { info: None, .. } => {
                // attached: keep the numbers from before; refresh free memory from the beacon if any
                if let Some(b) = &beacon { if b.free_mb > 0 { n.available_mb = b.free_mb; } }
                if n.total_mb == 0 { if let Some(b) = &beacon { n.total_mb = b.ram_mb; n.usable_mb = usable_lan_mb(b.free_mb); } }
                n.stage = NodeStage::Connected;
                n.error = None;
            }
            Check::Fatal(msg) => {
                n.stage = if msg.contains("протокол") || msg.contains("устройства") { NodeStage::Error } else { NodeStage::Offline };
                n.error = Some(msg);
            }
            Check::Down(msg) => {
                if fails >= FAILS_TO_OFFLINE || n.stage != NodeStage::Connected {
                    n.stage = NodeStage::Offline;
                    n.error = Some(msg);
                }
            }
        }
    }
}

fn display_name(l: &LanNode) -> String {
    if l.name.trim().is_empty() { l.host.clone() } else { l.name.trim().to_string() }
}

async fn add_node(st: &Arc<AppState>, node: LanNode) -> Result<String, String> {
    let id = node.id();
    {
        let mut s = st.settings.lock().await;
        if s.lan_nodes.iter().any(|n| n.id() == id) { return Err("эта нода уже добавлена".into()); }
        s.lan_nodes.push(node);
    }
    st.save_settings().await;
    Ok(id)
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn add_lan_node(state: State<'_, Arc<AppState>>, address: String, name: Option<String>) -> Result<String, String> {
    let (host, port) = parse_host_port(&address)?;
    let st = state.inner().clone();
    let id = add_node(&st, LanNode { host, port, name: name.unwrap_or_default(), auto: false }).await?;
    refresh(&st).await;
    Ok(id)
}

/// "Добавить" in the "Найдено в сети" list.
#[tauri::command]
pub async fn add_discovered_node(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    let st = state.inner().clone();
    let d = st.discovered.lock().await.get(&id).cloned().ok_or("нода больше не видна в сети")?;
    let id = add_node(&st, LanNode { host: d.host.clone(), port: d.port, name: d.name.clone(), auto: true }).await?;
    refresh(&st).await;
    Ok(id)
}

#[tauri::command]
pub async fn set_auto_add(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.settings.lock().await.auto_add_discovered = enabled;
    state.save_settings().await;
    Ok(())
}

#[tauri::command]
pub async fn remove_lan_node(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.settings.lock().await.lan_nodes.retain(|n| n.id() != id);
    state.nodes.lock().await.remove(&id);
    fail_counts().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    state.save_settings().await;
    Ok(())
}

/// Probe without adding (the "Проверить" button in the dialog).
#[tauri::command]
pub async fn test_lan_node(address: String) -> Result<String, String> {
    let (host, port) = parse_host_port(&address)?;
    let ep = format!("{host}:{port}");
    let info = rpc_query(&ep).await.map_err(|e| format!("{e:#}"))?;
    let free: u64 = info.mem.iter().map(|m| m.0).sum::<u64>() / 1024 / 1024;
    let total: u64 = info.mem.iter().map(|m| m.1).sum::<u64>() / 1024 / 1024;
    Ok(format!("rpc v{}.{}.{} · {} устр. · {:.1} / {:.1} GB свободно",
        info.version.0, info.version.1, info.version.2, info.mem.len(), free as f64 / 1024.0, total as f64 / 1024.0))
}

#[tauri::command]
pub async fn set_node_weight(state: State<'_, Arc<AppState>>, id: String, weight: f32) -> Result<(), String> {
    let w = weight.clamp(0.1, 8.0);
    state.settings.lock().await.weights.insert(id, w);
    state.save_settings().await;
    Ok(())
}

#[tauri::command]
pub async fn set_node_enabled(state: State<'_, Arc<AppState>>, id: String, enabled: bool) -> Result<(), String> {
    {
        let mut s = state.settings.lock().await;
        if enabled { s.disabled.remove(&id); s.enabled.insert(id); } else { s.enabled.remove(&id); s.disabled.insert(id); }
    }
    state.save_settings().await;
    Ok(())
}

/// «Обновить по Wi-Fi»: tells the node app (unicast to its beacon socket) to check http://<pc>:50055 now and
/// ASK its user — Android always needs a tap on the device; nothing is installed remotely.
#[tauri::command]
pub async fn wifi_node_update(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    let d = state.discovered.lock().await.get(&id).cloned()
        .ok_or("нода не слышна в сети — нужен Wi-Fi режим в Argus Node 0.3.1+")?;
    if d.vc.is_none() {
        return Err("эта нода старше 0.3.1 и не умеет обновляться по Wi-Fi — обновите её один раз по USB или вручную (Argus-Node.apk)".into());
    }
    if d.src_port == 0 { return Err("адрес ответа ноды неизвестен — подождите пару секунд".into()); }
    let helper = crate::helper::lan_assignment(&state, &id).await;
    let msg = host_reply(crate::updater::NODE_UPDATE_PORT, crate::cluster::bundled_code(&state.bundled_node_version), true, Some(helper));
    let msg = crate::roles_extra::extend_lan_reply(msg, &id);
    let sock = UdpSocket::bind(("0.0.0.0", 0)).await.map_err(|e| e.to_string())?;
    sock.send_to(&msg, (d.host.as_str(), d.src_port)).await.map_err(|e| e.to_string())?;
    Ok(format!("Запрос отправлен на «{}» — подтвердите обновление на самом устройстве", d.name))
}

#[tauri::command]
pub async fn set_fast_start(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.settings.lock().await.fast_start = enabled;
    state.save_settings().await;
    Ok(())
}

