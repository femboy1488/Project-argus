use crate::adb::Adb;
use crate::settings::Settings;
use crate::split::{self, SplitEntry};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::{process::Child, sync::Mutex};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind { Gpu, Cpu, Phone, Tablet, Lan }

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeStage { Unauthorized, Installing, Updating, Forwarding, Starting, Syncing, Connected, Offline, Error }

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Transport { Local, Usb, Lan }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub name: String,
    pub kind: NodeKind,
    pub stage: NodeStage,
    pub transport: Transport,
    pub endpoint: Option<String>,
    pub total_mb: u64,
    pub available_mb: u64,
    pub usable_mb: u64,
    pub load_pct: f32,
    pub error: Option<String>,
    /// speed weight for --tensor-split (1.0 = phone/Pi CPU baseline)
    pub weight: f32,
    /// included in --rpc / --device
    pub enabled: bool,
    /// llama.cpp device name for local GPUs ("Vulkan0")
    pub device: Option<String>,
    /// node app versionName (USB) / RPC protocol version (LAN)
    pub version: Option<String>,
    /// devices exposed by this rpc-server (normally 1)
    pub rpc_devices: u32,
    /// integrated GPU (shares system RAM)
    pub integrated: bool,
}

impl Node {
    pub fn device(serial: &str, name: &str) -> Self {
        Self { id: serial.into(), name: name.into(), kind: NodeKind::Phone, stage: NodeStage::Installing,
               transport: Transport::Usb, endpoint: None, total_mb: 0, available_mb: 0, usable_mb: 0,
               load_pct: 0.0, error: None, weight: 1.0, enabled: true, device: None, version: None,
               rpc_devices: 1, integrated: false }
    }
    pub fn lan(id: &str, name: &str, endpoint: &str) -> Self {
        Self { kind: NodeKind::Lan, stage: NodeStage::Offline, transport: Transport::Lan,
               endpoint: Some(endpoint.into()), ..Self::device(id, name) }
    }
    pub fn local(id: &str, name: &str, kind: NodeKind) -> Self {
        Self { kind, stage: NodeStage::Connected, transport: Transport::Local, ..Self::device(id, name) }
    }
    pub fn online(&self) -> bool { self.stage == NodeStage::Connected }
}

/// Per-device bookkeeping that the UI does not need.
#[derive(Default)]
pub struct DeviceRuntime {
    pub local_port: u16,
    pub last_cpu: Option<(u64, u64)>, // (idle, total) jiffies from /proc/stat
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSnapshot {
    /// The PC. With GPUs: aggregate of enabled local GPUs. Without: host CPU/RAM.
    pub host: Node,
    /// Every GPU llama.cpp sees on this PC (each one its own device in --device / -ts).
    pub gpus: Vec<Node>,
    /// USB + LAN rpc nodes, in --rpc order.
    pub nodes: Vec<Node>,
    pub total_mb: u64,
    pub usable_mb: u64,
    pub rpc_servers: Vec<String>,
    /// Planned layer split (memory x speed weight), same order as --device / -ts.
    pub split: Vec<SplitEntry>,
    pub fast_start: bool,
    pub bundled_node_version: String,
}

pub struct AppState {
    pub adb: Adb,
    pub apk_path: Option<PathBuf>,
    pub bundled_node_version: String,
    pub llama_bin: PathBuf,
    pub config_path: Option<PathBuf>,
    pub settings: Mutex<Settings>,
    pub host: Mutex<Node>,
    pub gpus: Mutex<Vec<Node>>,
    pub nodes: Mutex<HashMap<String, Node>>,
    pub runtime: Mutex<HashMap<String, DeviceRuntime>>,
    pub provisioning: Mutex<HashSet<String>>,
    pub engine: Mutex<Option<Child>>,
    /// serialized launch args of the running engine (to skip identical relaunches)
    pub engine_sig: Mutex<Option<String>>,
    /// last screenshot scale for agent mouse coordinates: (scale, origin_x, origin_y)
    pub shot_scale: Mutex<(f64, i32, i32)>,
}

impl AppState {
    pub fn new(adb: Adb, apk_path: Option<PathBuf>, bundled_node_version: String, llama_bin: PathBuf,
               config_path: Option<PathBuf>) -> Self {
        let host = Node::local("host", "Host", NodeKind::Cpu);
        let settings = Settings::load(&config_path);
        Self { adb, apk_path, bundled_node_version, llama_bin, config_path, settings: Mutex::new(settings),
               host: Mutex::new(host), gpus: Default::default(), nodes: Default::default(),
               runtime: Default::default(), provisioning: Default::default(), engine: Mutex::new(None),
               engine_sig: Mutex::new(None), shot_scale: Mutex::new((1.0, 0, 0)) }
    }

    pub async fn save_settings(&self) {
        let s = self.settings.lock().await.clone();
        s.save(&self.config_path);
    }

    pub async fn snapshot(&self) -> ClusterSnapshot {
        let settings = self.settings.lock().await.clone();
        let apply = |n: &mut Node| {
            let default_w = if n.kind == NodeKind::Gpu && !n.integrated { 2.0 } else { 1.0 };
            n.weight = settings.weights.get(&n.id).copied().unwrap_or(default_w);
            n.enabled = if n.integrated { settings.enabled.contains(&n.id) } else { !settings.disabled.contains(&n.id) };
        };
        let mut gpus = self.gpus.lock().await.clone();
        gpus.iter_mut().for_each(apply);
        let mut host = self.host.lock().await.clone();
        // host = aggregate of enabled local GPUs (keeps the old single-host UI meaningful)
        let en: Vec<&Node> = gpus.iter().filter(|g| g.enabled).collect();
        if !en.is_empty() {
            host.kind = NodeKind::Gpu;
            host.name = if en.len() == 1 { en[0].name.clone() } else { format!("{} × GPU", en.len()) };
            host.total_mb = en.iter().map(|g| g.total_mb).sum();
            host.available_mb = en.iter().map(|g| g.available_mb).sum();
            host.usable_mb = en.iter().map(|g| g.usable_mb).sum();
            host.load_pct = en.iter().map(|g| g.load_pct).fold(0.0, f32::max);
        }
        let mut nodes: Vec<Node> = self.nodes.lock().await.values().cloned().collect();
        nodes.iter_mut().for_each(apply);
        // USB first (by serial), then LAN (by id): this IS the --rpc order
        nodes.sort_by(|a, b| ((a.transport == Transport::Lan), &a.id).cmp(&((b.transport == Transport::Lan), &b.id)));
        let active: Vec<&Node> = nodes.iter().filter(|n| n.online() && n.enabled).collect();
        let total_mb = host.total_mb + active.iter().map(|n| n.total_mb).sum::<u64>();
        let usable_mb = host.usable_mb + active.iter().map(|n| n.usable_mb).sum::<u64>();
        let rpc_servers = active.iter().filter_map(|n| n.endpoint.clone()).collect();
        let split = split::plan(&nodes, &gpus, None);
        ClusterSnapshot { host, gpus, nodes, total_mb, usable_mb, rpc_servers, split,
                          fast_start: settings.fast_start, bundled_node_version: self.bundled_node_version.clone() }
    }
}
