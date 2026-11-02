// Thin bridge to the Rust side. Falls back to mock data when opened in a plain browser (npm run dev).
import type {
  ClusterNode, ClusterSnapshot, DiscoveredNode, EngineView, HelperDelta, HelperReply, HelperView, ModelEntry, NodeRole, SplitEntry, SplitPlan,
  UpdateProgress, UpdateStatus,
} from "./types";

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export const LLAMA_URL = "http://127.0.0.1:8080";

async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function onClusterUpdate(cb: (s: ClusterSnapshot) => void): Promise<() => void> {
  if (!isTauri) {
    cb(mockSnapshot());
    const t = setInterval(() => cb(mockSnapshot()), 1500);
    return () => clearInterval(t);
  }
  const { listen } = await import("@tauri-apps/api/event");
  cb(await tauriInvoke<ClusterSnapshot>("get_snapshot"));
  return listen<ClusterSnapshot>("cluster-update", (e) => cb(e.payload));
}

export async function onLlamaLog(cb: (line: string) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<string>("llama-log", (e) => cb(e.payload));
}

export async function onDownload(cb: (bytes: number) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<number>("llama-dl", (e) => cb(e.payload));
}

export async function onEngineState(cb: (e: EngineView) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<EngineView>("engine-state", (e) => cb(e.payload));
}

export interface Toast { kind: "info" | "warn" | "error" | "ok"; text: string }
export async function onToast(cb: (t: Toast) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<Toast>("argus-toast", (e) => cb(e.payload));
}

// ---- running generations: aborted with a clear reason when the engine dies or loses a node
/** Abort reason used when the engine (not the user) ended the request. */
export class EngineDownError extends Error {
  constructor(message: string) { super(message); this.name = "EngineDownError"; }
}
const generations = new Set<AbortController>();
export function trackGeneration(c: AbortController): () => void {
  generations.add(c);
  return () => { generations.delete(c); };
}
export function abortGenerations(reason: string) {
  for (const c of generations) { try { c.abort(new EngineDownError(reason)); } catch { c.abort(); } }
  generations.clear();
}
/** Human-readable Russian text for a failed chat request. */
export function chatErrorText(e: unknown, signal?: AbortSignal): string | null {
  const r = signal?.reason;
  if (r instanceof EngineDownError) return `Генерация прервана: ${r.message}`;
  if (e instanceof EngineDownError) return `Генерация прервана: ${e.message}`;
  const err = e as Error;
  if (err?.name === "AbortError") return null; // the user pressed Stop
  const m = String(err?.message ?? e);
  if (err instanceof TypeError || /Failed to fetch|NetworkError|Load failed|network error|ECONNREFUSED/i.test(m))
    return "Нет связи с движком (llama-server не отвечает) — возможно, он остановился. Подробности в журнале движка (Models).";
  return m;
}

// ---- self-update (0.3.1): every install starts from a user click
export async function onUpdateAvailable(cb: (s: UpdateStatus) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<UpdateStatus>("update-available", (e) => cb(e.payload));
}
export async function onUpdateProgress(cb: (p: UpdateProgress) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<UpdateProgress>("update-progress", (e) => cb(e.payload));
}
const mockUpdate = (): UpdateStatus => ({
  current: "0.3.1", installKind: "nsis", available: true, skipped: false, checkEnabled: true, checking: false,
  lastCheck: Math.floor(Date.now() / 1000), error: null, busy: false, phase: "", done: 0, total: 0, nodeServerError: "",
  latest: { version: "0.3.2", tag: "v0.3.2", name: "Argus 0.3.2", url: "https://github.com/femboy1488/Project-argus/releases",
    publishedAt: new Date().toISOString(), hasSums: true, asset: { name: "Argus-Setup.exe", url: "", size: 46_000_000 },
    notes: "## Что нового\n- Пример заметок к релизу\n- **markdown** поддерживается" },
});
export const updateStatus = () => (isTauri ? tauriInvoke<UpdateStatus>("update_status") : Promise.resolve(mockUpdate()));
export const checkUpdates = () => (isTauri ? tauriInvoke<UpdateStatus>("check_updates") : Promise.resolve(mockUpdate()));
export const setUpdateCheck = (enabled: boolean) => (isTauri ? tauriInvoke<void>("set_update_check", { enabled }) : Promise.resolve());
export const skipUpdateVersion = (version: string) => (isTauri ? tauriInvoke<void>("skip_update_version", { version }) : Promise.resolve());
/** "installing" (the app exits and the installer relaunches it) | "opened" (release page opened) */
export const installUpdate = () => (isTauri ? tauriInvoke<string>("install_update") : Promise.resolve("opened"));
export const updateUsbNode = (id: string) => (isTauri ? tauriInvoke<void>("update_usb_node", { id }) : Promise.resolve());
export const skipNodeUpdate = (version: string) => (isTauri ? tauriInvoke<void>("skip_node_update", { version }) : Promise.resolve());
export const wifiNodeUpdate = (id: string) => (isTauri ? tauriInvoke<string>("wifi_node_update", { id }) : Promise.resolve("Запрос отправлен (mock)"));

export const rescan = () => (isTauri ? tauriInvoke<void>("rescan") : Promise.resolve());
export const reconnectNode = (id: string) => (isTauri ? tauriInvoke<void>("reconnect_node", { id }) : Promise.resolve());
export const restartEngine = () => (isTauri ? tauriInvoke<string>("restart_engine") : Promise.resolve(LLAMA_URL));
export const addDiscoveredNode = (id: string) =>
  isTauri ? tauriInvoke<string>("add_discovered_node", { id }) : (mockAddLan(id.replace(/^lan:/, ""), mock.found.find((d) => d.id === id)?.name), Promise.resolve(id));
export const setAutoAdd = (enabled: boolean) => (isTauri ? tauriInvoke<void>("set_auto_add", { enabled }) : (mock.autoAdd = enabled, Promise.resolve()));

export interface PlatformInfo { os: "windows" | "linux" | "macos"; shell: string; screen: boolean }
export const platformInfo = (): Promise<PlatformInfo> =>
  isTauri ? tauriInvoke<PlatformInfo>("platform_info") : Promise.resolve({ os: "windows", shell: "PowerShell", screen: true });

export interface StartOpts {
  ctx?: number; lan?: boolean; needMb?: number;
  draftHf?: string; draftMax?: number; cacheReuse?: number; keepLoaded?: boolean;
  /** 0.3.2: system prompt of the current chat, processed once right after load («Прогрев…») */
  warmupSystem?: string;
}

export async function startModel(m: ModelEntry, opts: StartOpts = {}) {
  if (!isTauri) {
    mock.engine = { status: "ready", alias: m.id, warmUntil: Date.now() + 6000 };
    if (opts.needMb) mock.needMb = opts.needMb;
    return LLAMA_URL;
  }
  return tauriInvoke<string>("start_model", { args: {
    hfRepo: m.hfRepo, hfFile: m.hfFile ?? null, ctx: opts.ctx ?? 8192, engine: "llama.cpp", alias: m.id, lan: !!opts.lan,
    needMb: opts.needMb ?? null, draftHf: opts.draftHf ?? null, draftMax: opts.draftMax ?? null,
    cacheReuse: opts.cacheReuse ?? null, keepLoaded: !!opts.keepLoaded,
    warmupSystem: opts.warmupSystem?.trim() ? opts.warmupSystem : null,
  } });
}

export const stopModel = () => (isTauri ? tauriInvoke<void>("stop_model") : (mock.engine = { status: "idle", alias: null, warmUntil: 0 }, Promise.resolve()));

// ---- 0.3.2: speed test, roles, split plan, «Помощник»
export const setNodeRole = (id: string, role: NodeRole) =>
  isTauri ? tauriInvoke<void>("set_node_role", { id, role }) : mockSetRole(id, role);
/** «Повторить тест» (USB nodes). Resolves with the new GB/s. */
export const rebenchNode = (id: string) =>
  isTauri ? tauriInvoke<number>("rebench_node", { id })
    : new Promise<number>((r) => setTimeout(() => { const g = +(6 + Math.random() * 14).toFixed(1); mock.bench[id] = g; r(g); }, 1800));
/** Split preview for a model of `needMb` (weights + context). */
export const planSplit = (needMb: number | null) =>
  isTauri ? tauriInvoke<SplitPlan>("plan_split", { needMb }) : Promise.resolve(mockPlanExtras(mockPlan(needMb)));
export const setRolesEnabled = (enabled: boolean) =>
  isTauri ? tauriInvoke<void>("set_roles_enabled", { enabled }) : (mock.rolesEnabled = enabled, Promise.resolve());
export const setHelperModel = (model: string) =>
  isTauri ? tauriInvoke<void>("set_helper_model", { model }) : mockSetHelperModel(model);
export const retryHelper = (id: string) =>
  isTauri ? tauriInvoke<void>("retry_helper", { id }) : (mock.helperSince[id] = Date.now(), Promise.resolve());

export interface HelperChatOpts {
  /** a specific helper (Параллельный тест); omitted = the least busy ready one */
  node?: string;
  maxTokens?: number;
  temperature?: number;
  /** pieces of the answer arrive as `helper-stream` events with this id */
  streamId?: string;
}
export const helperChat = (messages: ApiMessage[], o: HelperChatOpts = {}) =>
  isTauri ? tauriInvoke<HelperReply>("helper_chat", {
    node: o.node ?? null, messages: normalizeMessages(messages), maxTokens: o.maxTokens ?? null, streamId: o.streamId ?? null, temperature: o.temperature ?? null,
  }) : mockHelperChat(messages, o);
export const helperCancel = (streamId: string) =>
  isTauri ? tauriInvoke<void>("helper_cancel", { streamId }) : (mock.cancelled.add(streamId), Promise.resolve());

const mockStreamListeners = new Set<(d: HelperDelta) => void>();
/** `helper-stream`: {streamId, delta} pieces of a streaming helper answer. */
export async function onHelperStream(cb: (d: HelperDelta) => void): Promise<() => void> {
  if (!isTauri) { mockStreamListeners.add(cb); return () => { mockStreamListeners.delete(cb); }; }
  const { listen } = await import("@tauri-apps/api/event");
  return listen<HelperDelta>("helper-stream", (e) => cb(e.payload));
}

/** Ready helpers of a snapshot (only while the experiment is on). */
export const readyHelpers = (s: ClusterSnapshot | null): HelperView[] => (s?.rolesEnabled ? s.helpers.filter((h) => h.ready) : []);

/** Removes Qwen3 `<think>…</think>` blocks. */
export function stripThink(t: string): string {
  let out = t;
  for (;;) {
    const a = out.indexOf("<think>");
    if (a < 0) break;
    const b = out.indexOf("</think>", a);
    if (b < 0) { out = out.slice(0, a); break; }
    out = out.slice(0, a) + out.slice(b + 8);
  }
  return out.trim();
}

// ---- 0.3.3 (eng-core commands): chat KV sessions, shareable bench results, draft toggle
/** Saves the engine's KV cache for this chat (fast resume later). Resolves with the saved file name; rejects when no engine. */
export const saveChatSession = (chatId: string) =>
  isTauri ? tauriInvoke<string>("save_chat_session", { chatId }) : Promise.resolve(`${chatId}.bin`);
/** Restores a saved KV cache for this chat; false when nothing was saved. */
export const restoreChatSession = (chatId: string) =>
  isTauri ? tauriInvoke<boolean>("restore_chat_session", { chatId }) : Promise.resolve(false);
/** Plain-text report of device speed tests (for «Поделиться результатами»). */
export const exportBenchResults = () =>
  isTauri ? tauriInvoke<string>("export_bench_results") : Promise.resolve(mockBenchReport());
/** «Ускорение черновиком»: speculative decoding on/off (Rust picks the draft by model family). */
export const setDraftEnabled = (enabled: boolean, path?: string | null) =>
  isTauri ? tauriInvoke<void>("set_draft_enabled", { enabled, path: path ?? null }) : Promise.resolve();

function mockBenchReport(): string {
  const s = mockSnapshot();
  const rows = [...s.gpus, ...s.nodes].map((n) =>
    `- ${n.name} (${n.kind}, ${n.transport}): ${n.benchGbps != null ? `${n.benchGbps.toFixed(1)} ГБ/с` : "не измерено"}${n.rttMs != null ? `, RTT ${n.rttMs} мс` : ""}, ${gbS(n.usableMb)} ГБ`);
  return [`## Argus ${s.appVersion} — результаты теста скорости (демо)`, ...rows].join("\n");
}
const gbS = (mb: number) => (mb / 1024).toFixed(1);

/** mock: 0.3.3 plan fields (CPU layers, MoE experts on phones, gpu-only). */
function mockPlanExtras(p: SplitPlan): SplitPlan {
  const gpuOnly = p.rpcIds.length === 0 && p.fits;
  return { ...p, gpuOnly, cpuLayers: gpuOnly ? 0 : 4, threads: 6, moe: null, draft: false, draftMb: 0 };
}
/** mock: temperatures (Note10+ runs hot). */
const MOCK_TEMP: Record<string, number> = { R52T1234: 36.5, R58N9ABC: 44.2, RZ8R40XYZ: 33.0 };

export const modelStatus = () => (isTauri ? tauriInvoke<boolean>("model_status") : Promise.resolve(true));

// ---- nodes / settings
export const addLanNode = (address: string, name?: string) =>
  isTauri ? tauriInvoke<string>("add_lan_node", { address, name: name ?? null }) : mockAddLan(address, name);
export const removeLanNode = (id: string) =>
  isTauri ? tauriInvoke<void>("remove_lan_node", { id }) : (mock.lan = mock.lan.filter((n) => n.id !== id), Promise.resolve());
export const testLanNode = (address: string) =>
  isTauri ? tauriInvoke<string>("test_lan_node", { address }) : Promise.resolve("rpc v8.0.0 · 1 устр. · 5.8 / 7.6 GB свободно (демо)");
export const setNodeWeight = (id: string, weight: number) =>
  isTauri ? tauriInvoke<void>("set_node_weight", { id, weight }) : (mock.weights[id] = weight, Promise.resolve());
export const setNodeEnabled = (id: string, enabled: boolean) =>
  isTauri ? tauriInvoke<void>("set_node_enabled", { id, enabled }) : (mock.disabled[id] = !enabled, Promise.resolve());
export const setKeepLoaded = (enabled: boolean) =>
  isTauri ? tauriInvoke<void>("set_keep_loaded", { enabled }) : Promise.resolve();
export const setFastStart = (enabled: boolean) =>
  isTauri ? tauriInvoke<void>("set_fast_start", { enabled }) : Promise.resolve();

/** Opens a URL in the default browser (Rust side: rundll32/cmd start). */
export async function openExternal(url: string) {
  if (isTauri) return tauriInvoke<void>("open_external", { target: url });
  window.open(url, "_blank", "noopener");
}

// ---- agent tools (Rust commands). Each returns text for the model; screenshot also returns an image.
export interface ToolOutput { text: string; image?: string }

export async function runTool(name: string, args: Record<string, unknown>): Promise<ToolOutput> {
  if (!isTauri) return mockTool(name, args);
  switch (name) {
    case "run_command": {
      const r = await tauriInvoke<{ code: number | null; stdout: string; stderr: string; timedOut: boolean }>(
        "agent_run_command", { command: String(args.command ?? ""), cwd: (args.cwd as string) ?? null, timeoutSec: (args.timeout_sec as number) ?? null });
      return { text: `exit=${r.code ?? "?"}${r.timedOut ? " (таймаут)" : ""}\n--- stdout ---\n${r.stdout}${r.stderr ? `\n--- stderr ---\n${r.stderr}` : ""}` };
    }
    case "read_file": return { text: await tauriInvoke<string>("agent_read_file", { path: String(args.path ?? ""), maxBytes: null }) };
    case "write_file": return { text: await tauriInvoke<string>("agent_write_file", { path: String(args.path ?? ""), content: String(args.content ?? "") }) };
    case "list_dir": return { text: await tauriInvoke<string>("agent_list_dir", { path: String(args.path ?? ".") }) };
    case "open_url":
    case "open_app": await tauriInvoke<void>("open_external", { target: String(args.target ?? args.url ?? args.app ?? "") }); return { text: "открыто" };
    case "screenshot": {
      const s = await tauriInvoke<{ pngBase64: string; width: number; height: number; screenWidth: number; screenHeight: number }>("agent_screenshot", { maxWidth: 1280 });
      return { text: `скриншот ${s.width}x${s.height} (экран ${s.screenWidth}x${s.screenHeight}); координаты мыши — в пикселях этого изображения`,
               image: `data:image/png;base64,${s.pngBase64}` };
    }
    case "mouse_click":
      return { text: await tauriInvoke<string>("agent_mouse", { action: String(args.button === "right" ? "right_click" : args.double ? "double_click" : "click"), x: Number(args.x), y: Number(args.y) }) };
    case "mouse_move": return { text: await tauriInvoke<string>("agent_mouse", { action: "move", x: Number(args.x), y: Number(args.y) }) };
    case "type_text": return { text: await tauriInvoke<string>("agent_type_text", { text: String(args.text ?? "") }) };
    case "key_press": return { text: await tauriInvoke<string>("agent_key_press", { keys: String(args.keys ?? "") }) };
    case "search_files": {
      const hits = await ragSearch(String(args.query ?? ""), Number(args.k ?? 5));
      return { text: hits.length ? hits.map((h, i) => `[${i + 1}] ${h.source} (score ${h.score.toFixed(2)})\n${h.text}`).join("\n\n") : "ничего не найдено" };
    }
    case "wiki_search": {
      const hits = await kiwixSearch(String(args.query ?? ""));
      return { text: hits.length ? hits.map((h) => `${h.title} — path: ${h.path}${h.snippet ? `\n  ${h.snippet}` : ""}`).join("\n") : "ничего не найдено" };
    }
    case "wiki_read": return { text: await kiwixRead(String(args.path ?? "")) };
    default: throw new Error(`неизвестный инструмент: ${name}`);
  }
}

/** Resolves when llama-server's /health returns 200 (it returns 503 while loading weights).
 *  Rejects as soon as the engine process dies, instead of spinning until the timeout. */
export async function waitHealthy(timeoutMs = 60 * 60_000) {
  if (!isTauri) return new Promise<void>((r) => setTimeout(r, 1200)); // browser demo
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    if (!(await modelStatus())) throw new Error("Движок остановился при загрузке — причина в журнале движка ниже");
    try {
      const r = await fetch(`${LLAMA_URL}/health`);
      if (r.ok) return;
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error("Модель не загрузилась за 60 минут");
}

/** Is an engine already serving (kept loaded from a previous session)? Returns the model alias. */
export async function probeRunningEngine(): Promise<string | null> {
  if (!isTauri) return null;
  try {
    const h = await fetch(`${LLAMA_URL}/health`);
    if (!h.ok) return null;
    const j = await (await fetch(`${LLAMA_URL}/v1/models`)).json();
    return j?.data?.[0]?.id ?? "argus";
  } catch { return null; }
}

/** llama-server /props: modalities.vision is true when an mmproj is loaded. */
export async function engineHasVision(): Promise<boolean> {
  if (!isTauri) return true;
  try { const j = await (await fetch(`${LLAMA_URL}/props`)).json(); return !!j?.modalities?.vision; } catch { return false; }
}

/** OpenAI-compatible streaming chat against llama-server. */
export interface ChatStats { tps: number; promptTps?: number; tokens: number; ttftMs?: number; final?: boolean }
export type ApiMessage = { role: string; content: unknown; tool_calls?: unknown; tool_call_id?: string; name?: string };

/** Many chat templates (Mistral, Gemma, Llama 2…) throw "Conversation roles must alternate user/assistant".
 *  Make the history safe for any template: one system message first, no empty turns, no two user/assistant
 *  turns in a row (merged), and the first turn after system is a user turn. Tool turns are left as they are. */
export function normalizeMessages(input: ApiMessage[]): ApiMessage[] {
  const isEmpty = (c: unknown) => c == null || (typeof c === "string" && !c.trim()) || (Array.isArray(c) && c.length === 0);
  const toParts = (c: unknown): unknown[] => Array.isArray(c) ? c : [{ type: "text", text: String(c ?? "") }];
  const join = (a: unknown, b: unknown): unknown =>
    typeof a === "string" && typeof b === "string" ? `${a}\n\n${b}` : [...toParts(a), ...toParts(b)];
  const sys: string[] = [];
  const out: ApiMessage[] = [];
  for (const m of input) {
    if (m.role === "system") { const t = typeof m.content === "string" ? m.content : JSON.stringify(m.content); if (t.trim()) sys.push(t); continue; }
    const plain = (m.role === "user" || m.role === "assistant") && !m.tool_calls && !m.tool_call_id;
    if (plain && isEmpty(m.content)) continue;
    const prev = out[out.length - 1];
    if (plain && prev && prev.role === m.role && !prev.tool_calls && !prev.tool_call_id) { prev.content = join(prev.content, m.content); continue; }
    out.push({ ...m });
  }
  while (out.length && out[0].role !== "user") out.shift();
  return [...(sys.length ? [{ role: "system", content: sys.join("\n\n") }] : []), ...out];
}

export async function* streamChat(messages: ApiMessage[], signal?: AbortSignal, onStats?: (s: ChatStats) => void, maxTokens?: number, base: string = LLAMA_URL) {
  if (!isTauri) { // browser demo
    const demo = "Слои 0–15 лежат в VRAM видеокарты, слои 16–31 считает телефон через rpc-server по USB.\n\n"
      + "```python\nfrom openai import OpenAI\nc = OpenAI(base_url=\"http://127.0.0.1:8080/v1\", api_key=\"argus\")\n```\n"
      + "Каждый токен пересекает кабель **дважды** как небольшой тензор активаций, поэтому канал далёк от насыщения.";
    let n = 0; for (const w of demo.split(/(?<= )/)) { if (signal?.aborted || (maxTokens && n >= maxTokens)) return; await new Promise((r) => setTimeout(r, 20)); n++; onStats?.({ tps: 7.4, promptTps: 41.2, tokens: n, ttftMs: 820 }); yield w; }
    return;
  }
  const res = await fetch(`${base.replace(/\/+$/, "")}/v1/chat/completions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ messages: normalizeMessages(messages), stream: true, timings_per_token: true, ...(maxTokens ? { max_tokens: maxTokens } : {}) }),
    signal,
  });
  if (!res.ok || !res.body) throw new Error(`llama-server: HTTP ${res.status} ${await res.text().catch(() => "")}`);
  // a reader stuck on a dead socket must end as soon as the request is aborted
  signal?.addEventListener("abort", () => { res.body?.cancel(signal.reason).catch(() => {}); }, { once: true });
  const reader = res.body.getReader();
  const dec = new TextDecoder();
  let buf = "";
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += dec.decode(value, { stream: true });
    const lines = buf.split("\n");
    buf = lines.pop() ?? "";
    for (const line of lines) {
      const l = line.trim();
      if (!l.startsWith("data:")) continue;
      const data = l.slice(5).trim();
      if (data === "[DONE]") return;
      try {
        const j = JSON.parse(data);
        const delta = j.choices?.[0]?.delta?.content;
        const t = j.timings;
        if (t && onStats) onStats({ tps: t.predicted_per_second ?? 0, promptTps: t.prompt_per_second,
          tokens: t.predicted_n ?? 0, ttftMs: t.prompt_ms, final: j.choices?.[0]?.finish_reason != null });
        if (delta) yield delta as string;
      } catch { /* partial line */ }
    }
  }
}

export interface Completion {
  content: string;
  toolCalls: { id: string; name: string; arguments: string }[];
  stats?: ChatStats;
}

/** Non-streaming call with OpenAI tools (agent mode). llama-server parses tool calls with --jinja. */
export async function chatWithTools(messages: ApiMessage[], tools: unknown[], signal?: AbortSignal): Promise<Completion> {
  if (!isTauri) return mockAgentTurn(messages);
  const t0 = performance.now();
  const res = await fetch(`${LLAMA_URL}/v1/chat/completions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ messages: normalizeMessages(messages), tools, tool_choice: "auto", stream: false, parse_tool_calls: true }),
    signal,
  });
  if (!res.ok) throw new Error(`llama-server: HTTP ${res.status} ${await res.text().catch(() => "")}`);
  const j = await res.json();
  const msg = j.choices?.[0]?.message ?? {};
  const t = j.timings;
  return {
    content: msg.content ?? "",
    toolCalls: (msg.tool_calls ?? []).map((c: { id?: string; function: { name: string; arguments: string | object } }, i: number) => ({
      id: c.id ?? `call_${i}_${Date.now()}`, name: c.function.name,
      arguments: typeof c.function.arguments === "string" ? c.function.arguments : JSON.stringify(c.function.arguments),
    })),
    stats: t ? { tps: t.predicted_per_second ?? 0, promptTps: t.prompt_per_second, tokens: t.predicted_n ?? 0, ttftMs: t.prompt_ms ?? performance.now() - t0, final: true } : undefined,
  };
}

// ---------------------------------------------------------------- browser mocks

const mock = {
  found: [
    { id: "lan:192.168.1.77:50052", endpoint: "192.168.1.77:50052", name: "Pixel 7 (Wi-Fi)", ramMb: 7600, freeMb: 3900, variant: "i8mm", ageSec: 1, added: false, appVersion: "0.3", appVc: 4, outdated: true },
    { id: "lan:192.168.1.30:50052", endpoint: "192.168.1.30:50052", name: "raspberrypi 5", ramMb: 8000, freeMb: 7100, variant: "aarch64", ageSec: 0, added: false, appVersion: null, appVc: null, outdated: false },
  ] as DiscoveredNode[],
  autoAdd: false,
  lan: [] as ClusterNode[],
  weights: {} as Record<string, number>,
  disabled: {} as Record<string, boolean>,
  // 0.3.2
  roles: { R58N9ABC: "helper" } as Record<string, NodeRole>,
  bench: { R52T1234: 18.2, R58N9ABC: 9.1 } as Record<string, number>,
  rolesEnabled: true,
  helperModel: "qwen3-0.6b",
  /** when a node got the «Помощник» role (drives the fake prepare → ready progression); 0 = long ago */
  helperSince: { R58N9ABC: 0 } as Record<string, number>,
  cancelled: new Set<string>(),
  needMb: 4900 as number | null,
  engine: { status: "idle", alias: null, warmUntil: 0 } as { status: "idle" | "ready"; alias: string | null; warmUntil: number },
};

const MOCK_HELPER_MODELS = [
  { id: "qwen3-0.6b", label: "Qwen3 0.6B · 0.4 ГБ · быстрый", sizeMb: 364 },
  { id: "qwen3-1.7b", label: "Qwen3 1.7B · 1.1 ГБ · умнее", sizeMb: 1007 },
  { id: "qwen3-4b-2507", label: "Qwen3 4B Instruct 2507 · 2.4 ГБ · для 8+ ГБ ОЗУ", sizeMb: 2265 },
];

function mockSetRole(id: string, role: NodeRole) {
  if (role === "helper" && !mock.rolesEnabled) return Promise.reject("Роль «Помощник» доступна, когда включён эксперимент «Роли»");
  if (role === "auto") delete mock.roles[id]; else mock.roles[id] = role;
  if (role === "helper") mock.helperSince[id] = Date.now();
  return Promise.resolve();
}

function mockSetHelperModel(model: string) {
  const m = model.trim();
  const custom = m.split("/").filter(Boolean);
  if (!MOCK_HELPER_MODELS.some((x) => x.id === m) && !(custom.length >= 3 && m.toLowerCase().endsWith(".gguf")))
    return Promise.reject("Укажите модель из списка или путь вида owner/repo/Файл-Q4_0.gguf");
  mock.helperModel = m;
  for (const id of Object.keys(mock.roles)) if (mock.roles[id] === "helper") mock.helperSince[id] = Date.now();
  return Promise.resolve();
}

const mockHelperLabel = () => MOCK_HELPER_MODELS.find((x) => x.id === mock.helperModel)?.label.split(" · ")[0] ?? mock.helperModel.split("/").pop()!.replace(/\.gguf$/i, "");

function mockHelperView(n: ClusterNode): HelperView {
  const t = mock.helperSince[n.id] ? (Date.now() - mock.helperSince[n.id]) / 1000 : 999;
  const base = { id: n.id, name: n.name, model: mockHelperLabel(), pct: null as number | null, ready: false };
  if (t < 4) return { ...base, state: "pc-download", detail: "Качаю модель на ПК", pct: Math.round((t / 4) * 100) };
  if (t < 8) return { ...base, state: "node-download", detail: "Передаю модель на ноду", pct: Math.round(((t - 4) / 4) * 100) };
  if (t < 10) return { ...base, state: "loading", detail: "Загружает модель в память" };
  return { ...base, state: "ready", detail: "Готов", ready: true };
}

/** Same rules as split.rs, simplified: helpers/free never hold layers, GPU + forced first, then fastest «Авто». */
function mockPlan(needMb: number | null, snap?: { gpus: ClusterNode[]; nodes: ClusterNode[] }): SplitPlan {
  const s = snap ?? mockNodes();
  const role = (n: ClusterNode): NodeRole => (n.role === "helper" && !mock.rolesEnabled ? "auto" : n.role);
  const cand = s.nodes.filter((n) => n.stage === "connected" && n.enabled && n.endpoint && role(n) !== "free" && role(n) !== "helper");
  const gpus = s.gpus.filter((g) => g.enabled && g.device);
  let chosen = cand.filter((n) => role(n) === "model");
  if (needMb) {
    let have = gpus.reduce((a, g) => a + g.usableMb, 0) + chosen.reduce((a, n) => a + n.usableMb, 0);
    for (const n of cand.filter((x) => role(x) === "auto").sort((a, b) => b.speed - a.speed)) {
      if (have >= needMb) break;
      chosen.push(n); have += n.usableMb;
    }
  } else chosen = cand;
  chosen = cand.filter((n) => chosen.includes(n)); // keep --rpc order
  const devs = [...chosen.map((n, i) => ({ n, device: `RPC${i}` })), ...gpus.map((g) => ({ n: g, device: g.device! }))];
  const total = devs.reduce((a, d) => a + d.n.usableMb, 0);
  const need = needMb ?? total * 0.6;
  const fits = !needMb || needMb <= total;
  const amt = new Map<string, number>();
  let left = need;
  for (const d of [...devs].sort((a, b) => b.n.speed - a.n.speed)) { const take = Math.min(d.n.usableMb, Math.max(0, left)); amt.set(d.n.id, take); left -= take; }
  for (const d of devs) if (!amt.get(d.n.id) && d.n.transport !== "local") amt.set(d.n.id, need * 0.04);
  const sum = [...amt.values()].reduce((a, x) => a + x, 0) || 1;
  const entries: SplitEntry[] = devs.map((d) => ({ id: d.n.id, device: d.device, name: d.n.name, share: (amt.get(d.n.id) ?? 0) / sum }));
  let ms = 0;
  for (const d of devs) { ms += ((amt.get(d.n.id) ?? 0) / 1024 / d.n.speed) * 1000; if (d.n.transport !== "local") ms += 4 * (d.n.rttMs ?? 2); }
  const freeIds = cand.filter((n) => !chosen.includes(n)).map((n) => n.id);
  return { entries, rpcIds: chosen.map((n) => n.id), freeIds, estTps: needMb && fits && ms > 0 ? 1000 / ms : null, fits, needMb };
}

function mockHelperChat(messages: ApiMessage[], o: HelperChatOpts): Promise<HelperReply> {
  if (!mock.rolesEnabled) return Promise.reject("Эксперимент «Роли» выключен");
  const { nodes } = mockNodes();
  const ready = nodes.filter((n) => n.helper?.ready);
  const n = o.node ? ready.find((x) => x.id === o.node) : ready[0];
  if (!n) return Promise.reject("Нет готового Помощника");
  const sys = String(messages.find((m) => m.role === "system")?.content ?? "");
  const text = /назван/i.test(sys) ? "Распределение слоёв по USB"
    : /сводк|кратко/i.test(sys) ? "- Обсуждали, как слои модели делятся между видеокартой и телефоном.\n- Токен проходит кабель дважды, канал не перегружен.\n- Для подключения нужен OpenAI-совместимый клиент на 127.0.0.1:8080."
    : `Это ответ Помощника «${n.name}». Маленькая модель отвечает быстро, но проще: для заголовков, сводок и черновиков этого хватает, а сложные вопросы лучше оставить основной модели.`;
  const tps = n.id === "R58N9ABC" ? 21.4 : 14.8;
  const words = text.split(/(?<= )/);
  const t0 = Date.now();
  return new Promise((resolve) => {
    let i = 0;
    const tick = () => {
      const stop = o.streamId && mock.cancelled.has(o.streamId);
      if (i >= words.length || stop) {
        if (o.streamId) mock.cancelled.delete(o.streamId);
        resolve({ content: words.slice(0, i).join("").trim(), node: n.id, nodeName: n.name, model: mockHelperLabel(), tps, tokens: i, ms: Date.now() - t0 });
        return;
      }
      if (o.streamId) mockStreamListeners.forEach((cb) => cb({ streamId: o.streamId!, delta: words[i] }));
      i++;
      setTimeout(tick, 1000 / tps);
    };
    setTimeout(tick, 250);
  });
}

const base = (o: Partial<ClusterNode> & Pick<ClusterNode, "id" | "name" | "kind">): ClusterNode => ({
  stage: "connected", transport: "usb", endpoint: null, totalMb: 0, availableMb: 0, usableMb: 0, loadPct: 0, error: null,
  weight: 1, enabled: true, device: null, version: null, rpcDevices: 1, integrated: false,
  attempts: 0, retryIn: null, variant: null, beacon: false, inEngine: false,
  appVc: null, appVersion: null, updateAvailable: null, updateSkipped: false,
  benchGbps: null, rttMs: null, speed: 4, speedMeasured: false, role: "auto", assigned: "", helper: null, ...o,
});

/** fills the 0.3.2 speed/role fields the way state.rs does */
function mockSpeed(n: ClusterNode, est: number): ClusterNode {
  const b = mock.bench[n.id] ?? n.benchGbps ?? null;
  return { ...n, benchGbps: b, speedMeasured: b != null, speed: (b ?? est) * n.weight,
    role: n.transport === "local" ? "auto" : mock.roles[n.id] ?? "auto" };
}

function mockAddLan(address: string, name?: string) {
  const [host, port = "50052"] = address.split(":");
  const id = `lan:${host}:${port}`;
  if (!mock.lan.some((n) => n.id === id))
    mock.lan.push(base({ id, name: name || host, kind: "lan", transport: "lan", endpoint: `${host}:${port}`, totalMb: 7800, availableMb: 6100,
      usableMb: 4880, version: "rpc v8.0.0" }));
  return Promise.resolve(id);
}

let jit = 0;
function mockNodes() {
  const j = () => Math.round(Math.abs(Math.sin(jit++ * 1.7)) * 30);
  const w = (id: string, d: number) => mock.weights[id] ?? d;
  const en = (id: string) => !mock.disabled[id];
  const gpus = [
    mockSpeed(base({ id: "gpu:Vulkan0", name: "GeForce GTX 960", kind: "gpu", transport: "local", device: "Vulkan0", totalMb: 4096, availableMb: 3500,
      usableMb: 3584, loadPct: 20 + j(), weight: w("gpu:Vulkan0", 1), enabled: en("gpu:Vulkan0") }), 60),
  ];
  const tab = mockSpeed(base({ id: "R52T1234", name: "Galaxy S20", kind: "phone", endpoint: "127.0.0.1:50052", totalMb: 8192,
    availableMb: 4100, usableMb: 2800, loadPct: 10 + j(), version: "0.3.2 (6)", appVc: 6, rttMs: 1.8, weight: w("R52T1234", 1), enabled: en("R52T1234") }), 4);
  const note = mockSpeed(base({ id: "R58N9ABC", name: "Galaxy Note10+", kind: "phone", endpoint: "127.0.0.1:50053", totalMb: 12288,
    availableMb: 7400, usableMb: 5600, loadPct: 6 + j() / 2, version: "0.3.2 (6)", appVc: 6, rttMs: 2.1, weight: w("R58N9ABC", 1), enabled: en("R58N9ABC") }), 4);
  const a52 = mockSpeed(base({ id: "RZ8R40XYZ", name: "Galaxy A52", kind: "phone", endpoint: "127.0.0.1:50054", totalMb: 6144,
    availableMb: 3900, usableMb: 3000, loadPct: 3 + j() / 4, version: "0.3 (4)", appVc: 4, updateAvailable: "0.3.2 (6)", rttMs: 2.6, weight: w("RZ8R40XYZ", 1), enabled: en("RZ8R40XYZ") }), 4);
  const lan = mock.lan.map((n) => mockSpeed({ ...n, weight: w(n.id, 1), enabled: en(n.id), loadPct: 5 + j() / 3, rttMs: 6.5 }, 4));
  const nodes = [tab, note, a52, ...lan];
  for (const n of nodes) { n.tempC = MOCK_TEMP[n.id] ?? null; n.throttled = (n.tempC ?? 0) >= 43; }
  for (const n of nodes) n.helper = mock.rolesEnabled && n.role === "helper" && n.stage === "connected" ? mockHelperView(n) : null;
  return { gpus, nodes, j };
}

function mockSnapshot(): ClusterSnapshot {
  const { gpus, nodes: raw, j } = mockNodes();
  const plan = mockPlan(mock.needMb, { gpus, nodes: raw });
  const nodes = raw.map((n) => ({ ...n, inEngine: mock.engine.status === "ready" && plan.rpcIds.includes(n.id),
    assigned: (!n.enabled || n.stage !== "connected" ? "off" : n.role === "helper" && mock.rolesEnabled ? "helper" : plan.rpcIds.includes(n.id) ? "model" : "free") as ClusterNode["assigned"] }));
  const g = gpus.filter((x) => x.enabled);
  const host = base({ id: "host", name: g.length === 1 ? g[0].name : `${g.length} × GPU`, kind: "gpu", transport: "local",
    totalMb: g.reduce((a, x) => a + x.totalMb, 0), availableMb: g.reduce((a, x) => a + x.availableMb, 0),
    usableMb: g.reduce((a, x) => a + x.usableMb, 0), loadPct: gpus[0].loadPct });
  const act = nodes.filter((n) => n.enabled && n.stage === "connected");
  const helpers = nodes.map((n) => n.helper).filter((h): h is HelperView => !!h);
  const running = mock.engine.status === "ready";
  return { host, gpus, nodes, totalMb: host.totalMb + act.reduce((a, n) => a + n.totalMb, 0),
    usableMb: host.usableMb + act.reduce((a, n) => a + n.usableMb, 0),
    rpcServers: plan.rpcIds.map((id) => nodes.find((n) => n.id === id)!.endpoint!), split: plan.entries,
    fastStart: true, bundledNodeVersion: "0.3.2 (6)", appVersion: "0.3.2", bundledNodeVc: 6,
    discovered: mock.found.map((d) => ({ ...d, added: mock.lan.some((n) => n.id === d.id) })), autoAddDiscovered: mock.autoAdd,
    beaconError: "", adbOk: true, platform: "windows",
    engine: { status: mock.engine.status, reason: null, alias: mock.engine.alias, nodes: running ? plan.rpcIds.map((id) => nodes.find((n) => n.id === id)!.name) : [],
      pendingRestart: false, newNodes: false, canRestart: running, warming: running && Date.now() < mock.engine.warmUntil },
    needMb: mock.needMb, freeNodes: plan.freeIds, estTps: plan.estTps, fits: plan.fits,
    rolesEnabled: mock.rolesEnabled, helperModel: mock.helperModel, helperModels: MOCK_HELPER_MODELS, helpers };
}

let mockStep = 0;
function mockAgentTurn(messages: ApiMessage[]): Promise<Completion> {
  const last = messages[messages.length - 1];
  return new Promise((r) => setTimeout(() => {
    if (last.role === "user") { mockStep = 0; }
    mockStep++;
    if (mockStep === 1) r({ content: "Посмотрю, что лежит в домашней папке.", toolCalls: [{ id: "c1", name: "list_dir", arguments: JSON.stringify({ path: "~" }) }] });
    else if (mockStep === 2) r({ content: "", toolCalls: [{ id: "c2", name: "run_command", arguments: JSON.stringify({ command: "Get-ChildItem ~\\Desktop | Measure-Object", cwd: "~" }) }] });
    else r({ content: "Готово: на рабочем столе 14 файлов, в домашней папке — Documents, Downloads, Desktop.", toolCalls: [] });
  }, 700));
}

function mockTool(name: string, args: Record<string, unknown>): Promise<ToolOutput> {
  const out: Record<string, string> = {
    list_dir: `C:\\Users\\demo\n[DIR]  Desktop\n[DIR]  Documents\n[DIR]  Downloads\n       notes.txt  (812 B)`,
    run_command: "exit=0\n--- stdout ---\nCount    : 14",
  };
  return Promise.resolve({ text: out[name] ?? `демо: ${name} ${JSON.stringify(args)}` });
}

// ---- 0.4: voice / phone services / optional commands from other modules
/** whisper.cpp transcription of a 16 kHz mono WAV (base64); base = whisper server URL, else a phone's whisper via adb forward */
export const transcribe = (wavB64: string, base?: string) =>
  isTauri ? tauriInvoke<string>("transcribe", { wavB64, base: base || null }) : Promise.resolve("(демо) распознанный текст");
export interface NodeService { device: string; service: string; pc_port: number; state: string }
export const nodesvcList = () => (isTauri ? tauriInvoke<NodeService[]>("nodesvc_list") : Promise.resolve([] as NodeService[]));
// «Голос» / «Библиотека» on a node (roles_extra.rs, Argus Node 0.3.3+)
export type VoiceModelId = "tiny" | "base" | "small";
export interface NodeRoleReport { state?: string; pct?: number; model?: string; zim?: string; error?: string; import_pct?: number }
export interface NodeRolesView {
  voice: boolean; voiceModel: VoiceModelId; library: boolean;
  models: { id: VoiceModelId; label: string; sizeMb: number }[];
  downloadPct: number | null; downloadError: string | null; modelOnPc: boolean;
  node: { voice?: NodeRoleReport; library?: NodeRoleReport } | null; note: string;
}
const mockRoles: NodeRolesView = {
  voice: false, voiceModel: "base", library: false, downloadPct: null, downloadError: null, modelOnPc: false, node: null, note: "",
  models: [{ id: "tiny", label: "tiny · 75 МБ · самый быстрый", sizeMb: 75 }, { id: "base", label: "base · 142 МБ · баланс", sizeMb: 142 }, { id: "small", label: "small · 466 МБ · точнее, медленнее", sizeMb: 466 }],
};
export const nodeRolesGet = (id: string) => (isTauri ? tauriInvoke<NodeRolesView>("node_roles_get", { id }) : Promise.resolve(mockRoles));
export const setNodeVoice = (id: string, enabled: boolean, model?: VoiceModelId) =>
  (isTauri ? tauriInvoke<void>("set_node_voice", { id, enabled, model: model ?? null }) : Promise.resolve());
export const setNodeLibrary = (id: string, enabled: boolean) => (isTauri ? tauriInvoke<void>("set_node_library", { id, enabled }) : Promise.resolve());
/** invoke a command that may not exist in this build: resolves null instead of throwing */
export async function invokeOptional<T>(cmd: string, args?: Record<string, unknown>): Promise<T | null> {
  if (!isTauri) return null;
  try { return await tauriInvoke<T>(cmd, args); } catch (e) { console.warn(`${cmd}:`, e); return null; }
}

// ---------------------------------------------------------------- «Возможности» (features.rs)
import type { FeaturesConfig as _FeaturesConfig, FeaturesView as _FeaturesView, RouteTarget as _RouteTarget } from "./types";

export async function featuresGet(): Promise<_FeaturesView | null> {
  if (!isTauri) return null;
  return tauriInvoke<_FeaturesView>("features_get");
}

export async function featuresSet(features: _FeaturesConfig): Promise<_FeaturesView | null> {
  if (!isTauri) return null;
  return tauriInvoke<_FeaturesView>("features_set", { features });
}

/** cluster model or a ready «Помощник» for a simple prompt (when routing.simpleToHelper is on) */
export async function routeTarget(prompt: string, hasAttachments = false): Promise<_RouteTarget> {
  if (!isTauri) return { base: LLAMA_URL, model: "argus", kind: "main" };
  return tauriInvoke<_RouteTarget>("route_target", { prompt, hasAttachments });
}

// ---- «Очередь» и «Знания» (queue.rs / rag.rs / kiwix.rs) ----
export type QueueKind = "translate" | "summarize" | "custom";
export interface QueueJob {
  id: string; kind: QueueKind; prompt: string; lang: string; inDir: string; outDir: string; startAt: number | null;
  status: "pending" | "running" | "done" | "error"; progress: number; message: string; doneFiles: string[]; totalFiles: number;
  current: string | null; errors: string[]; created: number;
}
export interface QueueState { running: boolean; jobs: QueueJob[] }
export interface NewQueueJob { kind: QueueKind; prompt: string; lang: string; inDir: string; outDir: string; startAt: number | null }
const noTauri = () => Promise.reject(new Error("Доступно только в приложении"));
export const queueList = () => (isTauri ? tauriInvoke<QueueState>("queue_list") : Promise.resolve({ running: false, jobs: [] }));
export const queueAdd = (job: NewQueueJob) => (isTauri ? tauriInvoke<QueueState>("queue_add", { job }) : noTauri());
export const queueRemove = (id: string) => (isTauri ? tauriInvoke<QueueState>("queue_remove", { id }) : noTauri());
export const queueStart = () => (isTauri ? tauriInvoke<QueueState>("queue_start") : noTauri());
export const queuePause = () => (isTauri ? tauriInvoke<QueueState>("queue_pause") : noTauri());
export async function onQueueUpdate(cb: (s: QueueState) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<QueueState>("queue-update", (e) => cb(e.payload));
}

export interface RagStatus {
  folder: string; files: number; chunks: number; model: string; customUrl: string; indexing: boolean;
  done: number; total: number; current: string; message: string; errors: string[];
}
export interface RagHit { source: string; text: string; score: number }
export const ragStatus = () => (isTauri ? tauriInvoke<RagStatus>("rag_status") : noTauri());
export const ragIndex = (folder: string, url: string | null) => (isTauri ? tauriInvoke<RagStatus>("rag_index", { folder, url }) : noTauri());
export const ragSearch = (query: string, k = 5) => (isTauri ? tauriInvoke<RagHit[]>("rag_search", { query, k }) : noTauri());
export async function onRagProgress(cb: (s: RagStatus) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<RagStatus>("rag-progress", (e) => cb(e.payload));
}

export interface KiwixInfo { url: string; base: string | null; phone: boolean }
export interface KiwixBook { name: string; title: string; path: string; language: string; summary: string }
export interface WikiHit { title: string; path: string; snippet: string; book: string }
export const kiwixInfo = () => (isTauri ? tauriInvoke<KiwixInfo>("kiwix_info") : noTauri());
export const kiwixSetUrl = (url: string) => (isTauri ? tauriInvoke<void>("kiwix_set_url", { url }) : noTauri());
export const kiwixBooks = () => (isTauri ? tauriInvoke<KiwixBook[]>("kiwix_books") : noTauri());
export const kiwixSearch = (q: string) => (isTauri ? tauriInvoke<WikiHit[]>("kiwix_search", { q }) : noTauri());
export const kiwixRead = (path: string) => (isTauri ? tauriInvoke<string>("kiwix_read", { path }) : noTauri());
