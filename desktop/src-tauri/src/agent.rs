//! PC tools for the chat "Agent mode". The UI asks the user to confirm every call before it reaches
//! these commands; nothing here runs on its own.
use crate::adb::hidden_command;
use crate::state::AppState;
use serde::Serialize;
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tauri::State;
use tokio::io::AsyncReadExt;

const MAX_OUT: usize = 16 * 1024;

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max { return s.to_string(); }
    let mut end = max;
    while !s.is_char_boundary(end) { end -= 1; }
    format!("{}\n…[обрезано: {} байт всего]", &s[..end], s.len())
}

fn expand(path: &str) -> PathBuf {
    let p = path.trim();
    if let Some(rest) = p.strip_prefix("~") {
        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            return PathBuf::from(home).join(rest.trim_start_matches(['/', '\\']));
        }
    }
    PathBuf::from(p)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult { pub code: Option<i32>, pub stdout: String, pub stderr: String, pub timed_out: bool }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo { pub os: &'static str, pub shell: &'static str, pub screen: bool }

/// What the agent runs on: the UI adapts tool descriptions (PowerShell vs bash) and hides screen tools.
#[tauri::command]
pub fn platform_info() -> PlatformInfo {
    PlatformInfo {
        os: crate::state::platform_name(),
        shell: if cfg!(windows) { "PowerShell" } else { "bash" },
        screen: cfg!(windows),
    }
}

/// PowerShell on Windows, bash (or sh) on Linux/macOS.
#[tauri::command]
pub async fn agent_run_command(command: String, cwd: Option<String>, timeout_sec: Option<u64>) -> Result<CommandResult, String> {
    #[cfg(windows)]
    let mut c = {
        let mut c = hidden_command("powershell.exe");
        // UTF-8 output so Cyrillic paths/messages survive
        c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command",
                &format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {command}")]);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let shell = if std::path::Path::new("/bin/bash").exists() || std::path::Path::new("/usr/bin/bash").exists() { "bash" } else { "sh" };
        let mut c = hidden_command(shell);
        c.args(["-c", &command]).env("LANG", std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()));
        c
    };
    if let Some(d) = cwd.as_deref().filter(|d| !d.trim().is_empty()) { c.current_dir(expand(d)); }
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true)
        .spawn().map_err(|e| format!("не удалось запустить: {e}"))?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let rd = tokio::spawn(async move { let mut b = vec![]; let _ = so.read_to_end(&mut b).await; b });
    let re = tokio::spawn(async move { let mut b = vec![]; let _ = se.read_to_end(&mut b).await; b });
    let secs = timeout_sec.unwrap_or(60).clamp(1, 600);
    let (code, timed_out) = match tokio::time::timeout(Duration::from_secs(secs), child.wait()).await {
        Ok(Ok(st)) => (st.code(), false),
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => { let _ = child.kill().await; (None, true) }
    };
    let stdout = String::from_utf8_lossy(&rd.await.unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&re.await.unwrap_or_default()).into_owned();
    Ok(CommandResult { code, stdout: clip(&stdout, MAX_OUT), stderr: clip(&stderr, MAX_OUT / 2), timed_out })
}

#[tauri::command]
pub async fn agent_read_file(path: String, max_bytes: Option<usize>) -> Result<String, String> {
    let p = expand(&path);
    let bytes = tokio::fs::read(&p).await.map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(clip(&String::from_utf8_lossy(&bytes), max_bytes.unwrap_or(64 * 1024).min(256 * 1024)))
}

#[tauri::command]
pub async fn agent_write_file(path: String, content: String) -> Result<String, String> {
    let p = expand(&path);
    if let Some(dir) = p.parent() { let _ = tokio::fs::create_dir_all(dir).await; }
    tokio::fs::write(&p, content.as_bytes()).await.map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!("записано {} байт в {}", content.len(), p.display()))
}

#[tauri::command]
pub async fn agent_list_dir(path: String) -> Result<String, String> {
    let p = expand(if path.trim().is_empty() { "." } else { &path });
    let mut rd = tokio::fs::read_dir(&p).await.map_err(|e| format!("{}: {e}", p.display()))?;
    let mut rows = vec![];
    while let Ok(Some(e)) = rd.next_entry().await {
        let md = e.metadata().await.ok();
        let dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
        rows.push((dir, e.file_name().to_string_lossy().into_owned(), size));
        if rows.len() >= 500 { break; }
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    let mut out = format!("{}\n", p.display());
    for (dir, name, size) in rows {
        if dir { out += &format!("[DIR]  {name}\n"); } else { out += &format!("       {name}  ({size} B)\n"); }
    }
    Ok(out)
}

/// Opens a URL, file, folder or app with the default handler ("Открыть на HF" uses this too).
#[tauri::command]
pub async fn open_external(target: String) -> Result<(), String> {
    let t = target.trim().to_string();
    if t.is_empty() { return Err("пустая цель".into()); }
    #[cfg(windows)]
    let r = if t.starts_with("http://") || t.starts_with("https://") {
        hidden_command("rundll32").args(["url.dll,FileProtocolHandler", &t]).spawn() // no cmd.exe parsing of & in URLs
    } else {
        hidden_command("cmd").args(["/C", "start", "", &t]).spawn()
    };
    #[cfg(target_os = "macos")]
    let r = hidden_command("open").arg(&t).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = hidden_command("xdg-open").arg(&t).spawn();
    r.map(|_| ()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- screen + input (Windows)

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot { pub png_base64: String, pub width: u32, pub height: u32, pub screen_width: u32, pub screen_height: u32 }

/// Primary monitor, downscaled to <= max_width (default 1280). Mouse tools take coordinates in
/// this image's pixel space; the scale is remembered here.
#[tauri::command]
pub async fn agent_screenshot(state: State<'_, Arc<AppState>>, max_width: Option<u32>) -> Result<Shot, String> {
    let maxw = max_width.unwrap_or(1280).clamp(320, 3840);
    let (shot, scale, ox, oy) = tokio::task::spawn_blocking(move || platform::screenshot(maxw)).await.map_err(|e| e.to_string())??;
    *state.shot_scale.lock().await = (scale, ox, oy);
    Ok(shot)
}

async fn to_screen(state: &AppState, x: f64, y: f64) -> (i32, i32) {
    let (s, ox, oy) = *state.shot_scale.lock().await;
    (ox + (x * s).round() as i32, oy + (y * s).round() as i32)
}

/// action: move | click | double_click | right_click
#[tauri::command]
pub async fn agent_mouse(state: State<'_, Arc<AppState>>, action: String, x: f64, y: f64) -> Result<String, String> {
    let (sx, sy) = to_screen(&state, x, y).await;
    let a = action.clone();
    tokio::task::spawn_blocking(move || platform::mouse(&a, sx, sy)).await.map_err(|e| e.to_string())??;
    Ok(format!("{action} @ ({x},{y}) → экран ({sx},{sy})"))
}

#[tauri::command]
pub async fn agent_type_text(text: String) -> Result<String, String> {
    let n = text.chars().count();
    tokio::task::spawn_blocking(move || platform::type_text(&text)).await.map_err(|e| e.to_string())??;
    Ok(format!("введено {n} символов"))
}

/// keys: "enter", "ctrl+c", "alt+tab", "win+r", "f5" ...
#[tauri::command]
pub async fn agent_key_press(keys: String) -> Result<String, String> {
    let k = keys.clone();
    tokio::task::spawn_blocking(move || platform::key_combo(&k)).await.map_err(|e| e.to_string())??;
    Ok(format!("нажато {keys}"))
}

#[cfg(windows)]
mod platform {
    use super::Shot;
    use base64::Engine as _;
    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
    use xcap::image::{imageops::FilterType, ImageEncoder};

    pub fn screenshot(maxw: u32) -> Result<(Shot, f64, i32, i32), String> {
        let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
        let m = monitors.iter().find(|m| m.is_primary().unwrap_or(false)).or(monitors.first())
            .ok_or("монитор не найден")?;
        let (ox, oy) = (m.x().unwrap_or(0), m.y().unwrap_or(0));
        let img = m.capture_image().map_err(|e| e.to_string())?;
        let (w, h) = (img.width(), img.height());
        let (img, scale) = if w > maxw {
            let nh = (h as f64 * maxw as f64 / w as f64).round() as u32;
            (xcap::image::imageops::resize(&img, maxw, nh, FilterType::Triangle), w as f64 / maxw as f64)
        } else { (img, 1.0) };
        let mut png = Vec::new();
        xcap::image::codecs::png::PngEncoder::new(&mut png)
            .write_image(img.as_raw(), img.width(), img.height(), xcap::image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string())?;
        Ok((Shot { png_base64: base64::engine::general_purpose::STANDARD.encode(&png), width: img.width(), height: img.height(),
                   screen_width: w, screen_height: h }, scale, ox, oy))
    }

    fn enigo() -> Result<Enigo, String> { Enigo::new(&Settings::default()).map_err(|e| e.to_string()) }

    pub fn mouse(action: &str, x: i32, y: i32) -> Result<(), String> {
        let mut e = enigo()?;
        e.move_mouse(x, y, Coordinate::Abs).map_err(|e| e.to_string())?;
        let r = match action {
            "move" => Ok(()),
            "click" | "left_click" => e.button(Button::Left, Direction::Click),
            "double_click" => e.button(Button::Left, Direction::Click).and_then(|_| e.button(Button::Left, Direction::Click)),
            "right_click" => e.button(Button::Right, Direction::Click),
            "scroll_down" => e.scroll(5, Axis::Vertical),
            "scroll_up" => e.scroll(-5, Axis::Vertical),
            other => return Err(format!("неизвестное действие мыши: {other}")),
        };
        r.map_err(|e| e.to_string())
    }

    pub fn type_text(text: &str) -> Result<(), String> { enigo()?.text(text).map_err(|e| e.to_string()) }

    fn key_of(name: &str) -> Result<Key, String> {
        let n = name.trim().to_lowercase();
        Ok(match n.as_str() {
            "ctrl" | "control" => Key::Control, "alt" => Key::Alt, "shift" => Key::Shift,
            "win" | "meta" | "super" | "cmd" => Key::Meta,
            "enter" | "return" => Key::Return, "tab" => Key::Tab, "esc" | "escape" => Key::Escape,
            "space" => Key::Space, "backspace" => Key::Backspace, "delete" | "del" => Key::Delete,
            "up" => Key::UpArrow, "down" => Key::DownArrow, "left" => Key::LeftArrow, "right" => Key::RightArrow,
            "home" => Key::Home, "end" => Key::End, "pageup" => Key::PageUp, "pagedown" => Key::PageDown,
            "f1" => Key::F1, "f2" => Key::F2, "f3" => Key::F3, "f4" => Key::F4, "f5" => Key::F5, "f6" => Key::F6,
            "f7" => Key::F7, "f8" => Key::F8, "f9" => Key::F9, "f10" => Key::F10, "f11" => Key::F11, "f12" => Key::F12,
            s if s.chars().count() == 1 => Key::Unicode(s.chars().next().unwrap()),
            other => return Err(format!("неизвестная клавиша: {other}")),
        })
    }

    pub fn key_combo(combo: &str) -> Result<(), String> {
        let keys: Vec<Key> = combo.split('+').map(key_of).collect::<Result<_, _>>()?;
        let mut e = enigo()?;
        let (last, mods) = keys.split_last().ok_or("пустая комбинация")?;
        for k in mods { e.key(*k, Direction::Press).map_err(|e| e.to_string())?; }
        let r = e.key(*last, Direction::Click).map_err(|e| e.to_string());
        for k in mods.iter().rev() { let _ = e.key(*k, Direction::Release); }
        r
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Shot;
    const NA: &str = "скриншоты и управление мышью/клавиатурой пока доступны только в Windows-сборке Argus";
    pub fn screenshot(_: u32) -> Result<(Shot, f64, i32, i32), String> { Err(NA.into()) }
    pub fn mouse(_: &str, _: i32, _: i32) -> Result<(), String> { Err(NA.into()) }
    pub fn type_text(_: &str) -> Result<(), String> { Err(NA.into()) }
    pub fn key_combo(_: &str) -> Result<(), String> { Err(NA.into()) }
}
