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
    /// 0.3.2 autobench: GB/s of int8 weights this node processes (None = not measured)
    pub bench_gbps: Option<f32>,
    /// rpc round trip measured at connect (ms)
    pub rtt_ms: Option<f32>,
    /// effective speed used by the split: bench (or estimate) × weight
    pub speed: f32,
    /// `speed` comes from a real measurement (false = estimate)
    pub speed_measured: bool,
    /// role chosen by the user: "auto" | "model" | "free" | "helper"
    pub role: String,
    /// what the current plan does with it: "model" (layers) | "free" | "helper" | "off" (disabled/offline)
    pub assigned: String,
    /// «Помощник» status (experiment «Роли»)
    pub helper: Option<crate::helper::HelperView>,
    /// battery/SoC temperature °C (beacon "temp_c" / USB dumpsys `argus-status {json}`)
    pub temp_c: Option<f32>,
    /// the node reports thermal throttling
    pub throttled: Option<bool>,
}

impl Node {
    pub fn device(serial: &str, name: &str) -> Self {
        Self { id: serial.into(), name: name.into(), kind: NodeKind::Phone, stage: NodeStage::Installing,
               transport: Transport::Usb, endpoint: None, total_mb: 0, available_mb: 0, usable_mb: 0,
               load_pct: 0.0, error: None, weight: 1.0, enabled: true, device: None, version: None,
               rpc_devices: 1, integrated: false, attempts: 0, retry_in: None, variant: None, beacon: false,
               in_engine: false, app_vc: None, app_version: None, update_available: None, update_skipped: false,
               bench_gbps: None, rtt_ms: None, speed: 0.0, speed_measured: false, role: "auto".into(), assigned: String::new(), helper: None,
               temp_c: None, throttled: None }
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
    /// refresh_metrics calls since the last `argus-status` dumpsys read
    pub status_tick: u32,
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
    /// 0.3.2: the first tiny generation after load is running («Прогрев…»)
    pub warming: bool,
}

impl Default for EngineState {
    fn default() -> Self {
        Self { status: EngineStatus::Idle, reason: None, args: None, nodes: vec![], started: None, ever_healthy: false,
               pending_restart: false, restarts: vec![], adopted_pid: None, cluster_sig: String::new(),
               cluster_stable_since: Instant::now(), last_err: None, new_nodes: false, last_health_check: None, restarting: false, warming: false }
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
    /// 0.3.2 «Прогрев…»
    pub warming: bool,
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
    /// 0.3.2: model size (weights + context) the split was planned for (running / last model)
    pub need_mb: Option<u64>,
    /// online nodes the model doesn't need («Свободна»)
    pub free_nodes: Vec<String>,
    /// rough tok/s estimate for the planned split
    pub est_tps: Option<f32>,
    pub fits: bool,
    /// experiment «Роли»
    pub roles_enabled: bool,
    pub helper_model: String,
    pub helper_models: Vec<crate::helper::HelperModelView>,
    /// nodes with the «Помощник» role (any state)
    pub helpers: Vec<crate::helper::HelperView>,
    /// PC CPU pseudo-node «Процессор ПК»
    pub cpu: CpuInfo,
    /// full split plan (gpuOnly, cpuLayers, threads, draftMb, moe …)
    pub plan: split::Plan,
    pub draft_enabled: bool,
    /// GGUF header info of local models seen so far (path -> info), for launch plans
    #[serde(skip)]
    pub gguf: HashMap<String, split::ModelInfo>,
    /// draft model to use: (path, MB reserved on the GPU)
    #[serde(skip)]
    pub draft: Option<(String, u64)>,
}

/// PC CPU as a model device (bench result + RAM it may lend to the model).
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    pub brand: String,
    /// threads for llama.cpp -t (physical cores − 1)
    pub threads: u32,
    /// int8 GB/s from the 1 s bench (None = not measured yet)
    pub gbps: Option<f32>,
    /// host RAM the model may use (50 % of available)
    pub usable_mb: u64,
    pub benching: bool,
}

impl CpuInfo {
    pub fn dev(&self) -> Option<split::CpuDev> {
        self.gbps.filter(|g| *g > 0.0).map(|g| split::CpuDev { gbps: g, usable_mb: self.usable_mb, threads: self.threads })
    }
}

/// llama.cpp -t default: physical cores − 1 (min 1).
pub fn cpu_threads() -> u32 {
    let sys = sysinfo::System::new();
    let phys = sys.physical_core_count()
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(2);
    (phys.saturating_sub(1)).max(1) as u32
}

/// ~1 s int8 multiply-accumulate over a 64 MB buffer with `threads` threads → GB/s of int8 weights.
pub fn cpu_bench(threads: u32, secs: f32) -> f32 {
    const BUF: usize = 64 << 20;
    let w: Vec<i8> = (0..BUF).map(|i| ((i * 31 + 7) % 251) as i8).collect();
    let x: Vec<i8> = (0..4096).map(|i| ((i * 17 + 3) % 127) as i8 - 63).collect();
    let t = threads.max(1) as usize;
    let chunk = BUF / t;
    let start = Instant::now();
    let total: u64 = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..t).map(|k| {
            let (w, x) = (&w[k * chunk..(k + 1) * chunk], &x);
            sc.spawn(move || {
                let mut bytes = 0u64;
                let mut acc = 0i32;
                let t0 = Instant::now();
                while t0.elapsed().as_secs_f32() < secs {
                    for row in w.chunks_exact(4096) {
                        let mut s = 0i32;
                        for (a, b) in row.iter().zip(x.iter()) { s += *a as i32 * *b as i32; }
                        acc = acc.wrapping_add(s);
                    }
                    bytes += w.len() as u64;
                }
                std::hint::black_box(acc);
                bytes
            })
        }).collect();
        hs.into_iter().map(|h| h.join().unwrap_or(0)).sum()
    });
    let el = start.elapsed().as_secs_f64().max(1e-3);
    (total as f64 / el / 1e9) as f32
}

/// settings.bench key of the PC CPU
pub fn cpu_bench_key(brand: &str, threads: u32) -> String { format!("cpu|{}|{threads}", brand.trim()) }

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
    /// 0.3.2 «Помощник» runtime per node id
    pub helpers: Mutex<HashMap<String, crate::helper::HelperRt>>,
    /// helper model downloads on the PC: file name -> progress
    pub helper_dl: Mutex<HashMap<String, crate::helper::Download>>,
    /// devices whose autobench is running (serial)
    pub benching: Mutex<HashSet<String>>,
    /// PC CPU pseudo-node
    pub cpu: Mutex<CpuInfo>,
    /// model header info cache: path -> (len, info)
    pub gguf_cache: Mutex<HashMap<String, (u64, split::ModelInfo)>>,
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
               adb_ok: Mutex::new(true), shot_scale: Mutex::new((1.0, 0, 0)), update: Default::default(),
               helpers: Default::default(), helper_dl: Default::default(), benching: Default::default(),
               cpu: Default::default(), gguf_cache: Default::default() }
    }

    /// GGUF header info of a local model file (cached by path + size).
    pub async fn model_info(&self, path: &str) -> Option<split::ModelInfo> {
        let len = std::fs::metadata(path).ok()?.len();
        if let Some((l, m)) = self.gguf_cache.lock().await.get(path) { if *l == len { return Some(m.clone()); } }
        let p = path.to_string();
        let info = tokio::task::spawn_blocking(move || crate::llama::gguf_info(std::path::Path::new(&p))).await.ok()??;
        self.gguf_cache.lock().await.insert(path.to_string(), (len, info.clone()));
        Some(info)
    }

    /// Threads a «Помощник» may use on a node that also holds model layers (half of its bench threads); None = all.
    pub async fn helper_threads(&self, id: &str) -> Option<u32> {
        if !self.engine_state.lock().await.nodes.iter().any(|n| n.id == id) { return None; }
        let s = self.settings.lock().await;
        let t = s.bench.iter().filter(|(k, _)| k.starts_with(&format!("{id}|"))).map(|(_, b)| b.threads).max().unwrap_or(4);
        Some((t / 2).max(1))
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
            pending_restart: e.pending_restart, new_nodes: e.new_nodes, can_restart: e.args.is_some(), warming: e.warming,
        }
    }

    pub async fn snapshot(&self) -> ClusterSnapshot {
        let settings = self.settings.lock().await.clone();
        // 0.3.2: speed comes from the autobench (or an estimate); the weight is only a manual multiplier (1.0)
        let apply = |n: &mut Node| {
            n.weight = settings.weights.get(&n.id).copied().unwrap_or(1.0);
            n.enabled = if n.integrated { settings.enabled.contains(&n.id) } else { !settings.disabled.contains(&n.id) };
            if n.transport == Transport::Usb {
                if let Some(b) = n.version.as_ref().and_then(|v| settings.bench.get(&bench_key(&n.id, v))) { n.bench_gbps = Some(b.gbps); }
            }
            n.speed_measured = n.bench_gbps.is_some();
            n.speed = split::node_speed(n);
            if n.transport != Transport::Local { n.role = settings.roles.get(&n.id).cloned().unwrap_or_else(|| "auto".into()); }
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
        {
            let disc = self.discovered.lock().await;
            for n in nodes.iter_mut().filter(|n| n.transport == Transport::Lan) {
                if let Some(d) = disc.get(&n.id) {
                    if d.temp_c.is_some() { n.temp_c = d.temp_c; }
                    if d.throttled.is_some() { n.throttled = d.throttled; }
                }
            }
        }
        for n in nodes.iter_mut() { n.speed = split::node_speed(n); }
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
        // plan for the running model (or the last one): roles, minimal node set, speed-first split
        let need_mb = {
            let e = self.engine_state.lock().await;
            e.args.as_ref().and_then(|a| a.need_mb).or_else(|| settings.last_launch.as_ref().and_then(|a| a.need_mb))
        };
        let roles_on = settings.roles_enabled;
        let cpu = self.cpu.lock().await.clone();
        let last = self.engine_state.lock().await.args.clone().or_else(|| settings.last_launch.clone());
        // header info only from the cache (filled by launch / plan_split): no disk walk every second
        let model = last.as_ref().and_then(|a| {
            let cache = self.gguf_cache.try_lock().ok()?;
            match (&a.model_path, a.hf_file.as_deref().and_then(|f| f.rsplit(['/', '\\']).next())) {
                (Some(p), _) => cache.get(p).map(|x| x.1.clone()),
                (None, Some(base)) => cache.iter().find(|(k, _)| k.ends_with(base)).map(|(_, v)| v.1.clone()),
                _ => None,
            }
        });
        let opts = split::PlanOpts { gpu_only: last.as_ref().and_then(|a| a.gpu_only).unwrap_or(false), cpu: cpu.dev(),
                                     draft_mb: crate::llama::draft_mb(&settings), model };
        let plan = split::plan_ex(&nodes, &gpus, need_mb, &|n: &Node| role_of(n, roles_on), &opts);
        let rpc_servers: Vec<String> = plan.rpc_ids.iter()
            .filter_map(|id| nodes.iter().find(|n| &n.id == id).and_then(|n| n.endpoint.clone())).collect();
        let helpers_rt = self.helpers.lock().await;
        for n in nodes.iter_mut() {
            let r = role_of(n, roles_on);
            n.assigned = if !n.online() || !n.enabled { "off" }
                else if r == split::Role::Helper { "helper" }
                else if plan.rpc_ids.contains(&n.id) { "model" }
                else { "free" }.into();
            n.helper = helpers_rt.get(&n.id).filter(|_| r == split::Role::Helper).map(|h| h.view(&n.id, &n.name));
        }
        drop(helpers_rt);
        let helpers: Vec<crate::helper::HelperView> = nodes.iter().filter_map(|n| n.helper.clone()).collect();
        let split = plan.entries.clone();
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
                          app_version: crate::updater::CURRENT, bundled_node_vc: bundled_vc,
                          need_mb, free_nodes: plan.free_ids.clone(), est_tps: plan.est_tps, fits: plan.fits,
                          roles_enabled: roles_on, helper_model: crate::helper::model_id(&settings.helper_model).to_string(),
                          helper_models: crate::helper::model_views(), helpers,
                          cpu, plan, draft_enabled: settings.draft_enabled,
                          gguf: self.gguf_cache.lock().await.iter().map(|(k, v)| (k.clone(), v.1.clone())).collect(),
                          draft: crate::llama::draft_file(&settings).map(|p| (p.display().to_string(), crate::llama::draft_mb(&settings))) }
    }
}

/// autobench cache key: device + node app version (a new app version measures again)
pub fn bench_key(id: &str, version: &str) -> String { format!("{id}|{version}") }

/// Role of a node; «Помощник» only counts while the experiment «Роли» is on (else it behaves like «Авто»).
pub fn role_of(n: &Node, roles_enabled: bool) -> split::Role {
    match split::Role::parse(Some(n.role.as_str())) {
        split::Role::Helper if !roles_enabled => split::Role::Auto,
        r => r,
    }
}

pub fn platform_name() -> &'static str {
    if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" }
}
