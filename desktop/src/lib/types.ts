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
  // ---- 0.3.2
  benchGbps: number | null;  // autobench: GB/s of int8 weights (null = not measured)
  rttMs: number | null;      // rpc round trip measured at connect
  speed: number;             // effective speed used by the split: bench (or estimate) × weight
  speedMeasured: boolean;    // speed comes from a real measurement (false = estimate)
  role: NodeRole;            // chosen by the user
  assigned: NodeAssigned;    // what the current plan does with it
  helper: HelperView | null; // «Помощник» status (experiment «Роли»)
  // ---- 0.3.3 (eng-core): device temperature
  tempC?: number | null;     // °C (battery/SoC for phones), null = unknown
  throttled?: boolean;       // the device slows down from heat («греется»)
}

export type NodeRole = "auto" | "model" | "free" | "helper";
export type NodeAssigned = "model" | "free" | "helper" | "off" | "";

/** helper.rs HelperView */
// helper.rs emits prepare/pc-download/node-download/loading/ready/error; "downloading"/"starting" accepted too (v032-api.md)
export type HelperState = "prepare" | "pc-download" | "node-download" | "downloading" | "loading" | "starting" | "ready" | "error" | "";
export interface HelperView { id: string; name: string; model: string; state: HelperState; detail: string; pct: number | null; ready: boolean }
export interface HelperModelView { id: string; label: string; sizeMb: number }
/** helper.rs HelperReply */
export interface HelperReply { content: string; node: string; nodeName: string; model: string; tps: number | null; tokens: number | null; ms: number }
/** `helper-stream` event payload */
export interface HelperDelta { streamId: string; delta: string }

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
  warming: boolean; // 0.3.2: first tiny generation after load («Прогрев…»)
  toast?: string;
}

export interface SplitEntry {
  id: string; device: string; name: string; share: number;
  cpu?: boolean;     // 0.3.3: the PC CPU pseudo-node (id "cpu"), not passed to --device
  thermal?: number;  // 0.3.3: speed factor from temperature (1 normal, 0.8 hot, 0.6 throttled)
}

/** split.rs Plan (plan_split) */
export interface SplitPlan {
  entries: SplitEntry[];
  rpcIds: string[];
  freeIds: string[];
  estTps: number | null;
  fits: boolean;
  needMb: number | null;
  // ---- 0.3.3 (eng-core)
  gpuOnly?: boolean;            // the whole model sits in the video card(s): «Только видеокарта»
  cpuLayers?: number | null;    // layers computed by the PC processor («Процессор ПК: N слоёв»)
  threads?: number | null;      // CPU threads for those layers
  nLayers?: number | null;      // block count from the GGUF header (when known)
  draftMb?: number;             // memory reserved on the GPU for the draft model (0 = none)
  draft?: boolean;              // draft model will be used
  moe?: SplitMoe | null;        // MoE: attention+KV on GPU, expert tensors of some blocks on nodes («Эксперты на телефонах»)
}
/** split.rs MoePlan (see /workspace/work/eng-api.md) */
export interface SplitMoe {
  expertCount: number; expertUsed: number; blockCount: number; expertsMb: number; denseMb: number;
  gpuBlocks: number[];
  offload: { id: string; name: string; target: string; blocks: number[]; mb: number }[];
}

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
  // ---- 0.3.2
  needMb: number | null;         // model size the split was planned for (running / last model)
  freeNodes: string[];           // online nodes the model doesn't need («Свободна»)
  estTps: number | null;         // rough tok/s estimate for the planned split
  fits: boolean;
  rolesEnabled: boolean;         // experiment «Роли»
  helperModel: string;           // id from helperModels or "owner/repo/file.gguf"
  helperModels: HelperModelView[];
  helpers: HelperView[];         // nodes with the «Помощник» role (any state)
  // ---- 0.3.3 (eng-core)
  cpu?: { brand: string; threads: number; gbps?: number | null; usableMb: number; benching: boolean };
  plan?: SplitPlan;              // full plan for the last model
  draftEnabled?: boolean;
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
  nLayers?: number;    // transformer blocks (for «Процессор ПК: N слоёв» and layer math)
  experts?: number;    // MoE: experts per layer
  expertsActive?: number; // MoE: experts used per token
  phoneVariant?: string;  // id of the Q4_0 twin (faster on phone CPUs)
  baseId?: string;        // for a Q4_0 twin: id of the curated model it replaces
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
  /** user-attached photos (JPEG data URIs, ≤1024px); not persisted */
  images?: string[];
  /** model that answered (routing label) */
  model?: string;
  /** «Искать в моих файлах»: sources of the chunks given to the model */
  sources?: string[];
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
  /** 0.3.2: the title was already asked from a «Помощник» (done once per chat) */
  autoTitled?: boolean;
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

// ---------------------------------------------------------------- «Возможности» (features.rs, argus-features.json)
export interface FeaturesConfig {
  webchat: { enabled: boolean; pin: string };
  telegram: { enabled: boolean; token: string; allowedIds: number[] };
  routing: { simpleToHelper: boolean };
  rag: { enabled: boolean; folders: string[]; topK?: number | null };
  library: { enabled: boolean; url: string };
  voice: { enabled: boolean; url: string };
  lang: "ru" | "en";
  /** fields owned by other features (queue, media…) round-trip untouched */
  [extra: string]: unknown;
}

export interface FeaturesView extends FeaturesConfig {
  lanIp: string | null;
  webchatPort: number;
  webchatUrl: string | null;
  webchatRunning: boolean;
  webchatError: string;
  telegramStatus: string;
}

/** route_target: which OpenAI-compatible backend answers a prompt */
export interface RouteTarget { base: string; model: string; kind: "main" | "helper" }
