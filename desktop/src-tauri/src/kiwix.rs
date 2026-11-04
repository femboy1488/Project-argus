//! «Библиотека»: offline Wikipedia & co. through kiwix-serve — on a phone (USB: forwarded PC port from nodesvc.rs,
//! Wi-Fi: http://<phone ip>:50063 directly) or at a URL the user gives (saved in argus-kiwix.json). Search: /search?pattern=&books.name= (HTML), fallback /suggest;
//! read: an article page stripped to text (≤ 8000 chars); books: /catalog/v2/entries (OPDS Atom).
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tauri::State;

/// first USB phone's forward; used when nodesvc has not listed the phone yet
const PHONE_URL: &str = "http://127.0.0.1:50163";
const MAX_READ: usize = 8000;

#[derive(Serialize, Deserialize, Default)]
struct Cfg { #[serde(default)] url: String }

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Book { pub name: String, pub title: String, pub path: String, pub language: String, pub summary: String }

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WikiHit { pub title: String, pub path: String, pub snippet: String, pub book: String }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KiwixInfo { pub url: String, pub base: Option<String>, pub phone: bool }

fn cfg_path(st: &AppState) -> Option<std::path::PathBuf> { crate::queue::data_dir(st).map(|d| d.join("argus-kiwix.json")) }
fn load_cfg(st: &AppState) -> Cfg {
    cfg_path(st).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn client() -> reqwest::Client { reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default() }

async fn reachable(c: &reqwest::Client, base: &str) -> bool {
    c.get(format!("{base}/catalog/v2/root.xml")).timeout(Duration::from_millis(2500)).send().await.map(|r| r.status().as_u16() < 500).unwrap_or(false)
}

async fn base(st: &AppState) -> Result<String, String> {
    let c = client();
    let url = load_cfg(st).url.trim().trim_end_matches('/').to_string();
    if !url.is_empty() {
        if reachable(&c, &url).await { return Ok(url); }
        return Err(format!("Библиотека по адресу {url} не отвечает"));
    }
    let mut cands = crate::nodesvc::ready_bases("kiwix");
    if !cands.iter().any(|b| b == PHONE_URL) { cands.push(PHONE_URL.into()); }
    for b in cands { if reachable(&c, &b).await { return Ok(b); } }
    Err("Библиотека не найдена: нет телефона с библиотекой и не указан адрес kiwix-serve".into())
}

async fn get_text(c: &reqwest::Client, url: &str) -> Result<String, String> {
    let r = c.get(url).send().await.map_err(|e| format!("библиотека не отвечает: {e}"))?;
    if !r.status().is_success() { return Err(format!("библиотека вернула {}", r.status())); }
    r.text().await.map_err(|e| e.to_string())
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

fn tag_text(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let Some(a) = xml.find(&open) else { return String::new() };
    let Some(gt) = xml[a..].find('>') else { return String::new() };
    let start = a + gt + 1;
    let close = format!("</{tag}>");
    let end = xml[start..].find(&close).map(|e| start + e).unwrap_or(start);
    crate::textx::decode_entities(xml[start..end].trim())
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let k = format!("{name}=\"");
    let a = tag.find(&k)? + k.len();
    let e = tag[a..].find('"')? + a;
    Some(crate::textx::decode_entities(&tag[a..e]))
}

async fn books(c: &reqwest::Client, base: &str) -> Result<Vec<Book>, String> {
    let xml = get_text(c, &format!("{base}/catalog/v2/entries?count=100")).await?;
    let mut out = Vec::new();
    for part in xml.split("<entry").skip(1) {
        let entry = part.split("</entry>").next().unwrap_or("");
        let mut path = String::new();
        for l in entry.split("<link").skip(1) {
            let t = l.split('>').next().unwrap_or("");
            if attr(t, "type").as_deref() == Some("text/html") { path = attr(t, "href").unwrap_or_default(); }
        }
        let name = tag_text(entry, "name");
        let name = if name.is_empty() { path.trim_start_matches("/content/").trim_matches('/').to_string() } else { name };
        out.push(Book { name, title: tag_text(entry, "title"), path, language: tag_text(entry, "language"), summary: tag_text(entry, "summary") });
    }
    Ok(out)
}

/// Parses kiwix-serve's full-text search page: <li><a href="/content/book/A/Path">Title</a> … <cite>snippet</cite>
fn parse_search(html: &str, book: &str) -> Vec<WikiHit> {
    let mut out = Vec::new();
    let body = html.find("<ul").map(|i| &html[i..]).unwrap_or(html);
    for li in body.split("<li").skip(1) {
        let li = li.split("</li>").next().unwrap_or("");
        let Some(a) = li.find("<a") else { continue };
        let t = &li[a..];
        let Some(href) = attr(t.split('>').next().unwrap_or(""), "href") else { continue };
        if !href.starts_with('/') || href.contains("pattern=") || href.contains("start=") { continue; }
        let title = t.find('>').and_then(|g| t[g + 1..].find("</a>").map(|e| crate::textx::strip_html(&t[g + 1..g + 1 + e]))).unwrap_or_default();
        let snippet = li.find("<cite").map(|c| crate::textx::strip_html(li[c..].split("</cite>").next().unwrap_or("").split_once('>').map(|x| x.1).unwrap_or(""))).unwrap_or_default();
        if title.is_empty() { continue; }
        out.push(WikiHit { title, path: href, snippet, book: book.to_string() });
    }
    out
}

async fn search_in(c: &reqwest::Client, base: &str, book: &Book, q: &str) -> Vec<WikiHit> {
    let url = format!("{base}/search?pattern={}&books.name={}&pageLength=10", enc(q), enc(&book.name));
    if let Ok(html) = get_text(c, &url).await {
        let hits = parse_search(&html, &book.name);
        if !hits.is_empty() { return hits; }
    }
    // older servers / books without a full-text index: title suggestions
    let content = book.path.trim_start_matches("/content/").trim_matches('/');
    for url in [format!("{base}/suggest?books.name={}&term={}&count=10", enc(&book.name), enc(q)),
                format!("{base}/suggest?content={}&term={}&count=10", enc(content), enc(q))] {
        let Ok(txt) = get_text(c, &url).await else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { continue };
        let hits: Vec<WikiHit> = v.as_array().map(|a| a.iter().filter_map(|e| {
            let path = e.get("path").and_then(|p| p.as_str())?;
            if e.get("kind").and_then(|k| k.as_str()) == Some("pattern") { return None; }
            let path = if path.starts_with('/') { path.to_string() } else { format!("{}/{}", book.path.trim_end_matches('/'), path) };
            Some(WikiHit { title: crate::textx::strip_html(e.get("label").or(e.get("value")).and_then(|l| l.as_str()).unwrap_or("")), path, snippet: String::new(), book: book.name.clone() })
        }).collect()).unwrap_or_default();
        if !hits.is_empty() { return hits; }
    }
    Vec::new()
}

#[tauri::command]
pub async fn kiwix_info(state: State<'_, Arc<AppState>>) -> Result<KiwixInfo, String> {
    let url = load_cfg(&state).url;
    let b = base(&state).await.ok();
    Ok(KiwixInfo { phone: b.is_some() && url.trim().is_empty(), url, base: b })
}

#[tauri::command]
pub async fn kiwix_set_url(state: State<'_, Arc<AppState>>, url: String) -> Result<(), String> {
    let mut u = url.trim().trim_end_matches('/').to_string();
    if !u.is_empty() && !u.starts_with("http") { u = format!("http://{u}"); }
    let p = cfg_path(&state).ok_or("нет папки настроек")?;
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    std::fs::write(p, serde_json::to_string(&Cfg { url: u }).unwrap_or_default()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn kiwix_books(state: State<'_, Arc<AppState>>) -> Result<Vec<Book>, String> {
    let b = base(&state).await?;
    books(&client(), &b).await
}

#[tauri::command]
pub async fn kiwix_search(state: State<'_, Arc<AppState>>, q: String) -> Result<Vec<WikiHit>, String> {
    let b = base(&state).await?;
    let c = client();
    let list = books(&c, &b).await?;
    if list.is_empty() { return Err("В библиотеке нет книг".into()); }
    let mut out = Vec::new();
    for book in list.iter().take(6) {
        out.extend(search_in(&c, &b, book, q.trim()).await);
        if out.len() >= 15 { break; }
    }
    out.truncate(15);
    Ok(out)
}

#[tauri::command]
pub async fn kiwix_read(state: State<'_, Arc<AppState>>, path: String) -> Result<String, String> {
    let b = base(&state).await?;
    let p = path.trim();
    let url = if p.starts_with("http") { p.to_string() } else if p.starts_with('/') { format!("{b}{p}") } else { format!("{b}/{p}") };
    let html = get_text(&client(), &url).await?;
    let text = crate::textx::strip_html(&html);
    Ok(if text.chars().count() > MAX_READ { text.chars().take(MAX_READ).collect::<String>() + "\n…[обрезано]" } else { text })
}
