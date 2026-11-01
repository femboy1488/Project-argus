use crate::adb::Adb;
use crate::llama::StartModelArgs;
use crate::settings::Settings;
use crate::split::{self, SplitEntry};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;
use tokio::{process::Child, sync::Mutex, task::AbortHandle};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind { Gpu, Cpu, Phone, Tablet, Lan }

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeStage { Unauthorized, Installing, Updating, Forwarding, Starting, Syncing, Connected, Offline, Error }

impl NodeStage {
    /// provisioning in progress: the cluster is not settled yet
    pub fn transitional(self) -> bool {
        matches!(self, Self::Installing | Self::Updating | Self::Forwarding | Self::Starting | Self::Syncing)
    }
}

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
    /// failed provisioning attempts in a row (USB)
    pub attempts: u32,
    /// seconds until the next automatic retry (USB, stage error)
    pub retry_in: Option<u32>,
    /// rpc-server build reported by a Wi-Fi beacon ("dotprod", "x64-vulkan" …)
    pub variant: Option<String>,
    /// LAN node announced by UDP beacon (Wi-Fi mode); offline when the beacon stops
    pub beacon: bool,
    /// part of the running engine's --rpc list
    pub in_engine: bool,
    /// node app versionCode (USB: dumpsys; Wi-Fi: beacon "vc")
    pub app_vc: Option<u64>,
    /// node app versionName from the Wi-Fi beacon "vn" (USB: in `version`)
    pub app_version: Option<String>,
    /// bundled node version ("0.3.1 (5)") when this node runs an older Argus Node app (USB or Wi-Fi beacon).
    /// Never installed without the user's click («Обновить ноду на …?»).
    pub update_available: Option<String>,
    /// the user chose «Пропустить эту версию» for exactly this bundled version
    pub update_skipped: bool,
}

impl Node {
    pub fn device(serial: &str, name: &str) -> Self {
        Self { id: serial.into(), name: name.into(), kind: NodeKind::Phone, stage: NodeStage::Installing,
               transport: Transport::Usb, endpoint: None, total_mb: 0, available_mb: 0, usable_mb: 0,
               load_pct: 0.0, error: None, weight: 1.0, enabled: true, device: None, version: None,
               rpc_devices: 1, integrated: false, attempts: 0, retry_in: None, variant: None, beacon: false,
               in_engine: false, app_vc: None, app_version: None, update_available: None, update_skipped: false }
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

/// Per-device bookkeeping that the UI does not need. Removed on unplug (the port is kept in `ports`).
#[derive(Default)]
pub struct DeviceRuntime {
    pub last_cpu: Option<(u64, u64)>, // (idle, total) jiffies from /proc/stat
    /// adb transport id seen when provisioned; a different id == the cable was re-plugged
    pub transport_id: Option<u64>,
    pub attempts: u32,
    pub retry_at: Option<Instant>,
    /// next provisioning must restart RpcService (manual "Переподключить")
    pub force_restart: bool,
    /// in-flight provisioning task (aborted on unplug)
    pub task: Option<AbortHandle>,
    /// bumped on every unplug/re-plug; a provisioning task only writes results for its own generation
    pub gen: u64,
    /// consecutive metric read failures while Connected
    pub metric_failures: u32,
    /// when the device went OFFLINE (forgotten after 10 min)
    pub offline_since: Option<Instant>,
    /// the user clicked «Обновить» for this device: the next provisioning may `adb install -r` the bundled APK
    pub approve_update: bool,
}

pub use crate::beacon::Discovered;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredView {
    pub id: String,
    pub endpoint: String,
    pub name: String,
    pub ram_mb: u64,
    pub free_mb: u64,
    pub variant: Option<String>,
    pub age_sec: u64,
    pub added: bool,
    /// Argus Node app version from the beacon (0.3.1+)
    pub app_version: Option<String>,
    pub app_vc: Option<u64>,
    /// older than the APK bundled with this desktop
    pub outdated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EngineStatus {
    /// nothing running (or stopped by the user)
    Idle,
    /// process started, /health not 200 yet (download / weight upload)
    Loading,
    Ready,
    /// died or lost a node; waiting for the cluster to come back before an auto-restart
    Broken,
}

#[derive(Debug, Clone)]
pub struct EngineNode { pub id: String, pub name: String, pub endpoint: String }

/// Supervisor state of llama-server (see llama.rs `supervise`).
pub struct EngineState {
    pub status: EngineStatus,
    pub reason: Option<String>,
    /// model the user asked for; None after "Stop". Kept while Broken -> auto-restart target.
    pub args: Option<StartModelArgs>,
    /// rpc nodes of the running engine (in --rpc order)
    pub nodes: Vec<EngineNode>,
    pub started: Option<Instant>,
    /// /health returned 200 at least once for this process (crash during load => no auto-restart)
    pub ever_healthy: bool,
    pub pending_restart: bool,
    /// auto-restart timestamps (budget: 3 per 10 min)
    pub restarts: Vec<Instant>,
    /// engine left running by a previous Argus session (keep loaded), watched by pid
    pub adopted_pid: Option<u32>,
    /// rpc endpoint set + stages, to debounce "cluster settled"
    pub cluster_sig: String,
    pub cluster_stable_since: Instant,
    /// last error-looking line of the engine log
    pub last_err: Option<String>,
    /// an online node is not used by the running engine
    pub new_nodes: bool,
    pub last_health_check: Option<Instant>,
    /// the current Loading is an automatic restart (toast when it becomes Ready)
    pub restarting: bool,
}

impl Default for EngineState {
    fn default() -> Self {
        Self { status: EngineStatus::Idle, reason: None, args: None, nodes: vec![], started: None, ever_healthy: false,
               pending_restart: false, restarts: vec![], adopted_pid: None, cluster_sig: String::new(),
               cluster_stable_since: Instant::now(), last_err: None, new_nodes: false, last_health_check: None, restarting: false }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineView {
    pub status: EngineStatus,
    pub reason: Option<String>,
    pub alias: Option<String>,
    /// names of the rpc nodes in use
    pub nodes: Vec<String>,
    pub pending_restart: bool,
    pub new_nodes: bool,
    pub can_restart: bool,
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
    /// nodes heard on UDP :50053 in the last minute
    pub discovered: Vec<DiscoveredView>,
    pub auto_add_discovered: bool,
    /// "" = listening; otherwise why the beacon listener is not running
    pub beacon_error: String,
    pub adb_ok: bool,
    pub engine: EngineView,
    pub platform: &'static str,
    /// desktop app version (Cargo.toml)
    pub app_version: &'static str,
    /// versionCode of the bundled node.apk (from node.version "0.3.1 (5)")
    pub bundled_node_vc: Option<u64>,
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
    /// serial -> PC port, kept across unplug so a re-plugged device gets the same 127.0.0.1:<port>
    pub ports: Mutex<HashMap<String, u16>>,
    /// serials with a provisioning task queued/running (per-device lock)
    pub provisioning: Mutex<HashSet<String>>,
    pub engine: Mutex<Option<Child>>,
    /// serialized launch args of the running engine (to skip identical relaunches)
    pub engine_sig: Mutex<Option<String>>,
    pub engine_state: Mutex<EngineState>,
    /// serializes start / stop / auto-restart
    pub launch_lock: Mutex<()>,
    pub discovered: Mutex<HashMap<String, Discovered>>,
    pub beacon_error: Mutex<String>,
    pub adb_ok: Mutex<bool>,
    /// last screenshot scale for agent mouse coordinates: (scale, origin_x, origin_y)
    pub shot_scale: Mutex<(f64, i32, i32)>,
    /// self-update state (updater.rs)
    pub update: Mutex<crate::updater::UpdState>,
}

impl AppState {
    pub fn new(adb: Adb, apk_path: Option<PathBuf>, bundled_node_version: String, llama_bin: PathBuf,
               config_path: Option<PathBuf>) -> Self {
        let host = Node::local("host", "Host", NodeKind::Cpu);
        let settings = Settings::load(&config_path);
        Self { adb, apk_path, bundled_node_version, llama_bin, config_path, settings: Mutex::new(settings),
               host: Mutex::new(host), gpus: Default::default(), nodes: Default::default(),
               runtime: Default::default(), ports: Default::default(), provisioning: Default::default(),
               engine: Mutex::new(None), engine_sig: Mutex::new(None), engine_state: Mutex::new(EngineState::default()),
               launch_lock: Mutex::new(()), discovered: Default::default(), beacon_error: Mutex::new(String::new()),
               adb_ok: Mutex::new(true), shot_scale: Mutex::new((1.0, 0, 0)), update: Default::default() }
    }

    pub async fn save_settings(&self) {
        let s = self.settings.lock().await.clone();
        s.save(&self.config_path);
    }

    pub async fn engine_view(&self) -> EngineView {
        let e = self.engine_state.lock().await;
        EngineView {
            status: e.status, reason: e.reason.clone(),
            alias: e.args.as_ref().and_then(|a| a.alias.clone()),
            nodes: e.nodes.iter().map(|n| n.name.clone()).collect(),
            pending_restart: e.pending_restart, new_nodes: e.new_nodes, can_restart: e.args.is_some(),
        }
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
        let engine_ids: HashSet<String> = self.engine_state.lock().await.nodes.iter().map(|n| n.id.clone()).collect();
        let mut nodes: Vec<Node> = self.nodes.lock().await.values().cloned().collect();
        nodes.iter_mut().for_each(apply);
        nodes.iter_mut().for_each(|n| n.in_engine = engine_ids.contains(&n.id));
        let bundled_vc = crate::cluster::bundled_code(&self.bundled_node_version);
        for n in nodes.iter_mut() {
            n.update_skipped = n.update_available.is_some() && settings.skipped_node_version.as_deref() == Some(self.bundled_node_version.as_str());
        }
        // retry countdown for failed USB nodes
        {
            let rt = self.runtime.lock().await;
            let now = Instant::now();
            for n in nodes.iter_mut() {
                if let Some(r) = rt.get(&n.id) {
                    n.attempts = r.attempts;
                    n.retry_in = r.retry_at.filter(|_| n.stage == NodeStage::Error)
                        .map(|t| t.saturating_duration_since(now).as_secs() as u32);
                }
            }
        }
        // USB first (by serial), then LAN (by id): this IS the --rpc order
        nodes.sort_by(|a, b| ((a.transport == Transport::Lan), &a.id).cmp(&((b.transport == Transport::Lan), &b.id)));
        let active: Vec<&Node> = nodes.iter().filter(|n| n.online() && n.enabled).collect();
        let total_mb = host.total_mb + active.iter().map(|n| n.total_mb).sum::<u64>();
        let usable_mb = host.usable_mb + active.iter().map(|n| n.usable_mb).sum::<u64>();
        let rpc_servers = active.iter().filter_map(|n| n.endpoint.clone()).collect();
        let split = split::plan(&nodes, &gpus, None);
        let added: HashSet<String> = settings.lan_nodes.iter().map(|l| l.id()).collect();
        let now = Instant::now();
        let mut discovered: Vec<DiscoveredView> = self.discovered.lock().await.values().map(|d| DiscoveredView {
            id: d.id(), endpoint: d.endpoint(), name: d.name.clone(), ram_mb: d.ram_mb, free_mb: d.free_mb,
            variant: d.variant.clone(), age_sec: now.saturating_duration_since(d.last_seen).as_secs(), added: added.contains(&d.id()),
            app_version: d.vn.clone(), app_vc: d.vc,
            outdated: matches!((d.vc, bundled_vc), (Some(a), Some(b)) if a < b),
        }).collect();
        discovered.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
        ClusterSnapshot { host, gpus, nodes, total_mb, usable_mb, rpc_servers, split,
                          fast_start: settings.fast_start, bundled_node_version: self.bundled_node_version.clone(),
                          discovered, auto_add_discovered: settings.auto_add_discovered,
                          beacon_error: self.beacon_error.lock().await.clone(), adb_ok: *self.adb_ok.lock().await,
                          engine: self.engine_view().await, platform: platform_name(),
                          app_version: crate::updater::CURRENT, bundled_node_vc: bundled_vc }
    }
}

pub fn platform_name() -> &'static str {
    if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" }
}
