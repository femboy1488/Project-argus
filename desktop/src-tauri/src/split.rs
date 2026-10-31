//! Layer split planning: which llama.cpp device gets which share of the model.
//!
//! llama.cpp b12008 device naming/order (src/llama.cpp, ggml-rpc.cpp):
//!  * every `--rpc host:port` endpoint registers its devices as RPC0, RPC1, ... in --rpc order
//!    (a global counter; a plain rpc-server exposes exactly one device);
//!  * local GPUs are Vulkan0, Vulkan1, ... (CUDA0.. on a CUDA build), as `--list-devices` prints;
//!  * default order puts RPC devices first, then discrete GPUs.
//! We pass the same order explicitly with `--device RPC0,..,Vulkan0,..` so `-ts` lines up 1:1.
//! `--rpc` must come before `--device` on the command line (devices are resolved while parsing).
use crate::state::Node;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitEntry {
    pub id: String,        // node id
    pub device: String,    // llama.cpp device name
    pub name: String,
    pub share: f32,        // 0..1
}

/// Devices in --device order: RPC devices of enabled online nodes (in --rpc order), then enabled local GPUs.
/// share ∝ usable memory × speed weight, then water-filled so no device is asked for more than it can hold
/// (when the model size is known).
pub fn plan(nodes: &[Node], gpus: &[Node], need_mb: Option<u64>) -> Vec<SplitEntry> {
    struct Dev { id: String, device: String, name: String, cap: f64, w: f64 }
    let mut devs: Vec<Dev> = vec![];
    let mut rpc_i = 0u32;
    for n in nodes.iter().filter(|n| n.online() && n.enabled && n.endpoint.is_some()) {
        let k = n.rpc_devices.max(1);
        for j in 0..k {
            devs.push(Dev {
                id: n.id.clone(), device: format!("RPC{rpc_i}"),
                name: if k > 1 { format!("{} #{j}", n.name) } else { n.name.clone() },
                cap: n.usable_mb as f64 / k as f64, w: n.weight.max(0.0) as f64,
            });
            rpc_i += 1;
        }
    }
    for g in gpus.iter().filter(|g| g.enabled) {
        if let Some(d) = &g.device {
            devs.push(Dev { id: g.id.clone(), device: d.clone(), name: g.name.clone(), cap: g.usable_mb as f64, w: g.weight.max(0.0) as f64 });
        }
    }
    devs.retain(|d| d.cap > 0.0 && d.w > 0.0);
    if devs.is_empty() { return vec![]; }

    let score: Vec<f64> = devs.iter().map(|d| d.cap * d.w).collect();
    let total_cap: f64 = devs.iter().map(|d| d.cap).sum();
    let shares: Vec<f64> = match need_mb.map(|n| n as f64) {
        Some(need) if need > 0.0 && need <= total_cap => {
            // water-filling: distribute `need` by score, clamp at capacity, redistribute the rest
            let mut amt = vec![0.0f64; devs.len()];
            let mut open: Vec<usize> = (0..devs.len()).collect();
            let mut left = need;
            while left > 1e-6 && !open.is_empty() {
                let s: f64 = open.iter().map(|&i| score[i]).sum();
                let mut clamped = false;
                let mut next = vec![];
                for &i in &open {
                    let want = left * score[i] / s;
                    if amt[i] + want >= devs[i].cap { left -= devs[i].cap - amt[i]; amt[i] = devs[i].cap; clamped = true; }
                    else { next.push(i); }
                }
                if !clamped {
                    for &i in &next { amt[i] += left * score[i] / s; }
                    left = 0.0;
                }
                open = next;
            }
            amt.iter().map(|a| a / need).collect()
        }
        // unknown size, or it won't fit anyway: plain memory x weight
        _ => { let s: f64 = score.iter().sum(); score.iter().map(|x| x / s).collect() }
    };
    devs.into_iter().zip(shares).map(|(d, s)| SplitEntry { id: d.id, device: d.device, name: d.name, share: s as f32 }).collect()
}

/// `-ts` value: integer per-mille proportions (llama.cpp normalises them).
pub fn ts_arg(plan: &[SplitEntry]) -> String {
    plan.iter().map(|e| format!("{}", (e.share * 1000.0).round().max(0.0) as u32)).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Node, NodeKind, NodeStage};
    fn n(id: &str, mb: u64, w: f32) -> Node { let mut x = Node::device(id, id); x.stage = NodeStage::Connected; x.endpoint = Some(id.into()); x.usable_mb = mb; x.weight = w; x }
    fn g(id: &str, mb: u64, w: f32) -> Node { let mut x = Node::local(id, id, NodeKind::Gpu); x.device = Some(id.into()); x.usable_mb = mb; x.weight = w; x }
    #[test]
    fn order_and_fill() {
        let p = plan(&[n("a", 4000, 1.0), n("b", 4000, 1.0)], &[g("Vulkan0", 3500, 2.0)], Some(6000));
        assert_eq!(p.iter().map(|e| e.device.as_str()).collect::<Vec<_>>(), ["RPC0", "RPC1", "Vulkan0"]);
        let gpu = p[2].share as f64 * 6000.0;
        assert!(gpu <= 3500.5, "gpu {gpu}");
        assert!((p.iter().map(|e| e.share).sum::<f32>() - 1.0).abs() < 1e-3);
        assert!(p[2].share > p[0].share);
        assert_eq!(ts_arg(&p).split(',').count(), 3);
    }
}
