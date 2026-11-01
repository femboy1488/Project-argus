import { create } from "zustand";
import type { ChatMessage, ClusterSnapshot, Conversation, ModelRunState, SpeedSettings, UpdateProgress, UpdateStatus } from "../lib/types";
import { uid } from "../lib/format";

export type View = "chat" | "devices" | "models" | "api";

export const SYSTEM_PRESETS: { id: string; label: string; prompt: string }[] = [
  { id: "none", label: "Без системного промпта", prompt: "" },
  { id: "assistant", label: "Ассистент (RU)", prompt: "Ты полезный ассистент. Отвечай по-русски, кратко и по делу." },
  { id: "coder", label: "Программист", prompt: "You are a senior software engineer. Answer with correct, minimal code in fenced blocks, then a short explanation." },
  { id: "translator", label: "Переводчик RU⇄EN", prompt: "Переводи текст пользователя: с русского на английский, с любого другого — на русский. Только перевод, без комментариев." },
  { id: "argus-net", label: "Помощник: сеть кластера Argus", prompt: "Ты — помощник по кластеру Argus 0.3. Отвечай по-русски, коротко, по шагам. Команды cmd давай одной строкой.\n\nКак устроен кластер: ПК с Windows — хост, на нём llama-server (llama.cpp b12008) держит часть слоёв модели в видеопамяти и раздаёт остальные нодам через RPC. Android-нода — приложение Argus Node с rpc-server на порту 50052; подключается по USB, ПК сам ставит и обновляет приложение и пробрасывает порт (adb forward, на ПК порты 50053, 50054 и дальше). Сетевая нода — любое устройство в той же сети с rpc-server: Android в «Wi-Fi режиме» или Linux/Raspberry Pi со скриптом argus-node-linux.sh сами появляются в Cluster → «Найдено в сети» (кнопка «Добавить»), остальное — вручную: «Добавить ноду по IP» → IP:50052. Слои делятся по свободной памяти и весу скорости нод; самое медленное звено тормозит всех.\n\nПравила сети: только закрытая домашняя сеть, у RPC нет пароля — порт 50052 никогда не открывать в интернет. USB быстрее и стабильнее Wi-Fi; по сети лучше кабель или 5 ГГц. Все ноды на одной версии llama.cpp (b12008), иначе ошибка протокола. Если нода отключилась, движок останавливается, а когда она вернётся — модель перезапускается сама (или кнопка «Перезапустить движок»); новую ноду в работающую модель добавляет «Перезапустить движок». Зависшую ноду лечит кнопка «Переподключить» в инспекторе.\n\nПодключение Android: Режим разработчика → Отладка по USB, кабель с данными, принять RSA-запрос («Всегда разрешать»). Стадии: Installing/Updating → Forwarding → Starting → Syncing → ONLINE. На устройстве строка VARIANT (baseline / dotprod / i8mm) показывает сборку под процессор.\n\nЧастые проблемы:\n- Устройство не видно: другой кабель или порт, переподключить отладку, принять RSA.\n- «Shell does not have permission to access user 150»: Защищённая папка или Dual Messenger Samsung; исправлено в новой сборке, обновить Argus.\n- Нода не стартует: открыть Argus Node на устройстве, посмотреть лог (Copy).\n- Модель не грузится: Models → Engine log или %TEMP%\\argus-engine.log, смотреть первые строки ошибки; проверить fit-check.\n- Медленно: убрать слабые ноды или снизить их вес, меньше контекст, модель поменьше.\n- API для Hermes, Open WebUI, Jan: http://127.0.0.1:8080/v1; Hermes нужен контекст 64K.\n\nЕсли не хватает данных — попроси точный текст ошибки, модель устройства, VARIANT и tok/s. Не выдумывай функций, которых нет." },
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
  toasts: { id: string; kind: "info" | "warn" | "error" | "ok"; text: string }[];
  /** self-update (0.3.1) — the app always asks; «Позже» lasts for this session */
  update: UpdateStatus | null;
  updateDialog: boolean;
  aboutOpen: boolean;
  updProgress: UpdateProgress | null;
  updateLater: string[];        // desktop versions answered «Позже» this session
  nodeLater: string[];          // "<node id>|<version>" answered «Позже» this session
  setUpdate: (u: UpdateStatus) => void;
  setUpdateDialog: (o: boolean) => void;
  setAbout: (o: boolean) => void;
  setUpdProgress: (p: UpdateProgress | null) => void;
  laterUpdate: (v: string) => void;
  laterNode: (key: string) => void;
  pushToast: (kind: "info" | "warn" | "error" | "ok", text: string) => void;
  dismissToast: (id: string) => void;
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
    toasts: [],
    update: null,
    updateDialog: false,
    aboutOpen: false,
    updProgress: null,
    updateLater: [],
    nodeLater: [],
    setUpdate: (update) => set({ update }),
    setUpdateDialog: (updateDialog) => set({ updateDialog }),
    setAbout: (aboutOpen) => set({ aboutOpen }),
    setUpdProgress: (updProgress) => set({ updProgress }),
    laterUpdate: (v) => set((s) => ({ updateLater: [...s.updateLater, v], updateDialog: false })),
    laterNode: (k) => set((s) => ({ nodeLater: [...s.nodeLater, k] })),
    pushToast: (kind, text) => {
      const id = uid();
      set((s) => (s.toasts.some((t) => t.text === text) ? {} : { toasts: [...s.toasts.slice(-4), { id, kind, text }] }));
      setTimeout(() => get().dismissToast(id), kind === "error" || kind === "warn" ? 9000 : 6000);
    },
    dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
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
