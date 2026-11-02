//! «Ночная очередь»: batch jobs over a folder (translate / summarize / custom prompt) run one at a time
//! through the local llama-server (OpenAI API, non-stream). Saved in argus-queue.json next to the settings;
//! after a restart finished files are skipped (resume at file granularity). Emits `queue-update`.
use crate::llama::LLAMA_PORT;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::{path::{Path, PathBuf}, sync::{Arc, OnceLock}, time::Duration};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub kind: String, // translate | summarize | custom
    #[serde(default)] pub prompt: String,
    #[serde(default)] pub lang: String,
    pub in_dir: String,
    pub out_dir: String,
    #[serde(default)] pub start_at: Option<i64>, // unix seconds
    pub status: String, // pending | running | done | error
    #[serde(default)] pub progress: f32, // 0..1
    #[serde(default)] pub message: String,
    #[serde(default)] pub done_files: Vec<String>,
    #[serde(default)] pub total_files: usize,
    #[serde(default)] pub current: Option<String>,
    #[serde(default)] pub errors: Vec<String>,
    #[serde(default)] pub created: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct QueueView { pub running: bool, pub jobs: Vec<Job> }

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewJob {
    pub kind: String,
    #[serde(default)] pub prompt: String,
    #[serde(default)] pub lang: String,
    pub in_dir: String,
    #[serde(default)] pub out_dir: String,
    #[serde(default)] pub start_at: Option<i64>,
}

struct Q { data: Mutex<QueueView>, path: Option<PathBuf>, app: AppHandle }
static QUEUE: OnceLock<Q> = OnceLock::new();

fn now() -> i64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

pub fn data_dir(st: &AppState) -> Option<PathBuf> { st.config_path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())) }

fn save(q: &Q, v: &QueueView) {
    let Some(p) = &q.path else { return };
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, s).is_ok() { let _ = std::fs::rename(&tmp, p); }
    }
}

async fn publish(q: &Q) {
    let v = q.data.lock().await.clone();
    save(q, &v);
    let _ = q.app.emit("queue-update", v);
}

async fn patch(q: &Q, id: &str, f: impl FnOnce(&mut Job)) {
    { let mut d = q.data.lock().await; if let Some(j) = d.jobs.iter_mut().find(|j| j.id == id) { f(j); } }
    publish(q).await;
}

/// Called once from main's setup: loads the saved queue and starts the worker.
pub fn init(app: AppHandle, st: Arc<AppState>) {
    let path = data_dir(&st).map(|d| d.join("argus-queue.json"));
    let mut v: QueueView = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    for j in v.jobs.iter_mut() { if j.status == "running" { j.status = "pending".into(); j.current = None; } }
    let _ = QUEUE.set(Q { data: Mutex::new(v), path, app });
    tauri::async_runtime::spawn(worker());
}

async fn worker() {
    let Some(q) = QUEUE.get() else { return };
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let next = {
            let d = q.data.lock().await;
            if !d.running { continue; }
            d.jobs.iter().find(|j| j.status == "pending" && j.start_at.map(|t| t <= now()).unwrap_or(true)).map(|j| j.id.clone())
        };
        let Some(id) = next else { continue };
        run_job(q, &id).await;
    }
}

async fn running(q: &Q) -> bool { q.data.lock().await.running }

async fn llama_up() -> bool {
    matches!(tokio::time::timeout(Duration::from_millis(400), tokio::net::TcpStream::connect(("127.0.0.1", LLAMA_PORT))).await, Ok(Ok(_)))
}

async fn run_job(q: &Q, id: &str) {
    let Some(job) = q.data.lock().await.jobs.iter().find(|j| j.id == id).cloned() else { return };
    let files = crate::textx::list_files(Path::new(&job.in_dir), false);
    if !Path::new(&job.in_dir).is_dir() {
        patch(q, id, |j| { j.status = "error".into(); j.message = "Папка с файлами не найдена".into(); }).await;
        return;
    }
    if files.is_empty() {
        patch(q, id, |j| { j.status = "error".into(); j.message = "В папке нет файлов .txt, .md, .docx или .pdf".into(); }).await;
        return;
    }
    let total = files.len();
    let _ = std::fs::create_dir_all(&job.out_dir);
    patch(q, id, |j| { j.status = "running".into(); j.total_files = total; j.message.clear(); }).await;
    let client = reqwest::Client::new();
    for f in files.iter() {
        let key = f.to_string_lossy().to_string();
        if q.data.lock().await.jobs.iter().find(|j| j.id == id).map(|j| j.done_files.contains(&key)).unwrap_or(true) { continue; }
        if !running(q).await { patch(q, id, |j| { j.status = "pending".into(); j.current = None; j.message = "На паузе".into(); }).await; return; }
        if !llama_up().await {
            patch(q, id, |j| { j.status = "pending".into(); j.current = None; j.message = "Жду модель — запустите её во вкладке Chat".into(); }).await;
            tokio::time::sleep(Duration::from_secs(15)).await;
            return;
        }
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let stem = f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let out = Path::new(&job.out_dir).join(format!("{stem}.{}.md", job.kind));
        patch(q, id, |j| { j.current = Some(name.clone()); j.message = String::new(); }).await;

        let res = if out.exists() { Ok(None) } else { process_file(q, id, &client, &job, f, total).await.map(Some) };
        match res {
            Ok(Some(text)) => { if let Err(e) = std::fs::write(&out, text) { patch(q, id, |j| j.errors.push(format!("{name}: не удалось записать результат: {e}"))).await; } }
            Ok(None) => {}
            Err(e) if e == "paused" => { patch(q, id, |j| { j.status = "pending".into(); j.current = None; j.message = "На паузе".into(); }).await; return; }
            Err(e) if e == "removed" => return,
            Err(e) if e == "nollama" => {
                patch(q, id, |j| { j.status = "pending".into(); j.current = None; j.message = "Модель остановилась — продолжу, когда она снова запустится".into(); }).await;
                tokio::time::sleep(Duration::from_secs(15)).await;
                return;
            }
            Err(e) => { patch(q, id, |j| j.errors.push(format!("{name}: {e}"))).await; }
        }
        patch(q, id, |j| {
            j.done_files.push(key.clone());
            j.progress = j.done_files.len() as f32 / total.max(1) as f32;
        }).await;
    }
    patch(q, id, |j| {
        j.status = "done".into(); j.current = None; j.progress = 1.0;
        j.message = if j.errors.is_empty() { format!("Готово: {total} файл(ов)") } else { format!("Готово, но с ошибками: {}", j.errors.len()) };
    }).await;
}

fn lang_name(l: &str) -> String { if l.trim().is_empty() { "Russian".into() } else { l.trim().to_string() } }

async fn process_file(q: &Q, id: &str, client: &reqwest::Client, job: &Job, f: &Path, total: usize) -> Result<String, String> {
    let text = crate::textx::extract(f)?;
    if text.trim().is_empty() { return Err("файл пустой".into()); }
    let size = if job.kind == "translate" { 3000 } else { 6000 };
    let chunks = crate::textx::chunk(&text, size, 0);
    let lang = lang_name(&job.lang);
    let sys = match job.kind.as_str() {
        "translate" => format!("You are a professional translator. Translate the user's text into {lang}. Keep the meaning, tone, formatting and Markdown. Output only the translation, without comments."),
        "summarize" => format!("Summarize the user's text in {lang}: the key points as a short Markdown list, then a 2–3 sentence conclusion. Output only the summary."),
        _ => format!("{}\n\nAnswer in {lang}.", job.prompt.trim()),
    };
    let mut parts = Vec::new();
    let done_before = q.data.lock().await.jobs.iter().find(|j| j.id == id).map(|j| j.done_files.len()).unwrap_or(0);
    for (i, c) in chunks.iter().enumerate() {
        {
            let d = q.data.lock().await;
            if !d.jobs.iter().any(|j| j.id == id) { return Err("removed".into()); }
            if !d.running { return Err("paused".into()); }
        }
        if !llama_up().await { return Err("nollama".into()); }
        let user = if chunks.len() > 1 && job.kind != "translate" { format!("(Part {} of {})\n\n{c}", i + 1, chunks.len()) } else { c.clone() };
        parts.push(complete(client, &sys, &user).await?);
        let frac = (done_before as f32 + (i + 1) as f32 / chunks.len() as f32 / if job.kind == "summarize" && chunks.len() > 1 { 1.1 } else { 1.0 }) / total.max(1) as f32;
        patch(q, id, |j| { j.progress = frac.min(1.0); j.message = if chunks.len() > 1 { format!("часть {} из {}", i + 1, chunks.len()) } else { String::new() }; }).await;
    }
    Ok(match job.kind.as_str() {
        "summarize" if parts.len() > 1 => {
            let joined = parts.join("\n\n");
            let sys2 = format!("Below are summaries of consecutive parts of one document. Merge them into one summary in {lang}: the key points as a Markdown list, then a 2–3 sentence conclusion. Output only the summary.");
            complete(client, &sys2, &joined).await.unwrap_or(joined)
        }
        "translate" => parts.join("\n\n"),
        _ => parts.join("\n\n---\n\n"),
    })
}

async fn complete(client: &reqwest::Client, sys: &str, user: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "messages": [{"role": "system", "content": sys}, {"role": "user", "content": user}],
        "stream": false, "temperature": 0.3,
    });
    let r = client.post(format!("http://127.0.0.1:{LLAMA_PORT}/v1/chat/completions")).json(&body)
        .timeout(Duration::from_secs(60 * 30)).send().await.map_err(|e| format!("модель не ответила: {e}"))?;
    let status = r.status();
    let v: serde_json::Value = r.json().await.map_err(|e| format!("непонятный ответ модели: {e}"))?;
    if !status.is_success() {
        let m = v.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or("ошибка");
        return Err(format!("модель вернула {status}: {m}"));
    }
    let s = v.pointer("/choices/0/message/content").and_then(|c| c.as_str()).unwrap_or("").to_string();
    Ok(strip_think(&s))
}

fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a { break; }
        out.replace_range(a..b + 8, "");
    }
    if let Some(b) = out.find("</think>") { out = out[b + 8..].to_string(); }
    out.trim().to_string()
}

fn q() -> Result<&'static Q, String> { QUEUE.get().ok_or_else(|| "очередь ещё не готова".to_string()) }

#[tauri::command]
pub async fn queue_list() -> Result<QueueView, String> { Ok(q()?.data.lock().await.clone()) }

#[tauri::command]
pub async fn queue_add(_state: State<'_, Arc<AppState>>, job: NewJob) -> Result<QueueView, String> {
    let q = q()?;
    if !matches!(job.kind.as_str(), "translate" | "summarize" | "custom") { return Err("неизвестный тип задания".into()); }
    if job.kind == "custom" && job.prompt.trim().is_empty() { return Err("Напишите, что сделать с каждым файлом".into()); }
    let in_dir = job.in_dir.trim().to_string();
    if !Path::new(&in_dir).is_dir() { return Err("Папка с файлами не найдена".into()); }
    let out_dir = if job.out_dir.trim().is_empty() { in_dir.clone() } else { job.out_dir.trim().to_string() };
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("не удалось создать папку для результатов: {e}"))?;
    let j = Job {
        id: format!("job-{}-{}", now(), q.data.lock().await.jobs.len()),
        kind: job.kind, prompt: job.prompt, lang: job.lang, in_dir, out_dir, start_at: job.start_at,
        status: "pending".into(), total_files: crate::textx::list_files(Path::new(&job.in_dir.trim()), false).len(),
        created: now(), ..Default::default()
    };
    q.data.lock().await.jobs.push(j);
    publish(q).await;
    Ok(q.data.lock().await.clone())
}

#[tauri::command]
pub async fn queue_remove(id: String) -> Result<QueueView, String> {
    let q = q()?;
    q.data.lock().await.jobs.retain(|j| j.id != id);
    publish(q).await;
    Ok(q.data.lock().await.clone())
}

#[tauri::command]
pub async fn queue_start() -> Result<QueueView, String> {
    let q = q()?;
    {
        let mut d = q.data.lock().await;
        d.running = true;
        for j in d.jobs.iter_mut() { if j.status == "pending" { j.message.clear(); } }
    }
    publish(q).await;
    Ok(q.data.lock().await.clone())
}

#[tauri::command]
pub async fn queue_pause() -> Result<QueueView, String> {
    let q = q()?;
    q.data.lock().await.running = false;
    publish(q).await;
    Ok(q.data.lock().await.clone())
}
