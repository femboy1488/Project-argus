// Thin bridge to the Rust side. Falls back to mock data when opened in a plain browser (npm run dev).
import type { ClusterNode, ClusterSnapshot, ModelEntry, SplitEntry } from "./types";

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

export const rescan = () => (isTauri ? tauriInvoke<void>("rescan") : Promise.resolve());

export interface StartOpts {
  ctx?: number; lan?: boolean; needMb?: number;
  draftHf?: string; draftMax?: number; cacheReuse?: number; keepLoaded?: boolean;
}

export async function startModel(m: ModelEntry, opts: StartOpts = {}) {
  if (!isTauri) return LLAMA_URL;
  return tauriInvoke<string>("start_model", { args: {
    hfRepo: m.hfRepo, hfFile: m.hfFile ?? null, ctx: opts.ctx ?? 8192, engine: "llama.cpp", alias: m.id, lan: !!opts.lan,
    needMb: opts.needMb ?? null, draftHf: opts.draftHf ?? null, draftMax: opts.draftMax ?? null,
    cacheReuse: opts.cacheReuse ?? null, keepLoaded: !!opts.keepLoaded,
  } });
}

export const stopModel = () => (isTauri ? tauriInvoke<void>("stop_model") : Promise.resolve());

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
    default: throw new Error(`неизвестный инструмент: ${name}`);
  }
}

/** Resolves when llama-server's /health returns 200 (it returns 503 while loading weights).
 *  Rejects as soon as the engine process dies, instead of spinning until the timeout. */
export async function waitHealthy(timeoutMs = 60 * 60_000) {
  if (!isTauri) return new Promise<void>((r) => setTimeout(r, 1200)); // browser demo
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    if (!(await modelStatus())) throw new Error("Engine stopped — see the engine log for the reason");
    try {
      const r = await fetch(`${LLAMA_URL}/health`);
      if (r.ok) return;
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error("Model did not become ready in 60 min");
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

export async function* streamChat(messages: ApiMessage[], signal?: AbortSignal, onStats?: (s: ChatStats) => void) {
  if (!isTauri) { // browser demo
    const demo = "Слои 0–15 лежат в VRAM видеокарты, слои 16–31 считает планшет через rpc-server по USB.\n\n"
      + "```python\nfrom openai import OpenAI\nc = OpenAI(base_url=\"http://127.0.0.1:8080/v1\", api_key=\"argus\")\n```\n"
      + "Каждый токен пересекает кабель **дважды** как небольшой тензор активаций, поэтому канал далёк от насыщения.";
    let n = 0; for (const w of demo.split(/(?<= )/)) { if (signal?.aborted) return; await new Promise((r) => setTimeout(r, 20)); n++; onStats?.({ tps: 7.4, promptTps: 41.2, tokens: n, ttftMs: 820 }); yield w; }
    return;
  }
  const res = await fetch(`${LLAMA_URL}/v1/chat/completions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ messages, stream: true, timings_per_token: true }),
    signal,
  });
  if (!res.ok || !res.body) throw new Error(`llama-server: HTTP ${res.status} ${await res.text().catch(() => "")}`);
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
    body: JSON.stringify({ messages, tools, tool_choice: "auto", stream: false, parse_tool_calls: true }),
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
  lan: [] as ClusterNode[],
  weights: {} as Record<string, number>,
  disabled: {} as Record<string, boolean>,
};

const base = (o: Partial<ClusterNode> & Pick<ClusterNode, "id" | "name" | "kind">): ClusterNode => ({
  stage: "connected", transport: "usb", endpoint: null, totalMb: 0, availableMb: 0, usableMb: 0, loadPct: 0, error: null,
  weight: 1, enabled: true, device: null, version: null, rpcDevices: 1, integrated: false, ...o,
});

function mockAddLan(address: string, name?: string) {
  const [host, port = "50052"] = address.split(":");
  const id = `lan:${host}:${port}`;
  if (!mock.lan.some((n) => n.id === id))
    mock.lan.push(base({ id, name: name || host, kind: "lan", transport: "lan", endpoint: `${host}:${port}`, totalMb: 7800, availableMb: 6100,
      usableMb: 4880, version: "rpc v8.0.0" }));
  return Promise.resolve(id);
}

function mockSnapshot(): ClusterSnapshot {
  const j = () => Math.round(Math.random() * 30);
  const w = (id: string, d: number) => mock.weights[id] ?? d;
  const en = (id: string) => !mock.disabled[id];
  const gpus = [
    base({ id: "gpu:Vulkan0", name: "GeForce GTX 960", kind: "gpu", transport: "local", device: "Vulkan0", totalMb: 4096, availableMb: 3500,
      usableMb: 3584, loadPct: 20 + j(), weight: w("gpu:Vulkan0", 2), enabled: en("gpu:Vulkan0") }),
  ];
  const tab = base({ id: "R52T1234", name: "Galaxy Tab S6 Lite", kind: "tablet", endpoint: "127.0.0.1:50052", totalMb: 8192,
    availableMb: 5200, usableMb: 3640, loadPct: 10 + j(), version: "0.2.0", weight: w("R52T1234", 1), enabled: en("R52T1234") });
  const lan = mock.lan.map((n) => ({ ...n, weight: w(n.id, 1), enabled: en(n.id), loadPct: 5 + j() / 3 }));
  const nodes = [tab, ...lan];
  const g = gpus.filter((x) => x.enabled);
  const host = base({ id: "host", name: g.length === 1 ? g[0].name : `${g.length} × GPU`, kind: "gpu", transport: "local",
    totalMb: g.reduce((a, x) => a + x.totalMb, 0), availableMb: g.reduce((a, x) => a + x.availableMb, 0),
    usableMb: g.reduce((a, x) => a + x.usableMb, 0), loadPct: gpus[0].loadPct });
  const act = nodes.filter((n) => n.enabled && n.stage === "connected");
  const devs = [...act.map((n, i) => ({ id: n.id, device: `RPC${i}`, name: n.name, s: n.usableMb * n.weight })),
                ...g.map((x) => ({ id: x.id, device: x.device!, name: x.name, s: x.usableMb * x.weight }))];
  const tot = devs.reduce((a, d) => a + d.s, 0) || 1;
  const split: SplitEntry[] = devs.map((d) => ({ id: d.id, device: d.device, name: d.name, share: d.s / tot }));
  return { host, gpus, nodes, totalMb: host.totalMb + act.reduce((a, n) => a + n.totalMb, 0),
    usableMb: host.usableMb + act.reduce((a, n) => a + n.usableMb, 0), rpcServers: act.map((n) => n.endpoint!), split,
    fastStart: true, bundledNodeVersion: "0.2.0" };
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
