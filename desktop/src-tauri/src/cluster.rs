//! ADB polling, plug-and-play provisioning, telemetry and the background loops.
//!
//! Stability rules (0.3):
//!  * every loop runs on its own task and every iteration in a child task: a hang or panic in one
//!    (adb, nvidia-smi, a LAN probe) can no longer freeze the others or the UI snapshot;
//!  * every adb call has a timeout (adb.rs); provisioning as a whole has one too;
//!  * one provisioning task per device (`provisioning` set + abort on unplug + generation counter);
//!  * a device that disappears, goes `offline`, or re-appears with a new adb transport id is
//!    disconnected: node OFFLINE, forward removed, provisioning aborted. When it is back as `device`
//!    it is provisioned again on the same PC port;
//!  * failed provisioning retries with backoff (3 s … 60 s, 8 attempts) instead of a sticky FAULT;
//!  * forwards are verified every few seconds (`adb forward --list`) and re-created if adb lost them;
//!  * `adb devices` failing 3× in a row restarts the adb server.
use crate::adb::AdbDevice;
use crate::gpu;
use crate::lan;
use crate::state::{AppState, ClusterSnapshot, Node, NodeKind, NodeStage, Transport};
use anyhow::{anyhow, bail, Result};
use std::{collections::{HashMap, HashSet}, future::Future, sync::Arc, time::{Duration, Instant}};
use tauri::{AppHandle, Emitter, State};
use tokio::{io::AsyncReadExt, net::TcpStream, time::{sleep, timeout}};

pub const DEVICE_RPC_PORT: u16 = 50052;   // port rpc-server binds on the device
pub const FIRST_LOCAL_PORT: u16 = 50052;  // PC side: 50052, 50053, ... one per device
pub const NODE_PKG: &str = "com.harkcluster.node";
const POLL: Duration = Duration::from_millis(1500);
const GPU_HEADROOM_MB: u64 = 512;         // display/driver + CUDA context
const PROVISION_TIMEOUT: Duration = Duration::from_secs(360);
const MAX_AUTO_ATTEMPTS: u32 = 8;
/// an unplugged USB node stays in the list (OFFLINE) this long
const FORGET_OFFLINE_AFTER: Duration = Duration::from_secs(600);

/// Android kills apps aggressively; only promise ~70% of what is free right now.
fn usable_device_mb(avail_mb: u64) -> u64 { avail_mb * 70 / 100 }

fn backoff(attempts: u32) -> Duration {
    Duration::from_secs((3u64 << attempts.saturating_sub(1).min(5)).min(60))
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn get_snapshot(state: State<'_, Arc<AppState>>) -> Result<ClusterSnapshot, String> {
    Ok(state.snapshot().await)
}

/// Retry every failed device now, restart adb if it is unhealthy, re-list GPUs when idle.
#[tauri::command]
pub async fn rescan(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let st = state.inner().clone();
    {
        let nodes = st.nodes.lock().await;
        let mut rt = st.runtime.lock().await;
        for (id, n) in nodes.iter() {
            if n.transport == Transport::Usb && n.stage == NodeStage::Error {
                let r = rt.entry(id.clone()).or_default();
                r.attempts = 0;
                r.retry_at = Some(Instant::now());
            }
        }
    }
    if !*st.adb_ok.lock().await { st.adb.restart_server().await; }
    if !crate::llama::engine_running(&st).await { refresh_gpus(&st).await; }
    Ok(())
}

/// "Переподключить": USB -> drop forward, restart RpcService, provision again; LAN -> probe now.
#[tauri::command]
pub async fn reconnect_node(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    let st = state.inner().clone();
    if id.starts_with("lan:") {
        lan::refresh(&st).await;
        emit(&app, &st).await;
        return Ok(());
    }
    let devices = st.adb.devices().await.map_err(|e| format!("{e:#}"))?;
    if !devices.iter().any(|d| d.serial == id && d.state == "device") {
        return Err("Устройство не видно по USB — проверьте кабель и разрешение отладки".into());
    }
    disconnect_device(&st, &id, "переподключение…").await;
    {
        let mut rt = st.runtime.lock().await;
        let r = rt.entry(id.clone()).or_default();
        r.force_restart = true;
        r.attempts = 0;
        r.retry_at = None;
        r.metric_failures = 0;
    }
    emit(&app, &st).await;
    Ok(())
}

/// «Обновить ноду на <device>?» → «Обновить»: the user approved `adb install -r` of the bundled APK.
/// Re-provisions the device (an engine using it is restarted by the supervisor once the node is back).
#[tauri::command]
pub async fn update_usb_node(app: AppHandle, state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    if id.starts_with("lan:") { return Err("это Wi-Fi нода — используйте «Обновить по Wi-Fi»".into()); }
    if !state.apk_path.as_ref().map(|p| p.exists()).unwrap_or(false) { return Err("node.apk не найден в resources".into()); }
    state.runtime.lock().await.entry(id.clone()).or_default().approve_update = true; // before provisioning can restart
    let r = reconnect_node(app, state.clone(), id.clone()).await;
    if r.is_err() { state.runtime.lock().await.entry(id).or_default().approve_update = false; }
    r
}

/// «Пропустить эту версию» for the bundled node APK (empty = forget).
#[tauri::command]
pub async fn skip_node_update(state: State<'_, Arc<AppState>>, version: String) -> Result<(), String> {
    state.settings.lock().await.skipped_node_version = Some(version).filter(|v| !v.is_empty());
    state.save_settings().await;
    Ok(())
}

// ---------------------------------------------------------------- loops

/// Runs `body` every `period`; each iteration in its own task so a panic only loses one tick.
pub fn spawn_loop<F, Fut>(name: &'static str, period: Duration, body: F)
where F: Fn() -> Fut + Send + Sync + 'static, Fut: Future<Output = ()> + Send + 'static {
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(e) = tokio::spawn(body()).await { log::error!("{name}: iteration failed: {e}"); }
            sleep(period).await;
        }
    });
}

pub async fn start(app: AppHandle, st: Arc<AppState>) {
    if let Err(e) = st.adb.run_timeout(&["start-server"], Duration::from_secs(20)).await { log::error!("{e}"); }
    seed_ports_from_forwards(&st).await;
    crate::llama::startup(&app, &st).await; // orphan llama-server: adopt (keep loaded) or kill
    { let s = st.clone(); tauri::async_runtime::spawn(async move { refresh_gpus(&s).await }); }

    { let (a, s) = (app.clone(), st.clone()); spawn_loop("adb", POLL, move || { let (a, s) = (a.clone(), s.clone()); async move { adb_tick(&a, &s).await } }); }
    {
        let s = st.clone();
        let sys = Arc::new(tokio::sync::Mutex::new(sysinfo::System::new()));
        let n = Arc::new(std::sync::atomic::AtomicU64::new(0));
        spawn_loop("host", Duration::from_secs(2), move || {
            let (s, sys, n) = (s.clone(), sys.clone(), n.clone());
            async move {
                refresh_host(&s, &mut *sys.lock().await).await;
                let i = n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i % 30 == 29 && !crate::llama::engine_running(&s).await { refresh_gpus(&s).await; } // ~60 s while idle
            }
        });
    }
    { let s = st.clone(); spawn_loop("lan", Duration::from_secs(2), move || { let s = s.clone(); async move { lan::refresh(&s).await } }); }
    lan::start_beacon_listener(app.clone(), st.clone());
    { let (a, s) = (app.clone(), st.clone()); spawn_loop("engine", Duration::from_secs(1), move || { let (a, s) = (a.clone(), s.clone()); async move { crate::llama::supervise(&a, &s).await } }); }
    { let (a, s) = (app.clone(), st.clone()); spawn_loop("emit", Duration::from_secs(1), move || { let (a, s) = (a.clone(), s.clone()); async move { emit(&a, &s).await } }); }
}

pub async fn emit(app: &AppHandle, st: &AppState) {
    let _ = app.emit("cluster-update", st.snapshot().await);
}

pub fn toast(app: &AppHandle, kind: &str, text: &str) {
    let _ = app.emit("argus-toast", serde_json::json!({ "kind": kind, "text": text }));
}

/// Forwards that survived an Argus restart (adb server keeps them): reuse their ports.
async fn seed_ports_from_forwards(st: &AppState) {
    if let Ok(list) = st.adb.forward_list().await {
        let mut ports = st.ports.lock().await;
        for (serial, local, remote) in list {
            if remote == DEVICE_RPC_PORT && !ports.values().any(|p| *p == local) { ports.insert(serial, local); }
        }
    }
}

async fn adb_tick(app: &AppHandle, st: &Arc<AppState>) {
    static FAILS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let devices = match st.adb.devices().await {
        Ok(d) => { FAILS.store(0, std::sync::atomic::Ordering::Relaxed); *st.adb_ok.lock().await = true; d }
        Err(e) => {
            log::warn!("adb devices: {e:#}");
            *st.adb_ok.lock().await = false;
            if FAILS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1 >= 3 {
                FAILS.store(0, std::sync::atomic::Ordering::Relaxed);
                toast(app, "warn", "ADB не отвечает — перезапускаю adb-сервер");
                // forwards die with the server: every USB node must be provisioned again
                let usb: Vec<String> = st.nodes.lock().await.values().filter(|n| n.transport == Transport::Usb).map(|n| n.id.clone()).collect();
                for s in usb { disconnect_device(st, &s, "adb-сервер перезапущен").await; }
                st.adb.restart_server().await;
            }
            return;
        }
    };
    let by_serial: HashMap<&str, &AdbDevice> = devices.iter().map(|d| (d.serial.as_str(), d)).collect();

    // 1. Lost / flapping / re-plugged devices.
    let usb: Vec<(String, NodeStage)> = st.nodes.lock().await.values()
        .filter(|n| n.transport == Transport::Usb).map(|n| (n.id.clone(), n.stage)).collect();
    for (serial, stage) in &usb {
        match by_serial.get(serial.as_str()) {
            None => {
                if *stage != NodeStage::Offline { disconnect_device(st, serial, "кабель отключён").await; }
                let forget = st.runtime.lock().await.get(serial).and_then(|r| r.offline_since)
                    .map(|t| t.elapsed() > FORGET_OFFLINE_AFTER).unwrap_or(true);
                if forget { st.nodes.lock().await.remove(serial); st.runtime.lock().await.remove(serial); }
            }
            Some(d) if d.state == "device" => {
                let known = st.runtime.lock().await.get(serial).and_then(|r| r.transport_id);
                if stage.eq(&NodeStage::Connected) && known.is_some() && d.transport_id.is_some() && known != d.transport_id {
                    log::info!("{serial}: re-plugged (transport {known:?} -> {:?})", d.transport_id);
                    disconnect_device(st, serial, "кабель переподключён").await;
                }
            }
            Some(d) if d.state == "unauthorized" => {
                if *stage != NodeStage::Unauthorized { disconnect_device(st, serial, "ждёт разрешения отладки").await; }
            }
            Some(d) => {
                if *stage != NodeStage::Offline && *stage != NodeStage::Error {
                    disconnect_device(st, serial, &format!("adb: {}", d.state)).await;
                }
            }
        }
    }

    // 2. Present devices.
    let mut connected: Vec<String> = vec![];
    for d in &devices {
        let stage = st.nodes.lock().await.get(&d.serial).map(|n| n.stage);
        match d.state.as_str() {
            "unauthorized" => upsert(st, d, NodeStage::Unauthorized, None).await,
            "no" | "no permissions" => upsert(st, d, NodeStage::Error, Some(
                "Нет прав на USB-устройство: установите udev-правила Android (пакет android-sdk-platform-tools-common) и переподключите кабель".into())).await,
            "device" => match stage {
                Some(NodeStage::Connected) => connected.push(d.serial.clone()),
                Some(NodeStage::Error) => {
                    let due = st.runtime.lock().await.get(&d.serial).and_then(|r| r.retry_at).map(|t| Instant::now() >= t).unwrap_or(false);
                    if due { start_provision(app, st, d.clone()).await; }
                }
                _ => start_provision(app, st, d.clone()).await, // new, offline, unauthorized->allowed, stale transitional
            },
            _ => {} // offline / recovery / sideload: handled above
        }
    }

    // 3. Forwards still there? (adb drops them on a USB reset even when the device "comes back")
    if !connected.is_empty() {
        if let Ok(fw) = st.adb.forward_list().await {
            let ports = st.ports.lock().await.clone();
            for serial in &connected {
                let Some(port) = ports.get(serial) else { continue };
                if !fw.iter().any(|(s, l, r)| s == serial && l == port && *r == DEVICE_RPC_PORT) {
                    log::warn!("{serial}: forward tcp:{port} lost, re-provisioning");
                    disconnect_device(st, serial, "проброс порта потерян").await;
                }
            }
        }
    }

    // 4. Telemetry, concurrently, each with a timeout.
    let mut set = tokio::task::JoinSet::new();
    for serial in connected {
        let st = st.clone();
        set.spawn(async move {
            let ok = timeout(Duration::from_secs(5), refresh_metrics(&st, &serial)).await.unwrap_or(false);
            (serial, ok)
        });
    }
    while let Some(r) = set.join_next().await {
        let Ok((serial, ok)) = r else { continue };
        let fails = {
            let mut rt = st.runtime.lock().await;
            let r = rt.entry(serial.clone()).or_default();
            r.metric_failures = if ok { 0 } else { r.metric_failures + 1 };
            r.metric_failures
        };
        if fails >= 3 { disconnect_device(st, &serial, "устройство не отвечает").await; }
    }
}

// ---------------------------------------------------------------- provisioning

async fn start_provision(app: &AppHandle, st: &Arc<AppState>, d: AdbDevice) {
    if !st.provisioning.lock().await.insert(d.serial.clone()) { return; } // per-device lock
    let gen = st.runtime.lock().await.entry(d.serial.clone()).or_default().gen;
    let (app2, st2) = (app.clone(), st.clone());
    let serial = d.serial.clone();
    let handle = tokio::spawn(async move {
        let r = match timeout(PROVISION_TIMEOUT, provision(&app2, &st2, &d, gen)).await {
            Err(_) => Err(anyhow!("подготовка устройства зависла (> {} с)", PROVISION_TIMEOUT.as_secs())),
            Ok(r) => r,
        };
        let current = { st2.runtime.lock().await.get(&d.serial).map(|r| r.gen) == Some(gen) };
        if current {
            match r {
                Ok(()) => {
                    let mut rt = st2.runtime.lock().await;
                    if let Some(x) = rt.get_mut(&d.serial) {
                        x.attempts = 0; x.retry_at = None; x.force_restart = false; x.metric_failures = 0;
                        x.transport_id = d.transport_id; x.offline_since = None;
                    }
                }
                Err(e) => {
                    let attempts = {
                        let mut rt = st2.runtime.lock().await;
                        let x = rt.entry(d.serial.clone()).or_default();
                        x.attempts += 1;
                        x.retry_at = (x.attempts < MAX_AUTO_ATTEMPTS).then(|| Instant::now() + backoff(x.attempts));
                        x.attempts
                    };
                    let tail = if attempts >= MAX_AUTO_ATTEMPTS { " — автоповторы исчерпаны, нажмите «Переподключить»" } else { "" };
                    set_error(&st2, &d.serial, format!("{e:#}{tail}")).await;
                }
            }
            st2.provisioning.lock().await.remove(&d.serial);
        }
        emit(&app2, &st2).await;
    });
    if let Some(r) = st.runtime.lock().await.get_mut(&serial) { r.task = Some(handle.abort_handle()); }
}

async fn provision(app: &AppHandle, st: &Arc<AppState>, d: &AdbDevice, gen: u64) -> Result<()> {
    let serial = d.serial.as_str();
    let adb = &st.adb;
    let fast_start = st.settings.lock().await.fast_start;
    let force_restart = st.runtime.lock().await.get(serial).map(|r| r.force_restart).unwrap_or(false);

    // 1. Identify + install node app if missing.
    upsert(st, d, NodeStage::Installing, None).await;
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
            .ok_or_else(|| anyhow!("приложение ноды не установлено, а node.apk не найден в resources"))?;
        adb.install(serial, apk).await?;
        fresh_install = true;
    }
    // Update: installed versionCode/versionName older than the bundled node.apk. 0.3.1: NEVER without the
    // user's click — the node keeps running its installed app (same rpc protocol) and the UI asks
    // «Обновить ноду на <device>?»; «Обновить» sets approve_update and re-provisions (update_usb_node).
    let mut installed = adb.package_version(serial, NODE_PKG).await;
    let outdated = !fresh_install && node_outdated(installed.as_ref(), &st.bundled_node_version);
    let approved = std::mem::take(&mut st.runtime.lock().await.entry(serial.to_string()).or_default().approve_update);
    if outdated && !approved {
        log::info!("{serial}: node {:?} older than bundled {} — waiting for the user", installed, st.bundled_node_version);
    }
    if outdated && approved {
        if let Some(apk) = st.apk_path.as_ref().filter(|p| p.exists()) {
            set_stage(st, serial, NodeStage::Updating).await;
            emit(app, st).await;
            log::info!("{serial}: node {:?} -> {}", installed, st.bundled_node_version);
            let _ = adb.shell(serial, &format!("am force-stop --user 0 {NODE_PKG}")).await; // old rpc-server must go
            adb.install(serial, apk).await?;
            fresh_install = true;
            installed = adb.package_version(serial, NODE_PKG).await;
        }
    }
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        n.version = installed.as_ref().map(|(name, code)| match code { Some(c) => format!("{name} ({c})"), None => name.clone() });
        n.app_vc = installed.as_ref().and_then(|(_, c)| *c);
        n.update_available = node_outdated(installed.as_ref(), &st.bundled_node_version).then(|| st.bundled_node_version.clone());
    }

    // 2. Port forward: PC 127.0.0.1:<local> -> device 127.0.0.1:50052 (same PC port as last time).
    set_stage(st, serial, NodeStage::Forwarding).await;
    emit(app, st).await;
    let local = forward_with_fallback(st, serial).await?;
    let endpoint = format!("127.0.0.1:{local}");

    // Fast start: rpc-server already serving on the device and free -> reuse it (no service restart).
    // A real HELLO is used (not just "socket stays open"): a wedged or busy rpc-server gets restarted.
    let engine_attached = crate::llama::engine_uses_endpoint(st, &endpoint).await;
    let reuse = !force_restart && !fresh_install && (fast_start || engine_attached) && if engine_attached {
        probe_rpc(local).await // the kept-loaded engine holds the single rpc-server slot
    } else {
        lan::rpc_query(&endpoint).await.is_ok()
    };
    if reuse { log::info!("{serial}: fast start, rpc-server already running"); }

    // 3. (Re)start the foreground service that runs rpc-server.
    set_stage(st, serial, NodeStage::Starting).await;
    emit(app, st).await;
    if !reuse {
        if force_restart || !fresh_install {
            let _ = adb.shell(serial, &format!("am stopservice --user 0 -n {NODE_PKG}/.RpcService")).await;
            sleep(Duration::from_millis(700)).await;
        }
        let threads: u32 = adb.shell(serial, "nproc").await.ok()
            .and_then(|s| s.trim().parse().ok()).unwrap_or(4);
        let out = adb.shell(serial, &format!(
            "am start-foreground-service --user 0 -n {NODE_PKG}/.RpcService --ei port {DEVICE_RPC_PORT} --ei threads {threads}"
        )).await?;
        if out.contains("Error") { bail!("не удалось запустить сервис ноды: {}", out.trim()); }

        let mut up = false;
        let mut last_err = String::new();
        for _ in 0..40 {
            match lan::rpc_query(&endpoint).await {
                Ok(_) => { up = true; break; }
                Err(e) => last_err = format!("{e:#}"),
            }
            if last_err.contains("протокол") { bail!("{last_err}"); }
            sleep(Duration::from_millis(500)).await;
        }
        if !up { bail!("rpc-server не запустился на порту {DEVICE_RPC_PORT} устройства (откройте Argus Node, там лог): {last_err}"); }
    }

    // 4. Sync memory.
    set_stage(st, serial, NodeStage::Syncing).await;
    emit(app, st).await;
    let (total_kb, avail_kb) = adb.meminfo(serial).await?;
    if st.runtime.lock().await.get(serial).map(|r| r.gen) != Some(gen) { bail!("устройство переподключено во время подготовки"); }
    let mut nodes = st.nodes.lock().await;
    let n = nodes.get_mut(serial).ok_or_else(|| anyhow!("устройство пропало"))?;
    n.total_mb = total_kb / 1024;
    n.available_mb = avail_kb / 1024;
    n.usable_mb = usable_device_mb(n.available_mb);
    n.endpoint = Some(endpoint);
    n.stage = NodeStage::Connected;
    n.error = None;
    Ok(())
}

/// Forward on the remembered port; if something else holds it, take the next free one.
async fn forward_with_fallback(st: &AppState, serial: &str) -> Result<u16> {
    let mut last = None;
    for _ in 0..6 {
        let port = allocate_port(st, serial).await;
        match st.adb.forward(serial, port, DEVICE_RPC_PORT).await {
            Ok(()) => return Ok(port),
            Err(e) => {
                let msg = format!("{e:#}");
                log::warn!("{serial}: forward tcp:{port} failed: {msg}");
                if msg.contains("bind") || msg.contains("in use") || msg.contains("cannot") {
                    // port taken by another program: blacklist it for this device and retry
                    st.ports.lock().await.insert(format!("!busy:{port}"), port);
                    st.ports.lock().await.remove(serial);
                    last = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("не удалось пробросить порт")))
}

/// Through `adb forward`, connect() always succeeds on the PC side; if nothing listens on the
/// device, adb closes the socket immediately. rpc-server instead waits silently for the client's
/// HELLO, so "read times out" == "server alive". Only used when an engine already holds the server.
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
    let mut ports = st.ports.lock().await;
    if let Some(p) = ports.get(serial) { return *p; }
    let mut used: HashSet<u16> = ports.values().copied().collect();
    // a LAN node on this PC (rpc-server on 127.0.0.1) must not collide with a forward
    for l in st.settings.lock().await.lan_nodes.iter() {
        if l.host == "127.0.0.1" || l.host == "localhost" { used.insert(l.port); }
    }
    let port = (FIRST_LOCAL_PORT..u16::MAX).find(|p| !used.contains(p)).unwrap_or(FIRST_LOCAL_PORT);
    ports.insert(serial.into(), port);
    port
}

/// Node -> OFFLINE (or UNAUTHORIZED), forward removed, provisioning aborted. The PC port stays
/// reserved for this serial so the engine's `--rpc` address is the same after a re-plug.
pub async fn disconnect_device(st: &AppState, serial: &str, reason: &str) {
    let (task, port) = {
        let mut rt = st.runtime.lock().await;
        let r = rt.entry(serial.to_string()).or_default();
        r.gen += 1;
        r.transport_id = None;
        r.metric_failures = 0;
        r.last_cpu = None;
        if r.offline_since.is_none() { r.offline_since = Some(Instant::now()); }
        (r.task.take(), st.ports.lock().await.get(serial).copied())
    };
    if let Some(t) = task { t.abort(); }
    st.provisioning.lock().await.remove(serial);
    if let Some(p) = port { let _ = st.adb.remove_forward(p).await; } // usually gone already
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        n.stage = if reason.contains("разрешения") { NodeStage::Unauthorized } else { NodeStage::Offline };
        n.error = Some(reason.to_string());
        n.load_pct = 0.0;
    }
    log::info!("{serial}: disconnected ({reason})");
}

// ---------------------------------------------------------------- telemetry

async fn refresh_metrics(st: &AppState, serial: &str) -> bool {
    let mem = st.adb.meminfo(serial).await.ok();
    let cpu = st.adb.cpu_sample(serial).await.ok();
    let ok = mem.is_some() || cpu.is_some();
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
        if n.stage != NodeStage::Connected { return true; }
        if let Some((_, avail_kb)) = mem { n.available_mb = avail_kb / 1024; }
        if let Some(p) = load { n.load_pct = p; }
        // usable_mb is frozen at sync time on purpose: llama.cpp's own allocation lowers MemAvailable.
    }
    ok
}

/// Re-list local GPUs through llama-server --list-devices (exact names/order the engine uses).
/// Fallback when that fails: Vulkan devices from `vulkaninfo` (display only, llama.cpp default split).
pub async fn refresh_gpus(st: &AppState) {
    let Some(devs) = gpu::list_llama_devices(&st.llama_bin).await else {
        log::warn!("llama-server --list-devices failed; falling back to nvidia-smi / vulkaninfo");
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
    let vk = if !have_llama_gpus && nv.is_empty() { gpu::vulkan_fallback().await } else { None };
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
    } else if let Some(g) = vk {
        // AMD/Intel without a working --list-devices (e.g. Linux without the bundled engine yet)
        host.kind = NodeKind::Gpu;
        host.name = format!("{} (Vulkan)", g.name);
        host.total_mb = g.total_mb;
        host.available_mb = g.total_mb;
        host.usable_mb = g.total_mb.saturating_sub(GPU_HEADROOM_MB);
        host.load_pct = 0.0;
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

/// versionCode from a node.version spec "0.3.1 (5)".
pub fn bundled_code(spec: &str) -> Option<u64> {
    spec.split_once('(').and_then(|(_, c)| c.trim_end_matches(')').trim().parse().ok())
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

async fn upsert(st: &AppState, d: &AdbDevice, stage: NodeStage, error: Option<String>) {
    let mut nodes = st.nodes.lock().await;
    let name = d.model.clone().unwrap_or_else(|| d.serial.clone());
    let n = nodes.entry(d.serial.clone()).or_insert_with(|| Node::device(&d.serial, &name));
    n.stage = stage;
    if stage != NodeStage::Connected { n.endpoint = None; }
    if error.is_some() || stage == NodeStage::Installing { n.error = error; }
}

async fn set_stage(st: &AppState, serial: &str, stage: NodeStage) {
    if let Some(n) = st.nodes.lock().await.get_mut(serial) { n.stage = stage; }
}

async fn set_error(st: &AppState, serial: &str, msg: String) {
    log::error!("{serial}: {msg}");
    if let Some(n) = st.nodes.lock().await.get_mut(serial) {
        n.stage = NodeStage::Error;
        n.endpoint = None;
        n.error = Some(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(3));
        assert_eq!(backoff(2), Duration::from_secs(6));
        assert_eq!(backoff(4), Duration::from_secs(24));
        assert_eq!(backoff(9), Duration::from_secs(60));
    }
    #[test]
    fn versions() {
        assert!(node_outdated(Some(&("0.2.0".into(), Some(2))), "0.2 (3)"));
        assert!(!node_outdated(Some(&("0.2".into(), Some(3))), "0.2 (3)"));
        assert!(version_lt("0.1.0", "0.2"));
        assert!(node_outdated(Some(&("0.3".into(), Some(4))), "0.3.1 (5)"));
        assert!(!node_outdated(Some(&("0.3.1".into(), Some(5))), "0.3.1 (5)"));
        assert_eq!(bundled_code("0.3.1 (5)"), Some(5));
        assert_eq!(bundled_code("0.3"), None);
        assert_eq!(crate::default_node_version(), "0.3.1 (5)");
    }
}
