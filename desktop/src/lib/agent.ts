// Agent mode: OpenAI-style tool calling against the local llama-server (--jinja).
// Every tool call goes through `confirm` (UI card) before it reaches Rust.
import { chatWithTools, runTool, type ApiMessage, type PlatformInfo } from "./api";
import type { ChatMessage, ToolCall } from "./types";
import { uid } from "./format";

export const MAX_STEPS = 20;

/** Host OS (Rust platform_info): PowerShell on Windows, bash on Linux; screen tools only where supported. */
export let PLATFORM: PlatformInfo = { os: "windows", shell: "PowerShell", screen: true };
export function setPlatform(p: PlatformInfo) {
  PLATFORM = p;
  TOOL_LABEL.run_command = `Команда ${p.shell}`;
}

interface ToolDef { name: string; description: string; dangerous: boolean; parameters: Record<string, unknown> }
const obj = (props: Record<string, unknown>, required: string[] = []) => ({ type: "object", properties: props, required });
const str = (d: string) => ({ type: "string", description: d });
const num = (d: string) => ({ type: "number", description: d });

export const TOOLS: ToolDef[] = [
  { name: "list_dir", dangerous: false, description: "List files and folders in a directory on the user's PC.", parameters: obj({ path: str("Directory path, e.g. C:\\Users\\me\\Documents or ~") }, ["path"]) },
  { name: "read_file", dangerous: false, description: "Read a text file from the user's PC.", parameters: obj({ path: str("File path") }, ["path"]) },
  { name: "open_url", dangerous: false, description: "Open a web page in the default browser.", parameters: obj({ url: str("https://...") }, ["url"]) },
  { name: "screenshot", dangerous: false, description: "Take a screenshot of the primary monitor. Mouse coordinates for other tools are in this image's pixels.", parameters: obj({}) },
  { name: "run_command", dangerous: true, description: "Run a PowerShell command on the user's Windows PC and return stdout/stderr.", parameters: obj({ command: str("PowerShell command"), cwd: str("Working directory (optional)"), timeout_sec: num("Timeout in seconds, default 60") }, ["command"]) },
  { name: "write_file", dangerous: true, description: "Create or overwrite a text file on the user's PC.", parameters: obj({ path: str("File path"), content: str("Full file content") }, ["path", "content"]) },
  { name: "open_app", dangerous: true, description: "Open an application, file or folder with its default handler (e.g. notepad, calc, C:\\file.pdf).", parameters: obj({ target: str("App name, path or URL") }, ["target"]) },
  { name: "mouse_click", dangerous: true, description: "Move the mouse to (x, y) in screenshot pixels and click.", parameters: obj({ x: num("x"), y: num("y"), button: { type: "string", enum: ["left", "right"] }, double: { type: "boolean" } }, ["x", "y"]) },
  { name: "mouse_move", dangerous: true, description: "Move the mouse to (x, y) in screenshot pixels.", parameters: obj({ x: num("x"), y: num("y") }, ["x", "y"]) },
  { name: "type_text", dangerous: true, description: "Type text with the keyboard into the focused window.", parameters: obj({ text: str("Text to type") }, ["text"]) },
  { name: "key_press", dangerous: true, description: "Press a key or combination, e.g. enter, ctrl+s, alt+tab, win+r.", parameters: obj({ keys: str("Key combination joined with +") }, ["keys"]) },
];

export const TOOL_LABEL: Record<string, string> = {
  list_dir: "Список файлов", read_file: "Прочитать файл", open_url: "Открыть ссылку", screenshot: "Скриншот экрана",
  run_command: "Команда PowerShell", write_file: "Записать файл", open_app: "Открыть приложение",
  mouse_click: "Клик мышью", mouse_move: "Переместить мышь", type_text: "Ввести текст", key_press: "Нажать клавиши",
};

export const isDangerous = (name: string) => TOOLS.find((t) => t.name === name)?.dangerous ?? true;

const SCREEN_TOOLS = ["screenshot", "mouse_click", "mouse_move", "type_text", "key_press"];

function describe(t: ToolDef): ToolDef {
  if (t.name !== "run_command" || PLATFORM.os === "windows") return t;
  return { ...t, description: `Run a ${PLATFORM.shell} command on the user's ${PLATFORM.os === "linux" ? "Linux" : "macOS"} PC and return stdout/stderr.`,
    parameters: obj({ command: str(`${PLATFORM.shell} command`), cwd: str("Working directory (optional)"), timeout_sec: num("Timeout in seconds, default 60") }, ["command"]) };
}

export function toolSchemas(dangerous: boolean, vision: boolean) {
  return TOOLS.filter((t) => (dangerous || !t.dangerous) && (vision || !["mouse_click", "mouse_move"].includes(t.name))
      && (PLATFORM.screen || !SCREEN_TOOLS.includes(t.name)))
    .map(describe)
    .map((t) => ({ type: "function", function: { name: t.name, description: t.description, parameters: t.parameters } }));
}

export const AGENT_SYSTEM = (vision: boolean, dangerous: boolean) =>
  `You are Argus Agent, running locally on the user's ${PLATFORM.os === "linux" ? "Linux" : PLATFORM.os === "macos" ? "macOS" : "Windows"} PC (shell: ${PLATFORM.shell}). Use the provided tools to complete the task step by step. ` +
  `Call one tool at a time, look at the result, then decide the next step. When the task is done, reply with a short summary in the user's language without calling tools. ` +
  (dangerous ? "" : "Only read-only tools are enabled; if the task needs commands, file writes or input control, say that the user must enable dangerous tools. ") +
  (!PLATFORM.screen ? "Screen capture and mouse/keyboard control are not available on this OS. " :
   vision ? "You can see screenshots: take one before clicking, and use pixel coordinates from the latest screenshot. " : "You cannot see images; do not try to control the mouse. ") +
  "Every action is shown to the user, who may decline it.";

export type Decision = "allow" | "allow-all" | "deny";

export interface AgentHooks {
  push: (m: ChatMessage) => void;
  patch: (id: string, p: Partial<ChatMessage>) => void;
  confirm: (call: ToolCall) => Promise<Decision>;
  onStep: (n: number) => void;
  log: (line: string) => void;
}

/** Turns stored conversation messages into API messages. Only the newest screenshot is sent as an image. */
export function toApi(msgs: ChatMessage[], vision: boolean): ApiMessage[] {
  const lastImg = [...msgs].reverse().find((m) => m.image)?.id;
  const out: ApiMessage[] = [];
  for (const m of msgs) {
    if (m.role === "system") { out.push({ role: "system", content: m.content }); continue; }
    if (m.role === "tool") {
      out.push({ role: "tool", tool_call_id: m.toolCallId, content: m.content });
      if (m.image && vision && m.id === lastImg)
        out.push({ role: "user", content: [{ type: "text", text: "Screenshot from the tool call above:" }, { type: "image_url", image_url: { url: m.image } }] });
      continue;
    }
    if (m.role === "assistant" && m.toolCalls?.length) {
      out.push({ role: "assistant", content: m.content || "", tool_calls: m.toolCalls.map((c) => ({ id: c.id, type: "function", function: { name: c.name, arguments: c.arguments } })) });
      continue;
    }
    out.push({ role: m.role, content: withFiles(m) });
  }
  return out;
}

/** User text + attached text files inlined as fenced blocks. */
export function withFiles(m: ChatMessage): string {
  if (!m.files?.length) return m.content;
  const f = m.files.map((x) => `Файл: ${x.name}\n\`\`\`\n${x.text}\n\`\`\``).join("\n\n");
  return `${f}\n\n${m.content}`;
}

export async function runAgent(history: ChatMessage[], system: string, opts: { dangerous: boolean; vision: boolean; signal: AbortSignal }, h: AgentHooks) {
  const local: ChatMessage[] = [{ id: "sys", role: "system", content: system }, ...history];
  const tools = toolSchemas(opts.dangerous, opts.vision);
  for (let step = 1; step <= MAX_STEPS; step++) {
    if (opts.signal.aborted) return;
    h.onStep(step);
    const aid = uid();
    h.push({ id: aid, role: "assistant", content: "", pending: true });
    const r = await chatWithTools(toApi(local, opts.vision), tools, opts.signal);
    const am: ChatMessage = { id: aid, role: "assistant", content: r.content, toolCalls: r.toolCalls, pending: false,
      tps: r.stats?.tps, promptTps: r.stats?.promptTps, tokens: r.stats?.tokens, ttftMs: r.stats?.ttftMs };
    h.patch(aid, am);
    local.push(am);
    if (!r.toolCalls.length) { h.log(`шаг ${step}: готово`); return; }
    for (const call of r.toolCalls) {
      if (opts.signal.aborted) return;
      let args: Record<string, unknown> = {};
      try { args = call.arguments ? JSON.parse(call.arguments) : {}; } catch { /* model produced bad JSON */ }
      const tid = uid();
      let text: string; let image: string | undefined;
      if (isDangerous(call.name) && !opts.dangerous) {
        text = "Инструмент отключён: пользователь не разрешил опасные действия.";
        h.log(`✕ ${call.name} — опасные инструменты выключены`);
      } else {
        const d = await h.confirm(call);
        if (opts.signal.aborted) return;
        if (d === "deny") { text = "Пользователь отклонил это действие."; h.log(`✕ ${call.name} — отклонено`); }
        else {
          try {
            const o = await runTool(call.name, args);
            text = o.text; image = o.image;
            h.log(`✓ ${call.name} ${short(args)}`);
          } catch (e) { text = `Ошибка: ${String(e)}`; h.log(`! ${call.name}: ${String(e)}`); }
        }
      }
      const tm: ChatMessage = { id: tid, role: "tool", content: text, toolCallId: call.id, toolName: call.name, image };
      h.push(tm);
      local.push(tm);
    }
  }
  h.push({ id: uid(), role: "assistant", content: `Достигнут лимит в ${MAX_STEPS} шагов — остановлено. Напишите «продолжай», чтобы продолжить.` });
}

export const short = (a: Record<string, unknown>) => {
  const s = JSON.stringify(a);
  return s.length > 90 ? s.slice(0, 90) + "…" : s;
};
