//! Launches the inference engine with every connected node as an RPC backend.
use crate::adb::hidden_command;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::{process::Stdio, sync::Arc};
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncReadExt;

pub const LLAMA_PORT: u16 = 8080;

#[derive(Deserialize, Serialize, Clone, Default)]
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

pub async fn engine_running(st: &AppState) -> bool {
    let mut g = st.engine.lock().await;
    match g.as_mut() {
        Some(c) => matches!(c.try_wait(), Ok(None)),
        None => false,
    }
}

#[tauri::command]
pub async fn start_model(app: AppHandle, state: State<'_, Arc<AppState>>, args: StartModelArgs) -> Result<String, String> {
    let snap = state.snapshot().await;
    let rpc = snap.rpc_servers.join(",");
    let sig = serde_json::to_string(&(&args.hf_repo, &args.hf_file, &args.model_path, &args.ctx, &args.lan,
        &args.draft_hf, &args.cache_reuse, &rpc, snap.split.iter().map(|e| &e.device).collect::<Vec<_>>())).unwrap_or_default();
    // keep-loaded: the identical engine is already up -> nothing to do
    if args.keep_loaded.unwrap_or(false) && engine_running(&state).await
        && state.engine_sig.lock().await.as_deref() == Some(sig.as_str()) {
        let _ = app.emit("llama-log", "[argus] same model already loaded — reusing the running engine".to_string());
        return Ok(format!("http://127.0.0.1:{LLAMA_PORT}"));
    }
    stop(&state).await;

    let mut cmd = match args.engine.as_deref().unwrap_or("llama.cpp") {
        // Upstream Ollama has NO RPC support (PR #6729 was never merged). This branch only works
        // with a fork that reads OLLAMA_RPC_SERVERS; kept for experimentation.
        "ollama" => {
            let mut c = hidden_command("ollama");
            c.arg("serve").env("OLLAMA_RPC_SERVERS", &rpc);
            c
        }
        _ => {
            let mut c = hidden_command(&state.llama_bin);
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

    let app_for_dl = app.clone();
    let keep = args.keep_loaded.unwrap_or(false);
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(!keep)
        .spawn().map_err(|e| format!("failed to start engine: {e}"))?;

    // Stream the engine log to the UI and to %TEMP%\argus-engine.log. llama.cpp draws download
    // progress with '\r', so split on both '\r' and '\n' or the UI looks frozen during a download.
    if let Some(mut err) = child.stderr.take() {
        let log_path = std::env::temp_dir().join("argus-engine.log");
        let _ = app.emit("llama-log", format!("[argus] engine log file: {}", log_path.display()));
        tauri::async_runtime::spawn(async move {
            let mut file = std::fs::File::create(&log_path).ok();
            let mut buf = [0u8; 4096];
            let mut line: Vec<u8> = Vec::new();
            let mut last_progress = std::time::Instant::now() - std::time::Duration::from_secs(1);
            loop {
                let n = match err.read(&mut buf).await { Ok(0) | Err(_) => break, Ok(n) => n };
                if let Some(f) = file.as_mut() { use std::io::Write; let _ = f.write_all(&buf[..n]); }
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        if !line.is_empty() {
                            let text = strip_ansi(&String::from_utf8_lossy(&line)).trim_end().to_string();
                            // throttle \r progress redraws to ~4/s
                            if b == b'\n' || last_progress.elapsed().as_millis() > 250 {
                                last_progress = std::time::Instant::now();
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
        let app2 = app_for_dl.clone();
        let cache = model_cache_dir();
        tauri::async_runtime::spawn(async move {
            let start = dir_size(&cache);
            let mut last = start; let mut still = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                let now = dir_size(&cache);
                if now != last { still = 0; let _ = app2.emit("llama-dl", now.saturating_sub(start)); } else { still += 1; }
                last = now;
                if still > 20 { break; } // nothing written for 20 s: download done (or cached)
            }
        });
    }
    *state.engine.lock().await = Some(child);
    *state.engine_sig.lock().await = Some(sig);
    Ok(format!("http://127.0.0.1:{LLAMA_PORT}"))
}

#[tauri::command]
pub async fn stop_model(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    stop(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn model_status(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    if engine_running(&state).await { return Ok(true); }
    // an engine left running by a previous Argus session (keep-loaded) also counts
    Ok(state.engine.lock().await.is_none() && port_open().await)
}

async fn port_open() -> bool {
    matches!(tokio::time::timeout(std::time::Duration::from_millis(300),
        tokio::net::TcpStream::connect(("127.0.0.1", LLAMA_PORT))).await, Ok(Ok(_)))
}

async fn stop(state: &AppState) {
    *state.engine_sig.lock().await = None;
    if let Some(mut c) = state.engine.lock().await.take() {
        let _ = c.kill().await;
        return;
    }
    // not our child: a kept-loaded engine from a previous session still holding :8080
    if port_open().await {
        #[cfg(windows)]
        { let _ = hidden_command("taskkill").args(["/F", "/IM", "llama-server.exe"]).output().await; }
        #[cfg(not(windows))]
        { let _ = hidden_command("pkill").args(["-f", "llama-server"]).output().await; }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// ASCII-only model cache: %ARGUS_MODELS% > %ProgramData%\Argus\models > C:\ArgusModels.
pub fn model_cache_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("ARGUS_MODELS") { return p.into(); }
    // respect a cache the user already set up, as long as llama.cpp can use it
    for k in ["LLAMA_CACHE", "HF_HOME"] {
        if let Ok(p) = std::env::var(k) { if !p.is_empty() && p.is_ascii() { return p.into(); } }
    }
    let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    let p = std::path::PathBuf::from(base).join("Argus").join("models");
    if p.to_string_lossy().is_ascii() { p } else { "C:\\ArgusModels".into() }
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
