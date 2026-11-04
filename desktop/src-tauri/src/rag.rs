//! «Поиск по моим файлам»: indexes a folder (txt/md/docx/pdf via textx) into ~800-char chunks with
//! 100-char overlap and embeddings. Embedding service, in order: a custom URL, the phone service
//! (PC port 50160 when it answers), else a PC llama-server on 127.0.0.1:50070 with nomic-embed-text-v1.5.
//! Index: argus-rag.json next to the settings (vectors as base64 little-endian f32, file mtimes to skip
//! unchanged files, the model name — search is refused when the current service runs another model).
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::{Path, PathBuf}, process::Stdio, sync::{Arc, OnceLock}, time::Duration};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

const PHONE_URL: &str = "http://127.0.0.1:50160";
const PC_PORT: u16 = 50070;
const PC_MODEL: &str = "nomic-ai/nomic-embed-text-v1.5-GGUF:Q8_0";
const CHUNK: usize = 800;
const OVERLAP: usize = 100;

#[derive(Serialize, Deserialize, Clone, Default)]
struct Chunk { text: String, v: String }

#[derive(Serialize, Deserialize, Clone, Default)]
struct FileEntry { mtime: u64, chunks: Vec<Chunk>, #[serde(default)] error: Option<String> }

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct Index {
    folder: String,
    model: String,
    #[serde(default)] custom_url: String,
    files: HashMap<String, FileEntry>,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RagStatus {
    pub folder: String,
    pub files: usize,
    pub chunks: usize,
    pub model: String,
    pub custom_url: String,
    pub indexing: bool,
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub message: String,
    pub errors: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct Hit { pub source: String, pub text: String, pub score: f32 }

struct Rag { index: Mutex<Option<Index>>, status: Mutex<RagStatus>, pc_server: Mutex<Option<tokio::process::Child>> }
static RAG: OnceLock<Rag> = OnceLock::new();
fn rag() -> &'static Rag { RAG.get_or_init(|| Rag { index: Mutex::new(None), status: Mutex::new(RagStatus::default()), pc_server: Mutex::new(None) }) }

fn index_path(st: &AppState) -> Option<PathBuf> { crate::queue::data_dir(st).map(|d| d.join("argus-rag.json")) }

async fn load(st: &AppState) {
    let r = rag();
    let mut g = r.index.lock().await;
    if g.is_none() {
        *g = Some(index_path(st).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default());
    }
}

fn save(st: &AppState, idx: &Index) {
    let Some(p) = index_path(st) else { return };
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    if let Ok(s) = serde_json::to_string(idx) {
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, s).is_ok() { let _ = std::fs::rename(&tmp, &p); }
    }
}

// ---------- base64 f32 ----------
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
fn enc(v: &[f32]) -> String {
    let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    let mut s = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(B64[(n >> 18) as usize & 63] as char);
        s.push(B64[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    s
}
fn dec(s: &str) -> Vec<f32> {
    let mut bytes = Vec::with_capacity(s.len() * 3 / 4);
    let val = |c: u8| B64.iter().position(|&b| b == c).map(|p| p as u32);
    for q in s.as_bytes().chunks(4) {
        let mut n = 0u32; let mut k: usize = 0;
        for &c in q { if let Some(v) = val(c) { n = n << 6 | v; k += 1; } }
        n <<= 6 * (4 - k);
        let out = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        bytes.extend_from_slice(&out[..k.saturating_sub(1)]);
    }
    bytes.chunks(4).filter(|c| c.len() == 4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 { v.iter_mut().for_each(|x| *x /= n); }
}

// ---------- embedding service ----------

async fn health(client: &reqwest::Client, base: &str) -> bool {
    client.get(format!("{base}/health")).timeout(Duration::from_millis(1500)).send().await.map(|r| r.status().is_success()).unwrap_or(false)
}

fn norm_model(id: &str) -> String {
    let l = id.to_lowercase();
    if l.contains("nomic-embed-text-v1.5") || l.contains("nomic-embed-text-v1_5") { "nomic-embed-text-v1.5".into() } else { l }
}

async fn model_name(client: &reqwest::Client, base: &str) -> String {
    let id = async {
        let v: serde_json::Value = client.get(format!("{base}/v1/models")).timeout(Duration::from_secs(5)).send().await.ok()?.json().await.ok()?;
        v.pointer("/data/0/id").and_then(|s| s.as_str()).map(|s| s.to_string())
    }.await;
    id.map(|s| norm_model(&s)).unwrap_or_else(|| "nomic-embed-text-v1.5".into())
}

/// Picks the embedding service and returns (base url, model name).
async fn service(st: &AppState, app: Option<&AppHandle>, custom: &str) -> Result<(String, String), String> {
    let client = reqwest::Client::new();
    let custom = custom.trim().trim_end_matches('/').to_string();
    if !custom.is_empty() {
        return Ok((custom.clone(), model_name(&client, &custom).await));
    }
    let mut phones = crate::nodesvc::ready_bases("embeddings");
    if !phones.iter().any(|b| b == PHONE_URL) { phones.push(PHONE_URL.into()); }
    for b in phones { if health(&client, &b).await { return Ok((b.clone(), model_name(&client, &b).await)); } }
    let pc = format!("http://127.0.0.1:{PC_PORT}");
    if health(&client, &pc).await { return Ok((pc.clone(), model_name(&client, &pc).await)); }
    // start our own small embedding server on the PC CPU
    {
        let r = rag();
        let mut g = r.pc_server.lock().await;
        let alive = match g.as_mut() { Some(c) => matches!(c.try_wait(), Ok(None)), None => false };
        if !alive {
            if !st.llama_bin.exists() { return Err("Не найден llama-server для сервиса поиска; укажите адрес сервиса вручную".into()); }
            let mut c = crate::adb::hidden_command(&st.llama_bin);
            crate::llama::lib_path_env(&mut c, &st.llama_bin);
            let cache = crate::llama::model_cache_dir();
            let _ = std::fs::create_dir_all(&cache);
            c.env("HF_HOME", &cache).env("HF_HUB_CACHE", cache.join("hub")).env("LLAMA_CACHE", &cache);
            c.args(["-hf", PC_MODEL, "--embeddings", "--pooling", "mean", "-ngl", "0", "--host", "127.0.0.1", "--port", &PC_PORT.to_string(), "-c", "2048", "-b", "2048", "-ub", "2048"]);
            c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
            *g = Some(c.spawn().map_err(|e| format!("не удалось запустить сервис поиска: {e}"))?);
        }
    }
    if let Some(a) = app { set_msg(a, "Запускаю сервис поиска на ПК (первый раз скачивается модель ~140 МБ)…").await; }
    for _ in 0..600 {
        if health(&client, &pc).await { return Ok((pc, "nomic-embed-text-v1.5".into())); }
        {
            let mut g = rag().pc_server.lock().await;
            if let Some(c) = g.as_mut() { if !matches!(c.try_wait(), Ok(None)) { *g = None; return Err("Сервис поиска на ПК завершился при запуске".into()); } }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err("Сервис поиска на ПК не запустился за 10 минут".into())
}

async fn embed(client: &reqwest::Client, base: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    let r = client.post(format!("{base}/v1/embeddings")).json(&serde_json::json!({"input": texts, "model": "embed"}))
        .timeout(Duration::from_secs(300)).send().await;
    if let Ok(r) = r {
        if r.status().is_success() {
            if let Ok(v) = r.json::<serde_json::Value>().await {
                if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
                    let mut out: Vec<(usize, Vec<f32>)> = arr.iter().enumerate().filter_map(|(i, e)| {
                        let idx = e.get("index").and_then(|x| x.as_u64()).map(|x| x as usize).unwrap_or(i);
                        let vec: Vec<f32> = e.get("embedding")?.as_array()?.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect();
                        Some((idx, vec))
                    }).collect();
                    out.sort_by_key(|x| x.0);
                    if out.len() == texts.len() { return Ok(out.into_iter().map(|x| x.1).collect()); }
                }
            }
        }
    }
    // fallback: llama.cpp native /embedding, one text at a time
    let mut res = Vec::new();
    for t in texts {
        let v: serde_json::Value = client.post(format!("{base}/embedding")).json(&serde_json::json!({"content": t}))
            .timeout(Duration::from_secs(300)).send().await.map_err(|e| format!("сервис поиска не отвечает: {e}"))?
            .json().await.map_err(|e| format!("непонятный ответ сервиса поиска: {e}"))?;
        let e = if v.is_array() { v.pointer("/0/embedding").cloned() } else { v.get("embedding").cloned() }
            .ok_or("сервис поиска не вернул вектор")?;
        let e = if e.get(0).map(|x| x.is_array()).unwrap_or(false) { e[0].clone() } else { e };
        res.push(e.as_array().ok_or("сервис поиска не вернул вектор")?.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect());
    }
    Ok(res)
}

async fn set_msg(app: &AppHandle, m: &str) {
    let s = { let mut g = rag().status.lock().await; g.message = m.to_string(); g.clone() };
    let _ = app.emit("rag-progress", s);
}

fn mtime(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

async fn status_now() -> RagStatus {
    let r = rag();
    let mut s = r.status.lock().await.clone();
    if let Some(idx) = r.index.lock().await.as_ref() {
        s.folder = idx.folder.clone();
        s.model = idx.model.clone();
        s.custom_url = idx.custom_url.clone();
        s.files = idx.files.values().filter(|f| !f.chunks.is_empty()).count();
        s.chunks = idx.files.values().map(|f| f.chunks.len()).sum();
    }
    s
}

#[tauri::command]
pub async fn rag_status(state: State<'_, Arc<AppState>>) -> Result<RagStatus, String> {
    load(&state).await;
    Ok(status_now().await)
}

/// Starts indexing in the background; progress comes as `rag-progress` events.
#[tauri::command]
pub async fn rag_index(app: AppHandle, state: State<'_, Arc<AppState>>, folder: String, url: Option<String>) -> Result<RagStatus, String> {
    load(&state).await;
    let folder = folder.trim().to_string();
    if !Path::new(&folder).is_dir() { return Err("Папка не найдена".into()); }
    {
        let mut s = rag().status.lock().await;
        if s.indexing { return Err("Индексация уже идёт".into()); }
        *s = RagStatus { indexing: true, message: "Готовлюсь…".into(), ..Default::default() };
    }
    let st = state.inner().clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let res = run_index(&app2, &st, folder, url.unwrap_or_default()).await;
        if let Some(idx) = rag().index.lock().await.as_ref() { save(&st, idx); }
        {
            let mut g = rag().status.lock().await;
            g.indexing = false;
            g.current.clear();
            g.message = match res { Ok(m) => m, Err(e) => e };
        }
        let _ = app2.emit("rag-progress", status_now().await);
    });
    Ok(status_now().await)
}

async fn run_index(app: &AppHandle, st: &AppState, folder: String, url: String) -> Result<String, String> {
    let (base, model) = service(st, Some(app), &url).await?;
    let client = reqwest::Client::new();
    let files = crate::textx::list_files(Path::new(&folder), true);
    if files.is_empty() { return Err("В папке нет файлов .txt, .md, .docx или .pdf".into()); }
    // a different folder or model ⇒ start over
    {
        let mut g = rag().index.lock().await;
        let idx = g.get_or_insert_with(Index::default);
        if idx.folder != folder || idx.model != model {
            *idx = Index { folder: folder.clone(), model: model.clone(), custom_url: url.trim().to_string(), files: HashMap::new() };
        }
        idx.custom_url = url.trim().to_string();
        let keep: std::collections::HashSet<String> = files.iter().map(|f| f.to_string_lossy().to_string()).collect();
        idx.files.retain(|k, _| keep.contains(k));
    }
    let total = files.len();
    let mut errors = Vec::new();
    let mut fresh = 0;
    for (i, f) in files.iter().enumerate() {
        let key = f.to_string_lossy().to_string();
        let mt = mtime(f);
        {
            let mut s = rag().status.lock().await;
            s.done = i; s.total = total; s.current = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            s.message = format!("Индексирую {} из {total}", i + 1);
        }
        let _ = app.emit("rag-progress", status_now().await);
        if rag().index.lock().await.as_ref().and_then(|x| x.files.get(&key)).map(|e| e.mtime == mt).unwrap_or(false) { continue; }
        let entry = match crate::textx::extract(f) {
            Err(e) => { errors.push(format!("{}: {e}", f.display())); FileEntry { mtime: mt, chunks: vec![], error: Some(e) } }
            Ok(text) => {
                let pieces = crate::textx::chunk(&text, CHUNK, OVERLAP);
                let mut chunks = Vec::new();
                for batch in pieces.chunks(16) {
                    let inputs: Vec<String> = batch.iter().map(|t| format!("search_document: {t}")).collect();
                    let vecs = embed(&client, &base, &inputs).await?;
                    for (t, mut v) in batch.iter().zip(vecs) { normalize(&mut v); chunks.push(Chunk { text: t.clone(), v: enc(&v) }); }
                }
                fresh += 1;
                FileEntry { mtime: mt, chunks, error: None }
            }
        };
        let mut g = rag().index.lock().await;
        if let Some(idx) = g.as_mut() { idx.files.insert(key, entry); if fresh % 10 == 0 { save(st, idx); } }
    }
    if let Some(idx) = rag().index.lock().await.as_ref() { save(st, idx); }
    rag().status.lock().await.errors = errors.clone();
    rag().status.lock().await.done = total;
    Ok(if errors.is_empty() { format!("Готово: {total} файл(ов), новых или изменённых {fresh}") }
       else { format!("Готово: {total} файл(ов), не прочитано {}", errors.len()) })
}

#[tauri::command]
pub async fn rag_search(state: State<'_, Arc<AppState>>, query: String, k: Option<usize>) -> Result<Vec<Hit>, String> {
    load(&state).await;
    let (model, custom, empty) = {
        let g = rag().index.lock().await;
        let idx = g.as_ref().ok_or("Индекса пока нет")?;
        (idx.model.clone(), idx.custom_url.clone(), idx.files.values().all(|f| f.chunks.is_empty()))
    };
    if empty { return Err("Индекс пуст — сначала проиндексируйте папку во вкладке «Знания»".into()); }
    let (base, cur) = service(&state, None, &custom).await?;
    if cur != model {
        return Err(format!("Индекс построен моделью «{model}», а сейчас сервис поиска работает с «{cur}». Переиндексируйте папку."));
    }
    let client = reqwest::Client::new();
    let mut q = embed(&client, &base, &[format!("search_query: {}", query.trim())]).await?.pop().ok_or("пустой ответ")?;
    normalize(&mut q);
    let k = k.unwrap_or(5).clamp(1, 20);
    let g = rag().index.lock().await;
    let idx = g.as_ref().ok_or("Индекса пока нет")?;
    let mut hits: Vec<Hit> = Vec::new();
    for (src, f) in idx.files.iter() {
        for c in &f.chunks {
            let v = dec(&c.v);
            if v.len() != q.len() { continue; }
            let score: f32 = v.iter().zip(&q).map(|(a, b)| a * b).sum();
            hits.push(Hit { source: src.clone(), text: c.text.clone(), score });
        }
    }
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(k);
    Ok(hits)
}

/// App exit: stop the PC embedding server we started.
pub fn shutdown() {
    if let Some(r) = RAG.get() {
        if let Ok(mut g) = r.pc_server.try_lock() { if let Some(c) = g.as_mut() { let _ = c.start_kill(); } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn b64_roundtrip() {
        for n in [1usize, 2, 3, 7, 768] {
            let v: Vec<f32> = (0..n).map(|i| i as f32 * 0.5 - 3.0).collect();
            assert_eq!(dec(&enc(&v)), v);
        }
    }
}
