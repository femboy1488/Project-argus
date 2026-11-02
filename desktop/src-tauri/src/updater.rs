//! Self-update (0.3.1).
//!
//! Desktop: on start (+5 s) and every 6 h — only when «Проверять обновления» is on — GET GitHub
//! `releases/latest` (User-Agent, 10 s timeout, failures silent). Tag `vX.Y.Z` newer than this build ⇒
//! event `update-available`; the UI ASKS («Доступна версия X. Хотите обновить?» — Обновить / Позже /
//! Пропустить эту версию). Nothing is downloaded or installed without that click; there is no auto-install.
//! «Обновить»: download the platform asset to temp with progress, verify SHA-256 against SHA256SUMS.txt
//! of the same release (refuse when missing or different), stop the engine, then
//!   * Windows (NSIS install): run `Argus-Setup.exe /S /UPDATE` and exit — the installer waits for Argus,
//!     replaces files, keeps user data (%APPDATA%, models) and relaunches Argus.exe;
//!   * Linux AppImage: replace `$APPIMAGE` atomically, relaunch, exit;
//!   * .deb / portable / anything else: open the release page.
//!
//! Wi-Fi nodes: while Argus runs it serves the bundled node APK on LAN port 50055
//! (`/node.version`, `/node.apk`, `/node.sha256`) and answers every node beacon with a unicast
//! `{"argus":"host","v":1,"update_port":50055,"node_vc":N}` (see lan.rs). The node app asks its user.
use crate::state::AppState;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{path::{Path, PathBuf}, sync::{Arc, OnceLock}, time::{Duration, SystemTime, UNIX_EPOCH}};
use tauri::{AppHandle, Emitter, State};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, time::{sleep, timeout}};

pub const REPO: &str = "femboy1488/Project-argus";
const LATEST_URL: &str = "https://api.github.com/repos/femboy1488/Project-argus/releases/latest";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// LAN HTTP port that serves the bundled node.apk to Wi-Fi nodes
pub const NODE_UPDATE_PORT: u16 = 50055;
const SETUP_ASSET: &str = "Argus-Setup.exe";
const SUMS_ASSET: &str = "SHA256SUMS.txt";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset { pub name: String, pub url: String, pub size: u64 }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub tag: String,
    pub name: String,
    /// markdown
    pub notes: String,
    pub url: String,
    pub published_at: String,
    /// the asset this install can apply (None ⇒ «Обновить» opens the release page)
    pub asset: Option<Asset>,
    pub has_sums: bool,
}

#[derive(Default)]
pub struct UpdState {
    pub latest: Option<Release>,
    pub checking: bool,
    pub last_check: Option<u64>,
    pub error: Option<String>,
    pub busy: bool,
    pub phase: String,
    pub done: u64,
    pub total: u64,
    /// «не удалось запустить HTTP :50055» for the status bar
    pub node_server_error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub current: String,
    /// nsis | portable | appimage | deb | other
    pub install_kind: &'static str,
    pub latest: Option<Release>,
    pub available: bool,
    pub skipped: bool,
    pub check_enabled: bool,
    pub checking: bool,
    pub last_check: Option<u64>,
    pub error: Option<String>,
    pub busy: bool,
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub node_server_error: String,
}

// ---------------------------------------------------------------- versions

/// "v0.3.1" / "0.3.1" / "v0.4" / "v1.2.3-rc1" → (major, minor, patch). Tags like "V2" are NOT versions → None.
pub fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let t = tag.trim().trim_start_matches(['v', 'V']);
    let core = t.split(['-', '+']).next()?;
    let parts: Vec<&str> = core.split('.').collect();
    if !(2..=3).contains(&parts.len()) { return None; }
    let n: Vec<u64> = parts.iter().map(|p| p.parse::<u64>().ok()).collect::<Option<_>>()?;
    Some((n[0], n[1], *n.get(2).unwrap_or(&0)))
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) { (Some(a), Some(b)) => a > b, _ => false }
}

/// sha256sum format: `<hex>  name` or `<hex> *name` (path prefixes are ignored).
pub fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| {
        let l = l.trim();
        let (hex, rest) = l.split_once(char::is_whitespace)?;
        if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
        let name = rest.trim().trim_start_matches('*');
        let name = name.rsplit(['/', '\\']).next().unwrap_or(name).to_string();
        Some((name, hex.to_ascii_lowercase()))
    }).collect()
}

// ---------------------------------------------------------------- install kind

pub fn install_kind() -> &'static str {
    let exe = std::env::current_exe().ok();
    #[cfg(windows)]
    {
        // our NSIS installer writes uninstall.exe next to Argus.exe
        if exe.as_ref().and_then(|e| e.parent()).map(|d| d.join("uninstall.exe").exists()).unwrap_or(false) { "nsis" } else { "portable" }
    }
    #[cfg(not(windows))]
    {
        if std::env::var_os("APPIMAGE").is_some() { return "appimage"; }
        if exe.as_ref().map(|e| e.starts_with("/usr")).unwrap_or(false) { "deb" } else { "other" }
    }
}

fn pick_asset(kind: &str, assets: &[Asset]) -> Option<Asset> {
    match kind {
        "nsis" => assets.iter().find(|a| a.name == SETUP_ASSET).cloned(),
        "appimage" => {
            let arch = if cfg!(target_arch = "aarch64") { ["aarch64", "arm64"] } else { ["x86_64", "amd64"] };
            let imgs: Vec<&Asset> = assets.iter().filter(|a| a.name.ends_with(".AppImage")).collect();
            imgs.iter().find(|a| arch.iter().any(|x| a.name.contains(x))).or(imgs.first()).map(|a| (*a).clone())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- http

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(format!("Argus-Cluster/{CURRENT} (+https://github.com/{REPO})"))
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()?)
}

async fn fetch_latest(kind: &str) -> Result<Release> {
    let r = client()?.get(LATEST_URL).header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10)).send().await?;
    if r.status() == reqwest::StatusCode::NOT_FOUND { bail!("релизов пока нет"); }
    if !r.status().is_success() { bail!("GitHub ответил {}", r.status()); }
    let j: serde_json::Value = r.json().await?;
    let tag = j["tag_name"].as_str().unwrap_or_default().to_string();
    let assets: Vec<Asset> = j["assets"].as_array().map(|a| a.iter().filter_map(|x| Some(Asset {
        name: x["name"].as_str()?.to_string(),
        url: x["browser_download_url"].as_str()?.to_string(),
        size: x["size"].as_u64().unwrap_or(0),
    })).collect()).unwrap_or_default();
    let version = parse_version(&tag).map(|(a, b, c)| format!("{a}.{b}.{c}")).unwrap_or_default();
    Ok(Release {
        version, name: j["name"].as_str().unwrap_or(&tag).to_string(),
        notes: j["body"].as_str().unwrap_or_default().chars().take(20_000).collect(),
        url: j["html_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/latest")),
        published_at: j["published_at"].as_str().unwrap_or_default().to_string(),
        asset: pick_asset(kind, &assets), has_sums: assets.iter().any(|a| a.name == SUMS_ASSET),
        tag,
    })
}

pub async fn status(st: &AppState) -> UpdateStatus {
    let (check_enabled, skipped_version) = {
        let s = st.settings.lock().await;
        (s.check_updates, s.skipped_version.clone())
    };
    let u = st.update.lock().await;
    let available = u.latest.as_ref().map(|r| is_newer(&r.tag, CURRENT)).unwrap_or(false);
    let skipped = available && u.latest.as_ref().map(|r| Some(&r.version) == skipped_version.as_ref()).unwrap_or(false);
    UpdateStatus {
        current: CURRENT.into(), install_kind: install_kind(), latest: u.latest.clone(), available, skipped, check_enabled,
        checking: u.checking, last_check: u.last_check, error: u.error.clone(), busy: u.busy, phase: u.phase.clone(),
        done: u.done, total: u.total, node_server_error: u.node_server_error.clone(),
    }
}

async fn check(st: &AppState) -> UpdateStatus {
    st.update.lock().await.checking = true;
    let r = fetch_latest(install_kind()).await;
    {
        let mut u = st.update.lock().await;
        u.checking = false;
        u.last_check = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs());
        match r {
            Ok(rel) => { log::info!("update check: latest {} (current {CURRENT})", rel.tag); u.latest = Some(rel); u.error = None; }
            Err(e) => { log::info!("update check failed: {e:#}"); u.error = Some(format!("{e:#}")); }
        }
    }
    status(st).await
}

/// Background loop: start + every 6 h; silent on failure; emits `update-available` (the UI asks).
pub fn start(app: AppHandle, st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        sleep(Duration::from_secs(5)).await;
        loop {
            if st.settings.lock().await.check_updates {
                let s = check(&st).await;
                if s.available && !s.skipped { let _ = app.emit("update-available", &s); }
            }
            sleep(CHECK_EVERY).await;
        }
    });
}

// ---------------------------------------------------------------- download + apply

fn progress(app: &AppHandle, phase: &str, done: u64, total: u64) {
    let _ = app.emit("update-progress", serde_json::json!({ "phase": phase, "done": done, "total": total }));
}

async fn set_phase(st: &AppState, app: &AppHandle, phase: &str, done: u64, total: u64) {
    { let mut u = st.update.lock().await; u.phase = phase.into(); u.done = done; u.total = total; }
    progress(app, phase, done, total);
}

/// Streams `url` into `dest`, returns the sha256 hex.
async fn download_to(st: &AppState, app: &AppHandle, url: &str, dest: &Path, size_hint: u64) -> Result<String> {
    let mut r = client()?.get(url).send().await?;
    if !r.status().is_success() { bail!("загрузка: HTTP {}", r.status()); }
    let total = r.content_length().unwrap_or(size_hint);
    let mut f = tokio::fs::File::create(dest).await.with_context(|| format!("не удалось создать {}", dest.display()))?;
    let mut h = Sha256::new();
    let mut done = 0u64;
    let mut last = std::time::Instant::now();
    while let Some(chunk) = r.chunk().await? {
        h.update(&chunk);
        f.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if last.elapsed() > Duration::from_millis(200) { last = std::time::Instant::now(); set_phase(st, app, "download", done, total).await; }
    }
    f.flush().await?;
    f.sync_all().await.ok();
    drop(f);
    set_phase(st, app, "download", done, total.max(done)).await;
    Ok(format!("{:x}", h.finalize()))
}

async fn expected_sha(rel: &Release, asset: &str) -> Result<String> {
    if !rel.has_sums { bail!("в релизе {} нет {SUMS_ASSET} — обновление отклонено (нельзя проверить файл)", rel.tag); }
    let url = format!("https://github.com/{REPO}/releases/download/{}/{SUMS_ASSET}", rel.tag);
    let r = client()?.get(&url).timeout(Duration::from_secs(30)).send().await?;
    if !r.status().is_success() { bail!("{SUMS_ASSET}: HTTP {}", r.status()); }
    let text = r.text().await?;
    parse_sums(&text).into_iter().find(|(n, _)| n == asset).map(|(_, h)| h)
        .ok_or_else(|| anyhow!("в {SUMS_ASSET} нет строки для {asset} — обновление отклонено"))
}

/// Returns "installing" (app exits right after) or "opened" (release page opened).
async fn install(app: &AppHandle, st: &Arc<AppState>) -> Result<&'static str> {
    let rel = st.update.lock().await.latest.clone().ok_or_else(|| anyhow!("сначала проверьте обновления"))?;
    if !is_newer(&rel.tag, CURRENT) { bail!("установлена актуальная версия {CURRENT}"); }
    let kind = install_kind();
    let Some(asset) = rel.asset.clone() else {
        crate::agent::open_external(rel.url.clone()).await.map_err(|e| anyhow!(e))?;
        return Ok("opened");
    };
    let want = expected_sha(&rel, &asset.name).await?;

    let dir = std::env::temp_dir().join("argus-update");
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::create_dir_all(&dir).await?;
    let file = dir.join(&asset.name);
    let got = download_to(st, app, &asset.url, &file, asset.size).await?;
    if got != want {
        let _ = tokio::fs::remove_file(&file).await;
        bail!("SHA-256 не совпадает ({}… ≠ {}…) — файл удалён, обновление отклонено", &got[..12], &want[..12]);
    }
    log::info!("update {} verified sha256 {got}", asset.name);
    set_phase(st, app, "stopping", 0, 0).await;
    crate::llama::stop_for_update(st).await;

    match kind {
        "nsis" => {
            // adb.exe in resources\ must not be running or the installer can't overwrite it
            let _ = st.adb.run_timeout(&["kill-server"], Duration::from_secs(5)).await;
            launch_detached(&file, &["/S", "/UPDATE"])?;
        }
        "appimage" => {
            let target = PathBuf::from(std::env::var_os("APPIMAGE").ok_or_else(|| anyhow!("$APPIMAGE не задан"))?);
            replace_appimage(&file, &target).await?;
            launch_detached(&target, &[])?;
        }
        _ => unreachable!("asset is only picked for nsis/appimage"),
    }
    set_phase(st, app, "restarting", 0, 0).await;
    let a = app.clone();
    tauri::async_runtime::spawn(async move { sleep(Duration::from_millis(600)).await; a.exit(0); });
    Ok("installing")
}

async fn replace_appimage(new: &Path, target: &Path) -> Result<()> {
    let dir = target.parent().ok_or_else(|| anyhow!("неверный путь AppImage"))?;
    let tmp = dir.join(format!(".{}.update", target.file_name().and_then(|n| n.to_str()).unwrap_or("Argus.AppImage")));
    tokio::fs::copy(new, &tmp).await
        .with_context(|| format!("нет прав на запись в {} — скачайте новую версию вручную со страницы релиза", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).await?;
    }
    tokio::fs::rename(&tmp, target).await.with_context(|| format!("не удалось заменить {}", target.display()))?; // running inode stays valid
    Ok(())
}

fn launch_detached(bin: &Path, args: &[&str]) -> Result<()> {
    let mut c = std::process::Command::new(bin);
    c.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0); // survive our exit
    }
    c.spawn().with_context(|| format!("не удалось запустить {}", bin.display()))?;
    Ok(())
}

// ---------------------------------------------------------------- node.apk over LAN (:50055)

fn apk_sha(path: &Path) -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let bytes = std::fs::read(path).ok()?;
        Some(format!("{:x}", Sha256::digest(&bytes)))
    }).clone()
}

/// Serves resources/node.apk to Wi-Fi nodes while Argus runs: GET /node.version, /node.apk, /node.sha256.
pub fn start_node_server(st: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match TcpListener::bind(("0.0.0.0", NODE_UPDATE_PORT)).await {
                Ok(l) => {
                    st.update.lock().await.node_server_error.clear();
                    log::info!("node update server on :{NODE_UPDATE_PORT}");
                    loop {
                        let Ok((sock, peer)) = l.accept().await else { break };
                        let s = st.clone();
                        tokio::spawn(async move {
                            if let Err(e) = serve(sock, &s).await { log::debug!("node http {peer}: {e:#}"); }
                        });
                    }
                }
                Err(e) => {
                    st.update.lock().await.node_server_error =
                        format!("TCP-порт {NODE_UPDATE_PORT} занят ({e}) — обновление Wi-Fi нод с этого ПК недоступно");
                    log::warn!("node update server bind: {e}");
                }
            }
            sleep(Duration::from_secs(30)).await;
        }
    });
}

async fn serve(mut s: TcpStream, st: &AppState) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    let mut n = 0;
    timeout(Duration::from_secs(5), async {
        while n < buf.len() {
            let k = s.read(&mut buf[n..]).await?;
            if k == 0 { break; }
            n += k;
            if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        }
        Ok::<_, std::io::Error>(())
    }).await.map_err(|_| anyhow!("timeout"))??;
    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
    let mut it = head.split_whitespace();
    let (method, path) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
    let head_only = method == "HEAD";
    if method != "GET" && !head_only { return reply(&mut s, "405 Method Not Allowed", "text/plain", b"GET only", false).await; }
    let apk = st.apk_path.clone().filter(|p| p.exists());
    match path.split('?').next().unwrap_or("") {
        "/node.version" => reply(&mut s, "200 OK", "text/plain; charset=utf-8", format!("{}\n", st.bundled_node_version).as_bytes(), head_only).await,
        "/node.sha256" => match apk.as_deref().and_then(apk_sha) {
            Some(h) => reply(&mut s, "200 OK", "text/plain", format!("{h}\n").as_bytes(), head_only).await,
            None => reply(&mut s, "404 Not Found", "text/plain", b"no node.apk", head_only).await,
        },
        "/node.apk" => {
            let Some(p) = apk else { return reply(&mut s, "404 Not Found", "text/plain", b"no node.apk", head_only).await };
            let mut f = tokio::fs::File::open(&p).await?;
            let len = f.metadata().await?.len();
            s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.android.package-archive\r\nContent-Length: {len}\r\n\
                Content-Disposition: attachment; filename=\"Argus-Node.apk\"\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            if !head_only { tokio::io::copy(&mut f, &mut s).await?; }
            s.shutdown().await.ok();
            Ok(())
        }
        // 0.3.2 «Помощник»: the helper model, downloaded once on the PC (helper.rs), for USB (adb reverse) and Wi-Fi nodes
        p if p.starts_with("/helper/") => {
            let name = &p["/helper/".len()..];
            let file = crate::helper::helpers_dir().join(name);
            if !crate::helper::safe_file(name) || !file.is_file() {
                return reply(&mut s, "404 Not Found", "text/plain", b"no such helper model", head_only).await;
            }
            let mut f = tokio::fs::File::open(&file).await?;
            let len = f.metadata().await?.len();
            s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            if !head_only { tokio::io::copy(&mut f, &mut s).await?; }
            s.shutdown().await.ok();
            Ok(())
        }
        _ => reply(&mut s, "404 Not Found", "text/plain", b"not found", head_only).await,
    }
}

async fn reply(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8], head_only: bool) -> Result<()> {
    s.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
    if !head_only { s.write_all(body).await?; }
    s.shutdown().await.ok();
    Ok(())
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn update_status(state: State<'_, Arc<AppState>>) -> Result<UpdateStatus, String> {
    Ok(status(&state).await)
}

/// «Проверить обновления» (works even when automatic checks are off).
#[tauri::command]
pub async fn check_updates(state: State<'_, Arc<AppState>>) -> Result<UpdateStatus, String> {
    Ok(check(&state).await)
}

#[tauri::command]
pub async fn set_update_check(state: State<'_, Arc<AppState>>, enabled: bool) -> Result<(), String> {
    state.settings.lock().await.check_updates = enabled;
    state.save_settings().await;
    Ok(())
}

/// «Пропустить эту версию» (empty = forget the skip).
#[tauri::command]
pub async fn skip_update_version(state: State<'_, Arc<AppState>>, version: String) -> Result<(), String> {
    state.settings.lock().await.skipped_version = Some(version).filter(|v| !v.is_empty());
    state.save_settings().await;
    Ok(())
}

/// «Обновить» — only ever called from the user's click.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let st = state.inner().clone();
    {
        let mut u = st.update.lock().await;
        if u.busy { return Err("обновление уже идёт".into()); }
        u.busy = true;
    }
    let r = install(&app, &st).await;
    {
        let mut u = st.update.lock().await;
        if !matches!(r, Ok("installing")) { u.busy = false; u.phase.clear(); u.done = 0; u.total = 0; }
    }
    match r {
        Ok(x) => Ok(x.to_string()),
        Err(e) => { progress(&app, "error", 0, 0); Err(format!("{e:#}")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.3.1"), Some((0, 3, 1)));
        assert_eq!(parse_version("0.4"), Some((0, 4, 0)));
        assert_eq!(parse_version("v1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_version("V2"), None); // the existing "V2" release is not a version tag
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer("v0.3.2", "0.3.1"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("v0.3.1", "0.3.1"));
        assert!(!is_newer("V2", "0.3.1"));
    }
    #[test]
    fn sums() {
        let h = "a".repeat(64);
        let t = format!("{h}  Argus-Setup.exe\n{h} *dist/Argus-Node.apk\nnot a line\n");
        let v = parse_sums(&t);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].0, "Argus-Node.apk");
    }
    #[test]
    fn assets() {
        let a = |n: &str| Asset { name: n.into(), url: String::new(), size: 0 };
        let list = vec![a("Argus-Setup.exe"), a("Argus_0.3.1_amd64.AppImage"), a("Argus_0.3.1_amd64.deb")];
        assert_eq!(pick_asset("nsis", &list).unwrap().name, "Argus-Setup.exe");
        assert!(pick_asset("appimage", &list).unwrap().name.ends_with(".AppImage"));
        assert!(pick_asset("deb", &list).is_none());
    }
}
