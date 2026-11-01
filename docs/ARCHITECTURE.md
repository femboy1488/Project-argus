# Argus 0.3.1 — pool PC GPUs + Android RAM + LAN/Wi-Fi machines for local LLMs (Windows + Linux)

UI: Palantir Blueprint-style dark ops console (topology graph, object inspector, dense data tables).

```
┌──────────────────────── PC (Host) ─ Tauri app ───────────────────────────┐
│ React UI  ──invoke/events──►  Rust core                                   │
│  Chat · Devices · Models       ├─ cluster.rs  loops; adb poll → provision │
│        │                       ├─ adb.rs      adb CLI wrapper (timeouts)   │
│        │ SSE /v1/chat          ├─ gpu.rs      --list-devices/nvidia-smi/vk │
│        │ tools (agent)         ├─ lan.rs      LAN rpc + UDP beacons :50053 │
│        │                       ├─ split.rs    --device / -ts planning      │
│        │                       ├─ agent.rs    PC tools (cmd/files/screen)  │
│        ▼                       └─ llama.rs    llama-server + supervisor    │
│  llama-server :8080  (Vulkan, --rpc …, --device RPC0..,Vulkan0.., -ts …)  │
└──────────────┬───────────────────────────────────────────────────────────┘
               │ USB cable · adb forward tcp:5005N → device tcp:50052
┌──────────────▼──────── Android (Node) ─ native Kotlin ───────────────────┐
│ RpcService (foreground, wakelock) → exec librpc_server.so                 │
│   rpc-server -H 127.0.0.1 -p 50052 -t <big> -c                            │
└──────────────────────────────────────────────────────────────────────────┘
```

## What's new in 0.3.1 — self-update (always asks)

Rule: **nothing is installed without the user's click.** No «update automatically» option exists.

### Desktop (`src-tauri/src/updater.rs`, UI `components/Updates.tsx`)
- `updater::start`: 5 s after start and every 6 h, only if settings `checkUpdates` (UI «Проверять обновления», default on):
  `GET https://api.github.com/repos/femboy1488/Project-argus/releases/latest` (User-Agent `Argus-Cluster/<ver>`, 10 s
  timeout, errors only logged). `tag_name` must be `vX.Y[.Z]` (pre-release suffix ignored); anything else (e.g. the old
  `V2` release) is not a version and never offered. Newer than `CARGO_PKG_VERSION` ⇒ event `update-available`.
- UI dialog «Доступна версия X. Хотите обновить?» + release notes (markdown): **Обновить / Позже / Пропустить эту версию**.
  «Позже» = zustand session list (asks again next start); «Пропустить» = settings `skippedVersion` (manual «Проверить
  обновления» in «О программе» still offers it). Top bar shows `⬆ X` while an update is pending.
- Install kind: `nsis` (uninstall.exe next to the exe), `portable`, `appimage` (`$APPIMAGE`), `deb` (exe under /usr), `other`.
  Asset: `Argus-Setup.exe` (nsis) / `*.AppImage` (arch-matched) / none ⇒ «Открыть страницу релиза».
- `install_update` (only from the button): download to `%TEMP%/argus-update/` with `update-progress` events, SHA-256 against
  `SHA256SUMS.txt` of the same release — **missing file, missing line or mismatch ⇒ refused**, file deleted.
  Then `llama::stop_for_update` (kills the engine even with keep-loaded), `adb kill-server`, and
  * Windows: `Argus-Setup.exe /S /UPDATE` (detached) → app exits. `windows-installer.nsi`: `/UPDATE` waits 1.5 s,
    `taskkill Argus.exe / llama-server.exe`, `adb kill-server`, installs into the registered `InstallLocation`, keeps user
    data (settings `%APPDATA%\com.harkcluster.desktop`, models in the llama.cpp cache), no WebView2 browser pop-up and no
    desktop shortcut re-creation when silent, and `${If} ${Silent}`/`/UPDATE` ⇒ `Exec Argus.exe` at the end.
  * AppImage: copy next to `$APPIMAGE` as `.<name>.update`, chmod 755, atomic rename over `$APPIMAGE`, relaunch, exit.
  * .deb / other: open the release page.
- HTTP via `reqwest 0.12` + rustls/ring + webpki roots (no OpenSSL, no system TLS — same on Windows and Linux).

### Nodes
- **USB** (cluster.rs `provision`): outdated installed app (versionCode, else versionName, vs `resources/node.version`)
  is **no longer auto-installed**. The node is provisioned with its current app (same rpc protocol) and gets
  `updateAvailable`; the UI asks «Обновить ноду на <device>?» (Обновить / Позже / Пропустить эту версию →
  settings `skippedNodeVersion`). «Обновить» = `update_usb_node`: `approve_update` + re-provision ⇒ `am force-stop` +
  `adb install -r` (stage UPDATING), engine supervisor restarts a model that used the node. A *missing* app is still
  installed on first connect (first-time setup, not an update). Inspector button «Обновить ноду до X» does the same.
- `main.rs`: `DEFAULT_NODE_VERSION` replaced by `default_node_version()` = `include_str!("../resources/node.version")`
  (runtime file still wins), so the fallback can't drift from the APK again (it said "0.3 (4)" while shipping 0.3).
- **Wi-Fi**: `updater::start_node_server` serves, while Argus runs, `http://0.0.0.0:50055/node.version` ("0.3.1 (5)"),
  `/node.apk`, `/node.sha256` (GET/HEAD, minimal HTTP/1.1, nothing else). Bind failure ⇒ message in «О программе».
  Beacon gets `"vn"` (versionName) and `"vc"` (versionCode) — `"v"` stays the beacon protocol version (1).
  lan.rs answers each beacon source (≤ 1 per 10 s per IP) with a unicast to the beacon's source port:
  `{"argus":"host","v":1,"update_port":50055,"node_vc":N}`; «Обновить по Wi-Fi» (`wifi_node_update`) sends the same with
  `"update":true`. Cluster shows «Нода устарела» (node table, «Найдено в сети» list) when beacon `vc` < bundled code.
- **Android node** (`Updater.kt`): the beacon thread waits for host replies on its socket (instead of sleeping 2 s).
  `node_vc` > own versionCode (re-checked ≤ every 10 min) or `update:true` ⇒ fetch `/node.version` (+`/node.sha256`) ⇒
  notification «Доступна версия Argus Node X» and, in the app, the dialog «Доступна версия X. Хотите обновить?» —
  **Обновить / Позже / Пропустить эту версию**. Only «Обновить» downloads: SHA-256 check (when published), package name
  and higher versionCode checked via `getPackageArchiveInfo`, then a `PackageInstaller` session (Android's own
  confirmation; «Установка неизвестных приложений» permission is requested the first time). `MY_PACKAGE_REPLACED` restarts
  RpcService if it was running. Button «Проверить обновление»: PC (if heard in the last 5 min) else GitHub `releases/latest`
  asset `Argus-Node.apk` (+ `SHA256SUMS.txt`). Manifest: `REQUEST_INSTALL_PACKAGES`, `usesCleartextTraffic` (HTTP on LAN),
  `UpdateReceiver`, MainActivity `singleTop`. Updates require the same signing key (argus-debug.jks, cert SHA-256
  `5b5ac318…8431`).

### Releases
- `.github/workflows/build.yml`: tag `v*` ⇒ job `release` (needs node-apk, desktop-windows, desktop-linux): checks tag ==
  `v<tauri.conf version>`, collects `Argus-Setup.exe` (now built with our `windows-installer.nsi` via makensis, paths passed
  with `-D…`), `Argus-Node.apk`, `*.AppImage`, `*.deb`, runs `scripts/make-release-assets.sh` ⇒ `SHA256SUMS.txt`,
  publishes with `softprops/action-gh-release@v2`. The node APK is re-signed with secret `ANDROID_KEYSTORE_B64`
  (+`ANDROID_KEYSTORE_PASS`); without it CI warns that the APK can't update existing installs.
- Manual: build locally, `scripts/make-release-assets.sh /workspace/work/out`, upload all files + `SHA256SUMS.txt` to a
  release tagged `vX.Y.Z`.

## What's new in 0.3 (desktop)

### Stability: unplug / replug no longer needs an app restart
Root causes found in 0.2 (all fixed):
1. **One serial poll loop with no timeouts.** `adb devices`, `adb shell cat /proc/meminfo` (telemetry of a device being
   unplugged), `nvidia-smi`, LAN probes and `--list-devices` all ran inside one loop without timeouts. One hung adb call
   (common right after a hard unplug on Windows) froze the loop → no snapshots, no provisioning: the app looked dead.
   Now: independent loops (`adb` 1.5 s, `host` 2 s, `lan` 2 s, `engine` 1 s, `emit` 1 s), each iteration in its own task
   (a panic loses one tick), every adb call has a timeout (15 s, install 240 s, `adb devices` 8 s) with `kill_on_drop`.
2. **`offline` state / fast replug ignored.** A USB reset shows the device as `offline` for a moment or re-enumerates it
   within one poll. 0.2 kept the node ONLINE while adb had already dropped its forward → every later request went
   nowhere. Now: non-`device` state, a new adb `transport_id`, a missing forward (`adb forward --list` every tick) or
   3 failed telemetry reads ⇒ node OFFLINE, forward removed, provisioning restarted.
3. **Provisioning could hang forever and block the device.** The `provisioning` set entry of a hung task was never
   cleared, so a replugged device was never provisioned again. Now: abortable task per device (aborted on unplug),
   generation counter (stale tasks can't write), 6-min overall timeout.
4. **Sticky FAULT.** Any transient error (rpc-server still starting after replug) left the node in Error until the user
   pressed Rescan. Now: retry with backoff 3 → 6 → 12 → 24 → 48 → 60 s, 8 attempts, countdown in the UI.
5. **Dead engine kept as "ready".** llama-server holding a dead `--rpc` socket either aborted or hung, but the UI stayed
   "ready" and the chat fetch hung or failed with an English network error. Now the **engine supervisor** (llama.rs):
   process exit or loss of any node it uses ⇒ process killed (it also holds the single client slot of every other
   rpc-server), state `broken`, `engine-state` event ⇒ the chat request is aborted with
   «Генерация прервана: нода «X» отключилась…». When every node it used is back and the cluster is unchanged for 3 s ⇒
   auto-restart with the current node set, toast «Кластер изменился — модель перезапущена» (≤3 per 10 min; never after a
   crash during loading — that's a config/memory error).
6. **Port churn.** The PC port was released on unplug, so a replugged device could land on a different port.
   Now serial → port is kept for the session and seeded from `adb forward --list` at start (forwards survive in the adb
   server), so `--rpc 127.0.0.1:5005N` stays valid.
7. **Fast start trusted any open socket.** "Socket stays open" also matches a busy/wedged rpc-server. Now a real HELLO
   (protocol v8) decides reuse; a wedged server gets `am stopservice` + restart.
8. **adb server wedged.** `adb devices` failing 3× ⇒ `adb kill-server` + `start-server`, all USB nodes re-provisioned.
9. **Orphan engines.** On start, llama-server processes from Argus (bundled exe path or our command line) are adopted
   when *keep loaded* is on and `/health` is 200 (its `--rpc` list is read from the command line and supervised), else
   killed. On exit the engine is stopped unless *keep loaded*.

UI: per-node **Переподключить** (USB: drop forward, restart RpcService, provision; LAN: probe now), **Перезапустить
движок** (Cluster header, banners in Chat/Cluster/Models), toasts, retry countdown, "● в модели" marker, adb/beacon status
in the status bar.

### Wi-Fi nodes (UDP beacon)
Nodes broadcast every 2 s to UDP **50053**:
`{"argus":"node","v":1,"name":"<model>","port":50052,"ram_mb":N,"free_mb":N,"variant":"dotprod"}`
(Argus Node "Wi-Fi режим" — rpc-server on `0.0.0.0:50052`; `scripts/argus-node-linux.sh` for Linux PCs / Raspberry Pi).
`lan.rs` listens on `0.0.0.0:50053` (re-binds every 10 s if busy; error shown in the UI) → **Cluster → Найдено в сети**
with *Добавить* and an *автодобавление* setting (`autoAddDiscovered`). Added beacon nodes are `lan_nodes` with `auto:true`.
Health: beacon heard in the last 60 s ⇒ beacon-tracked: >10 s silence ⇒ OFFLINE (engine supervisor treats it like an
unplug; when it's back the model restarts). Otherwise TCP/HELLO checks, OFFLINE after 2 consecutive failures (Wi-Fi
hiccups). While the engine is attached only a TCP connect is made (rpc-server serves one client). Manual add by IP:port
stays. ⚠ LAN RPC has no password/encryption — closed home network only (warning in the UI).

### Linux desktop
`cfg(windows)` vs unix everywhere: `hidden_command` (CREATE_NO_WINDOW only on Windows), binary lookup
`ARGUS_ADB`/`ARGUS_LLAMA_SERVER` env > bundled `resources/` > next to the exe > PATH (+x fixed best-effort),
`LD_LIBRARY_PATH` = llama-server's dir (RUNPATH is `$ORIGIN` anyway), models in `$ARGUS_MODELS` >
`$XDG_DATA_HOME/argus/models` > `~/.local/share/argus/models`, orphan kill via sysinfo (no taskkill/pkill), GPU:
`--list-devices` > nvidia-smi > `vulkaninfo` (display-only fallback), agent `run_command` = bash, `xdg-open`.
Screenshot / mouse / keyboard tools stay Windows-only (xcap on Linux needs pipewire/wayland dev libs); the agent hides
them via `platform_info`. CI job `desktop-linux` (ubuntu-22.04, webkit2gtk-4.1) bundles llama.cpp b12008
`ubuntu-vulkan-x64` llama-server + its `.so` files and platform-tools `adb`, builds AppImage + .deb.
USB on Linux needs Android udev rules (`android-sdk-platform-tools-common`); adb's `no permissions` state is shown.

### Linux / Raspberry Pi as a Wi-Fi node — `scripts/argus-node-linux.sh`
x86_64: llama.cpp b12008 `ubuntu-vulkan-x64` (or `ubuntu-x64` with `ARGUS_CPU=1`/no Vulkan) → `ggml-rpc-server`.
aarch64: our static musl builds from `android/app/src/main/jniLibs/arm64-v8a` (i8mm > dotprod > baseline by
`/proc/cpuinfo`), fallback to the official `ubuntu-arm64` archive. Runs `rpc-server -H 0.0.0.0 -p 50052 -t <nproc> -c`
(restarted if it dies) + a python3 (or socat) beacon loop that only beacons while rpc-server is alive.
`--systemd` installs a user service. Verified in the sandbox (aarch64): static i8mm build started, beacon received on
:50053, HELLO answered `v8.0.1`, rpc-server auto-restarted after `kill`.

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
| **Node update** | On provision: `dumpsys package com.harkcluster.node` → `versionName`/`versionCode`; older than bundled (`resources/node.version`) → since 0.3.1 the user is asked first (see 0.3.1), then `am force-stop` + `adb install -r`. |

## Quick start (2 files)

1. **PC:** run `Argus-Setup.exe` (bundles llama-server b12008 Vulkan, ADB, node.apk — no other installs).
   Linux: AppImage / .deb from CI (Vulkan driver + Android udev rules needed).
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
2. `pm list packages com.harkcluster.node`; missing → `adb install -r -g resources/node.apk`; installed version older than
   `resources/node.version` → UI asks «Обновить ноду на …?»; after «Обновить»: `am force-stop` + `adb install -r` (stage UPDATING).
3. Allocate a PC port (50052, 50053, …) → `adb -s <serial> forward tcp:<local> tcp:50052`.
4. Fast start: if the forwarded port already answers like rpc-server and nothing was (re)installed, skip to 6. Else
   `am start-foreground-service -n com.harkcluster.node/.RpcService --ei port 50052 --ei threads <nproc>`.
5. Probe the forwarded port until rpc-server holds the socket open (adb closes it instantly when nothing listens).
6. `/proc/meminfo` → node goes **Connected**; pool = GPU VRAM + Σ device RAM, shown in the UI.
7. Launch: `llama-server -hf <repo:quant> -c <ctx> --rpc <usb…,lan…> --device RPC0,…,Vulkan0 -ts <‰ per device> -ngl 999 -mmdev Vulkan0`
   (+ speed flags). The full command line is printed as the first line of the engine log.
8. Unplug / `offline` / new transport id / lost forward → node OFFLINE, forward removed, provisioning aborted; the engine
   supervisor stops a model that used it. Replug → same PC port, provisioning again (retry with backoff), model
   auto-restarted 3 s after the cluster settles.

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
Push the repo to GitHub → Actions → **build** → Run. You get `node.apk`, a Windows installer (`.exe`/`.msi`) that bundles llama-server b12008 CUDA 12.4, cudart, adb and the APK, and `argus-linux` (AppImage + .deb, llama-server b12008 Vulkan x64 + adb + APK).

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
                       DevicePanel (+ Найдено в сети, Переподключить), EngineBanner, Toaster, LanDialog, Topology,
                       MemoryPool, UsbWizard, ModelLibrary, HfBrowser, ApiPanel
    lib/               api.ts (Tauri bridge + SSE/tool chat + abort registry + mocks), agent.ts (tool loop, OS-aware),
                       launch.ts (deploy + applyEngine: UI ⇄ Rust supervisor),
                       models.ts (curated list, fit, best pick, drafts), types.ts
    store/cluster.ts   zustand store (conversations, speed settings)
  src-tauri/src/       main.rs, state.rs, settings.rs, adb.rs, cluster.rs, gpu.rs, lan.rs, beacon.rs, split.rs, llama.rs, agent.rs,
                       updater.rs (GitHub self-update + node.apk HTTP :50055)
  src-tauri/resources/ adb, llama-server + ggml dlls (Vulkan), node.apk, node.version (bundled)
scripts/argus-node-linux.sh                rpc-server b12008 + UDP beacon for Linux PCs / Raspberry Pi
scripts/make-release-assets.sh             SHA256SUMS.txt for a manual GitHub Release
android/
  app/src/main/java/com/harkcluster/node/  RpcService.kt, MainActivity.kt, Updater.kt (self-update, UpdateReceiver)
  scripts/build-rpc-server.sh              NDK cross-compile of rpc-server
```

## Next steps
- Verify with real devices; tune the 70% (USB) / 80% (LAN) headroom and default speed weights.
- Real per-node micro-benchmark to set speed weights automatically (today: manual slider).
- Auth/encryption for LAN RPC would need an upstream rpc-server change (or a TLS/WireGuard tunnel).
- Agent screenshots/mouse on Linux (xcap needs pipewire/wayland dev libs in CI).
- Hot-adding a node to a running engine is impossible in llama.cpp; Argus offers «Перезапустить движок» instead.
- Host RAM as a split target when a GPU is present (needs layer count → `-ngl` < all).
- An rpc-server exposing several devices is handled (RPC names counted from DEVICE_COUNT for LAN nodes); USB nodes assume one.

---

💀 Проект делается на GTX 960 и планшете. [Поддержать на Boosty](SUPPORT.md)
