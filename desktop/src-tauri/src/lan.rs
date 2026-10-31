//! Network (LAN) nodes: any machine running `rpc-server -H 0.0.0.0 -p 50052` from llama.cpp b12008
//! (Raspberry Pi, old laptop, second PC). Health + memory come from a minimal ggml-rpc client:
//! HELLO -> DEVICE_COUNT -> GET_DEVICE_MEMORY, then disconnect.
//! rpc-server serves ONE client at a time, so while llama-server is attached we only check that
//! the port accepts TCP and keep the last known numbers.
use crate::settings::{parse_host_port, LanNode};
use crate::state::{AppState, Node, NodeStage};
use anyhow::{bail, Result};
use std::{sync::Arc, time::Duration};
use tauri::State;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream, time::timeout};

const RPC_PROTO_MAJOR: u8 = 8; // ggml/include/ggml-rpc.h @ b12008
const CMD_GET_DEVICE_MEMORY: u8 = 11;
const CMD_HELLO: u8 = 14;
const CMD_DEVICE_COUNT: u8 = 15;
const CONN_CAPS: usize = 24;
/// Pi/laptop is usually dedicated to the job, but leave room for the OS.
fn usable_lan_mb(free_mb: u64) -> u64 { free_mb * 80 / 100 }

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
    matches!(timeout(Duration::from_millis(1200), TcpStream::connect(endpoint)).await, Ok(Ok(_)))
}

/// Called from the poll loop every few seconds.
pub async fn refresh(st: &Arc<AppState>, engine_running: bool) {
    let list = st.settings.lock().await.lan_nodes.clone();
    {
        let mut nodes = st.nodes.lock().await;
        let ids: Vec<String> = list.iter().map(|l| l.id()).collect();
        nodes.retain(|k, _| !k.starts_with("lan:") || ids.contains(k));
        for l in &list {
            nodes.entry(l.id()).or_insert_with(|| Node::lan(&l.id(), &display_name(l), &l.endpoint()));
        }
    }
    for l in list {
        let id = l.id();
        let ep = l.endpoint();
        let was_online = st.nodes.lock().await.get(&id).map(|n| n.online()).unwrap_or(false);
        if engine_running && was_online {
            let alive = tcp_alive(&ep).await;
            if let Some(n) = st.nodes.lock().await.get_mut(&id) {
                if !alive { n.stage = NodeStage::Offline; n.error = Some("нет TCP-ответа".into()); }
            }
            continue;
        }
        let r = rpc_query(&ep).await;
        let mut nodes = st.nodes.lock().await;
        let Some(n) = nodes.get_mut(&id) else { continue };
        match r {
            Ok(info) if !info.mem.is_empty() => {
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
            Ok(_) => { n.stage = NodeStage::Error; n.error = Some("rpc-server не сообщил ни одного устройства".into()); }
            Err(e) => {
                let msg = format!("{e:#}");
                n.stage = if msg.contains("протокол") { NodeStage::Error } else { NodeStage::Offline };
                n.error = Some(msg);
            }
        }
    }
}

fn display_name(l: &LanNode) -> String {
    if l.name.trim().is_empty() { l.host.clone() } else { l.name.trim().to_string() }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn add_lan_node(state: State<'_, Arc<AppState>>, address: String, name: Option<String>) -> Result<String, String> {
    let (host, port) = parse_host_port(&address)?;
    let node = LanNode { host, port, name: name.unwrap_or_default() };
    let id = node.id();
    {
        let mut s = state.settings.lock().await;
        if s.lan_nodes.iter().any(|n| n.id() == id) { return Err("эта нода уже добавлена".into()); }
        s.lan_nodes.push(node);
    }
    state.save_settings().await;
    let st = state.inner().clone();
    let running = crate::llama::engine_running(&st).await;
    refresh(&st, running).await;
    Ok(id)
}

#[tauri::command]
pub async fn remove_lan_node(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.settings.lock().await.lan_nodes.retain(|n| n.id() != id);
    state.nodes.lock().await.remove(&id);
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

#[tauri::command]
pub async fn set_fast_start(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.settings.lock().await.fast_start = enabled;
    state.save_settings().await;
    Ok(())
}
