//! Vision (multimodal) support: llama-server needs a projector (mmproj) next to the model.
//! `-hf repo:quant` already auto-fetches the repo's mmproj; these args cover a local projector file
//! or an explicit projector from another HF repo.

/// `--mmproj <path>` for a local file, or `--mmproj-url <HF resolve URL>` for `owner/repo/file.gguf`
/// (or a full https URL). `hf_repo` ("owner/repo[:quant]") supplies the repo when `mmproj` is a bare file name.
pub fn mmproj_args(mmproj: Option<&str>, hf_repo: Option<&str>) -> Vec<String> {
    let Some(m) = mmproj.map(str::trim).filter(|m| !m.is_empty()) else { return vec![] };
    if m.starts_with("http://") || m.starts_with("https://") {
        return vec!["--mmproj-url".into(), m.into()];
    }
    let local = std::path::Path::new(m);
    if local.is_absolute() || local.exists() || m.contains('\\') {
        return vec!["--mmproj".into(), m.into()];
    }
    let parts: Vec<&str> = m.split('/').collect();
    let url = if parts.len() >= 3 {
        // owner/repo/path/to/file.gguf
        Some(format!("https://huggingface.co/{}/{}/resolve/main/{}", parts[0], parts[1], parts[2..].join("/")))
    } else if parts.len() == 1 {
        hf_repo.map(|r| r.split(':').next().unwrap_or(r).trim().to_string()).filter(|r| r.contains('/'))
            .map(|r| format!("https://huggingface.co/{r}/resolve/main/{m}"))
    } else { None };
    match url {
        Some(u) => vec!["--mmproj-url".into(), u],
        None => vec!["--mmproj".into(), m.into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn args() {
        assert!(mmproj_args(None, Some("a/b")).is_empty());
        assert_eq!(mmproj_args(Some("mmproj-F16.gguf"), Some("unsloth/gemma-3-4b-it-GGUF:Q4_K_M")),
            vec!["--mmproj-url", "https://huggingface.co/unsloth/gemma-3-4b-it-GGUF/resolve/main/mmproj-F16.gguf"]);
        assert_eq!(mmproj_args(Some("o/r/sub/m.gguf"), None), vec!["--mmproj-url", "https://huggingface.co/o/r/resolve/main/sub/m.gguf"]);
        assert_eq!(mmproj_args(Some("/models/m.gguf"), None), vec!["--mmproj", "/models/m.gguf"]);
    }
}
