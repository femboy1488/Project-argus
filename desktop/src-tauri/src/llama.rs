//! Launches the inference engine with every connected node as an RPC backend, and supervises it.
//!
//! Supervisor (0.3, `supervise`, every second):
//!  * engine process exited            -> Broken ("движок завершился: <last error line>")
//!  * an rpc node it uses went offline  -> process killed (it would hang or abort anyway and it holds
//!    the single client slot of every other rpc-server) -> Broken ("нода «X» отключилась")
//!  * Broken + every node it used is back + cluster unchanged for 3 s -> auto-restart with the current
//!    node set, toast "Кластер изменился — модель перезапущена" (max 3 per 10 min; never after a crash
//!    during loading, that is a config/memory error the user must see).
//!  * the UI gets `engine-state` events and aborts a running chat request with a clear message.
use crate::adb::hidden_command;
use crate::state::{AppState, EngineNode, EngineStatus, EngineView};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path, process::Stdio, sync::Arc, time::{Duration, Instant}};
use tauri::{AppHandle, Emitter, State};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream, time::timeout};

pub const LLAMA_PORT: u16 = 8080;

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartModelArgs {
    pub hf_repo: Option<String>,    // "bartowski/...-GGUF:Q4_K_M" -> llama-server -hf
    pub hf_file: Option<String>,    // exact file in the repo -> --hf-file
    pub model_path: Option<String>, // or a local .gguf
    pub ctx: Option<u32>,
    pub engine: Option<String>,     // "llama.cpp" (default) | "ollama"
    pub alias: Option<String>,      // model id other apps see in GET /v1/models
    pub lan: Option<bool>,          // bind 0.0.0.0 so other machines (or a VM/WSL) can use the API
    pub need_mb: Option<u64>,       // estimated weights+KV, lets the split water-fill fast devices first
    // speed options
    pub draft_hf: Option<String>,   // speculative decoding draft model, "repo:quant" -> -hfd
    pub draft_max: Option<u32>,     // --spec-draft-n-max
    pub cache_reuse: Option<u32>,   // --cache-reuse N (prompt cache KV shifting)
    pub keep_loaded: Option<bool>,  // engine survives Argus restarts; identical relaunch is a no-op
}

/// Builds the llama-server argument list (pure; unit-tested).
pub fn build_args(args: &StartModelArgs, snap: &crate::state::ClusterSnapshot) -> Result<Vec<String>, String> {
    let mut a: Vec<String> = vec![];
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
    match (&args.model_path, &args.hf_repo) {
        (Some(p), _) => push(&["-m", p]),
        (None, Some(r)) => {
            push(&["-hf", r]);
            if let Some(f) = args.hf_file.as_deref().filter(|f| !f.is_empty()) { push(&["--hf-file", f]); }
        }
        _ => return Err("no model given".into()),
    }
    let host = if args.lan.unwrap_or(false) { "0.0.0.0" } else { "127.0.0.1" };
    let port = LLAMA_PORT.to_string();
    let ctx = args.ctx.unwrap_or(8192).to_string();
    // OpenAI-compatible API for Hermes / Open WebUI / LM Studio / Jan / Continue etc.
    // --jinja enables the model's chat template => proper tool/function calling for agents.
    push(&["--host", host, "--port", &port, "--jinja"]);
    push(&["--alias", args.alias.as_deref().unwrap_or("argus")]);
    push(&["-c", &ctx, "-np", "1"]);                         // one slot => the full context for one agent
    push(&["-fa", "on", "-ctk", "q8_0", "-ctv", "q8_0"]);   // q8 KV cache: half the memory, ~no quality loss

    // ---- devices: --rpc first (registers RPC0..), then --device in the same order as -ts
    let plan = crate::split::plan(&snap.nodes, &snap.gpus, args.need_mb);
    if !snap.rpc_servers.is_empty() { push(&["--rpc", &snap.rpc_servers.join(",")]); }
    let local_gpu = snap.gpus.iter().find(|g| g.enabled && g.device.is_some()).and_then(|g| g.device.clone());
    if !snap.gpus.is_empty() {
        // llama.cpp's device list is known: always pass it explicitly so -ts lines up and disabled GPUs stay idle
        if plan.is_empty() {
            push(&["--device", "none"]); // every device disabled: CPU only
        } else {
            let devs = plan.iter().map(|e| e.device.as_str()).collect::<Vec<_>>().join(",");
            push(&["--device", &devs]);
            if plan.len() > 1 { push(&["-ts", &crate::split::ts_arg(&plan)]); }
            push(&["-ngl", "999"]);
            // vision projector would default to the first device (an RPC phone): keep it on the local GPU
            match &local_gpu {
                Some(g) => push(&["-mmdev", g]),
                None => push(&["--no-mmproj-offload"]),
            }
        }
    } else if snap.host.kind == crate::state::NodeKind::Gpu {
        // GPU seen only through nvidia-smi (no --list-devices): llama.cpp default split
        push(&["-ngl", "999"]);
    }
    // else CPU-only host: leave -ngl unset so llama.cpp --fit spreads layers over RPC nodes + host RAM

    // ---- speed
    if let Some(n) = args.cache_reuse.filter(|n| *n > 0) { push(&["--cache-reuse", &n.to_string()]); }
    if let Some(d) = args.draft_hf.as_deref().filter(|d| !d.is_empty()) {
        push(&["-hfd", d, "--spec-type", "draft-simple"]);
        push(&["--spec-draft-n-max", &args.draft_max.unwrap_or(16).to_string(), "--spec-draft-n-min", "1"]);
        // the draft is small: keep it entirely on the local GPU (or CPU) — never on a phone over USB
        match &local_gpu {
            Some(g) => push(&["-devd", g, "-ngld", "99"]),
            None => push(&["-devd", "none"]),
        }
    }
    Ok(a)
}

const SETTLE: Duration = Duration::from_secs(3);
const RESTART_BUDGET: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(600);
/// an adopted engine (kept loaded across an Argus restart) gets this long for its nodes to come back
const ADOPT_GRACE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------- process helpers

/// On Linux the bundled llama-server finds its .so files via RUNPATH=$ORIGIN; LD_LIBRARY_PATH is a
/// belt-and-braces for copies that lost it (AppImage, distro repackaging). Windows: DLLs next to the exe.
pub fn lib_path_env(cmd: &mut tokio::process::Command, bin: &Path) {
    #[cfg(unix)]
    if let Some(dir) = bin.parent().filter(|d| !d.as_os_str().is_empty()) {
        let old = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let v = if old.is_empty() { dir.display().to_string() } else { format!("{}:{old}", dir.display()) };
        cmd.env("LD_LIBRARY_PATH", v);
    }
    #[cfg(not(unix))]
    { let _ = (cmd, bin); }
}

pub async fn engine_running(st: &AppState) -> bool {
    {
        let mut g = st.engine.lock().await;
        if let Some(c) = g.as_mut() { return matches!(c.try_wait(), Ok(None)); }
    }
    match st.engine_state.lock().await.adopted_pid {
        Some(pid) => pid_alive(pid).await,
        None => false,
    }
}

/// Endpoints the live engine is attached to (rpc-server serves one client: don't HELLO these).
pub async fn engine_endpoints(st: &AppState) -> HashSet<String> {
    let e = st.engine_state.lock().await;
    if matches!(e.status, EngineStatus::Loading | EngineStatus::Ready) {
        e.nodes.iter().map(|n| n.endpoint.clone()).collect()
    } else { HashSet::new() }
}

pub async fn engine_uses_endpoint(st: &AppState, ep: &str) -> bool {
    engine_endpoints(st).await.contains(ep)
}

async fn pid_alive(pid: u32) -> bool {
    tokio::task::spawn_blocking(move || {
        let mut sys = sysinfo::System::new();
        let p = sysinfo::Pid::from_u32(pid);
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[p]), true);
        sys.process(p).is_some()
    }).await.unwrap_or(false)
}

/// llama-server processes started by Argus (bundled exe, or our distinctive command line).
fn find_engines(llama_bin: &Path) -> Vec<(u32, Vec<String>)> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true,
        ProcessRefreshKind::new().with_exe(UpdateKind::Always).with_cmd(UpdateKind::Always));
    let want = std::fs::canonicalize(llama_bin).ok();
    let port = LLAMA_PORT.to_string();
    sys.processes().iter().filter_map(|(pid, p)| {
        let name = p.name().to_string_lossy().to_lowercase();
        if !name.starts_with("llama-server") { return None; }
        let cmd: Vec<String> = p.cmd().iter().map(|c| c.to_string_lossy().into_owned()).collect();
        let exe_match = matches!((&want, p.exe().and_then(|e| std::fs::canonicalize(e).ok())), (Some(w), Some(e)) if *w == e);
        let cmd_match = cmd.iter().any(|c| c == "--alias") && cmd.iter().any(|c| c == "--jinja") && cmd.iter().any(|c| *c == port);
        (exe_match || cmd_match).then(|| (pid.as_u32(), cmd))
    }).collect()
}

async fn kill_pids(pids: Vec<u32>) {
    if pids.is_empty() { return; }
    let _ = tokio::task::spawn_blocking(move || {
        let mut sys = sysinfo::System::new();
        let list: Vec<sysinfo::Pid> = pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&list), true);
        for p in list { if let Some(pr) = sys.process(p) { pr.kill(); } }
    }).await;
    // wait until :8080 is free (max 5 s) so the next launch can bind it
    for _ in 0..25 { if !port_open().await { break; } tokio::time::sleep(Duration::from_millis(200)).await; }
}

/// Kill our engine process (child, adopted pid, or an orphan holding :8080). Supervisor state untouched.
async fn kill_engine(st: &AppState) {
    *st.engine_sig.lock().await = None;
    let child = st.engine.lock().await.take();
    if let Some(mut c) = child {
        let _ = c.start_kill();
        let _ = timeout(Duration::from_secs(5), c.wait()).await;
    }
    let adopted = st.engine_state.lock().await.adopted_pid.take();
    let mut pids: Vec<u32> = adopted.into_iter().collect();
    if port_open().await {
        let bin = st.llama_bin.clone();
        let orphans = tokio::task::spawn_blocking(move || find_engines(&bin)).await.unwrap_or_default();
        pids.extend(orphans.into_iter().map(|(p, _)| p));
    }
    kill_pids(pids).await;
}

async fn health_ok() -> bool {
    let io = async {
        let mut s = TcpStream::connect(("127.0.0.1", LLAMA_PORT)).await.ok()?;
        s.write_all(b"GET /health HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").await.ok()?;
        let mut buf = [0u8; 64];
        let n = s.read(&mut buf).await.ok()?;
        let head = String::from_utf8_lossy(&buf[..n]).into_owned();
        Some(head.split_whitespace().nth(1) == Some("200"))
    };
    matches!(timeout(Duration::from_millis(1500), io).await, Ok(Some(true)))
}

fn rpc_from_cmdline(cmd: &[String]) -> Vec<String> {
    cmd.iter().position(|c| c == "--rpc").and_then(|i| cmd.get(i + 1))
        .map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
}

fn emit_engine(app: &AppHandle, view: EngineView, toast: Option<&str>) {
    let mut v = serde_json::to_value(view).unwrap_or_default();
    if let (Some(t), Some(o)) = (toast, v.as_object_mut()) { o.insert("toast".into(), t.into()); }
    let _ = app.emit("engine-state", v);
}

// ---------------------------------------------------------------- startup / shutdown

/// App start: an engine left by a previous session is adopted when "keep loaded" is on and it is
/// healthy; otherwise (crashed, stuck, keep-loaded off) every orphan llama-server is killed.
pub async fn startup(app: &AppHandle, st: &Arc<AppState>) {
    let bin = st.llama_bin.clone();
    let found = tokio::task::spawn_blocking(move || find_engines(&bin)).await.unwrap_or_default();
    if found.is_empty() { return; }
    let (keep, last) = { let s = st.settings.lock().await; (s.keep_loaded, s.last_launch.clone()) };
    let mut healthy = false;
    if keep && last.is_some() && found.len() == 1 {
        for _ in 0..3 { if health_ok().await { healthy = true; break; } tokio::time::sleep(Duration::from_millis(700)).await; }
    }
    if healthy {
        let (pid, cmd) = found[0].clone();
        let mut e = st.engine_state.lock().await;
        e.status = EngineStatus::Ready;
        e.args = last;
        e.adopted_pid = Some(pid);
        e.ever_healthy = true;
        e.started = Some(Instant::now());
        e.nodes = rpc_from_cmdline(&cmd).into_iter().map(|ep| EngineNode { id: String::new(), name: ep.clone(), endpoint: ep }).collect();
        log::info!("adopted running llama-server pid {pid} ({} rpc nodes)", e.nodes.len());
        drop(e);
        let _ = app.emit("llama-log", format!("[argus] подключились к уже запущенному движку (pid {pid})"));
    } else {
        log::info!("killing {} orphan llama-server process(es)", found.len());
        kill_pids(found.into_iter().map(|(p, _)| p).collect()).await;
    }
}

/// App exit: stop the engine unless the user chose to keep the model loaded.
pub async fn shutdown(st: &AppState) {
    let keep = st.settings.lock().await.keep_loaded;
    let running_ours = st.engine.lock().await.is_some() || st.engine_state.lock().await.adopted_pid.is_some();
    if !keep && running_ours { kill_engine(st).await; }
}

/// Self-update: stop the engine regardless of «держать модель загруженной» (the installer replaces llama-server).
pub async fn stop_for_update(st: &AppState) {
    {
        let mut e = st.engine_state.lock().await;
        e.args = None; e.pending_restart = false;
    }
    kill_engine(st).await;
}

// ---------------------------------------------------------------- launch

async fn launch(app: &AppHandle, st: &Arc<AppState>, args: StartModelArgs, auto: bool) -> Result<String, String> {
    let _guard = st.launch_lock.lock().await;
    let snap = st.snapshot().await;
    let rpc = snap.rpc_servers.join(",");
    let sig = serde_json::to_string(&(&args.hf_repo, &args.hf_file, &args.model_path, &args.ctx, &args.lan,
        &args.draft_hf, &args.cache_reuse, &rpc, snap.split.iter().map(|e| &e.device).collect::<Vec<_>>())).unwrap_or_default();
    // keep-loaded: the identical engine is already up -> nothing to do
    if !auto && args.keep_loaded.unwrap_or(false) && engine_running(st).await
        && st.engine_sig.lock().await.as_deref() == Some(sig.as_str()) {
        let _ = app.emit("llama-log", "[argus] same model already loaded — reusing the running engine".to_string());
        return Ok(format!("http://127.0.0.1:{LLAMA_PORT}"));
    }
    kill_engine(st).await;

    let used: Vec<EngineNode> = snap.nodes.iter().filter(|n| n.online() && n.enabled)
        .filter_map(|n| n.endpoint.clone().map(|ep| EngineNode { id: n.id.clone(), name: n.name.clone(), endpoint: ep })).collect();
    {
        let mut s = st.settings.lock().await;
        s.keep_loaded = args.keep_loaded.unwrap_or(false);
        s.last_launch = Some(args.clone());
    }
    st.save_settings().await;

    let mut cmd = match args.engine.as_deref().unwrap_or("llama.cpp") {
        // Upstream Ollama has NO RPC support (PR #6729 was never merged). This branch only works
        // with a fork that reads OLLAMA_RPC_SERVERS; kept for experimentation.
        "ollama" => {
            let mut c = hidden_command("ollama");
            c.arg("serve").env("OLLAMA_RPC_SERVERS", &rpc);
            c
        }
        _ => {
            let mut c = hidden_command(&st.llama_bin);
            lib_path_env(&mut c, &st.llama_bin);
            // llama.cpp can't create its HF cache under a non-ASCII profile (C:\Users\Марс -> "Access is denied").
            // Keep models in an ASCII-only folder; users may override with ARGUS_MODELS.
            let cache = model_cache_dir();
            let _ = std::fs::create_dir_all(&cache);
            c.env("HF_HOME", &cache).env("HF_HUB_CACHE", cache.join("hub")).env("LLAMA_CACHE", &cache);
            let a = build_args(&args, &snap)?;
            let _ = app.emit("llama-log", format!("[argus] llama-server {}", a.join(" ")));
            c.args(a);
            c
        }
    };

    let keep = args.keep_loaded.unwrap_or(false);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(!keep)
        .spawn().map_err(|e| format!("не удалось запустить движок ({}): {e}", st.llama_bin.display()))?;

    // Stream the engine log to the UI and to <temp>/argus-engine.log. llama.cpp draws download
    // progress with '\r', so split on both '\r' and '\n' or the UI looks frozen during a download.
    if let Some(mut err) = child.stderr.take() {
        let log_path = std::env::temp_dir().join("argus-engine.log");
        let _ = app.emit("llama-log", format!("[argus] engine log file: {}", log_path.display()));
        let (app, st) = (app.clone(), st.clone());
        tauri::async_runtime::spawn(async move {
            let mut file = std::fs::File::create(&log_path).ok();
            let mut buf = [0u8; 4096];
            let mut line: Vec<u8> = Vec::new();
            let mut last_progress = Instant::now() - Duration::from_secs(1);
            loop {
                let n = match err.read(&mut buf).await { Ok(0) | Err(_) => break, Ok(n) => n };
                if let Some(f) = file.as_mut() { use std::io::Write; let _ = f.write_all(&buf[..n]); }
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !line.is_empty() {
                            let text = strip_ansi(&String::from_utf8_lossy(&line)).trim_end().to_string();
                            if looks_like_error(&text) { st.engine_state.lock().await.last_err = Some(text.chars().take(240).collect()); }
                            // throttle \r progress redraws to ~4/s
                            if b == b'\n' || last_progress.elapsed().as_millis() > 250 {
                                last_progress = Instant::now();
                                let _ = app.emit("llama-log", text);
                            }
                            line.clear();
                        }
                    } else { line.push(b); }
                }
            }
            let _ = app.emit("llama-log", "[argus] engine process exited".to_string());
        });
    }
    // llama.cpp only draws download progress on a TTY, so report bytes landing in the cache instead.
    {
        let app2 = app.clone();
        let cache = model_cache_dir();
        tauri::async_runtime::spawn(async move {
            let start = dir_size(&cache);
            let mut last = start; let mut still = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(1000)).await;
                let now = dir_size(&cache);
                if now != last { still = 0; let _ = app2.emit("llama-dl", now.saturating_sub(start)); } else { still += 1; }
                last = now;
                if still > 20 { break; } // nothing written for 20 s: download done (or cached)
            }
        });
    }
    *st.engine.lock().await = Some(child);
    *st.engine_sig.lock().await = Some(sig);
    {
        let mut e = st.engine_state.lock().await;
        e.status = EngineStatus::Loading;
        e.reason = None;
        e.args = Some(args);
        e.nodes = used;
        e.started = Some(Instant::now());
        e.ever_healthy = false;
        e.pending_restart = false;
        e.adopted_pid = None;
        e.last_err = None;
        e.new_nodes = false;
        e.restarting = auto;
        if !auto { e.restarts.clear(); }
    }
    emit_engine(app, st.engine_view().await, None);
    Ok(format!("http://127.0.0.1:{LLAMA_PORT}"))
}

fn looks_like_error(l: &str) -> bool {
    let x = l.to_lowercase();
    (x.contains("error") || x.contains("failed") || x.contains("abort") || x.contains("exception") || x.contains("out of memory"))
        && !x.contains("error_log") && !x.contains("--")
}

// ---------------------------------------------------------------- supervisor

pub async fn supervise(app: &AppHandle, st: &Arc<AppState>) {
    let snap = st.snapshot().await;
    let alive = engine_running(st).await;
    // online endpoints -> (id, name)
    let online: Vec<(String, String, String)> = snap.nodes.iter().filter(|n| n.online())
        .filter_map(|n| n.endpoint.clone().map(|ep| (ep, n.id.clone(), n.name.clone()))).collect();
    let transitional = snap.nodes.iter().any(|n| n.stage.transitional());
    let cluster_sig = format!("{}|{}", snap.rpc_servers.join(","), transitional);

    let mut e = st.engine_state.lock().await;
    if e.cluster_sig != cluster_sig { e.cluster_sig = cluster_sig; e.cluster_stable_since = Instant::now(); }
    // learn names/ids of an adopted engine's endpoints
    for n in e.nodes.iter_mut() {
        if let Some((_, id, name)) = online.iter().find(|(ep, _, _)| *ep == n.endpoint) {
            if n.id.is_empty() { n.id = id.clone(); }
            n.name = name.clone();
        }
    }
    match e.status {
        EngineStatus::Idle => { e.new_nodes = false; }
        EngineStatus::Loading | EngineStatus::Ready => {
            if !alive {
                let healthy = e.ever_healthy;
                let err = e.last_err.clone();
                e.status = EngineStatus::Broken;
                e.pending_restart = healthy;
                e.reason = Some(match (healthy, err) {
                    (true, Some(x)) => format!("Движок остановился: {x}"),
                    (true, None) => "Движок неожиданно остановился".into(),
                    (false, Some(x)) => format!("Движок не запустился: {x}"),
                    (false, None) => "Движок не запустился — подробности в журнале движка".into(),
                });
                drop(e);
                *st.engine.lock().await = None;
                *st.engine_sig.lock().await = None;
                let v = st.engine_view().await;
                emit_engine(app, v, None);
                return;
            }
            let grace = e.adopted_pid.is_some() && e.started.map(|t| t.elapsed() < ADOPT_GRACE).unwrap_or(false);
            let lost: Vec<String> = e.nodes.iter().filter(|n| !online.iter().any(|(ep, _, _)| *ep == n.endpoint))
                .map(|n| n.name.clone()).collect();
            if !lost.is_empty() && !grace {
                let who = lost.iter().map(|n| format!("«{n}»")).collect::<Vec<_>>().join(", ");
                e.status = EngineStatus::Broken;
                e.pending_restart = true;
                e.reason = Some(format!("Нода {who} отключилась — движок остановлен. Модель перезапустится автоматически, когда нода вернётся"));
                drop(e);
                kill_engine(st).await;
                let v = st.engine_view().await;
                emit_engine(app, v, Some(&format!("Нода {who} отключилась — генерация остановлена")));
                return;
            }
            let used: HashSet<String> = e.nodes.iter().map(|x| x.endpoint.clone()).collect();
            e.new_nodes = snap.nodes.iter().any(|n| n.online() && n.enabled && n.endpoint.as_ref()
                .map(|ep| !used.contains(ep)).unwrap_or(false));
            if e.status == EngineStatus::Loading {
                let due = e.last_health_check.map(|t| t.elapsed() >= Duration::from_millis(900)).unwrap_or(true);
                if due {
                    e.last_health_check = Some(Instant::now());
                    drop(e);
                    if health_ok().await {
                        let mut e = st.engine_state.lock().await;
                        if e.status == EngineStatus::Loading {
                            e.status = EngineStatus::Ready;
                            e.ever_healthy = true;
                            let auto = std::mem::take(&mut e.restarting);
                            drop(e);
                            let v = st.engine_view().await;
                            emit_engine(app, v, auto.then_some("Кластер изменился — модель перезапущена"));
                        }
                    }
                }
            }
        }
        EngineStatus::Broken => {
            if !e.pending_restart || e.args.is_none() { return; }
            let disabled = st.settings.lock().await.disabled.clone();
            let missing: Vec<String> = e.nodes.iter()
                .filter(|n| !online.iter().any(|(ep, id, _)| *ep == n.endpoint || (!n.id.is_empty() && *id == n.id)))
                .filter(|n| n.id.is_empty() || !disabled.contains(&n.id))
                .map(|n| n.name.clone()).collect();
            if !missing.is_empty() {
                let who = missing.iter().map(|n| format!("«{n}»")).collect::<Vec<_>>().join(", ");
                let r = format!("Нода {who} отключилась — ждём её возвращения, затем модель перезапустится автоматически (или нажмите «Перезапустить движок»)");
                if e.reason.as_deref() != Some(r.as_str()) {
                    e.reason = Some(r);
                    drop(e);
                    let v = st.engine_view().await;
                    emit_engine(app, v, None);
                }
                return;
            }
            if transitional || e.cluster_stable_since.elapsed() < SETTLE { return; }
            e.restarts.retain(|t| t.elapsed() < RESTART_WINDOW);
            if e.restarts.len() >= RESTART_BUDGET {
                e.pending_restart = false;
                e.reason = Some(format!("{} — автоперезапуск отключён после {RESTART_BUDGET} попыток за 10 минут, нажмите «Перезапустить движок»",
                    e.reason.clone().unwrap_or_default()));
                drop(e);
                let v = st.engine_view().await;
                emit_engine(app, v, None);
                return;
            }
            e.restarts.push(Instant::now());
            e.pending_restart = false;
            let args = e.args.clone().unwrap_or_default();
            drop(e);
            let _ = app.emit("llama-log", "[argus] кластер восстановлен — перезапускаю движок".to_string());
            if let Err(err) = launch(app, st, args, true).await {
                let mut e = st.engine_state.lock().await;
                e.status = EngineStatus::Broken;
                e.reason = Some(format!("Автоперезапуск не удался: {err}"));
                drop(e);
                let v = st.engine_view().await;
                emit_engine(app, v, None);
            }
        }
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn start_model(app: AppHandle, state: State<'_, Arc<AppState>>, args: StartModelArgs) -> Result<String, String> {
    launch(&app, state.inner(), args, false).await
}

/// "Перезапустить движок": same model, current node set.
#[tauri::command]
pub async fn restart_engine(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let st = state.inner().clone();
    let args = {
        let e = st.engine_state.lock().await;
        e.args.clone()
    };
    let args = match args { Some(a) => a, None => st.settings.lock().await.last_launch.clone().ok_or("Нет модели для перезапуска — выберите её в Models")? };
    let mut args = args;
    args.keep_loaded = Some(false); // force a real restart even if the signature matches
    let keep = st.settings.lock().await.keep_loaded;
    let r = launch(&app, &st, args.clone(), false).await;
    // restore the user's keep-loaded choice (launch stored the forced value)
    {
        let mut s = st.settings.lock().await;
        s.keep_loaded = keep;
        if let Some(l) = s.last_launch.as_mut() { l.keep_loaded = Some(keep); }
    }
    if let Some(a) = st.engine_state.lock().await.args.as_mut() { a.keep_loaded = Some(keep); }
    st.save_settings().await;
    r
}

#[tauri::command]
pub async fn stop_model(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let st = state.inner().clone();
    let _guard = st.launch_lock.lock().await;
    {
        let mut e = st.engine_state.lock().await;
        e.status = EngineStatus::Idle;
        e.args = None;
        e.nodes.clear();
        e.pending_restart = false;
        e.reason = None;
        e.restarting = false;
    }
    kill_engine(&st).await;
    emit_engine(&app, st.engine_view().await, None);
    Ok(())
}

/// UI toggle "Держать модель загруженной": decides whether Argus stops the engine on exit.
#[tauri::command]
pub async fn set_keep_loaded(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.settings.lock().await.keep_loaded = enabled;
    state.save_settings().await;
    Ok(())
}

#[tauri::command]
pub async fn model_status(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    if engine_running(&state).await { return Ok(true); }
    // an engine left running by a previous Argus session (keep-loaded) also counts
    Ok(state.engine.lock().await.is_none() && port_open().await)
}

async fn port_open() -> bool {
    matches!(timeout(Duration::from_millis(300), TcpStream::connect(("127.0.0.1", LLAMA_PORT))).await, Ok(Ok(_)))
}

/// ASCII-only model cache.
/// Windows: %ARGUS_MODELS% > LLAMA_CACHE/HF_HOME > %ProgramData%\Argus\models > C:\ArgusModels.
/// Linux:   $ARGUS_MODELS > LLAMA_CACHE/HF_HOME > $XDG_DATA_HOME/argus/models > ~/.local/share/argus/models.
pub fn model_cache_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("ARGUS_MODELS") { if !p.is_empty() { return p.into(); } }
    // respect a cache the user already set up, as long as llama.cpp can use it
    for k in ["LLAMA_CACHE", "HF_HOME"] {
        if let Ok(p) = std::env::var(k) { if !p.is_empty() && p.is_ascii() { return p.into(); } }
    }
    #[cfg(windows)]
    {
        let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
        let p = std::path::PathBuf::from(base).join("Argus").join("models");
        if p.to_string_lossy().is_ascii() { p } else { "C:\\ArgusModels".into() }
    }
    #[cfg(not(windows))]
    {
        let data = std::env::var("XDG_DATA_HOME").ok().filter(|p| !p.is_empty()).map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local").join("share")))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        data.join("argus").join("models")
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') { it.next(); while let Some(n) = it.next() { if n.is_ascii_alphabetic() { break; } } }
        } else { out.push(c); }
    }
    out
}

fn dir_size(p: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten().map(|e| match e.metadata() {
        Ok(m) if m.is_dir() => dir_size(&e.path()),
        Ok(m) => m.len(),
        _ => 0,
    }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_cmdline() {
        let c: Vec<String> = ["llama-server", "-hf", "x", "--rpc", "127.0.0.1:50052,192.168.1.5:50052", "--device", "RPC0"].iter().map(|s| s.to_string()).collect();
        assert_eq!(rpc_from_cmdline(&c), vec!["127.0.0.1:50052", "192.168.1.5:50052"]);
        assert!(rpc_from_cmdline(&c[..3]).is_empty());
    }
    #[test]
    fn error_lines() {
        assert!(looks_like_error("ggml_backend_rpc: RPC error: failed to send"));
        assert!(!looks_like_error("main: server is listening on http://127.0.0.1:8080"));
    }
}
