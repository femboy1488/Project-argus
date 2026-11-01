// Mirrors src-tauri/src/state.rs (serde camelCase)
export type NodeKind = "gpu" | "cpu" | "phone" | "tablet" | "lan";
export type NodeStage =
  | "unauthorized" | "installing" | "updating" | "forwarding" | "starting" | "syncing" | "connected" | "offline" | "error";
export type Transport = "local" | "usb" | "lan";

export interface ClusterNode {
  id: string;
  name: string;
  kind: NodeKind;
  stage: NodeStage;
  transport: Transport;
  endpoint: string | null; // "127.0.0.1:50052" once forwarded, "192.168.1.20:50052" for LAN
  totalMb: number;
  availableMb: number;
  usableMb: number; // what we let llama.cpp use (headroom subtracted)
  loadPct: number;
  error: string | null;
  weight: number;   // speed weight for -ts (1.0 = CPU baseline)
  enabled: boolean; // included in --rpc / --device
  device: string | null;  // llama.cpp device name for local GPUs (Vulkan0)
  version: string | null; // node app version (USB) / rpc protocol (LAN)
  rpcDevices: number;
  integrated: boolean;
  attempts: number;        // failed provisioning attempts in a row (USB)
  retryIn: number | null;  // seconds until the automatic retry (USB, stage error)
  variant: string | null;  // rpc-server build from the Wi-Fi beacon
  beacon: boolean;         // announced by UDP beacon (Wi-Fi mode)
  inEngine: boolean;       // part of the running engine's --rpc list
  appVc: number | null;    // Argus Node versionCode (USB: dumpsys, Wi-Fi: beacon "vc")
  appVersion: string | null; // Argus Node versionName from the Wi-Fi beacon
  updateAvailable: string | null; // bundled node version when this node is older («Нода устарела»)
  updateSkipped: boolean;  // «Пропустить эту версию» chosen for exactly that version
}

/** A node heard on UDP :50053 (Argus Node Wi-Fi mode / argus-node-linux.sh). */
export interface DiscoveredNode {
  id: string; endpoint: string; name: string; ramMb: number; freeMb: number;
  variant: string | null; ageSec: number; added: boolean;
  appVersion: string | null; appVc: number | null; outdated: boolean;
}

export type EngineStatus = "idle" | "loading" | "ready" | "broken";

/** Rust-side supervisor view of llama-server (also pushed as the `engine-state` event, + optional toast). */
export interface EngineView {
  status: EngineStatus;
  reason: string | null;
  alias: string | null;
  nodes: string[];
  pendingRestart: boolean;
  newNodes: boolean;
  canRestart: boolean;
  toast?: string;
}

export interface SplitEntry { id: string; device: string; name: string; share: number }

export interface ClusterSnapshot {
  host: ClusterNode;
  gpus: ClusterNode[];
  nodes: ClusterNode[];
  totalMb: number;
  usableMb: number;
  rpcServers: string[];
  split: SplitEntry[];
  fastStart: boolean;
  bundledNodeVersion: string;
  discovered: DiscoveredNode[];
  autoAddDiscovered: boolean;
  beaconError: string;
  adbOk: boolean;
  engine: EngineView;
  platform: "windows" | "linux" | "macos";
  appVersion: string;            // desktop version (Cargo.toml)
  bundledNodeVc: number | null;  // versionCode of resources/node.apk
}

/** Mirrors updater.rs */
export interface ReleaseAsset { name: string; url: string; size: number }
export interface Release {
  version: string; tag: string; name: string; notes: string; url: string; publishedAt: string;
  asset: ReleaseAsset | null; hasSums: boolean;
}
export interface UpdateStatus {
  current: string;
  installKind: "nsis" | "portable" | "appimage" | "deb" | "other";
  latest: Release | null;
  available: boolean;
  skipped: boolean;
  checkEnabled: boolean;
  checking: boolean;
  lastCheck: number | null;
  error: string | null;
  busy: boolean;
  phase: string;
  done: number;
  total: number;
  nodeServerError: string;
}
export interface UpdateProgress { phase: string; done: number; total: number }

export interface ModelEntry {
  id: string;
  name: string;
  family: string;
  params: string;
  quant: string;
  sizeGb: number;      // GGUF file size (+ mmproj for vision models)
  hfRepo: string;      // passed to llama-server -hf
  hfFile?: string;     // exact file (--hf-file), for Hugging Face search results
  kvMbPer1k: number;   // KV cache (q8_0) per 1k tokens of context
  tools?: boolean;     // good at tool/function calling (agents like Hermes)
  vision?: boolean;    // ships an mmproj: can see screenshots
  moe?: boolean;       // mixture of experts: fast for its size
  score?: number;      // quality rank for "best pick" (higher = better)
  draft?: string;      // same-family small model for speculative decoding (repo:quant)
  note?: string;
  tags: string[];
}

export type FitLevel = "fits" | "tight" | "too-heavy";

export interface ToolCall { id: string; name: string; arguments: string }

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "system" | "tool";
  content: string;
  pending?: boolean;
  /** generation speed, tokens/s (live estimate while streaming, server timings at the end) */
  tps?: number;
  promptTps?: number;
  tokens?: number;
  ttftMs?: number;
  /** attached text files (inlined into the prompt) */
  files?: { name: string; size: number; text: string }[];
  /** agent mode */
  toolCalls?: ToolCall[];
  toolCallId?: string;
  toolName?: string;
  image?: string; // data URL (screenshot) shown in the log and sent to vision models
  hidden?: boolean;
}

export interface Conversation {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  systemPrompt: string;
  agent: boolean;
  messages: ChatMessage[];
}

export type ModelRunState =
  | { status: "idle" }
  | { status: "loading"; model: ModelEntry; log?: string }
  | { status: "ready"; model: ModelEntry; endpoint: string }
  /** engine died or lost a node; Rust restarts it when the cluster is back (pendingRestart) */
  | { status: "broken"; model: ModelEntry; reason: string }
  | { status: "error"; message: string };

export interface SpeedSettings {
  speculative: boolean;
  draft: string;        // "auto" | "none" | "repo:quant"
  draftMax: number;
  promptCache: boolean; // --cache-reuse 256
  keepLoaded: boolean;
  fastStart: boolean;   // mirrored in Rust settings
}
