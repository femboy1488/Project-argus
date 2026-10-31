//! Persistent user settings (JSON in the app config dir): LAN nodes, per-device speed weights,
//! disabled devices, fast start.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub const DEFAULT_LAN_PORT: u16 = 50052;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LanNode {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub name: String,
}

impl LanNode {
    pub fn id(&self) -> String { format!("lan:{}:{}", self.host, self.port) }
    pub fn endpoint(&self) -> String { format!("{}:{}", self.host, self.port) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub lan_nodes: Vec<LanNode>,
    /// node id -> speed weight (1.0 = CPU baseline)
    pub weights: HashMap<String, f32>,
    /// node ids excluded from --rpc / --device
    pub disabled: HashSet<String>,
    /// opt-in devices that are off by default (integrated GPUs next to a discrete one)
    pub enabled: HashSet<String>,
    /// skip re-provisioning a USB node whose rpc-server is already running the bundled version
    pub fast_start: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { lan_nodes: vec![], weights: HashMap::new(), disabled: HashSet::new(), enabled: HashSet::new(), fast_start: true }
    }
}

impl Settings {
    pub fn load(path: &Option<PathBuf>) -> Self {
        path.as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Option<PathBuf>) {
        let Some(p) = path else { return };
        if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            if let Err(e) = std::fs::write(p, s) { log::warn!("settings save {}: {e}", p.display()); }
        }
    }
}

/// Parse "host", "host:port" or "[v6]:port". Default port 50052.
pub fn parse_host_port(input: &str) -> Result<(String, u16), String> {
    let s = input.trim().trim_start_matches("tcp://").trim_end_matches('/');
    if s.is_empty() { return Err("пустой адрес".into()); }
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').ok_or("неверный IPv6 адрес")?;
        let port = tail.strip_prefix(':').map(|p| p.parse::<u16>()).transpose().map_err(|_| "неверный порт")?;
        (h.to_string(), port)
    } else if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':').unwrap();
        (h.to_string(), Some(p.parse::<u16>().map_err(|_| "неверный порт")?))
    } else {
        (s.to_string(), None)
    };
    if host.is_empty() || host.contains(char::is_whitespace) { return Err("неверный адрес".into()); }
    Ok((host, port.unwrap_or(DEFAULT_LAN_PORT)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_port() {
        assert_eq!(parse_host_port("192.168.1.5").unwrap(), ("192.168.1.5".into(), 50052));
        assert_eq!(parse_host_port("pi.local:6000").unwrap(), ("pi.local".into(), 6000));
        assert_eq!(parse_host_port("[fe80::1]:50053").unwrap(), ("fe80::1".into(), 50053));
        assert!(parse_host_port("a:b").is_err());
    }
}
