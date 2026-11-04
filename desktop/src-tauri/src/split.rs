//! Layer split planning: which llama.cpp device gets which share of the model (0.3.2).
//!
//! llama.cpp b12008 device naming/order (src/llama.cpp, ggml-rpc.cpp):
//!  * every `--rpc host:port` endpoint registers its devices as RPC0, RPC1, ... in --rpc order
//!    (a global counter; a plain rpc-server exposes exactly one device);
//!  * local GPUs are Vulkan0, Vulkan1, ... (CUDA0.. on a CUDA build), as `--list-devices` prints;
//!  * default order puts RPC devices first, then discrete GPUs.
//! We pass the same order explicitly with `--device RPC0,..,Vulkan0,..` so `-ts` lines up 1:1.
//! `--rpc` must come before `--device` on the command line (devices are resolved while parsing).
//!
//! 0.3.2: one token passes every device in turn (pipeline), so the time per token is the SUM of each
//! device's time for its layers plus the network round trips. That sum is smallest when the fastest device
//! is filled first (up to its memory), then the next fastest, and so on. So:
//!  1. speed of a device = autobench score (GB/s of int8 weights it can process) × the user's weight;
//!     GPUs and nodes without a bench get an estimate;
//!  2. node roles: «Свободна»/«Помощник» are never used, «Часть модели» always, «Авто» only when needed;
//!  3. minimal set: GPUs + forced nodes, then the fastest «Авто» nodes until model + context fit;
//!  4. -ts: fastest first up to each device's memory (a forced node keeps at least a small share);
//!  5. estimate of tok/s from the same numbers (rough: shown as «≈»).
//! Unknown model size (no `need_mb`): every «Авто»/«Часть модели» node is used, share ∝ speed, capped by memory.
use crate::state::{Node, NodeKind};
use serde::Serialize;

/// GB/s assumed for a discrete GPU when we have no measurement (GTX 960 class, Vulkan Q4 decode)
pub const GPU_GBPS_EST: f32 = 60.0;
pub const IGPU_GBPS_EST: f32 = 8.0;
/// phone/tablet/Pi without a bench result (old node app): a slow-ish arm64 CPU
pub const NODE_GBPS_EST: f32 = 4.0;
/// LAN machine without a beacon bench (x86 PC running rpc-server): a desktop CPU
pub const LAN_PC_GBPS_EST: f32 = 10.0;
/// round trips per token per rpc node (graph compute + tensor get/set), for the estimate
const RTT_PER_TOKEN: f32 = 4.0;
/// a forced «Часть модели» node that the greedy fill would leave empty still gets this much of the model
const FORCED_MIN_SHARE: f64 = 0.04;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitEntry {
    pub id: String,        // node id ("cpu" for the PC CPU pseudo-node)
    pub device: String,    // llama.cpp device name ("CPU" for the pseudo-node: not in --device)
    pub name: String,
    pub share: f32,        // 0..1
    /// the PC CPU pseudo-node «Процессор ПК» (layers kept on the host via -ngl < n_layer+1)
    #[serde(default)]
    pub cpu: bool,
    /// thermal speed factor applied (1.0 normal, 0.8 hot, 0.6 throttled)
    pub thermal: f32,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoeOffload {
    pub id: String,
    pub name: String,
    /// -ot buffer: "RPC0[127.0.0.1:50052]" or "CPU"
    pub target: String,
    pub blocks: Vec<u32>,
    pub mb: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoePlan {
    pub expert_count: u32,
    pub expert_used: u32,
    pub block_count: u32,
    pub experts_mb: u64,
    /// attention + embeddings + KV (+ draft): always on the GPU
    pub dense_mb: u64,
    /// blocks whose experts stay on the GPU
    pub gpu_blocks: Vec<u32>,
    pub offload: Vec<MoeOffload>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// devices in --device order (RPC first in --rpc order, then local GPUs); only devices that get layers.
    /// The CPU pseudo-node (cpu=true) is listed last and is NOT passed to --device / -ts.
    pub entries: Vec<SplitEntry>,
    /// node ids passed in --rpc, in order
    pub rpc_ids: Vec<String>,
    /// online «Авто» nodes left out because the model fits without them («Свободна»)
    pub free_ids: Vec<String>,
    /// rough tokens/s estimate (None when the model size is unknown)
    pub est_tps: Option<f32>,
    /// model + context fit into the selected devices
    pub fits: bool,
    pub need_mb: Option<u64>,
    /// only local GPUs were allowed
    pub gpu_only: bool,
    /// layers kept on the PC CPU (first layers; -ngl = n_layers + 1 - cpu_layers)
    pub cpu_layers: u32,
    pub n_layers: Option<u32>,
    /// -t for the PC CPU when it holds layers / experts
    pub threads: Option<u32>,
    /// GPU memory reserved for the speculative draft model
    pub draft_mb: u64,
    /// draft model will be used (set by the launcher)
    pub draft: bool,
    pub moe: Option<MoePlan>,
    /// --rpc endpoints in order (filled by the launcher; used for -ot buffer names)
    #[serde(skip)]
    pub rpc_endpoints: Vec<String>,
}

/// PC CPU as a pseudo-node «Процессор ПК».
#[derive(Debug, Clone, Default)]
pub struct CpuDev { pub gbps: f32, pub usable_mb: u64, pub threads: u32 }

/// What the GGUF header says (llama.rs `gguf_info`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelInfo {
    pub block_count: u32,
    pub expert_count: u32,
    pub expert_used: u32,
    /// MB of `ffn_*_exps` tensors (0 = dense model / unknown)
    pub experts_mb: u64,
    pub weights_mb: u64,
    /// general.architecture ("qwen3", "qwen3moe", "llama", "gemma2" …) — for the draft-model check
    pub arch: String,
    /// tokenizer.ggml.tokens length (0 = unknown) — a draft model must share the vocabulary
    pub n_vocab: u32,
}

#[derive(Debug, Clone, Default)]
pub struct PlanOpts {
    pub gpu_only: bool,
    pub cpu: Option<CpuDev>,
    pub draft_mb: u64,
    pub model: Option<ModelInfo>,
}

pub const CPU_NAME: &str = "Процессор ПК";

/// Node role chosen by the user (settings.roles); missing = Auto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role { Auto, Model, Free, Helper }

impl Role {
    pub fn parse(s: Option<&str>) -> Role {
        match s { Some("model") => Role::Model, Some("free") => Role::Free, Some("helper") => Role::Helper, _ => Role::Auto }
    }
    pub fn as_str(self) -> &'static str {
        match self { Role::Auto => "auto", Role::Model => "model", Role::Free => "free", Role::Helper => "helper" }
    }
}

/// Thermal slow-down: a throttling node ×0.6, a hot one (≥ 43 °C) ×0.8.
pub fn thermal_factor(n: &Node) -> f32 {
    if n.throttled == Some(true) { 0.6 } else if n.temp_c.map(|t| t >= 43.0).unwrap_or(false) { 0.8 } else { 1.0 }
}

/// Effective speed (GB/s × user weight × thermal) used for ordering, -ts and the estimate.
pub fn node_speed(n: &Node) -> f32 {
    let base = n.bench_gbps.filter(|g| *g > 0.0).unwrap_or_else(|| default_gbps(n));
    base * n.weight.max(0.05) * thermal_factor(n)
}

pub fn default_gbps(n: &Node) -> f32 {
    match n.kind {
        NodeKind::Gpu if n.integrated => IGPU_GBPS_EST,
        NodeKind::Gpu => GPU_GBPS_EST,
        NodeKind::Lan if n.variant.as_deref().map(|v| v.contains("x64")).unwrap_or(false) => LAN_PC_GBPS_EST,
        _ => NODE_GBPS_EST,
    }
}

#[derive(Clone)]
struct Dev { id: String, name: String, cap: f64, speed: f64, rtt_ms: f64, forced: bool, gpu: bool, cpu: bool, j: u32,
             device: Option<String>, thermal: f32 }

fn by_speed_desc(devs: &[Dev]) -> impl Fn(&usize, &usize) -> std::cmp::Ordering + '_ {
    move |&a, &b| devs[b].speed.partial_cmp(&devs[a].speed).unwrap_or(std::cmp::Ordering::Equal)
}

/// `role_of(node)` gives the node's role (already resolved against the «Роли» experiment switch).
#[allow(dead_code)]
pub fn plan(nodes: &[Node], gpus: &[Node], need_mb: Option<u64>, role_of: &dyn Fn(&Node) -> Role) -> Plan {
    plan_ex(nodes, gpus, need_mb, role_of, &PlanOpts::default())
}

pub fn plan_ex(nodes: &[Node], gpus: &[Node], need_mb: Option<u64>, role_of: &dyn Fn(&Node) -> Role, opts: &PlanOpts) -> Plan {
    let mut p = plan_inner(nodes, gpus, need_mb, role_of, opts);
    p.gpu_only = opts.gpu_only;
    p.draft_mb = opts.draft_mb;
    p.n_layers = opts.model.as_ref().map(|m| m.block_count).filter(|b| *b > 0);
    let cpu_used = p.entries.iter().any(|e| e.cpu && e.share > 0.0)
        || p.moe.as_ref().map(|m| m.offload.iter().any(|o| o.target == "CPU")).unwrap_or(false);
    if cpu_used { p.threads = opts.cpu.as_ref().map(|c| c.threads.max(1)); }
    p
}

fn plan_inner(nodes: &[Node], gpus: &[Node], need_mb: Option<u64>, role_of: &dyn Fn(&Node) -> Role, opts: &PlanOpts) -> Plan {
    // candidates: online + enabled rpc nodes that may hold layers, one Dev per rpc device
    let mut cand: Vec<Dev> = vec![];
    if !opts.gpu_only {
        for n in nodes.iter().filter(|n| n.online() && n.enabled && n.endpoint.is_some()) {
            let role = role_of(n);
            if matches!(role, Role::Free | Role::Helper) { continue; }
            let k = n.rpc_devices.max(1);
            for j in 0..k {
                cand.push(Dev { id: n.id.clone(), name: if k > 1 { format!("{} #{j}", n.name) } else { n.name.clone() },
                    cap: n.usable_mb as f64 / k as f64, speed: node_speed(n) as f64, rtt_ms: n.rtt_ms.unwrap_or(2.0) as f64,
                    forced: role == Role::Model, gpu: false, cpu: false, j, device: None, thermal: thermal_factor(n) });
            }
        }
    }
    let mut gdevs: Vec<Dev> = gpus.iter().filter(|g| g.enabled && g.device.is_some()).map(|g| Dev {
        id: g.id.clone(), name: g.name.clone(), cap: g.usable_mb as f64, speed: node_speed(g) as f64, rtt_ms: 0.0,
        forced: true, gpu: true, cpu: false, j: 0, device: g.device.clone(), thermal: thermal_factor(g) }).collect();
    // the draft model lives on the biggest GPU
    if opts.draft_mb > 0 {
        if let Some(g) = gdevs.iter_mut().max_by(|a, b| a.cap.partial_cmp(&b.cap).unwrap_or(std::cmp::Ordering::Equal)) {
            g.cap = (g.cap - opts.draft_mb as f64).max(0.0);
        }
    }
    let need = need_mb.map(|n| n as f64).filter(|n| *n > 0.0);
    let model = opts.model.clone().unwrap_or_default();
    // CPU pseudo-node: only next to a GPU (--device list known), when the model size and its layer count are known
    if let (Some(c), false, true, Some(_)) = (&opts.cpu, opts.gpu_only, !gdevs.is_empty(), need) {
        if c.gbps > 0.0 && c.usable_mb > 0 && (model.block_count > 0) {
            cand.push(Dev { id: "cpu".into(), name: CPU_NAME.into(), cap: c.usable_mb as f64, speed: c.gbps as f64, rtt_ms: 0.0,
                forced: false, gpu: false, cpu: true, j: 0, device: Some("CPU".into()), thermal: 1.0 });
        }
    }
    cand.retain(|d| d.cap > 0.0 && d.speed > 0.0);
    let gpu_cap: f64 = gdevs.iter().filter(|d| d.cap > 0.0).map(|d| d.cap).sum();

    // ---- MoE: attention + KV on the GPU, expert tensors of whole blocks elsewhere
    if let Some(need) = need {
        if model.expert_count > 1 && model.block_count > 0 && model.experts_mb > 0 && gpu_cap > 0.0 && gpu_cap < need && !cand.is_empty() {
            return plan_moe(cand, gdevs, need, &model);
        }
    }

    // ---- 1. selection
    let mut chosen = vec![false; cand.len()];
    match need {
        Some(need) => {
            let mut have = gpu_cap;
            for (i, d) in cand.iter().enumerate() { if d.forced { chosen[i] = true; have += d.cap; } }
            // a multi-device rpc-server is all or nothing: rank whole nodes by speed (then by memory)
            let mut order: Vec<usize> = (0..cand.len()).filter(|&i| !chosen[i] && cand[i].j == 0).collect();
            order.sort_by(|&a, &b| cand[b].speed.partial_cmp(&cand[a].speed).unwrap_or(std::cmp::Ordering::Equal)
                .then(cand[b].cap.partial_cmp(&cand[a].cap).unwrap_or(std::cmp::Ordering::Equal)));
            for i in order {
                if have >= need { break; }
                for (x, d) in cand.iter().enumerate() {
                    if d.id == cand[i].id { chosen[x] = true; have += d.cap; }
                }
            }
        }
        None => chosen.iter_mut().zip(&cand).for_each(|(c, d)| *c = !d.cpu),
    }
    let free_ids: Vec<String> = {
        let mut v: Vec<String> = vec![];
        for (i, d) in cand.iter().enumerate() { if !chosen[i] && !d.cpu && d.j == 0 && !v.contains(&d.id) { v.push(d.id.clone()); } }
        v
    };

    // devices in --device order: chosen rpc devices (original --rpc order), then GPUs, the CPU pseudo-node last
    let mut devs: Vec<Dev> = cand.into_iter().zip(chosen).filter(|(_, c)| *c).map(|(d, _)| d).collect();
    let cpu_dev: Option<Dev> = devs.iter().position(|d| d.cpu).map(|i| devs.remove(i));
    devs.extend(gdevs.into_iter().filter(|d| d.cap > 0.0 && d.speed > 0.0));
    devs.extend(cpu_dev);
    if devs.is_empty() { return Plan { free_ids, need_mb, fits: need.is_none(), ..Default::default() }; }
    let total_cap: f64 = devs.iter().map(|d| d.cap).sum();

    // ---- 2. amounts (MB per device)
    let mut amt = vec![0.0f64; devs.len()];
    let fits = need.map(|n| n <= total_cap).unwrap_or(true);
    match need {
        Some(need) if fits => {
            // fastest first, each up to its memory
            let mut order: Vec<usize> = (0..devs.len()).collect();
            order.sort_by(by_speed_desc(&devs));
            let mut left = need;
            for &i in &order {
                if left <= 1e-6 { break; }
                let take = devs[i].cap.min(left);
                amt[i] = take; left -= take;
            }
            // forced nodes (and every device of a chosen multi-device server) must not sit empty in --rpc
            for i in 0..devs.len() {
                if amt[i] > 0.0 || devs[i].gpu || devs[i].cpu { continue; }
                let want = (need * FORCED_MIN_SHARE).min(devs[i].cap);
                // take it from the slowest device that has layers
                let donor = order.iter().rev().copied().find(|&x| x != i && amt[x] > want);
                if let Some(x) = donor { amt[x] -= want; amt[i] = want; }
            }
        }
        // won't fit anyway: everything, by memory (llama.cpp will fail clearly or swap)
        Some(_) => { for i in 0..devs.len() { amt[i] = devs[i].cap; } }
        // unknown size: ∝ speed, capped by memory (water-filling)
        None => {
            let mut open: Vec<usize> = (0..devs.len()).collect();
            // nominal size: 60% of the pool, so fast devices fill up first and nobody exceeds its memory
            let mut left = total_cap * 0.6;
            while left > 1e-6 && !open.is_empty() {
                let s: f64 = open.iter().map(|&i| devs[i].speed).sum();
                let mut next = vec![];
                let mut clamped = false;
                for &i in &open {
                    let want = left * devs[i].speed / s;
                    if amt[i] + want >= devs[i].cap { left -= devs[i].cap - amt[i]; amt[i] = devs[i].cap; clamped = true; }
                    else { next.push(i); }
                }
                if !clamped { for &i in &next { amt[i] += left * devs[i].speed / s; } left = 0.0; }
                open = next;
            }
        }
    }
    let sum: f64 = amt.iter().sum::<f64>().max(1e-9);

    // ---- 3. drop rpc devices without layers (they would only hold an rpc slot), name RPC devices
    // A multi-device server stays if any of its devices has layers.
    let keep_ids: Vec<String> = devs.iter().zip(&amt).filter(|(_, a)| **a > 0.0).map(|(d, _)| d.id.clone()).collect();
    let mut entries = vec![];
    let mut rpc_ids: Vec<String> = vec![];
    let mut rpc_i = 0u32;
    let mut extra_free: Vec<String> = vec![];
    let mut t_ms = 0.0f64;
    let mut cpu_layers = 0u32;
    for (d, a) in devs.iter().zip(&amt) {
        if d.cpu {
            if *a <= 0.0 { continue; }
            let nl = model.block_count.max(1);
            cpu_layers = (((*a / sum) * nl as f64).round() as u32).clamp(1, nl);
        } else if !d.gpu && !keep_ids.contains(&d.id) {
            if d.j == 0 && !extra_free.contains(&d.id) { extra_free.push(d.id.clone()); }
            continue;
        }
        let device = match &d.device { Some(x) => x.clone(), None => { let x = format!("RPC{rpc_i}"); rpc_i += 1; x } };
        if !d.gpu && !d.cpu && !rpc_ids.contains(&d.id) { rpc_ids.push(d.id.clone()); t_ms += RTT_PER_TOKEN as f64 * d.rtt_ms; }
        if *a > 0.0 { t_ms += a / 1024.0 / d.speed * 1000.0; }
        entries.push(SplitEntry { id: d.id.clone(), device, name: d.name.clone(), share: (*a / sum) as f32, cpu: d.cpu, thermal: d.thermal });
    }
    let mut free_ids = free_ids;
    free_ids.extend(extra_free);
    let est_tps = need.filter(|_| fits && t_ms > 0.0).map(|_| (1000.0 / t_ms) as f32);
    Plan { entries, rpc_ids, free_ids, est_tps, fits, need_mb, cpu_layers, ..Default::default() }
}

/// MoE: dense part (attention, embeddings, KV, draft) on the GPU, then as many blocks' experts as fit there;
/// the remaining blocks' experts go to rpc nodes / the PC CPU, fastest first, each up to its memory.
fn plan_moe(cand: Vec<Dev>, gdevs: Vec<Dev>, need: f64, m: &ModelInfo) -> Plan {
    let nb = m.block_count;
    let experts = (m.experts_mb as f64).min(need);
    let dense = (need - experts).max(0.0);
    let per_block = experts / nb as f64;
    let gdevs: Vec<Dev> = gdevs.into_iter().filter(|d| d.cap > 0.0 && d.speed > 0.0).collect();
    let gpu_cap: f64 = gdevs.iter().map(|d| d.cap).sum();
    let gpu_speed: f64 = gdevs.iter().map(|d| d.speed * d.cap).sum::<f64>() / gpu_cap.max(1e-9);
    let gpu_blocks_n = (((gpu_cap - dense).max(0.0) / per_block.max(1e-9)).floor() as u32).min(nb);
    let mut left = nb - gpu_blocks_n;
    // offload targets: forced nodes always considered first, then by speed
    let mut order: Vec<usize> = (0..cand.len()).collect();
    order.sort_by(|&a, &b| cand[b].forced.cmp(&cand[a].forced)
        .then(cand[b].speed.partial_cmp(&cand[a].speed).unwrap_or(std::cmp::Ordering::Equal)));
    let mut take = vec![0u32; cand.len()];
    for &i in &order {
        if left == 0 { break; }
        let k = ((cand[i].cap / per_block.max(1e-9)).floor() as u32).min(left);
        take[i] = k; left -= k;
    }
    let fits = left == 0 && gpu_cap >= dense;
    // won't fit: dump the rest on the slowest... no: report and give the remainder to the CPU / last target
    if left > 0 {
        if let Some(&i) = order.iter().rev().find(|&&i| cand[i].cpu).or(order.last()) { take[i] += left; }
    }
    // assign block numbers: offloaded experts take the first blocks (in --rpc order), GPU keeps the last ones
    let mut next_block = 0u32;
    let mut entries = vec![];
    let mut rpc_ids: Vec<String> = vec![];
    let mut offload = vec![];
    let mut rpc_i = 0u32;
    let mut t_ms = 0.0f64;
    let act = (m.expert_used.max(1) as f64 / m.expert_count as f64).min(1.0);
    let mut cpu_entry: Option<SplitEntry> = None;
    let mut seen_ids: Vec<String> = vec![];
    for (i, d) in cand.iter().enumerate() {
        // rpc devices keep their RPC number even with zero blocks when another device of the same server is used
        let node_used = cand.iter().zip(&take).any(|(x, t)| x.id == d.id && *t > 0);
        if !node_used { continue; }
        let target = if d.cpu { "CPU".to_string() } else { let x = format!("RPC{rpc_i}"); rpc_i += 1; x };
        if !d.cpu && !seen_ids.contains(&d.id) { seen_ids.push(d.id.clone()); rpc_ids.push(d.id.clone()); t_ms += RTT_PER_TOKEN as f64 * d.rtt_ms; }
        if take[i] == 0 { continue; }
        let blocks: Vec<u32> = (next_block..next_block + take[i]).collect();
        next_block += take[i];
        let mb = take[i] as f64 * per_block;
        t_ms += mb * act / 1024.0 / d.speed * 1000.0;
        let e = SplitEntry { id: d.id.clone(), device: target.clone(), name: d.name.clone(), share: (mb / need) as f32, cpu: d.cpu, thermal: d.thermal };
        if d.cpu { cpu_entry = Some(e); } else { entries.push(e); }
        offload.push(MoeOffload { id: d.id.clone(), name: d.name.clone(), target, blocks, mb: mb.round() as u64 });
    }
    let gpu_blocks: Vec<u32> = (next_block..nb).collect();
    let gpu_mb = dense + gpu_blocks.len() as f64 * per_block;
    t_ms += (dense + gpu_blocks.len() as f64 * per_block * act) / 1024.0 / gpu_speed.max(1e-9) * 1000.0;
    for g in &gdevs {
        entries.push(SplitEntry { id: g.id.clone(), device: g.device.clone().unwrap_or_default(), name: g.name.clone(),
            share: (gpu_mb * g.cap / gpu_cap / need) as f32, cpu: false, thermal: g.thermal });
    }
    entries.extend(cpu_entry);
    let free_ids: Vec<String> = {
        let mut v: Vec<String> = vec![];
        for d in &cand { if !d.cpu && d.j == 0 && !rpc_ids.contains(&d.id) && !v.contains(&d.id) { v.push(d.id.clone()); } }
        v
    };
    let est_tps = (fits && t_ms > 0.0).then(|| (1000.0 / t_ms) as f32);
    Plan { entries, rpc_ids, free_ids, est_tps, fits, need_mb: Some(need as u64),
           moe: Some(MoePlan { expert_count: m.expert_count, expert_used: m.expert_used, block_count: nb,
                               experts_mb: experts.round() as u64, dense_mb: dense.round() as u64, gpu_blocks, offload }),
           ..Default::default() }
}

/// `-ts` value: integer per-mille proportions (llama.cpp normalises them). The CPU pseudo-node is not a device.
pub fn ts_arg(plan: &[SplitEntry]) -> String {
    let sum: f32 = plan.iter().filter(|e| !e.cpu).map(|e| e.share).sum::<f32>().max(1e-9);
    plan.iter().filter(|e| !e.cpu).map(|e| format!("{}", (e.share / sum * 1000.0).round().max(0.0) as u32)).collect::<Vec<_>>().join(",")
}

/// `-ts` for a plan: MoE plans put every layer on the GPU(s) (rpc devices get only expert tensors via -ot).
pub fn ts_for_plan(p: &Plan) -> String {
    if p.moe.is_some() {
        let gpu_sum: f32 = p.entries.iter().filter(|e| !e.cpu && !e.device.starts_with("RPC")).map(|e| e.share).sum::<f32>().max(1e-9);
        return p.entries.iter().filter(|e| !e.cpu).map(|e| if e.device.starts_with("RPC") { "0".to_string() }
            else { format!("{}", (e.share / gpu_sum * 1000.0).round() as u32) }).collect::<Vec<_>>().join(",");
    }
    ts_arg(&p.entries)
}

/// Devices for --device (CPU pseudo-node excluded).
pub fn device_list(p: &Plan) -> Vec<String> {
    p.entries.iter().filter(|e| !e.cpu).map(|e| e.device.clone()).collect()
}

/// `-ot` value for a MoE plan: "blk\.(0|1)\.ffn_(up|down|gate|gate_up)_(ch|)exps=RPC0[ep],...=CPU".
/// `endpoints` = --rpc endpoints in order (RPCn buffer names carry the endpoint).
pub fn ot_arg(p: &Plan, endpoints: &[String]) -> Option<String> {
    let m = p.moe.as_ref()?;
    let parts: Vec<String> = m.offload.iter().filter(|o| !o.blocks.is_empty()).map(|o| {
        let blocks = o.blocks.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("|");
        let target = match o.target.strip_prefix("RPC").and_then(|n| n.parse::<usize>().ok()) {
            // rpc devices are numbered per server device; a 1-device server maps RPCn → n-th endpoint
            Some(n) => match p.rpc_ids.iter().position(|id| id == &o.id).and_then(|i| endpoints.get(i)) {
                Some(ep) => format!("RPC{n}[{ep}]"),
                None => o.target.clone(),
            },
            None => "CPU".to_string(),
        };
        format!("blk\\.({blocks})\\.ffn_(up|down|gate|gate_up)_(ch|)exps={target}")
    }).collect();
    (!parts.is_empty()).then(|| parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Node, NodeKind, NodeStage};
    fn n(id: &str, mb: u64, gbps: f32) -> Node {
        let mut x = Node::device(id, id); x.stage = NodeStage::Connected; x.endpoint = Some(id.into()); x.usable_mb = mb;
        x.bench_gbps = Some(gbps); x
    }
    fn g(id: &str, mb: u64) -> Node { let mut x = Node::local(id, id, NodeKind::Gpu); x.device = Some(id.into()); x.usable_mb = mb; x }
    fn auto(_: &Node) -> Role { Role::Auto }

    #[test]
    fn unknown_size_uses_all() {
        let p = plan(&[n("a", 4000, 4.0), n("b", 4000, 4.0)], &[g("Vulkan0", 3500)], None, &auto);
        assert_eq!(p.entries.iter().map(|e| e.device.as_str()).collect::<Vec<_>>(), ["RPC0", "RPC1", "Vulkan0"]);
        assert!((p.entries.iter().map(|e| e.share).sum::<f32>() - 1.0).abs() < 1e-3);
        assert!(p.entries[2].share > p.entries[0].share);
        assert_eq!(ts_arg(&p.entries).split(',').count(), 3);
        assert!(p.free_ids.is_empty());
    }

    /// The user's case: GTX 960 + Tab S7+ (fast) + Note10+ (more free RAM, slower) + A52 (slow).
    /// Qwen3 4B Q4 @ 8K ≈ 4.9 GB: GPU + tablet are enough, Note and A52 stay «Свободна».
    #[test]
    fn minimal_set_fastest_first() {
        let nodes = [n("tab", 2800, 18.0), n("note", 5600, 9.0), n("a52", 3000, 6.0)];
        let p = plan(&nodes, &[g("Vulkan0", 3584)], Some(4900), &auto);
        assert_eq!(p.rpc_ids, vec!["tab"]);
        assert_eq!(p.free_ids.len(), 2);
        assert_eq!(p.entries.iter().map(|e| e.device.as_str()).collect::<Vec<_>>(), ["RPC0", "Vulkan0"]);
        let gpu_mb = p.entries[1].share as f64 * 4900.0;
        assert!((gpu_mb - 3584.0).abs() < 2.0, "GPU filled first: {gpu_mb}");
        assert!(p.fits);
        assert!(p.est_tps.unwrap() > 1.0);
    }

    #[test]
    fn roles_respected() {
        let nodes = [n("tab", 2800, 18.0), n("note", 5600, 9.0), n("a52", 3000, 6.0)];
        let role = |x: &Node| match x.id.as_str() { "tab" => Role::Helper, "a52" => Role::Model, _ => Role::Auto };
        // 7000 MB: GPU 3584 + forced a52 3000 is not enough, so the «Авто» note is needed too
        let p = plan(&nodes, &[g("Vulkan0", 3584)], Some(7000), &role);
        // tablet is a helper: never in --rpc; a52 forced; note needed for the rest
        assert!(!p.rpc_ids.contains(&"tab".to_string()));
        assert!(p.rpc_ids.contains(&"a52".to_string()));
        assert!(p.rpc_ids.contains(&"note".to_string()));
        // RPC numbering follows the --rpc order
        assert_eq!(p.entries[0].device, "RPC0");
        assert_eq!(p.entries[1].device, "RPC1");
        // a forced node gets layers even though faster devices could hold them
        assert!(p.entries.iter().all(|e| e.share > 0.0));
    }

    #[test]
    fn small_model_gpu_only() {
        let p = plan(&[n("tab", 2800, 18.0)], &[g("Vulkan0", 3584)], Some(1200), &auto);
        assert!(p.rpc_ids.is_empty());
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.free_ids, vec!["tab"]);
    }

    #[test]
    fn too_big_uses_everything() {
        let p = plan(&[n("tab", 2800, 18.0), n("note", 5600, 9.0)], &[g("Vulkan0", 3584)], Some(30000), &auto);
        assert!(!p.fits);
        assert_eq!(p.rpc_ids.len(), 2);
        assert!(p.est_tps.is_none());
    }

    fn opts_cpu(gbps: f32, mb: u64, layers: u32) -> PlanOpts {
        PlanOpts { cpu: Some(CpuDev { gbps, usable_mb: mb, threads: 7 }),
                   model: Some(ModelInfo { block_count: layers, ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn cpu_used_when_faster_than_slow_phone() {
        // GPU 3584 + need 6000: the slow a52 (6 GB/s) is beaten by a 12 GB/s PC CPU
        let nodes = [n("a52", 4000, 6.0)];
        let p = plan_ex(&nodes, &[g("Vulkan0", 3584)], Some(6000), &auto, &opts_cpu(12.0, 8000, 36));
        assert!(p.rpc_ids.is_empty(), "{:?}", p.rpc_ids);
        assert_eq!(p.free_ids, vec!["a52"]);
        let cpu = p.entries.iter().find(|e| e.cpu).expect("cpu entry");
        assert_eq!(cpu.name, CPU_NAME);
        assert!(p.cpu_layers > 0 && p.cpu_layers < 36);
        assert_eq!(p.threads, Some(7));
        assert_eq!(device_list(&p), vec!["Vulkan0"]);
        assert_eq!(ts_arg(&p.entries), "1000");
        // a fast phone beats the CPU
        let p = plan_ex(&[n("tab", 4000, 18.0)], &[g("Vulkan0", 3584)], Some(6000), &auto, &opts_cpu(12.0, 8000, 36));
        assert_eq!(p.rpc_ids, vec!["tab"]);
        assert_eq!(p.cpu_layers, 0);
        assert_eq!(p.threads, None);
    }

    #[test]
    fn gpu_only_and_draft() {
        let o = PlanOpts { gpu_only: true, draft_mb: 600, ..Default::default() };
        let p = plan_ex(&[n("tab", 4000, 18.0)], &[g("Vulkan0", 3584)], Some(3200), &auto, &o);
        assert!(p.rpc_ids.is_empty());
        assert!(!p.fits, "3584 - 600 draft < 3200");
        assert!(p.gpu_only);
        assert_eq!(p.draft_mb, 600);
    }

    #[test]
    fn thermal_slows_node() {
        let mut hot = n("tab", 2800, 18.0); hot.throttled = Some(true);
        assert!((node_speed(&hot) - 18.0 * 0.6).abs() < 1e-3);
        let mut warm = n("tab", 2800, 18.0); warm.temp_c = Some(44.0);
        assert!((node_speed(&warm) - 18.0 * 0.8).abs() < 1e-3);
        // a throttled tablet (10.8) loses to the note (12)
        let nodes = [hot, n("note", 5600, 12.0)];
        let p = plan(&nodes, &[g("Vulkan0", 3584)], Some(4900), &auto);
        assert_eq!(p.rpc_ids, vec!["note"]);
    }

    #[test]
    fn moe_experts_offloaded() {
        // 30B-A3B-like: 48 blocks, experts 15000 MB of 17000, GPU 3584
        let o = PlanOpts { cpu: Some(CpuDev { gbps: 10.0, usable_mb: 9000, threads: 5 }),
            model: Some(ModelInfo { block_count: 48, expert_count: 128, expert_used: 8, experts_mb: 15000, weights_mb: 16500, ..Default::default() }), ..Default::default() };
        let p = plan_ex(&[n("tab", 4000, 18.0), n("note", 5600, 9.0)], &[g("Vulkan0", 3584)], Some(17000), &auto, &o);
        let m = p.moe.as_ref().expect("moe plan");
        assert_eq!(m.dense_mb, 2000);
        let total: usize = m.offload.iter().map(|x| x.blocks.len()).sum::<usize>() + m.gpu_blocks.len();
        assert_eq!(total, 48);
        assert!(p.fits);
        let ot = ot_arg(&p, &["127.0.0.1:50052".into(), "127.0.0.1:50053".into()]).unwrap();
        assert!(ot.starts_with("blk\\.(0|1|"), "{ot}");
        assert!(ot.contains("=RPC0[127.0.0.1:50052]"), "{ot}");
        assert!(ts_for_plan(&p).split(',').take(p.rpc_ids.len()).all(|x| x == "0"));
        assert!(p.est_tps.unwrap() > 1.0);
    }
}
