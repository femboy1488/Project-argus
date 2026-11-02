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

/// download counter generation (only the newest launch's loop reports) and its byte base
static DL_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DL_BASE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

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
    /// 0.3.2 warm-up: system prompt of the current chat, processed once right after load (prompt cache)
    #[serde(default)]
    pub warmup_system: Option<String>,
    /// vision projector: local path, "owner/repo/file.gguf" or a file in hf_repo (-hf fetches its own mmproj)
    #[serde(default)] pub mmproj: Option<String>,
    /// only local GPUs (no phones, no PC CPU layers)
    #[serde(default)] pub gpu_only: Option<bool>,
}

/// 0.3.2: the split plan for a launch (roles + minimal node set + speed-first -ts).
pub fn launch_plan(args: &StartModelArgs, snap: &crate::state::ClusterSnapshot) -> crate::split::Plan {
    let roles_on = snap.roles_enabled;
    // header info from the snapshot cache only (filled by launch / plan_split): pure, no disk access
    let model = match (&args.model_path, args.hf_file.as_deref().and_then(|f| f.rsplit(['/', '\\']).next())) {
        (Some(p), _) => snap.gguf.get(p).cloned(),
        (None, Some(base)) => snap.gguf.iter().find(|(k, _)| k.ends_with(base)).map(|(_, v)| v.clone()),
        _ => None,
    };
    let opts = crate::split::PlanOpts {
        gpu_only: args.gpu_only.unwrap_or(false), cpu: snap.cpu.dev(),
        draft_mb: snap.draft.as_ref().map(|d| d.1).unwrap_or(0), model,
    };
    let mut p = crate::split::plan_ex(&snap.nodes, &snap.gpus, args.need_mb, &|n| crate::state::role_of(n, roles_on), &opts);
    p.draft = snap.draft.is_some();
    p.rpc_endpoints = plan_endpoints(&p, snap);
    p
}

/// Local .gguf of a launch: model_path, or the -hf file already in the model cache.
pub fn local_model_path(args: &StartModelArgs) -> Option<String> {
    if let Some(p) = args.model_path.as_ref().filter(|p| !p.is_empty()) { return Some(p.clone()); }
    let f = args.hf_file.as_deref().filter(|f| !f.is_empty())?;
    let base = f.rsplit(['/', '\\']).next()?.to_string();
    find_file(&model_cache_dir(), &base, 4).map(|p| p.display().to_string())
}

fn find_file(dir: &Path, name: &str, depth: u32) -> Option<std::path::PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = vec![];
    for e in rd.flatten() {
        let p = e.path();
        let fname = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() { subdirs.push(p); } else if fname == name || fname.ends_with(&format!("_{name}")) { return Some(p); }
    }
    if depth == 0 { return None; }
    subdirs.into_iter().find_map(|d| find_file(&d, name, depth - 1))
}

/// Draft model file when «Черновая модель» is on and the file exists.
pub fn draft_file(s: &crate::settings::Settings) -> Option<std::path::PathBuf> {
    if !s.draft_enabled { return None; }
    if let Some(p) = s.draft_path.as_ref().filter(|p| !p.is_empty()) {
        let p = std::path::PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let dir = model_cache_dir().join("draft");
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path())
        .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("gguf")).unwrap_or(false)).collect();
    v.sort();
    v.into_iter().next()
}

/// GPU memory for the draft model (weights + its q8 KV), 0 when no draft.
pub fn draft_mb(s: &crate::settings::Settings) -> u64 {
    draft_file(s).and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len() / 1_048_576 + 256).unwrap_or(0)
}

pub fn sessions_dir() -> std::path::PathBuf { model_cache_dir().join("sessions") }

// ---------------------------------------------------------------- GGUF header

/// Minimal GGUF reader: block_count / expert_count / expert_used_count + MB of expert tensors.
pub fn gguf_info(path: &Path) -> Option<crate::split::ModelInfo> {
    use std::io::{BufReader, Read, Seek, SeekFrom};
    let f = std::fs::File::open(path).ok()?;
    let file_len = f.metadata().ok()?.len();
    let mut r = BufReader::with_capacity(1 << 16, f);
    fn u32_(r: &mut impl Read) -> Option<u32> { let mut b = [0u8; 4]; r.read_exact(&mut b).ok()?; Some(u32::from_le_bytes(b)) }
    fn u64_(r: &mut impl Read) -> Option<u64> { let mut b = [0u8; 8]; r.read_exact(&mut b).ok()?; Some(u64::from_le_bytes(b)) }
    fn str_(r: &mut impl Read) -> Option<String> {
        let n = u64_(r)?; if n > 1 << 20 { return None; }
        let mut b = vec![0u8; n as usize]; r.read_exact(&mut b).ok()?; Some(String::from_utf8_lossy(&b).into_owned())
    }
    fn fixed(t: u32) -> Option<i64> { Some(match t { 0 | 1 | 7 => 1, 2 | 3 => 2, 4 | 5 | 6 => 4, 10 | 11 | 12 => 8, _ => return None }) }
    fn num<R: Read>(r: &mut R, t: u32) -> Option<u64> {
        Some(match t {
            0 | 1 | 7 => { let mut b = [0u8; 1]; r.read_exact(&mut b).ok()?; b[0] as u64 }
            2 | 3 => { let mut b = [0u8; 2]; r.read_exact(&mut b).ok()?; u16::from_le_bytes(b) as u64 }
            4 | 5 => u32_(r)? as u64,
            6 => { u32_(r)?; 0 }
            10 | 11 => u64_(r)?,
            12 => { u64_(r)?; 0 }
            _ => return None,
        })
    }
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" { return None; }
    let ver = u32_(&mut r)?;
    if ver < 2 { return None; }
    let n_tensors = u64_(&mut r)?;
    let n_kv = u64_(&mut r)?;
    let mut ints: std::collections::HashMap<String, u64> = Default::default();
    let mut arch = String::new();
    for _ in 0..n_kv {
        let key = str_(&mut r)?;
        let t = u32_(&mut r)?;
        match t {
            8 => { let v = str_(&mut r)?; if key == "general.architecture" { arch = v; } }
            9 => {
                let it = u32_(&mut r)?;
                let n = u64_(&mut r)?;
                if let Some(sz) = fixed(it) { r.seek(SeekFrom::Current(sz * n as i64)).ok()?; }
                else if it == 8 { for _ in 0..n { let l = u64_(&mut r)?; r.seek(SeekFrom::Current(l as i64)).ok()?; } }
                else { return None; } // nested arrays: not in real models
            }
            _ => { let v = num(&mut r, t)?; ints.insert(key, v); }
        }
    }
    let get = |k: &str| ints.get(&format!("{arch}.{k}")).copied().unwrap_or(0) as u32;
    let align = ints.get("general.alignment").copied().filter(|a| *a > 0).unwrap_or(32);
    // tensor infos: name, n_dims, dims, type, offset
    let mut tensors: Vec<(bool, u64)> = Vec::with_capacity(n_tensors.min(100_000) as usize);
    for _ in 0..n_tensors {
        let name = str_(&mut r)?;
        let nd = u32_(&mut r)?;
        if nd > 8 { return None; }
        for _ in 0..nd { u64_(&mut r)?; }
        u32_(&mut r)?;
        let off = u64_(&mut r)?;
        tensors.push((name.contains("_exps"), off));
    }
    let pos = r.stream_position().ok()?;
    let data_start = pos.div_ceil(align) * align;
    let mut order: Vec<usize> = (0..tensors.len()).collect();
    order.sort_by_key(|&i| tensors[i].1);
    let data_len = file_len.saturating_sub(data_start);
    let mut experts = 0u64;
    for (k, &i) in order.iter().enumerate() {
        let end = order.get(k + 1).map(|&j| tensors[j].1).unwrap_or(data_len);
        if tensors[i].0 { experts += end.saturating_sub(tensors[i].1); }
    }
    Some(crate::split::ModelInfo {
        block_count: get("block_count"), expert_count: get("expert_count"), expert_used: get("expert_used_count"),
        experts_mb: experts / 1_048_576, weights_mb: data_len / 1_048_576,
    })
}

/// `--rpc` endpoints of a plan, in order.
pub fn plan_endpoints(plan: &crate::split::Plan, snap: &crate::state::ClusterSnapshot) -> Vec<String> {
    plan.rpc_ids.iter().filter_map(|id| snap.nodes.iter().find(|n| &n.id == id).and_then(|n| n.endpoint.clone())).collect()
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
    let full = launch_plan(args, snap);
    let rpc = plan_endpoints(&full, snap);
    let devices = crate::split::device_list(&full);
    if !rpc.is_empty() { push(&["--rpc", &rpc.join(",")]); }
    let local_gpu = snap.gpus.iter().find(|g| g.enabled && g.device.is_some()).and_then(|g| g.device.clone());
    if !snap.gpus.is_empty() {
        // llama.cpp's device list is known: always pass it explicitly so -ts lines up and disabled GPUs stay idle
        if devices.is_empty() {
            push(&["--device", "none"]); // every device disabled: CPU only
        } else {
            push(&["--device", &devices.join(",")]);
            if devices.len() > 1 { push(&["-ts", &crate::split::ts_for_plan(&full)]); }
            // CPU pseudo-node: the FIRST cpu_layers layers stay on the PC CPU (-ngl < n_layer + 1)
            match full.n_layers.filter(|_| full.cpu_layers > 0 && full.moe.is_none()) {
                Some(nl) => push(&["-ngl", &(nl + 1).saturating_sub(full.cpu_layers).max(1).to_string()]),
                None => push(&["-ngl", "999"]),
            }
            // MoE: expert tensors of the offloaded blocks → rpc buffers / CPU (after --rpc)
            if let Some(ot) = crate::split::ot_arg(&full, &rpc) { push(&["-ot", &ot]); }
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

    if let Some(t) = full.threads { push(&["-t", &t.to_string()]); }
    // chat sessions: POST /slots/0?action=save|restore
    push(&["--slot-save-path", &sessions_dir().display().to_string()]);

    // ---- speed
    if let Some(n) = args.cache_reuse.filter(|n| *n > 0) { push(&["--cache-reuse", &n.to_string()]); }
    if let Some((d, _)) = snap.draft.as_ref() {
        // local draft model (settings «Черновая модель»): on the local GPU, q8 KV
        push(&["-md", d, "--spec-type", "draft-simple", "--spec-draft-n-max", "8", "--spec-draft-n-min", "2"]);
        match &local_gpu {
            Some(g) => push(&["-devd", g, "-ngld", "99"]),
            None => push(&["-devd", "none"]),
        }
        push(&["-ctkd", "q8_0", "-ctvd", "q8_0"]);
    } else if let Some(d) = args.draft_hf.as_deref().filter(|d| !d.is_empty()) {
        push(&["-hfd", d, "--spec-type", "draft-simple"]);
        push(&["--spec-draft-n-max", &args.draft_max.unwrap_or(16).to_string(), "--spec-draft-n-min", "1"]);
        // the draft is small: keep it entirely on the local GPU (or CPU) — never on a phone over USB
        match &local_gpu {
            Some(g) => push(&["-devd", g, "-ngld", "99"]),
            None => push(&["-devd", "none"]),
        }
    }
    a.extend(crate::vision::mmproj_args(args.mmproj.as_deref(), args.hf_repo.as_deref()));
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
    // GGUF header (layers / experts) for the plan: cached by path
    if let Some(p) = local_model_path(&args) { let _ = st.model_info(&p).await; }
    let snap = st.snapshot().await;
    let plan = launch_plan(&args, &snap);
    let rpc = plan_endpoints(&plan, &snap).join(",");
    let sig = serde_json::to_string(&(&args.hf_repo, &args.hf_file, &args.model_path, &args.ctx, &args.lan,
        &args.draft_hf, &args.cache_reuse, &rpc, plan.entries.iter().map(|e| (&e.device, (e.share * 1000.0).round() as u32)).collect::<Vec<_>>(),
        (plan.cpu_layers, plan.threads, plan.draft, plan.moe.as_ref().map(|m| m.gpu_blocks.len()), &args.gpu_only))).unwrap_or_default();
    if plan.cpu_layers > 0 { let _ = app.emit("llama-log", format!("[argus] {} слоёв на процессоре ПК ({} потоков)", plan.cpu_layers, plan.threads.unwrap_or(0))); }
    if let Some(m) = &plan.moe { let _ = app.emit("llama-log", format!("[argus] MoE: внимание и KV на GPU, эксперты {} блоков вынесены", m.block_count as usize - m.gpu_blocks.len())); }
    let _ = std::fs::create_dir_all(sessions_dir());
    // keep-loaded: the identical engine is already up -> nothing to do
    if !auto && args.keep_loaded.unwrap_or(false) && engine_running(st).await
        && st.engine_sig.lock().await.as_deref() == Some(sig.as_str()) {
        let _ = app.emit("llama-log", "[argus] same model already loaded — reusing the running engine".to_string());
        return Ok(format!("http://127.0.0.1:{LLAMA_PORT}"));
    }
    kill_engine(st).await;

    // only the nodes the plan needs (0.3.2): «Свободна» / «Помощник» nodes stay out of --rpc
    let used: Vec<EngineNode> = plan.rpc_ids.iter().filter_map(|id| snap.nodes.iter().find(|n| &n.id == id))
        .filter_map(|n| n.endpoint.clone().map(|ep| EngineNode { id: n.id.clone(), name: n.name.clone(), endpoint: ep })).collect();
    if !plan.free_ids.is_empty() {
        let names: Vec<String> = plan.free_ids.iter().filter_map(|id| snap.nodes.iter().find(|n| &n.id == id).map(|n| n.name.clone())).collect();
        let _ = app.emit("llama-log", format!("[argus] модель помещается без: {} — они свободны", names.join(", ")));
    }
    if let Some(t) = plan.est_tps { let _ = app.emit("llama-log", format!("[argus] оценка скорости: ≈{t:.1} ток/с")); }
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
    // One counter at a time: a newer launch bumps DL_GEN so the previous loop exits (no duplicate counters);
    // an automatic restart keeps the previous base so the shown downloaded amount does not jump back to 0.
    {
        use std::sync::atomic::Ordering;
        let gen = DL_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        let app2 = app.clone();
        let cache = model_cache_dir();
        tauri::async_runtime::spawn(async move {
            let c2 = cache.clone();
            let now0 = tokio::task::spawn_blocking(move || dir_size(&c2)).await.unwrap_or(0);
            let prev = DL_BASE.load(Ordering::SeqCst);
            let start = if auto && prev != u64::MAX && prev <= now0 { prev } else { now0 };
            DL_BASE.store(start, Ordering::SeqCst);
            let mut last = now0; let mut still = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(1000)).await;
                if DL_GEN.load(Ordering::SeqCst) != gen { break; }
                let c2 = cache.clone();
                let now = tokio::task::spawn_blocking(move || dir_size(&c2)).await.unwrap_or(last);
                if DL_GEN.load(Ordering::SeqCst) != gen { break; }
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
        e.warming = false;
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
            // «новая нода»: the plan for this model would now use a node the engine doesn't have
            let used: HashSet<String> = e.nodes.iter().map(|x| x.endpoint.clone()).collect();
            e.new_nodes = match e.args.as_ref() {
                Some(a) => plan_endpoints(&launch_plan(a, &snap), &snap).iter().any(|ep| !used.contains(ep)),
                None => false,
            };
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
                            e.warming = true;
                            let system = e.args.as_ref().and_then(|a| a.warmup_system.clone());
                            let auto = std::mem::take(&mut e.restarting);
                            drop(e);
                            let v = st.engine_view().await;
                            emit_engine(app, v, auto.then_some("Кластер изменился — модель перезапущена"));
                            let (app2, st2) = (app.clone(), st.clone());
                            tauri::async_runtime::spawn(async move { warmup(&app2, &st2, system).await });
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

/// 0.3.2 «Прогрев»: right after load, one tiny generation (1 token) with the chat's system prompt.
/// The first real request then finds the weights paged in, the compute graphs/buffers allocated on every node
/// and the system prompt already in the KV cache (llama-server keeps the slot's prompt), so it starts at once.
async fn warmup(app: &AppHandle, st: &Arc<AppState>, system: Option<String>) {
    let t0 = Instant::now();
    let mut messages = vec![];
    if let Some(sys) = system.filter(|s| !s.trim().is_empty()) { messages.push(serde_json::json!({ "role": "system", "content": sys })); }
    messages.push(serde_json::json!({ "role": "user", "content": "Привет" }));
    let messages = crate::features::prepare_messages(&format!("http://127.0.0.1:{LLAMA_PORT}"), &serde_json::Value::Array(messages)).await;
    let body = serde_json::json!({ "messages": messages, "max_tokens": 1, "temperature": 0, "cache_prompt": true,
                                   "chat_template_kwargs": { "enable_thinking": false } });
    let client = reqwest::Client::builder().no_proxy().build().unwrap_or_default();
    let r = client.post(format!("http://127.0.0.1:{LLAMA_PORT}/v1/chat/completions")).json(&body)
        .timeout(Duration::from_secs(180)).send().await;
    let msg = match r {
        Ok(x) if x.status().is_success() => format!("[argus] прогрев готов за {:.1} с", t0.elapsed().as_secs_f32()),
        Ok(x) => format!("[argus] прогрев: HTTP {} — не страшно, просто первый ответ будет медленнее", x.status()),
        Err(e) => format!("[argus] прогрев не удался ({e}) — первый ответ будет медленнее"),
    };
    let _ = app.emit("llama-log", msg);
    st.engine_state.lock().await.warming = false;
    let v = st.engine_view().await;
    emit_engine(app, v, None);
}

/// 0.3.2: split preview for a model size (Models: which nodes would work, which stay free, ≈ tok/s).
#[tauri::command]
pub async fn plan_split(state: State<'_, Arc<AppState>>, need_mb: Option<u64>, model_path: Option<String>,
                        gpu_only: Option<bool>) -> Result<crate::split::Plan, String> {
    let args = StartModelArgs { need_mb, model_path: model_path.filter(|p| !p.is_empty()), gpu_only, ..Default::default() };
    if let Some(p) = local_model_path(&args) { let _ = state.model_info(&p).await; }
    let snap = state.snapshot().await;
    Ok(launch_plan(&args, &snap))
}

// ---------------------------------------------------------------- chat sessions (KV save/restore)

fn session_file(chat_id: &str) -> Result<String, String> {
    let id: String = chat_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(64).collect();
    if id.is_empty() { return Err("пустой id чата".into()); }
    Ok(format!("{id}.bin"))
}

async fn slot_action(action: &str, file: &str) -> Result<reqwest::Response, String> {
    let client = reqwest::Client::builder().no_proxy().build().map_err(|e| e.to_string())?;
    client.post(format!("http://127.0.0.1:{LLAMA_PORT}/slots/0?action={action}"))
        .json(&serde_json::json!({ "filename": file })).timeout(Duration::from_secs(120)).send().await
        .map_err(|e| format!("движок не отвечает: {e}"))
}

/// Keep at most `keep` newest session files.
fn prune_sessions(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut v: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd.flatten()
        .filter(|e| e.path().extension().map(|x| x == "bin").unwrap_or(false))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in v.into_iter().skip(keep) { let _ = std::fs::remove_file(p); }
}

/// Saves the slot KV cache of the current chat to <models>/sessions/<chatId>.bin (≤ 12 files kept).
#[tauri::command]
pub async fn save_chat_session(chat_id: String) -> Result<String, String> {
    let file = session_file(&chat_id)?;
    let _ = std::fs::create_dir_all(sessions_dir());
    let r = slot_action("save", &file).await?;
    if !r.status().is_success() {
        let t = r.text().await.unwrap_or_default();
        return Err(format!("сохранение сессии: {}", t.chars().take(200).collect::<String>()));
    }
    prune_sessions(&sessions_dir(), 12);
    Ok(file)
}

/// Restores a saved chat KV cache; false when this chat has no saved session.
#[tauri::command]
pub async fn restore_chat_session(chat_id: String) -> Result<bool, String> {
    let file = session_file(&chat_id)?;
    let path = sessions_dir().join(&file);
    if !path.is_file() { return Ok(false); }
    let r = slot_action("restore", &file).await?;
    if !r.status().is_success() {
        let t = r.text().await.unwrap_or_default();
        return Err(format!("восстановление сессии: {}", t.chars().take(200).collect::<String>()));
    }
    // touch: recently used sessions survive pruning
    let _ = std::fs::File::options().append(true).open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    Ok(true)
}

/// «Черновая модель» (speculative decoding) on/off; optional local .gguf path. Applies at the next model start.
#[tauri::command]
pub async fn set_draft_enabled(state: State<'_, Arc<AppState>>, enabled: bool, path: Option<String>) -> Result<(), String> {
    {
        let mut s = state.settings.lock().await;
        s.draft_enabled = enabled;
        if let Some(p) = path { s.draft_path = Some(p).filter(|p| !p.trim().is_empty()); }
    }
    state.save_settings().await;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftStatus { pub enabled: bool, pub path: Option<String>, pub present: bool, pub size_mb: u64 }

#[tauri::command]
pub async fn get_draft_status(state: State<'_, Arc<AppState>>) -> Result<DraftStatus, String> {
    let s = state.settings.lock().await.clone();
    let on = crate::settings::Settings { draft_enabled: true, ..s.clone() };
    let f = draft_file(&on);
    let size_mb = f.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len() / 1_048_576).unwrap_or(0);
    Ok(DraftStatus { enabled: s.draft_enabled, path: f.map(|p| p.display().to_string()).or(s.draft_path), present: size_mb > 0, size_mb })
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
