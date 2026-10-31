# Argus 0.2 — pool PC GPUs + Android RAM + LAN machines for local LLMs

UI: Palantir Blueprint-style dark ops console (topology graph, object inspector, dense data tables).

```
┌──────────────────────── PC (Host) ─ Tauri app ───────────────────────────┐
│ React UI  ──invoke/events──►  Rust core                                   │
│  Chat · Devices · Models       ├─ cluster.rs  adb poll 1.5s → provision   │
│        │                       ├─ adb.rs      bundled adb CLI wrapper      │
│        │ SSE /v1/chat          ├─ gpu.rs      --list-devices + nvidia-smi  │
│        │ tools (agent)         ├─ lan.rs      LAN rpc nodes (HELLO/memory) │
│        │                       ├─ split.rs    --device / -ts planning      │
│        │                       ├─ agent.rs    PC tools (cmd/files/screen)  │
│        ▼                       └─ llama.rs    spawns llama-server          │
│  llama-server :8080  (Vulkan, --rpc …, --device RPC0..,Vulkan0.., -ts …)  │
└──────────────┬───────────────────────────────────────────────────────────┘
               │ USB cable · adb forward tcp:5005N → device tcp:50052
┌──────────────▼──────── Android (Node) ─ native Kotlin ───────────────────┐
│ RpcService (foreground, wakelock) → exec librpc_server.so                 │
│   rpc-server -H 127.0.0.1 -p 50052 -t <big> -c                            │
└──────────────────────────────────────────────────────────────────────────┘
```

## What's new in 0.2 (desktop)
| | |
|---|---|
| **Network nodes** | Cluster → *Добавить ноду по IP* (`host:port`, default 50052). Any machine running llama.cpp **b12008** `rpc-server -H 0.0.0.0 -p 50052 -c` (Raspberry Pi, laptop, 2nd PC). Saved in `%APPDATA%\com.harkcluster.desktop\argus-settings.json`. Health + memory via a tiny ggml-rpc client (HELLO → DEVICE_COUNT → GET_DEVICE_MEMORY, protocol v8; verified against the real b12008 rpc-server). Shown with a **LAN** badge, merged into `--rpc` after the USB nodes. ⚠ rpc-server has no auth/encryption — closed home LAN only. rpc-server serves one client at a time, so while the engine is attached only a TCP check runs. |
| **Multi-GPU** | GPUs come from `llama-server --list-devices` (exact names/order the engine uses: Vulkan0, Vulkan1 …); nvidia-smi adds live util/VRAM. Each GPU is its own row/device. An iGPU next to a discrete GPU is off by default (toggle in inspector). |
| **Smart split** | `--rpc` first (registers RPC0…), then `--device RPC0,…,RPCn,Vulkan0,…` and `-ts` in exactly that order (llama.cpp b12008: RPC devices are a global RPC*k* counter, `--device` must follow `--rpc`). Share ∝ allocatable memory × **speed weight** (slider in inspector; 1.0 = phone/Pi CPU, GPU default 2.0), water-filled so no device gets more than it holds. Vision projector pinned to the local GPU (`-mmdev`). No GPU on the PC → no `-ngl/-ts`, llama.cpp `--fit` spreads over RPC + host RAM. |
| **Speed toggles** (Models → Ускорение) | Speculative decoding: `-hfd <draft> --spec-type draft-simple --spec-draft-n-max N -devd <local GPU> -ngld 99` (draft auto-picked by family: Qwen3 0.6B / Llama 3.2 1B / Gemma 2 2B; b12008 removed `--draft-max`). Prompt cache `--cache-reuse 256`. *Keep model loaded*: engine survives Argus restarts (re-attach via /health), identical Deploy is a no-op. *Fast start*: reuse a node's running rpc-server when its app is already the bundled version. |
| **Hugging Face** | Models → *Hugging Face* tab / *Найти модель на Hugging Face*: search `api/models?search=…&filter=gguf&sort=downloads`, expand → `.gguf` files (split parts merged, mmproj detected), fit check, **Скачать и запустить** (`-hf repo:QUANT`, or `--hf-file` when the quant tag is ambiguous), **Открыть на HF** (default browser). Curated list = *Рекомендуемые* with a ★ best pick for the pool (MoE bonus, e.g. Qwen3-30B-A3B). Added Qwen2.5-VL 3B/7B (vision, "для управления мышкой") and Qwen3-30B-A3B. |
| **Chat** | Conversations sidebar (localStorage), markdown + code blocks with copy, `<think>` folding, stop, regenerate, edit last message, system-prompt presets, attach text files (inlined, ≤256 KB), tok/s · prompt tok/s · TTFT per answer. |
| **Agent mode** | Toggle in chat header. OpenAI tool calling against llama-server (`--jinja`). Tools (Rust): `run_command` (PowerShell, cwd, timeout), `read_file`, `write_file`, `list_dir`, `open_url`/`open_app`, `screenshot` (xcap, downscaled to 1280 px; image sent only if `/props` says vision), `mouse_click`/`mouse_move`/`type_text`/`key_press` (enigo; coords in screenshot pixels, rescaled). Every call shows a card: *Разрешить / Разрешить всё в этой сессии / Отклонить*; dangerous tools (commands, writes, input) off by default; 20-step limit; action log; Stop. Recommended: Qwen3-30B-A3B, Qwen3 8B, Hermes 3 8B, Qwen2.5-VL 7B (screen). |
| **Onboarding** | First-run 7-step tour (re-open with **?** in the top bar) + *Быстрый старт* card with one button "Подобрать модель и запустить". |
| **Node auto-update** | On provision: `dumpsys package com.harkcluster.node` → `versionName`; older than bundled (`resources/node.version`, default 0.2) → `am force-stop` + `adb install -r`. |

## Quick start (2 files)

1. **PC:** run `Argus-Setup.exe` (bundles llama-server b12008 Vulkan, ADB, node.apk — no other installs).
2. **Tablet/phone:** Settings → About → tap *Build number* 7× → Developer options → **USB debugging ON**.
   (`Argus-Node.apk` is installed automatically over USB; sideload it manually only if you want.)
3. Plug in USB, accept the RSA prompt. Argus installs the node, starts rpc-server, forwards the port → node goes ONLINE.
4. **Models** → pick context (8K chat / 64K agents) → Deploy. Weights download once from Hugging Face.
5. Chat in Argus, or point any OpenAI-compatible app at it (see API tab).

If Windows doesn't see a Samsung device: install "Samsung USB Driver for Mobile Phones".

## Use with Hermes Agent / Open WebUI / Jan / Continue

llama-server runs with `--jinja` (native tool calling), `--alias <model-id>`, one slot, q8_0 KV cache.

| | |
|---|---|
| Base URL | `http://127.0.0.1:8080/v1` (LAN toggle in API tab → `http://<pc-ip>:8080/v1`) |
| API key | any string, e.g. `argus` |
| Model | the id shown in the API tab, e.g. `qwen3-4b` |

Hermes (`~/.hermes/config.yaml`):
```yaml
model:
  default: qwen3-4b
  provider: custom
  base_url: http://127.0.0.1:8080/v1
  api_key: argus
```
Hermes requires ≥64K context: deploy with the 64K preset. 64K of KV cache costs ~4–5 GB, so on GTX 960 4GB + one
8GB tablet only ~3B models are borderline; add a second device for Qwen3 4B / 8B agents. Plain chat apps are fine at 8–32K.

## Plug-and-play sequence (cluster.rs)
1. `adb devices -l` every 1.5 s. `unauthorized` → UI asks the user to tap *Allow*.
2. `pm list packages com.harkcluster.node`; missing → `adb install -r -g resources/node.apk`; installed `versionName` older than
   `resources/node.version` → `am force-stop` + `adb install -r` (stage UPDATING).
3. Allocate a PC port (50052, 50053, …) → `adb -s <serial> forward tcp:<local> tcp:50052`.
4. Fast start: if the forwarded port already answers like rpc-server and nothing was (re)installed, skip to 6. Else
   `am start-foreground-service -n com.harkcluster.node/.RpcService --ei port 50052 --ei threads <nproc>`.
5. Probe the forwarded port until rpc-server holds the socket open (adb closes it instantly when nothing listens).
6. `/proc/meminfo` → node goes **Connected**; pool = GPU VRAM + Σ device RAM, shown in the UI.
7. Launch: `llama-server -hf <repo:quant> -c <ctx> --rpc <usb…,lan…> --device RPC0,…,Vulkan0 -ts <‰ per device> -ngl 999 -mmdev Vulkan0`
   (+ speed flags). The full command line is printed as the first line of the engine log.
8. Unplug → node dropped, forward removed. A running model must be restarted.

## Key decisions
- **llama.cpp, not Ollama, is the engine.** Upstream Ollama has no RPC support (`OLLAMA_RPC_SERVERS` came from an unmerged PR). The Ollama branch in llama.rs is only for forks.
- **Native Kotlin node app** — it's ~150 lines; React Native/Flutter would add 20–40 MB for a status screen.
- **Binary as `librpc_server.so` in jniLibs** — Android 10+ only allows exec from `nativeLibraryDir`; needs `useLegacyPackaging` + `extractNativeLibs`.
- **Loopback only** on the Android device; nothing exposed on Wi-Fi. LAN nodes are opt-in, user-run rpc-servers.
- **Usable memory is conservative**: GPU = VRAM − 512 MB, device = 70% of MemAvailable at sync time. "Total" (the big number) is raw. The model library checks fit against *usable*.
- **Same llama.cpp commit on both sides** — the RPC protocol version must match.

## Honest performance notes
- Weights are pushed over USB at load time. Through adb, expect roughly 20–40 MB/s on USB 2.0, so a few GB of layers takes minutes the first time; `rpc-server -c` caches them on the device for next time.
- Per token, only activations cross the cable (small), but each hop adds latency, and a phone CPU computes its layers much slower than a GPU. Pooling makes models *fit* that otherwise wouldn't; it won't make small models faster.
- Example: GTX 960 4 GB + 8 GB tablet → 12 GB raw, ~7 GB usable → Llama 3.1 8B / Gemma 2 9B Q4 fit; Mistral Nemo 12B is tight.

## Prebuilt in this repo
- `android/app/src/main/jniLibs/arm64-v8a/librpc_server.so`: ggml-rpc-server from llama.cpp **b12008** (344846b). Static aarch64 (musl, zig cc, armv8-a baseline, so it runs on any arm64 Android), 2.3 MB, smoke-tested. Run it straight from adb if you want to skip the APK:
  `adb push librpc_server.so /data/local/tmp/rpc && adb shell chmod +x /data/local/tmp/rpc && adb shell /data/local/tmp/rpc -p 50052 -t 8`
  then `adb forward tcp:50052 tcp:50052` and `llama-server ... --rpc 127.0.0.1:50052`.
- Node 0.2 (versionCode 3) ships three static builds (zig 0.13, musl), see `android/scripts/build-rpc-variants-zig.sh`:
  `librpc_server.so` (cortex_a53 baseline), `librpc_server_dotprod.so` (armv8.2-a+dotprod+fp16), `librpc_server_i8mm.so`
  (armv8.6-a+dotprod+fp16+i8mm, no SVE). RpcService picks i8mm > asimddp > baseline from `/proc/cpuinfo` Features and
  permanently drops a variant that dies with SIGILL. `-t` = big cores (cores above the lowest `cpuinfo_max_freq` tier;
  intent extra `force_threads` overrides; the old `threads` extra is ignored). Weight cache: `-c` with
  `LLAMA_CACHE=<filesDir>/llama-cache` → tensors in `<filesDir>/llama-cache/rpc`; the app shows its size and has *Clear cache*.
  Version check: `adb shell dumpsys package com.harkcluster.node | grep versionName`, or logcat tag `RpcService` line
  `argus-node-version 0.2 (3)`.

## One-click build (free, GitHub Actions)
Push the repo to GitHub → Actions → **build** → Run. You get `node.apk` and a Windows installer (`.exe`/`.msi`) that bundles llama-server b12008 CUDA 12.4, cudart, adb and the APK.

## Manual build
Android node:
```bash
cd android
ANDROID_NDK=$HOME/Android/Sdk/ndk/27.0.12077973 LLAMA_REF=b12008 ./scripts/build-rpc-server.sh
./gradlew assembleRelease   # copy app-release.apk → desktop/src-tauri/resources/node.apk
```
Desktop:
```bash
# llama-server with CUDA + RPC, same b12008:
cmake -B build -DGGML_CUDA=ON -DGGML_RPC=ON && cmake --build build --target llama-server -j
# copy build/bin/llama-server(.exe) and platform-tools adb(.exe + dlls) → desktop/src-tauri/resources/
cd desktop && npm i
npm run tauri icon path/to/logo.png   # generates src-tauri/icons
npm run tauri dev                     # or: npm run dev  (browser, mock telemetry)
```

## Tree
```
desktop/
  src/                 React + Tailwind UI
    components/        ChatView (chats, agent cards), MessageBubble, Markdown, PromptBar, QuickStart, Tour,
                       DevicePanel, LanDialog, Topology, MemoryPool, UsbWizard, ModelLibrary, HfBrowser, ApiPanel
    lib/               api.ts (Tauri bridge + SSE/tool chat + mocks), agent.ts (tool loop), launch.ts (deploy),
                       models.ts (curated list, fit, best pick, drafts), types.ts
    store/cluster.ts   zustand store (conversations, speed settings)
  src-tauri/src/       main.rs, state.rs, settings.rs, adb.rs, cluster.rs, gpu.rs, lan.rs, split.rs, llama.rs, agent.rs
  src-tauri/resources/ adb, llama-server + ggml dlls (Vulkan), node.apk, node.version (bundled)
android/
  app/src/main/java/com/harkcluster/node/  RpcService.kt, MainActivity.kt
  scripts/build-rpc-server.sh              NDK cross-compile of rpc-server
```

## Next steps
- Verify with real devices; tune the 70% (USB) / 80% (LAN) headroom and default speed weights.
- Real per-node micro-benchmark to set speed weights automatically (today: manual slider).
- mDNS discovery of LAN rpc-servers; auth would need an upstream rpc-server change.
- Host RAM as a split target when a GPU is present (needs layer count → `-ngl` < all).
- An rpc-server exposing several devices is handled (RPC names counted from DEVICE_COUNT for LAN nodes); USB nodes assume one.

---

💀 Проект делается на GTX 960 и планшете. [Поддержать на Boosty](SUPPORT.md)
