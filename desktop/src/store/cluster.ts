import { create } from "zustand";
import type { ChatMessage, ClusterSnapshot, Conversation, ModelRunState, SpeedSettings } from "../lib/types";
import { uid } from "../lib/format";

export type View = "chat" | "devices" | "models" | "api";

export const SYSTEM_PRESETS: { id: string; label: string; prompt: string }[] = [
  { id: "none", label: "Без системного промпта", prompt: "" },
  { id: "assistant", label: "Ассистент (RU)", prompt: "Ты полезный ассистент. Отвечай по-русски, кратко и по делу." },
  { id: "coder", label: "Программист", prompt: "You are a senior software engineer. Answer with correct, minimal code in fenced blocks, then a short explanation." },
  { id: "translator", label: "Переводчик RU⇄EN", prompt: "Переводи текст пользователя: с русского на английский, с любого другого — на русский. Только перевод, без комментариев." },
  { id: "analyst", label: "Аналитик документов", prompt: "Ты аналитик. Внимательно читай приложенные файлы, цитируй факты, отмечай, чего в них нет." },
];

const LS_CONV = "argus.conversations";
const LS_ACTIVE = "argus.activeConversation";
const LS_SPEED = "argus.speed";

const newConversation = (): Conversation => ({
  id: uid(), title: "Новый чат", createdAt: Date.now(), updatedAt: Date.now(), systemPrompt: "", agent: false, messages: [],
});

function loadConversations(): Conversation[] {
  try {
    const v = JSON.parse(localStorage.getItem(LS_CONV) ?? "[]") as Conversation[];
    if (Array.isArray(v) && v.length) return v.map((c) => ({ ...c, messages: c.messages.map((m) => ({ ...m, pending: false })) }));
  } catch { /* corrupted */ }
  return [newConversation()];
}

let saveTimer: ReturnType<typeof setTimeout> | undefined;
function persist(convs: Conversation[], active: string) {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    try {
      // screenshots are large: keep them out of localStorage
      const slim = convs.map((c) => ({ ...c, messages: c.messages.map((m) => (m.image ? { ...m, image: undefined, content: m.content + " [скриншот]" } : m)) }));
      localStorage.setItem(LS_CONV, JSON.stringify(slim));
      localStorage.setItem(LS_ACTIVE, active);
    } catch { /* quota */ }
  }, 300);
}

const defaultSpeed: SpeedSettings = { speculative: false, draft: "auto", draftMax: 16, promptCache: true, keepLoaded: true, fastStart: true };

interface State {
  view: View;
  snapshot: ClusterSnapshot | null;
  wizardOpen: boolean;
  lanDialog: boolean;
  tourOpen: boolean;
  conversations: Conversation[];
  activeId: string;
  run: ModelRunState;
  lan: boolean;
  ctx: number;
  speed: SpeedSettings;
  engineLog: string[];
  dlBytes: number;
  modelsTab: "curated" | "hf";
  setModelsTab: (t: "curated" | "hf") => void;
  setDl: (b: number) => void;
  pushEngineLog: (l: string) => void;
  clearEngineLog: () => void;
  setCtx: (v: number) => void;
  setLan: (v: boolean) => void;
  setSpeed: (p: Partial<SpeedSettings>) => void;
  setView: (v: View) => void;
  setSnapshot: (s: ClusterSnapshot) => void;
  setWizard: (o: boolean) => void;
  setLanDialog: (o: boolean) => void;
  setTour: (o: boolean) => void;
  setRun: (r: ModelRunState) => void;
  // conversations
  newChat: () => void;
  selectChat: (id: string) => void;
  deleteChat: (id: string) => void;
  patchChat: (id: string, p: Partial<Conversation>) => void;
  pushMessage: (m: ChatMessage, convId?: string) => void;
  patchMessage: (id: string, patch: Partial<ChatMessage>, convId?: string) => void;
  appendToMessage: (id: string, chunk: string, convId?: string) => void;
  truncateFrom: (msgId: string, convId?: string) => void;
  clearChat: () => void;
}

const initialConvs = loadConversations();
const initialActive = (() => { const a = localStorage.getItem(LS_ACTIVE); return initialConvs.some((c) => c.id === a) ? a! : initialConvs[0].id; })();

export const useCluster = create<State>((set, get) => {
  const mutateConv = (convId: string | undefined, f: (c: Conversation) => Conversation) => set((s) => {
    const id = convId ?? s.activeId;
    const conversations = s.conversations.map((c) => (c.id === id ? { ...f(c), updatedAt: Date.now() } : c));
    persist(conversations, s.activeId);
    return { conversations };
  });
  return {
    view: "chat",
    snapshot: null,
    wizardOpen: false,
    lanDialog: false,
    tourOpen: localStorage.getItem("argus.tourDone") !== "1",
    conversations: initialConvs,
    activeId: initialActive,
    run: { status: "idle" },
    lan: localStorage.getItem("argus.lan") === "1",
    ctx: Number(localStorage.getItem("argus.ctx") ?? 8192),
    speed: { ...defaultSpeed, ...JSON.parse(localStorage.getItem(LS_SPEED) ?? "{}") },
    engineLog: [],
    dlBytes: 0,
    modelsTab: "curated",
    setModelsTab: (modelsTab) => set({ modelsTab }),
    setDl: (dlBytes) => set({ dlBytes }),
    pushEngineLog: (l) => set((s) => {
      const log = s.engineLog.slice(-400);
      // collapse consecutive progress redraws of the same download into one line
      const prev = log[log.length - 1];
      if (prev && /%|MB\/s|\d+\/\d+/.test(l) && prev.slice(0, 24) === l.slice(0, 24)) log[log.length - 1] = l; else log.push(l);
      return { engineLog: log };
    }),
    clearEngineLog: () => set({ engineLog: [], dlBytes: 0 }),
    setCtx: (ctx) => { localStorage.setItem("argus.ctx", String(ctx)); set({ ctx }); },
    setLan: (lan) => { localStorage.setItem("argus.lan", lan ? "1" : "0"); set({ lan }); },
    setSpeed: (p) => { const speed = { ...get().speed, ...p }; localStorage.setItem(LS_SPEED, JSON.stringify(speed)); set({ speed }); },
    setView: (view) => set({ view }),
    setSnapshot: (snapshot) => set({ snapshot }),
    setWizard: (wizardOpen) => set({ wizardOpen }),
    setLanDialog: (lanDialog) => set({ lanDialog }),
    setTour: (tourOpen) => { if (!tourOpen) localStorage.setItem("argus.tourDone", "1"); set({ tourOpen }); },
    setRun: (run) => set({ run }),

    newChat: () => set((s) => {
      const cur = s.conversations.find((c) => c.id === s.activeId);
      if (cur && cur.messages.length === 0) return { view: "chat" }; // reuse the empty one
      const c = { ...newConversation(), systemPrompt: cur?.systemPrompt ?? "", agent: cur?.agent ?? false };
      const conversations = [c, ...s.conversations];
      persist(conversations, c.id);
      return { conversations, activeId: c.id, view: "chat" };
    }),
    selectChat: (activeId) => set((s) => { persist(s.conversations, activeId); return { activeId, view: "chat" }; }),
    deleteChat: (id) => set((s) => {
      let conversations = s.conversations.filter((c) => c.id !== id);
      if (!conversations.length) conversations = [newConversation()];
      const activeId = s.activeId === id ? conversations[0].id : s.activeId;
      persist(conversations, activeId);
      return { conversations, activeId };
    }),
    patchChat: (id, p) => mutateConv(id, (c) => ({ ...c, ...p })),
    pushMessage: (m, convId) => mutateConv(convId, (c) => {
      const title = c.messages.length === 0 && m.role === "user" ? (m.content.split("\n")[0].slice(0, 48) || "Новый чат") : c.title;
      return { ...c, title, messages: [...c.messages, m] };
    }),
    patchMessage: (id, patch, convId) => mutateConv(convId, (c) => ({ ...c, messages: c.messages.map((m) => (m.id === id ? { ...m, ...patch } : m)) })),
    appendToMessage: (id, chunk, convId) => mutateConv(convId, (c) => ({ ...c, messages: c.messages.map((m) => (m.id === id ? { ...m, content: m.content + chunk } : m)) })),
    truncateFrom: (msgId, convId) => mutateConv(convId, (c) => {
      const i = c.messages.findIndex((m) => m.id === msgId);
      return i < 0 ? c : { ...c, messages: c.messages.slice(0, i) };
    }),
    clearChat: () => mutateConv(undefined, (c) => ({ ...c, messages: [], title: "Новый чат" })),
  };
});

export const activeConversation = (s: State) => s.conversations.find((c) => c.id === s.activeId) ?? s.conversations[0];
export const allNodes = (s: ClusterSnapshot | null) => (s ? [s.host, ...s.nodes] : []);
