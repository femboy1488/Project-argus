//! Plain-text extraction for «Ночная очередь» and «Поиск по моим файлам»:
//! .txt/.md (UTF-8, lossy), .docx (own minimal ZIP reader + flate2), .pdf (rough: Tj/TJ strings of
//! FlateDecode streams; scanned or CID-font PDFs are reported as unreadable). Also an HTML → text helper.
use std::io::Read;
use std::path::Path;

pub const EXTS: &[&str] = &["txt", "md", "markdown", "docx", "pdf"];

pub fn supported(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| EXTS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

/// Supported files of a folder (recursive when `deep`), sorted, skipping our own outputs (*.translate.md etc.).
pub fn list_files(dir: &Path, deep: bool) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    fn walk(d: &Path, deep: bool, out: &mut Vec<std::path::PathBuf>, depth: u32) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with("~$") { continue; }
            if p.is_dir() { if deep && depth < 12 { walk(&p, deep, out, depth + 1); } continue; }
            let low = name.to_lowercase();
            if low.ends_with(".translate.md") || low.ends_with(".summarize.md") || low.ends_with(".custom.md") { continue; }
            if supported(&p) { out.push(p); }
        }
    }
    walk(dir, deep, &mut out, 0);
    out.sort();
    out
}

pub fn extract(p: &Path) -> Result<String, String> {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let bytes = std::fs::read(p).map_err(|e| format!("не удалось прочитать {}: {e}", p.display()))?;
    let text = match ext.as_str() {
        "txt" | "md" | "markdown" => String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).to_string(),
        "docx" => docx(&bytes)?,
        "pdf" => pdf(&bytes)?,
        _ => return Err(format!("формат .{ext} не поддерживается")),
    };
    Ok(text.replace("\r\n", "\n"))
}

// ---------------- ZIP / DOCX ----------------

fn u16le(b: &[u8], i: usize) -> usize { b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize).unwrap_or(0) }
fn u32le(b: &[u8], i: usize) -> usize { b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize).unwrap_or(0) }

/// Reads one entry of a ZIP archive (stored or deflate).
pub fn zip_entry(b: &[u8], want: &str) -> Result<Vec<u8>, String> {
    let bad = || "повреждённый архив".to_string();
    let min = b.len().saturating_sub(65_557);
    let eocd = (min..b.len().saturating_sub(21)).rev().find(|&i| b[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or_else(bad)?;
    let count = u16le(b, eocd + 10);
    let mut p = u32le(b, eocd + 16);
    for _ in 0..count {
        if b.get(p..p + 4) != Some(&[0x50, 0x4b, 0x01, 0x02][..]) { return Err(bad()); }
        let method = u16le(b, p + 10);
        let csize = u32le(b, p + 20);
        let nlen = u16le(b, p + 28);
        let elen = u16le(b, p + 30);
        let clen = u16le(b, p + 32);
        let off = u32le(b, p + 42);
        let name = String::from_utf8_lossy(b.get(p + 46..p + 46 + nlen).ok_or_else(bad)?).to_string();
        p += 46 + nlen + elen + clen;
        if name != want { continue; }
        if b.get(off..off + 4) != Some(&[0x50, 0x4b, 0x03, 0x04][..]) { return Err(bad()); }
        let start = off + 30 + u16le(b, off + 26) + u16le(b, off + 28);
        let data = b.get(start..start + csize).ok_or_else(bad)?;
        return match method {
            0 => Ok(data.to_vec()),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(data).read_to_end(&mut out).map_err(|_| bad())?;
                Ok(out)
            }
            m => Err(format!("сжатие {m} не поддерживается")),
        };
    }
    Err(format!("в архиве нет {want}"))
}

fn docx(b: &[u8]) -> Result<String, String> {
    let xml = zip_entry(b, "word/document.xml").map_err(|e| format!("DOCX: {e}"))?;
    let xml = String::from_utf8_lossy(&xml);
    let mut out = String::with_capacity(xml.len() / 4);
    let mut rest: &str = &xml;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else { break };
        let tag = &rest[lt + 1..lt + gt];
        if tag == "/w:p" || tag.starts_with("w:br") || tag.starts_with("w:cr") { out.push('\n'); }
        else if tag.starts_with("w:tab") && !tag.starts_with("w:tabs") { out.push('\t'); }
        rest = &rest[lt + gt + 1..];
    }
    Ok(decode_entities(&out))
}

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') { return s.to_string(); }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[..rest.len().min(12)].find(';');
        let Some(end) = end else { out.push('&'); rest = &rest[1..]; continue };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''),
            "nbsp" => Some(' '), "mdash" => Some('—'), "ndash" => Some('–'), "laquo" => Some('«'), "raquo" => Some('»'),
            "hellip" => Some('…'), "copy" => Some('©'),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch { Some(c) => { out.push(c); rest = &rest[end + 1..]; } None => { out.push('&'); rest = &rest[1..]; } }
    }
    out.push_str(rest);
    out
}

// ---------------- HTML ----------------

/// HTML → readable text: drops script/style/head, block tags become line breaks, entities decoded.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 3);
    let low = html.to_ascii_lowercase();
    let mut i = 0;
    let b = html.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let Some(gt) = html[i..].find('>') else { break };
            let tag = &low[i + 1..i + gt];
            let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if !tag.starts_with('/') && matches!(name.as_str(), "script" | "style" | "head" | "noscript" | "svg") {
                let close = format!("</{name}");
                if let Some(c) = low[i..].find(&close) {
                    i += c;
                    i += html[i..].find('>').map(|x| x + 1).unwrap_or(html.len() - i);
                    continue;
                }
            }
            if matches!(name.as_str(), "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table" | "section" | "dd" | "dt" | "blockquote") {
                out.push('\n');
            } else if name == "td" || name == "th" { out.push(' '); }
            i += gt + 1;
        } else {
            let next = html[i..].find('<').map(|x| i + x).unwrap_or(html.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let text = decode_entities(&out);
    let mut res = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let l = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if l.is_empty() { blank += 1; if blank == 1 && !res.is_empty() { res.push('\n'); } continue; }
        blank = 0;
        res.push_str(&l);
        res.push('\n');
    }
    res.trim().to_string()
}

// ---------------- PDF (rough) ----------------

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    if from >= h.len() { return None; }
    h[from..].windows(n.len()).position(|w| w == n).map(|p| p + from)
}

fn pdf(b: &[u8]) -> Result<String, String> {
    let mut out = String::new();
    let mut pos = 0;
    while let Some(s) = find(b, b"stream", pos) {
        // skip "endstream"
        if s >= 3 && &b[s - 3..s] == b"end" { pos = s + 6; continue; }
        let lo = s.saturating_sub(1000).max(pos);
        let dict_start = b[lo..s].windows(3).rposition(|w| w == b"obj").map(|x| lo + x).unwrap_or(lo);
        let dict = &b[dict_start..s];
        let mut d = s + 6;
        if b.get(d) == Some(&b'\r') { d += 1; }
        if b.get(d) == Some(&b'\n') { d += 1; }
        let Some(e) = find(b, b"endstream", d) else { break };
        let raw = &b[d..e];
        pos = e + 9;
        let dict_s = String::from_utf8_lossy(dict);
        if dict_s.contains("/Image") || dict_s.contains("/DCTDecode") { continue; }
        let data = if dict_s.contains("/FlateDecode") {
            let mut v = Vec::new();
            if flate2::read::ZlibDecoder::new(raw).read_to_end(&mut v).is_err() && v.is_empty() { continue; }
            v
        } else if dict_s.contains("/Filter") { continue } else { raw.to_vec() };
        pdf_text_ops(&data, &mut out);
    }
    let letters = out.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 20 {
        return Err("PDF не читается: нет текстового слоя (скан) или нестандартные шрифты. Сохраните его как .txt или .docx".into());
    }
    Ok(out)
}

/// Collects strings of Tj / TJ / ' / " inside BT…ET blocks.
fn pdf_text_ops(d: &[u8], out: &mut String) {
    let mut i = 0;
    let mut in_bt = false;
    let mut line = Vec::<u8>::new();
    while i < d.len() {
        let c = d[i];
        if c == b'(' && in_bt {
            let mut depth = 1;
            i += 1;
            while i < d.len() && depth > 0 {
                let ch = d[i];
                match ch {
                    b'\\' => {
                        i += 1;
                        if i >= d.len() { break; }
                        match d[i] {
                            b'n' => line.push(b'\n'), b'r' => {}, b't' => line.push(b' '),
                            b'(' | b')' | b'\\' => line.push(d[i]),
                            b'0'..=b'7' => {
                                let mut v: u32 = 0; let mut k = 0;
                                while k < 3 && i < d.len() && (b'0'..=b'7').contains(&d[i]) { v = v * 8 + (d[i] - b'0') as u32; i += 1; k += 1; }
                                line.push(v as u8);
                                continue;
                            }
                            _ => {}
                        }
                    }
                    b'(' => { depth += 1; line.push(ch); }
                    b')' => { depth -= 1; if depth > 0 { line.push(ch); } }
                    _ => line.push(ch),
                }
                i += 1;
            }
            continue;
        }
        if c == b'%' { while i < d.len() && d[i] != b'\n' { i += 1; } continue; }
        if d[i..].starts_with(b"BT") && (i == 0 || d[i - 1].is_ascii_whitespace()) { in_bt = true; i += 2; continue; }
        if d[i..].starts_with(b"ET") && (i == 0 || d[i - 1].is_ascii_whitespace()) {
            in_bt = false; i += 2;
            flush(&mut line, out);
            continue;
        }
        if in_bt && (d[i..].starts_with(b"Td") || d[i..].starts_with(b"TD") || d[i..].starts_with(b"T*")) {
            if !line.is_empty() && !line.ends_with(b" ") { line.push(b' '); }
        }
        i += 1;
    }
    flush(&mut line, out);
}

fn flush(line: &mut Vec<u8>, out: &mut String) {
    if line.is_empty() { return; }
    // PDFDocEncoding ≈ Latin-1 for printable text; UTF-16BE strings start with FE FF
    let s: String = if line.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = line[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else { line.iter().map(|&b| b as char).filter(|c| !c.is_control() || *c == '\n').collect() };
    out.push_str(s.trim());
    out.push('\n');
    line.clear();
}

/// Splits text into ~`size`-char pieces on paragraph / sentence boundaries, with `overlap` chars repeated.
pub fn chunk(text: &str, size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + size).min(chars.len());
        if end < chars.len() {
            let win = &chars[start..end];
            let lo = size / 2;
            let cut = win.iter().rposition(|&c| c == '\n').filter(|&p| p > lo)
                .or_else(|| win.windows(2).rposition(|w| matches!(w[0], '.' | '!' | '?' | '…') && w[1] == ' ').map(|p| p + 1).filter(|&p| p > lo))
                .or_else(|| win.iter().rposition(|&c| c == ' ').filter(|&p| p > lo));
            if let Some(c) = cut { end = start + c + 1; }
        }
        let piece: String = chars[start..end].iter().collect();
        if !piece.trim().is_empty() { out.push(piece.trim().to_string()); }
        if end >= chars.len() { break; }
        start = if end - start > overlap * 2 { end - overlap } else { end };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html() {
        let t = strip_html("<html><head><title>x</title></head><body><p>Привет &amp; мир</p><script>var a=1;</script><div>два</div></body></html>");
        assert_eq!(t, "Привет & мир\nдва");
    }
    #[test]
    fn chunks() {
        let t = "a".repeat(1000) + "\n" + &"b".repeat(1000);
        let c = chunk(&t, 800, 100);
        assert!(c.len() >= 3);
    }
}
