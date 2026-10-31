//! ADB polling, plug-and-play provisioning and telemetry.
use crate::adb::AdbDevice;
use crate::gpu;
use crate::state::{AppState, ClusterSnapshot, DeviceRuntime, Node, NodeKind, NodeStage};
use crate::lan;
use anyhow::{anyhow, bail, Result};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tauri::{AppHandle, Emitter, State};
use tokio::{io::AsyncReadExt, net::TcpStream, time::{sleep, timeout}};

pub const DEVICE_RPC_PORT: u16 = 50052;   // port rpc-server binds on the device
pub const FIRST_LOCAL_PORT: u16 = 50052;  // PC side: 50052, 50053, ... one per device
pub const NODE_PKG: &str = "com.harkcluster.node";
const POLL: Duration = Duration::from_millis(1500);
const GPU_HEADROOM_MB: u64 = 512;         // display/driver + CUDA context

/// Android kills apps aggressively; only promise ~70% of what is free right now.
fn usable_device_mb(avail_mb: u64) -> u64 { avail_mb * 70 / 100 }

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn get_snapshot(state: State<'_, Arc<AppState>>) -> Result<ClusterSnapshot, String> {
    Ok(state.snapshot().await)
}

/// Forget failed devices so the next poll re-provisions them.
#[tauri::command]
pub async fn rescan(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.nodes.lock().await.retain(|k, n| k.starts_with("lan:") || n.stage != NodeStage::Error);
    let st = state.inner().clone();
    if !crate::llama::engine_running(&st).await { refresh_gpus(&st).await; }
    Ok(())
}

// ---------------------------------------------------------------- loop

pub async fn run_poll_loop(app: AppHandle, st: Arc<AppState>) {
    if let Err(e) = st.adb.run(&["start-server"]).await { log::error!("{e}"); }
    let mut sys = sysinfo::System::new();
    refresh_gpus(&st).await;
    let mut n: u64 = 0;
    loop {
        refresh_host(&st, &mut sys).await;
        if let Err(e) = tick(&app, &st).await { log::warn!("adb poll: {e}"); }
        let running = crate::llama::engine_running(&st).await;
        if n % 3 == 0 { lan::refresh(&st, running).await; }          // LAN nodes every ~4.5 s
        if n % 40 == 39 && !running { refresh_gpus(&st).await; }     // re-list GPUs every ~60 s while idle
        emit(&app, &st).await;
        n += 1;
        sleep(POLL).await;
    }
}

async fn emit(app: &AppHandle, st: &AppState) {
    let _ = app.emit("cluster-update", st.snapshot().await);
}

async fn tick(app: &AppHandle, st: &Arc<AppState>) -> Result<()> {
    let devices = st.adb.devices().await?;
    let seen: HashSet<&str> = devices.iter().map(|d| d.serial.as_str()).collect();

    // Unplugged devices.
    let gone: Vec<String> = st.nodes.lock().await.keys().filter(|k| !k.starts_with("lan:") && !seen.contains(&k.as_str())).cloned().collect();
    for serial in gone { drop_device(st, &serial).await; }

    for d in devices {
        let stage = st.nodes.lock().await.get(&d.serial).map(|n| n.stage);
        let dev_state = d.state.clone();
        match dev_state.as_str() {
            "unauthorized" => upsert(st, &d, NodeStage::Unauthorized).await,
            "device" => match stage {
                Some(NodeStage::Connected) => refresh_metrics(st, &d.serial).await,
                Some(NodeStage::Error) => {} // wait for user to hit Rescan
                _ => {
                    if st.provisioning.lock().await.insert(d.serial.clone()) {
                        let (app, st) = (app.clone(), st.clone());
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = provision(&app, &st, &d).await {
                                set_error(&st, &d.serial, format!("{e:#}")).await;
                            }
                            st.provisioning.lock().await.remove(&d.serial);
                            emit(&app, &st).await;
                        });
                    }
                }
            },
            _ => {} // offline / recovery / sideload: ignore
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- provisioning

async fn provision(app: &AppHandle, st: &Arc<AppState>, d: &AdbDevice) -> Result<()> {
    let serial = d.serial.as_str();
    let adb = &st.adb;
    let fast_start = st.settings.lock().await.fast_start;

    // 1. Identify + install node app if missing.
    upsert(st, d, NodeStage::Installing).await;
    emit(app, st).await;
    let is_tablet = adb.getprop(serial, "ro.build.characteristics").await.contains("tablet");
    let brand = adb.getprop(serial, "ro.product.manufacturer").await;
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        n.kind = if is_tablet { NodeKind::Tablet } else { NodeKind::Phone };
        if let Some(model) = &d.model { n.name = format!("{brand} {model}").trim().to_string(); }
    }
    let mut fresh_install = false;
    if !adb.is_installed(serial, NODE_PKG).await? {
        let apk = st.apk_path.as_ref().filter(|p| p.exists())
            .ok_or_else(|| anyhow!("node app not installed and bundled node.apk is missing"))?;
        adb.install(serial, apk).await?;
        fresh_install = true;
    }
    // Auto-update: installed versionName older than the bundled node.apk -> adb install -r.
    let mut installed = adb.package_version(serial, NODE_PKG).await;
    if !fresh_install && node_outdated(installed.as_ref(), &st.bundled_node_version) {
        if let Some(apk) = st.apk_path.as_ref().filter(|p| p.exists()) {
            set_stage(st, serial, NodeStage::Updating).await;
            emit(app, st).await;
            log::info!("{serial}: node {:?} -> {}", installed, st.bundled_node_version);
            let _ = adb.shell(serial, &format!("am force-stop {NODE_PKG}")).await; // old rpc-server must go
            adb.install(serial, apk).await?;
            fresh_install = true;
            installed = adb.package_version(serial, NODE_PKG).await;
        }
    }
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        n.version = installed.as_ref().map(|(name, code)| match code { Some(c) => format!("{name} ({c})"), None => name.clone() });
    }

    // 2. Port forward: PC 127.0.0.1:<local> -> device 127.0.0.1:50052.
    set_stage(st, serial, NodeStage::Forwarding).await;
    emit(app, st).await;
    let local = allocate_port(st, serial).await;
    adb.forward(serial, local, DEVICE_RPC_PORT).await?;

    // Fast start: same version already serving on the device -> reuse it (no service restart).
    let reuse = fast_start && !fresh_install && probe_rpc(local).await;
    if reuse { log::info!("{serial}: fast start, rpc-server already running"); }

    // 3. Start the foreground service that runs rpc-server.
    set_stage(st, serial, NodeStage::Starting).await;
    emit(app, st).await;
    if !reuse {
        let threads: u32 = adb.shell(serial, "nproc").await.ok()
            .and_then(|s| s.trim().parse().ok()).unwrap_or(4);
        let out = adb.shell(serial, &format!(
            "am start-foreground-service -n {NODE_PKG}/.RpcService --ei port {DEVICE_RPC_PORT} --ei threads {threads}"
        )).await?;
        if out.contains("Error") { bail!("could not start node service: {}", out.trim()); }

        let mut up = false;
        for _ in 0..40 {
            if probe_rpc(local).await { up = true; break; }
            sleep(Duration::from_millis(500)).await;
        }
        if !up { bail!("rpc-server did not start on device port {DEVICE_RPC_PORT} (open the node app to see its log)"); }
    }

    // 4. Sync memory.
    set_stage(st, serial, NodeStage::Syncing).await;
    emit(app, st).await;
    let (total_kb, avail_kb) = adb.meminfo(serial).await?;
    let mut nodes = st.nodes.lock().await;
    let n = nodes.get_mut(serial).ok_or_else(|| anyhow!("device vanished"))?;
    n.total_mb = total_kb / 1024;
    n.available_mb = avail_kb / 1024;
    n.usable_mb = usable_device_mb(n.available_mb);
    n.endpoint = Some(format!("127.0.0.1:{local}"));
    n.stage = NodeStage::Connected;
    n.error = None;
    Ok(())
}

/// Through `adb forward`, connect() always succeeds on the PC side; if nothing listens on the
/// device, adb closes the socket immediately. rpc-server instead waits silently for the client's
/// HELLO, so "read times out" == "server alive".
async fn probe_rpc(port: u16) -> bool {
    let Ok(Ok(mut s)) = timeout(Duration::from_millis(500), TcpStream::connect(("127.0.0.1", port))).await else {
        return false;
    };
    let mut buf = [0u8; 1];
    match timeout(Duration::from_millis(400), s.read(&mut buf)).await {
        Err(_) => true,
        Ok(Ok(0)) | Ok(Err(_)) => false,
        Ok(Ok(_)) => true,
    }
}

async fn allocate_port(st: &AppState, serial: &str) -> u16 {
    let mut rt = st.runtime.lock().await;
    if let Some(r) = rt.get(serial) { return r.local_port; }
    let used: HashSet<u16> = rt.values().map(|r| r.local_port).collect();
    let port = (FIRST_LOCAL_PORT..).find(|p| !used.contains(p)).unwrap();
    rt.insert(serial.into(), DeviceRuntime { local_port: port, last_cpu: None });
    port
}

async fn drop_device(st: &AppState, serial: &str) {
    st.nodes.lock().await.remove(serial);
    if let Some(r) = st.runtime.lock().await.remove(serial) {
        let _ = st.adb.remove_forward(r.local_port).await; // adb usually drops it on unplug anyway
    }
    // NOTE: if a model is running, llama.cpp will error on the next token; the UI should prompt a restart.
}

// ---------------------------------------------------------------- telemetry

async fn refresh_metrics(st: &AppState, serial: &str) {
    let mem = st.adb.meminfo(serial).await.ok();
    let cpu = st.adb.cpu_sample(serial).await.ok();
    let load = {
        let mut rt = st.runtime.lock().await;
        match (rt.get_mut(serial), cpu) {
            (Some(r), Some((idle, total))) => {
                let pct = r.last_cpu.map(|(pi, pt)| {
                    let (di, dt) = (idle.saturating_sub(pi), total.saturating_sub(pt));
                    if dt == 0 { 0.0 } else { 100.0 * (1.0 - di as f32 / dt as f32) }
                });
                r.last_cpu = Some((idle, total));
                pct
            }
            _ => None,
        }
    };
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        if let Some((_, avail_kb)) = mem { n.available_mb = avail_kb / 1024; }
        if let Some(p) = load { n.load_pct = p; }
        // usable_mb is frozen at sync time on purpose: llama.cpp's own allocation lowers MemAvailable.
    }
}

/// Re-list local GPUs through llama-server --list-devices (exact names/order the engine uses).
pub async fn refresh_gpus(st: &AppState) {
    let Some(devs) = gpu::list_llama_devices(&st.llama_bin).await else {
        log::warn!("llama-server --list-devices failed; falling back to nvidia-smi only");
        return;
    };
    let any_discrete = devs.iter().any(|d| !gpu::looks_integrated(&d.desc));
    let list: Vec<Node> = devs.iter().map(|d| {
        let mut n = Node::local(&format!("gpu:{}", d.name), &d.desc.replace("NVIDIA ", ""), NodeKind::Gpu);
        n.device = Some(d.name.clone());
        n.total_mb = d.total_mb;
        n.available_mb = d.free_mb;
        n.usable_mb = d.total_mb.saturating_sub(GPU_HEADROOM_MB).min(d.free_mb.saturating_sub(256).max(d.total_mb / 2));
        n.integrated = any_discrete && gpu::looks_integrated(&d.desc);
        if n.integrated { n.usable_mb = d.free_mb / 2; } // shared system RAM: be careful
        n
    }).collect();
    *st.gpus.lock().await = list;
}

async fn refresh_host(st: &AppState, sys: &mut sysinfo::System) {
    let nv = gpu::query_nvidia_all().await;
    // live numbers for NVIDIA cards, matched to llama.cpp devices by name
    {
        let mut gpus = st.gpus.lock().await;
        let mut used = vec![false; nv.len()];
        for g in gpus.iter_mut() {
            if let Some(i) = nv.iter().enumerate().position(|(i, x)| !used[i] && g.name.contains(&x.name)) {
                used[i] = true;
                g.load_pct = nv[i].util_pct;
                g.available_mb = nv[i].total_mb.saturating_sub(nv[i].used_mb);
            }
        }
    }
    let have_llama_gpus = !st.gpus.lock().await.is_empty();
    let mut host = st.host.lock().await;
    if !have_llama_gpus && !nv.is_empty() {
        // --list-devices unavailable: old single-GPU behaviour via nvidia-smi
        let g = &nv[0];
        host.kind = NodeKind::Gpu;
        host.name = g.name.clone();
        host.total_mb = g.total_mb;
        host.available_mb = g.total_mb.saturating_sub(g.used_mb);
        host.usable_mb = g.total_mb.saturating_sub(GPU_HEADROOM_MB);
        host.load_pct = g.util_pct;
    } else {
        // no GPU (or all GPUs disabled: the snapshot replaces this with the GPU aggregate when any is enabled)
        sys.refresh_memory();
        sys.refresh_cpu_usage();
        host.kind = NodeKind::Cpu;
        host.name = "Host CPU".into();
        host.total_mb = sys.total_memory() / 1024 / 1024;
        host.available_mb = sys.available_memory() / 1024 / 1024;
        host.usable_mb = host.available_mb * 60 / 100;
        host.load_pct = sys.global_cpu_usage();
    }
}

/// Bundled spec is "versionName" or "versionName (versionCode)" (resources/node.version).
/// versionCode decides when both sides have one (an early APK was already named "0.2.0"); else versionName.
pub fn node_outdated(installed: Option<&(String, Option<u64>)>, bundled: &str) -> bool {
    let Some((name, code)) = installed else { return true };
    let (bname, bcode) = match bundled.split_once('(') {
        Some((n, c)) => (n.trim(), c.trim_end_matches(')').trim().parse::<u64>().ok()),
        None => (bundled.trim(), None),
    };
    match (code, bcode) {
        (Some(c), Some(b)) => *c < b,
        _ => version_lt(name, bname),
    }
}

/// "0.1.0" < "0.2" ; non-numeric parts compare as 0.
pub fn version_lt(a: &str, b: &str) -> bool {
    let p = |s: &str| s.trim().split(|c: char| c == '.' || c == '-').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (mut x, mut y) = (p(a), p(b));
    let n = x.len().max(y.len());
    x.resize(n, 0); y.resize(n, 0);
    x < y
}

// ---------------------------------------------------------------- helpers

async fn upsert(st: &AppState, d: &AdbDevice, stage: NodeStage) {
    let mut nodes = st.nodes.lock().await;
    let name = d.model.clone().unwrap_or_else(|| d.serial.clone());
    let n = nodes.entry(d.serial.clone()).or_insert_with(|| Node::device(&d.serial, &name));
    n.stage = stage;
}

async fn set_stage(st: &AppState, serial: &str, stage: NodeStage) {
    if let Some(n) = st.nodes.lock().await.get_mut(serial) { n.stage = stage; }
}

async fn set_error(st: &AppState, serial: &str, msg: String) {
    log::error!("{serial}: {msg}");
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        n.stage = NodeStage::Error;
        n.error = Some(msg);
    }
}
