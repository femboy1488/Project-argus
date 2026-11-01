//! Local GPU discovery.
//! * `llama-server --list-devices` is the source of truth: exactly the devices (and names) the engine
//!   will use, for any vendor (Vulkan build: NVIDIA, AMD, Intel; several cards).
//! * `nvidia-smi` adds live utilisation / VRAM for NVIDIA cards (Windows and Linux).
//! * `vulkaninfo` is a display-only fallback when --list-devices fails (no explicit --device then).
//! Every external tool runs with a timeout: a wedged driver must not stall the host loop.
use crate::adb::hidden_command;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct GpuInfo { pub name: String, pub total_mb: u64, pub used_mb: u64, pub util_pct: f32 }

#[derive(Debug, Clone, PartialEq)]
pub struct LlamaDevice { pub name: String, pub desc: String, pub total_mb: u64, pub free_mb: u64 }

/// All NVIDIA GPUs (one line each). Empty on AMD/Intel/no GPU.
pub async fn query_nvidia_all() -> Vec<GpuInfo> {
    let fut = hidden_command("nvidia-smi")
        .args(["--query-gpu=name,memory.total,memory.used,utilization.gpu", "--format=csv,noheader,nounits"])
        .kill_on_drop(true).output();
    let Ok(Ok(out)) = tokio::time::timeout(std::time::Duration::from_secs(5), fut).await else { return vec![] };
    if !out.status.success() { return vec![]; }
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| {
        let p: Vec<&str> = l.split(',').map(str::trim).collect();
        if p.len() < 4 { return None; }
        Some(GpuInfo { name: p[0].replace("NVIDIA ", ""), total_mb: p[1].parse().ok()?, used_mb: p[2].parse().ok()?,
                       util_pct: p[3].parse().unwrap_or(0.0) })
    }).collect()
}

/// Runs the bundled llama-server with --list-devices (≈1–2 s, loads the backends) and parses
/// "  Vulkan0: NVIDIA GeForce GTX 960 (4096 MiB, 3780 MiB free)". RPC devices are skipped.
pub async fn list_llama_devices(llama_bin: &Path) -> Option<Vec<LlamaDevice>> {
    let mut cmd = hidden_command(llama_bin);
    cmd.arg("--list-devices").kill_on_drop(true);
    crate::llama::lib_path_env(&mut cmd, llama_bin);
    let fut = cmd.output();
    let out = tokio::time::timeout(std::time::Duration::from_secs(25), fut).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    if !text.contains("Available devices") { return None; }
    Some(parse_list_devices(&text))
}

pub fn parse_list_devices(text: &str) -> Vec<LlamaDevice> {
    let mut v = vec![];
    let mut in_list = false;
    for line in text.lines() {
        if line.contains("Available devices") { in_list = true; continue; }
        if !in_list || !line.starts_with("  ") { continue; }
        let l = line.trim();
        let Some((name, rest)) = l.split_once(": ") else { continue };
        if name.starts_with("RPC") || name.contains(' ') { continue; }
        let Some(open) = rest.rfind(" (") else { continue };
        let desc = rest[..open].trim().to_string();
        let nums: Vec<u64> = rest[open..].split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse().ok()).collect();
        if nums.len() < 2 { continue; }
        v.push(LlamaDevice { name: name.to_string(), desc, total_mb: nums[0], free_mb: nums[1] });
    }
    v
}

/// Largest discrete Vulkan GPU from `vulkaninfo` (name, device-local heap). Cached: run once.
pub async fn vulkan_fallback() -> Option<GpuInfo> {
    static CACHE: tokio::sync::OnceCell<Option<GpuInfo>> = tokio::sync::OnceCell::const_new();
    CACHE.get_or_init(|| async {
        let fut = hidden_command("vulkaninfo").kill_on_drop(true).output();
        let out = tokio::time::timeout(std::time::Duration::from_secs(10), fut).await.ok()?.ok()?;
        parse_vulkaninfo(&String::from_utf8_lossy(&out.stdout)).into_iter().max_by_key(|g| g.total_mb)
    }).await.clone()
}

/// deviceName / deviceType / memoryHeaps (size + MEMORY_HEAP_DEVICE_LOCAL_BIT) per GPU block.
pub fn parse_vulkaninfo(text: &str) -> Vec<GpuInfo> {
    let mut out: Vec<GpuInfo> = vec![];
    let mut name: Option<String> = None;
    let mut cpu = false;
    let mut heap_size: u64 = 0;
    let mut local_mb: u64 = 0;
    let flush = |name: &mut Option<String>, cpu: bool, local_mb: &mut u64, out: &mut Vec<GpuInfo>| {
        if let Some(n) = name.take() {
            if !cpu && *local_mb > 0 && !out.iter().any(|g| g.name == n) {
                out.push(GpuInfo { name: n.replace("NVIDIA ", ""), total_mb: *local_mb, used_mb: 0, util_pct: 0.0 });
            }
        }
        *local_mb = 0;
    };
    for line in text.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("deviceName").and_then(|r| r.trim_start().strip_prefix('=')) {
            if name.as_deref() != Some(v.trim()) { flush(&mut name, cpu, &mut local_mb, &mut out); }
            name = Some(v.trim().to_string());
            cpu = false;
        } else if l.starts_with("deviceType") {
            cpu = l.contains("CPU");
        } else if let Some(v) = l.strip_prefix("size").and_then(|r| r.trim_start().strip_prefix('=')) {
            heap_size = v.split_whitespace().next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
        } else if l.contains("MEMORY_HEAP_DEVICE_LOCAL_BIT") && heap_size > 0 {
            local_mb = local_mb.max(heap_size / 1024 / 1024);
            heap_size = 0;
        }
    }
    flush(&mut name, cpu, &mut local_mb, &mut out);
    out
}

/// Integrated GPUs share system RAM; heuristic on the Vulkan device description.
pub fn looks_integrated(desc: &str) -> bool {
    let d = desc.to_lowercase();
    ["intel(r) uhd", "intel(r) hd", "iris", "radeon(tm) graphics", "radeon graphics", "vega 3", "vega 6", "vega 8",
     "vega 10", "vega 11", "intel(r) graphics", "intel(r) arc(tm) graphics", "microsoft basic"]
        .iter().any(|k| d.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse() {
        let t = "ggml_vulkan: Found 2 Vulkan devices:\nAvailable devices:\n  Vulkan0: NVIDIA GeForce GTX 960 (4096 MiB, 3780 MiB free)\n  Vulkan1: Intel(R) UHD Graphics 630 (8031 MiB, 8031 MiB free)\n  RPC0: 127.0.0.1:50052 (7000 MiB, 5000 MiB free)\n";
        let d = parse_list_devices(t);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].name, "Vulkan0");
        assert_eq!(d[0].desc, "NVIDIA GeForce GTX 960");
        assert_eq!((d[0].total_mb, d[0].free_mb), (4096, 3780));
        assert!(looks_integrated(&d[1].desc) && !looks_integrated(&d[0].desc));
    }
    #[test]
    fn vulkaninfo() {
        let t = "GPU0:\nVkPhysicalDeviceProperties:\n\tdeviceName = AMD Radeon RX 6600 (RADV NAVI23)\n\tdeviceType = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU\nVkPhysicalDeviceMemoryProperties:\n\tmemoryHeaps: count = 2\n\t\tmemoryHeaps[0]:\n\t\t\tsize   = 8573157376 (0x1ff000000) (7.98 GiB)\n\t\t\tflags:\n\t\t\t\tMEMORY_HEAP_DEVICE_LOCAL_BIT\n\t\tmemoryHeaps[1]:\n\t\t\tsize = 16000000000\n\t\t\tflags:\n\t\t\t\tNone\nGPU1:\n\tdeviceName = llvmpipe (LLVM 15.0.7, 256 bits)\n\tdeviceType = PHYSICAL_DEVICE_TYPE_CPU\n\t\t\tsize = 9999999999\n\t\t\t\tMEMORY_HEAP_DEVICE_LOCAL_BIT\n";
        let g = parse_vulkaninfo(t);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].total_mb, 8176);
        assert!(g[0].name.starts_with("AMD Radeon RX 6600"));
    }
}
