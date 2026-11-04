import { useCallback, useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Bot, Boxes, Check, Columns3, Copy, FolderSearch, Loader2, MessageSquarePlus, PanelRight, ScrollText, ShieldAlert, ShieldCheck, Trash2, X } from "lucide-react";
import { useCluster, activeConversation, SYSTEM_PRESETS } from "../store/cluster";
import { LLAMA_URL, chatErrorText, engineHasVision, featuresGet, invokeOptional, readyHelpers, restoreChatSession, routeTarget, saveChatSession, streamChat, trackGeneration, type ApiMessage } from "../lib/api";
import type { RouteTarget } from "../lib/types";
import { autoTitle, summarize, type SummaryJob } from "../lib/helpers";
import Markdown from "./Markdown";
import { uid } from "../lib/format";
import { MAX_STEPS, PLATFORM, TOOL_LABEL, isDangerous, runAgent, toApi, AGENT_SYSTEM, type Decision } from "../lib/agent";
import EngineBanner from "./EngineBanner";
import { AGENT_MODELS, MODELS } from "../lib/models";
import type { ChatMessage, ToolCall } from "../lib/types";
import MessageBubble from "./MessageBubble";
import PromptBar, { type Attachment, type ImageAttachment } from "./PromptBar";
import QuickStart from "./QuickStart";
import Drawer from "./Drawer";
import { helperPct, useWide } from "../lib/ui";

/** Attach user photos as OpenAI content parts to the matching user messages of toApi() output. */
function withImages(history: ChatMessage[], out: ApiMessage[]): ApiMessage[] {
  const users = history.filter((m) => m.role === "user");
  let i = 0;
  return out.map((a) => {
    if (a.role !== "user" || typeof a.content !== "string") return a;
    const m = users[i++];
    if (!m?.images?.length) return a;
    return { ...a, content: [{ type: "text", text: a.content || "Что на фото?" }, ...m.images.map((url) => ({ type: "image_url", image_url: { url } }))] };
  });
}

type RagChunk = { text?: string; content?: string; chunk?: string; source?: string; path?: string; file?: string; score?: number };
/** «Искать в моих файлах»: top chunks from rag_search (null when the command is missing or failed). */
async function ragContext(query: string): Promise<{ block: string; sources: string[] } | null> {
  const r = await invokeOptional<RagChunk[] | { results?: RagChunk[]; chunks?: RagChunk[] }>("rag_search", { query, k: 5 });
  const list = Array.isArray(r) ? r : r?.results ?? r?.chunks ?? [];
  const items = list.map((c) => ({ text: (c.text ?? c.content ?? c.chunk ?? "").trim(), src: c.source ?? c.path ?? c.file ?? "?" })).filter((c) => c.text).slice(0, 5);
  if (!items.length) return null;
  const block = items.map((c, n) => `[${n + 1}] ${c.src}\n${c.text}`).join("\n\n");
  return { block: `Фрагменты из файлов пользователя (используй их, если они относятся к вопросу, и ссылайся на номер источника [N]):\n\n${block}\n\n---\n\n`, sources: [...new Set(items.map((c) => c.src))] };
}

/** route_target: a ready «Помощник» for simple prompts (features → routing), else the main model. Never throws. */
async function route(prompt: string, hasAttachments: boolean): Promise<RouteTarget | null> {
  try { const r = await routeTarget(prompt, hasAttachments); return r?.kind === "helper" && r.base ? r : null; }
  catch (e) { console.warn("route_target", e); return null; }
}

export default function ChatView() {
  const s = useCluster();
  const conv = useCluster(activeConversation);
  const { run, setView, snapshot } = s;
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState<string | null>(null);
  const abort = useRef<AbortController | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true); // follow new tokens only while the user is at the bottom
  const wide = useWide();
  const [side, setSide] = useState(false);
  const ready = run.status === "ready";
  const warming = ready && !!snapshot?.engine.warming;
  const messages = conv.messages.filter((m) => !m.hidden);
  const helpers = readyHelpers(snapshot);
  const [rag, setRag] = useState(() => localStorage.getItem("argus.ragSearch") === "1");
  const [voiceUrl, setVoiceUrl] = useState<string | undefined>(undefined);
  useEffect(() => { featuresGet().then((f) => setVoiceUrl(f?.voice?.enabled && f.voice.url ? f.voice.url : undefined)).catch(() => {}); }, [conv.id]);

  // KV session save/restore on chat switch (eng-ui api; skipped when absent)
  const prevConv = useRef(conv.id);
  useEffect(() => {
    const prev = prevConv.current; prevConv.current = conv.id;
    if (prev === conv.id || run.status !== "ready") return;
    (async () => {
      try { await saveChatSession(prev); } catch (e) { console.warn("saveChatSession", e); }
      try { await restoreChatSession(conv.id); } catch (e) { console.warn("restoreChatSession", e); }
    })();
  }, [conv.id, run.status]);

  // «Сводка» (0.3.2): a ready «Помощник», otherwise the main model
  const [summary, setSummary] = useState<{ convId: string; by: string; text: string; busy: boolean; error?: string } | null>(null);
  const sumJob = useRef<SummaryJob | null>(null);
  useEffect(() => { sumJob.current?.cancel(); sumJob.current = null; setSummary(null); }, [conv.id]);
  function runSummary() {
    sumJob.current?.cancel();
    const convId = conv.id;
    const job = summarize(conv.messages, (text) => setSummary((x) => (x && x.convId === convId ? { ...x, text } : x)));
    if (!job) { s.pushToast("warn", "Для сводки нужен готовый Помощник или загруженная модель"); return; }
    sumJob.current = job;
    setSummary({ convId, by: job.by, text: "", busy: true });
    job.done.then((text) => setSummary((x) => (x && x.convId === convId ? { ...x, text, busy: false } : x)))
      .catch((e) => setSummary((x) => (x && x.convId === convId ? { ...x, busy: false, error: chatErrorText(e) ?? "остановлено" } : x)));
  }
  const canSummary = messages.some((m) => m.role === "assistant" && m.content.trim()) && (helpers.length > 0 || (ready && !busy));

  // agent state
  const [dangerous, setDangerous] = useState(() => localStorage.getItem("argus.agentDangerous") === "1");
  const [allowAll, setAllowAll] = useState(false);
  const [pending, setPending] = useState<{ call: ToolCall; resolve: (d: Decision) => void } | null>(null);
  const [step, setStep] = useState(0);
  const [actionLog, setActionLog] = useState<string[]>([]);
  const allowAllRef = useRef(false);
  useEffect(() => { allowAllRef.current = allowAll; }, [allowAll]);
  useEffect(() => { setAllowAll(false); }, [conv.id]); // "allow all" is per session

  // autoscroll: instant while streaming (smooth scrolling every token jittered), never when the user scrolled up to read
  const lastLen = messages[messages.length - 1]?.content.length ?? 0;
  useEffect(() => { stick.current = true; }, [conv.id]);
  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [conv.id, conv.messages.length, lastLen, pending, summary?.text]);
  const onScroll = () => { const el = scroller.current; if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80; };

  const sys = (): ChatMessage[] => (conv.systemPrompt.trim() ? [{ id: "sys", role: "system", content: conv.systemPrompt }] : []);

  async function generate(history: ChatMessage[]) {
    const convId = conv.id;
    const st = useCluster.getState();
    const ctrl = new AbortController();
    abort.current = ctrl;
    const untrack = trackGeneration(ctrl); // aborted with a reason if the engine dies / loses a node
    setBusy(true);
    try {
      if (conv.agent) {
        const vision = await engineHasVision();
        setStep(0);
        await runAgent(history, AGENT_SYSTEM(vision, dangerous) + (conv.systemPrompt ? `\n\n${conv.systemPrompt}` : ""),
          { dangerous, vision, signal: ctrl.signal }, {
            push: (m) => st.pushMessage(m, convId),
            patch: (id, p) => st.patchMessage(id, p, convId),
            onStep: setStep,
            log: (l) => setActionLog((x) => [...x.slice(-60), `${new Date().toLocaleTimeString()} ${l}`]),
            confirm: (call) => allowAllRef.current ? Promise.resolve("allow") : new Promise<Decision>((resolve) => {
              const onAbort = () => resolve("deny");
              abort.current?.signal.addEventListener("abort", onAbort, { once: true });
              setPending({ call, resolve: (d) => { setPending(null); if (d === "allow-all") setAllowAll(true); resolve(d); } });
            }),
          });
        return;
      }
      const lastUserMsg = [...history].reverse().find((m) => m.role === "user");
      const prompt = lastUserMsg?.content ?? "";
      const hasAtt = !!(lastUserMsg?.files?.length || lastUserMsg?.images?.length);
      const [ctxRag, routed0] = await Promise.all([rag && prompt ? ragContext(prompt) : Promise.resolve(null), route(prompt, hasAtt)]);
      const routed = rag && ctxRag ? null : routed0; // file context goes to the main model
      if (rag && prompt && !ctxRag) st.pushToast("info", "В ваших файлах ничего не нашлось (или поиск недоступен) — отвечаю без них");
      let apiMsgs = toApi([...sys(), ...history], false);
      if (history.some((m) => m.images?.length)) {
        if (await engineHasVision()) apiMsgs = withImages(history, apiMsgs);
        else st.pushToast("warn", "Загруженная модель не видит изображения — фото не отправлены. Выберите модель со зрением (vision).");
      }
      if (ctxRag) {
        const li = apiMsgs.map((a) => a.role).lastIndexOf("user");
        const a = apiMsgs[li];
        if (a) apiMsgs[li] = { ...a, content: typeof a.content === "string" ? ctxRag.block + a.content
          : Array.isArray(a.content) ? [{ type: "text", text: ctxRag.block }, ...a.content] : a.content };
      }
      const aid = uid();
      st.pushMessage({ id: aid, role: "assistant", content: "", pending: true, model: routed ? `${routed.model} · Помощник` : undefined, sources: ctxRag?.sources }, convId);
      const t0 = performance.now(); let first = 0, n = 0, server = false;
      const onStats = (x: { tps: number; promptTps?: number; tokens: number; ttftMs?: number }) => {
        server = true; st.patchMessage(aid, { tps: x.tps, promptTps: x.promptTps, tokens: x.tokens, ttftMs: x.ttftMs }, convId);
      };
      try {
        const pump = async (base: string) => {
          for await (const chunk of streamChat(apiMsgs, ctrl.signal, onStats, undefined, base)) {
            n++; const now = performance.now(); if (!first) first = now;
            if (!server && n > 1) st.patchMessage(aid, { tokens: n, ttftMs: first - t0, tps: (n - 1) / ((now - first) / 1000) }, convId);
            st.appendToMessage(aid, chunk, convId);
          }
        };
        if (routed) {
          try { await pump(routed.base); }
          catch (e) {
            if (n > 0 || ctrl.signal.aborted) throw e;
            console.warn("helper route failed, falling back to the main model", e);
            st.patchMessage(aid, { model: undefined }, convId);
            await pump(LLAMA_URL);
          }
        } else await pump(LLAMA_URL);
      } finally { st.patchMessage(aid, { pending: false }, convId); }
      autoTitle(convId); // «Помощник» names the chat after the first answer (no helper → title stays)
    } catch (e) {
      let text = chatErrorText(e, ctrl.signal);
      if (text?.startsWith("Нет связи")) {
        // the socket usually dies a moment before the supervisor's engine-state event: prefer its precise reason
        await new Promise((r) => setTimeout(r, 1500));
        const r = useCluster.getState().run;
        if (r.status === "broken") text = `Генерация прервана: ${r.reason}`;
      }
      if (text) st.pushMessage({ id: uid(), role: "assistant", content: `**[ошибка]** ${text}` }, convId);
    } finally {
      untrack();
      setPending(null);
      setBusy(false);
      // mark any dangling pending assistant message as finished
      useCluster.getState().conversations.find((c) => c.id === convId)?.messages.filter((m) => m.pending)
        .forEach((m) => useCluster.getState().patchMessage(m.id, { pending: false }, convId));
    }
  }

  function send(text: string, files: Attachment[], images: ImageAttachment[] = []) {
    if (images.length && conv.agent) s.pushToast("warn", "В режиме агента фото не отправляются — выключите «Агент», чтобы спросить модель о фото");
    const user: ChatMessage = { id: uid(), role: "user", content: text, files: files.length ? files : undefined,
      images: images.length && !conv.agent ? images.map((i) => i.url) : undefined };
    const history = [...conv.messages, user];
    stick.current = true;
    s.pushMessage(user);
    generate(history);
  }

  // regenerate: drop everything after the last user message and run again
  function regenerate() {
    const i = conv.messages.map((m) => m.role).lastIndexOf("user");
    if (i < 0) return;
    const next = conv.messages[i + 1];
    if (next) s.truncateFrom(next.id);
    generate(conv.messages.slice(0, i + 1));
  }

  // edit last user message: put it back into the prompt and remove it (+ replies)
  function editLast() {
    const i = conv.messages.map((m) => m.role).lastIndexOf("user");
    if (i < 0) return;
    setDraft(conv.messages[i].content);
    s.truncateFrom(conv.messages[i].id);
  }

  const stop = () => { abort.current?.abort(); pending?.resolve("deny"); };
  const lastUser = messages.map((m) => m.role).lastIndexOf("user");
  const lastAssistant = messages.map((m) => m.role).lastIndexOf("assistant");
  const draftUsed = useCallback(() => setDraft(null), []);
  // stable handlers so memoized messages don't re-render on every snapshot poll
  const regenRef = useRef(regenerate); regenRef.current = regenerate;
  const editRef = useRef(editLast); editRef.current = editLast;
  const onRegen = useCallback(() => regenRef.current(), []);
  const onEditLast = useCallback(() => editRef.current(), []);

  return (
    <div className="relative grid h-full grid-cols-[190px_minmax(0,1fr)] gap-px bg-pl-line lg:grid-cols-[210px_minmax(0,1fr)] xl:grid-cols-[220px_minmax(0,1fr)_270px]">
      <ChatList />
      <div className="flex min-h-0 min-w-0 flex-col bg-pl-bg0">
        <div className="panel-head gap-2">
          <span className="min-w-0 truncate" title={`${conv.title} · ${ready ? run.model.name : "нет модели"}`}>{conv.title} · {ready ? run.model.name : "нет модели"}</span>
          <div className="flex shrink-0 items-center gap-2 normal-case tracking-normal">
            <select value={SYSTEM_PRESETS.find((p) => p.prompt === conv.systemPrompt)?.id ?? "custom"}
              onChange={(e) => { const p = SYSTEM_PRESETS.find((x) => x.id === e.target.value); if (p) s.patchChat(conv.id, { systemPrompt: p.prompt }); }}
              className="h-6 w-[150px] min-w-0 border border-pl-line2 bg-pl-bg3 px-1 text-2xs text-pl-text 2xl:w-auto" title="Системный промпт">
              {SYSTEM_PRESETS.map((p) => <option key={p.id} value={p.id}>{p.label}</option>)}
              <option value="custom" disabled>Свой промпт</option>
            </select>
            <button data-tour="agent" onClick={() => s.patchChat(conv.id, { agent: !conv.agent })} disabled={busy}
              className={clsx("flex h-6 shrink-0 items-center gap-1 whitespace-nowrap border px-2 text-2xs", conv.agent ? "border-pl-gold/70 bg-pl-gold/10 text-pl-gold" : "border-pl-line2 text-pl-dim hover:text-pl-text")}>
              <Bot size={12} /> Агент <span className="hidden lg:inline">{conv.agent ? "ВКЛ" : "выкл"}</span>{conv.agent && <span className="lg:hidden">✓</span>}
            </button>
            {!conv.agent && (
              <button onClick={() => { const v = !rag; setRag(v); localStorage.setItem("argus.ragSearch", v ? "1" : "0"); }}
                className={clsx("flex h-6 shrink-0 items-center gap-1 whitespace-nowrap border px-2 text-2xs", rag ? "border-pl-green/70 bg-pl-green/10 text-pl-green" : "border-pl-line2 text-pl-dim hover:text-pl-text")}
                title="Искать в моих файлах: перед ответом найти подходящие фрагменты в проиндексированных файлах и передать их модели">
                <FolderSearch size={12} /> <span className="hidden lg:inline">Искать в моих файлах</span>{rag && <span className="lg:hidden">✓</span>}
              </button>
            )}
            <button className="flex h-6 shrink-0 items-center gap-1 whitespace-nowrap border border-pl-line2 px-2 text-2xs text-pl-dim hover:text-pl-text disabled:opacity-40"
              onClick={runSummary} disabled={!canSummary || !!summary?.busy}
              title={helpers.length ? `Сводка переписки — сделает Помощник «${helpers[0].name}», основная модель не отвлекается` : "Сводка переписки — сделает основная модель (Помощника нет)"}>
              {summary?.busy ? <Loader2 size={12} className="animate-spin" /> : <ScrollText size={12} />} <span className="hidden lg:inline">Сводка</span>
            </button>
            <button className="flex shrink-0 items-center gap-1 whitespace-nowrap text-pl-dim hover:text-pl-text" title="Очистить чат" onClick={() => s.clearChat()} disabled={busy}>
              <Trash2 size={12} /> <span className="hidden lg:inline">Очистить</span>
            </button>
            {!wide && (
              <button className={clsx("flex h-6 shrink-0 items-center gap-1 whitespace-nowrap border px-2 text-2xs", side ? "border-pl-blue text-pl-text" : "border-pl-line2 text-pl-dim hover:text-pl-text")}
                title={conv.agent ? "Панель агента: опасные инструменты, журнал действий" : "Свойства сессии"} onClick={() => setSide(!side)}>
                <PanelRight size={12} /> {conv.agent ? "Панель" : "Сессия"}
              </button>
            )}
          </div>
        </div>
        <EngineBanner compact />
        <div ref={scroller} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto [overflow-anchor:none]">
          {messages.length === 0 ? (
            <div className="dot-grid flex min-h-full flex-col items-center justify-center p-4 text-center lg:p-6">
              {run.status === "broken" ? (
                <div className="max-w-md text-xs text-pl-mute">Движок остановлен. Как только кластер восстановится, модель {run.model.name} перезапустится автоматически.</div>
              ) : ready ? (
                <>
                  <div className="label">{conv.agent ? "Режим агента" : "Движок готов"}</div>
                  <div className="mt-2 text-lg text-pl-text">{run.model.name} распределена по {snapshot?.split.length || 1} устройствам</div>
                  {conv.agent && <div className="mt-2 max-w-md text-xs text-pl-mute">Агент может читать файлы, запускать команды, открывать ссылки{dangerous ? ", управлять мышью и клавиатурой" : " (опасные инструменты выключены)"}. Каждое действие требует вашего подтверждения.</div>}
                </>
              ) : (
                <>
                  <QuickStart />
                  <button className="btn mt-3" onClick={() => setView("models")}><Boxes size={12} /> Открыть реестр моделей</button>
                </>
              )}
            </div>
          ) : (
            <div className="mx-auto max-w-4xl space-y-6 px-4 py-6 lg:px-6">
              {messages.map((m, i) => (
                <MessageBubble key={m.id} m={m} model={ready ? run.model.name : undefined}
                  onRegenerate={!busy && i === lastAssistant && i > lastUser ? onRegen : undefined}
                  onEdit={!busy && i === lastUser ? onEditLast : undefined} />
              ))}
              {pending && <ConfirmCard call={pending.call} onDecide={pending.resolve} />}
              {summary && summary.convId === conv.id && <SummaryCard sum={summary} onClose={() => { sumJob.current?.cancel(); setSummary(null); }} />}
            </div>
          )}
        </div>
        <PromptBar busy={busy} disabled={!ready} onSend={send} onStop={stop} draft={draft} onDraftUsed={draftUsed} voiceUrl={voiceUrl}
          hint={conv.agent ? `агент · шаг ${step}/${MAX_STEPS} · каждое действие подтверждается` : warming ? "прогрев… первый ответ может начаться чуть позже" : undefined}
          placeholder={ready ? (conv.agent ? "Задача для агента: например, «найди в Загрузках самый большой файл»" : `Запрос к ${run.model.name}…`)
            : run.status === "broken" ? "Движок остановлен — ждём ноды или нажмите «Перезапустить движок»"
            : run.status === "loading" ? `Загрузка ${run.model.name}…` : "Сначала загрузите модель"} />
      </div>
      <Drawer wide={wide} open={side} onClose={() => setSide(false)} title={conv.agent ? "Агент" : "Свойства сессии"} width={300}>
        {conv.agent
          ? <AgentPanel dangerous={dangerous} setDangerous={(v) => { setDangerous(v); localStorage.setItem("argus.agentDangerous", v ? "1" : "0"); }}
              allowAll={allowAll} setAllowAll={setAllowAll} step={step} log={actionLog} busy={busy} onStop={stop} />
          : <SessionPanel />}
      </Drawer>
    </div>
  );
}

function ChatList() {
  const { conversations, activeId, selectChat, newChat, deleteChat } = useCluster();
  const sorted = [...conversations].sort((a, b) => b.updatedAt - a.updatedAt);
  return (
    <aside className="flex min-h-0 flex-col bg-pl-bg1">
      <div className="panel-head"><span>Чаты</span>
        <button className="flex shrink-0 items-center gap-1 normal-case tracking-normal text-pl-blue hover:text-pl-text" onClick={newChat}><MessageSquarePlus size={12} /> Новый</button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {sorted.map((c) => (
          <div key={c.id} onClick={() => selectChat(c.id)}
            className={clsx("group flex cursor-pointer items-center gap-2 border-b border-pl-line px-3 py-2 text-xs",
              c.id === activeId ? "bg-pl-blue2/20 text-pl-text" : "text-pl-mute hover:bg-pl-bg3")}>
            {c.agent ? <Bot size={12} className="shrink-0 text-pl-gold" /> : <span className="h-1.5 w-1.5 shrink-0 bg-pl-line2" />}
            <div className="min-w-0 flex-1">
              <div className="truncate" title={c.title}>{c.title}</div>
              <div className="truncate font-mono text-2xs text-pl-dim">{c.messages.filter((m) => m.role === "user").length} сообщ. · {new Date(c.updatedAt).toLocaleDateString()}</div>
            </div>
            <button className="text-pl-dim opacity-0 hover:text-pl-red group-hover:opacity-100" title="Удалить" onClick={(e) => { e.stopPropagation(); deleteChat(c.id); }}><X size={12} /></button>
          </div>
        ))}
      </div>
    </aside>
  );
}

function SummaryCard({ sum, onClose }: { sum: { by: string; text: string; busy: boolean; error?: string }; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="border border-pl-violet/50 bg-pl-bg1">
      <div className="flex items-center gap-2 border-b border-pl-line px-3 py-1.5 text-xs">
        <ScrollText size={13} className="text-pl-violet" />
        <span className="font-semibold text-pl-violet">Сводка</span>
        <span className="truncate font-mono text-2xs text-pl-dim">{sum.by}</span>
        {sum.busy && <Loader2 size={11} className="animate-spin text-pl-violet" />}
        <span className="ml-auto flex gap-2 text-pl-dim">
          {sum.text && <button className="hover:text-pl-text" title="Копировать" onClick={() => { navigator.clipboard.writeText(sum.text); setCopied(true); setTimeout(() => setCopied(false), 1200); }}>{copied ? <Check size={12} /> : <Copy size={12} />}</button>}
          <button className="hover:text-pl-text" title={sum.busy ? "Остановить и закрыть" : "Закрыть"} onClick={onClose}><X size={12} /></button>
        </span>
      </div>
      <div className="px-3 py-2 text-sm">
        {sum.text ? <Markdown text={sum.text} /> : sum.busy ? <span className="text-xs text-pl-dim">читаю переписку…</span> : null}
        {sum.error && <div className="mt-1 text-xs text-pl-red">Сводка не получилась: {sum.error}</div>}
      </div>
      <div className="border-t border-pl-line px-3 py-1 text-2xs text-pl-dim">Сводка не попадает в историю чата и не отправляется модели.</div>
    </div>
  );
}

function ConfirmCard({ call, onDecide }: { call: ToolCall; onDecide: (d: Decision) => void }) {
  let args: Record<string, unknown> = {};
  try { args = JSON.parse(call.arguments || "{}"); } catch { /* */ }
  const danger = isDangerous(call.name);
  return (
    <div className={clsx("border bg-pl-bg1", danger ? "border-pl-orange/70" : "border-pl-blue/60")}>
      <div className={clsx("flex items-center gap-2 border-b px-3 py-1.5 text-xs font-semibold", danger ? "border-pl-orange/40 text-pl-orange" : "border-pl-line text-pl-blue")}>
        {danger ? <ShieldAlert size={13} /> : <ShieldCheck size={13} />} Агент хочет: {TOOL_LABEL[call.name] ?? call.name}
      </div>
      <pre className="max-h-48 overflow-auto whitespace-pre-wrap p-3 font-mono text-xs text-pl-text">
        {call.name === "run_command" ? `${PLATFORM.os === "windows" ? "PS>" : "$"} ${args.command}${args.cwd ? `\n(в папке ${args.cwd})` : ""}`
          : call.name === "write_file" ? `${args.path}\n──────\n${String(args.content ?? "").slice(0, 1500)}`
          : JSON.stringify(args, null, 2)}
      </pre>
      <div className="flex flex-wrap gap-2 border-t border-pl-line bg-pl-bg2 px-3 py-2">
        <button className="btn-primary" onClick={() => onDecide("allow")}>Разрешить</button>
        <button className="btn" onClick={() => onDecide("allow-all")}>Разрешить всё в этой сессии</button>
        <button className="btn ml-auto border-pl-red/60 text-pl-red" onClick={() => onDecide("deny")}>Отклонить</button>
      </div>
    </div>
  );
}

function AgentPanel({ dangerous, setDangerous, allowAll, setAllowAll, step, log, busy, onStop }: {
  dangerous: boolean; setDangerous: (v: boolean) => void; allowAll: boolean; setAllowAll: (v: boolean) => void;
  step: number; log: string[]; busy: boolean; onStop: () => void;
}) {
  const { run, setView } = useCluster();
  const recommended = MODELS.filter((m) => AGENT_MODELS.includes(m.id));
  return (
    <aside className="flex min-h-0 flex-col bg-pl-bg1">
      <div className="panel-head"><span>Агент</span>{busy && <button className="btn h-6 border-pl-red/60 text-pl-red" onClick={onStop}>Стоп</button>}</div>
      <div className="space-y-2 border-b border-pl-line p-3 text-xs">
        <Toggle on={dangerous} set={setDangerous} label="Опасные инструменты" sub="команды, запись файлов, мышь и клавиатура" warn />
        <Toggle on={allowAll} set={setAllowAll} label="Разрешить всё в этой сессии" sub="без карточек подтверждения" warn />
        <div className="flex justify-between font-mono text-2xs text-pl-dim"><span>шаг</span><span className="text-pl-text">{step} / {MAX_STEPS}</span></div>
      </div>
      <div className="panel-head"><span>Журнал действий</span></div>
      <div className="min-h-0 flex-1 overflow-y-auto p-2 font-mono text-2xs leading-5 text-pl-mute">
        {log.length ? log.map((l, i) => <div key={i} className={l.includes("✕") || l.includes("!") ? "text-pl-orange" : ""}>{l}</div>)
          : <div className="text-pl-dim">пока пусто</div>}
      </div>
      <div className="border-t border-pl-line p-3 text-2xs leading-5 text-pl-dim">
        <div className="label mb-1">Модели для агента</div>
        {recommended.map((m) => (
          <div key={m.id} className="flex justify-between gap-2"><span className={"truncate " + (run.status === "ready" && run.model.id === m.id ? "text-pl-green" : "text-pl-mute")}>{m.name}</span>
            <span className="shrink-0">{m.vision ? "видит экран" : m.moe ? "MoE" : "tools"}</span></div>
        ))}
        <button className="mt-2 text-pl-blue hover:underline" onClick={() => setView("models")}>открыть реестр →</button>
      </div>
    </aside>
  );
}

function Toggle({ on, set, label, sub, warn }: { on: boolean; set: (v: boolean) => void; label: string; sub?: string; warn?: boolean }) {
  return (
    <label className="flex cursor-pointer items-center justify-between gap-3">
      <span><span className={on && warn ? "text-pl-orange" : "text-pl-text"}>{label}</span>{sub && <span className="block text-2xs text-pl-dim">{sub}</span>}</span>
      <button onClick={() => set(!on)} className={clsx("relative h-4 w-8 shrink-0 rounded-full transition", on ? (warn ? "bg-pl-orange" : "bg-pl-blue2") : "bg-pl-bg4")}>
        <span className={clsx("absolute top-0.5 h-3 w-3 rounded-full bg-white transition-all", on ? "left-[18px]" : "left-0.5")} />
      </button>
    </label>
  );
}
export { Toggle };

function SessionPanel() {
  const { run, snapshot, ctx, speed } = useCluster();
  const conv = useCluster(activeConversation);
  const m = run.status === "ready" || run.status === "loading" ? run.model : null;
  const rows: [string, string][] = [
    ["Движок", "llama.cpp b12008"], ["Модель", m?.name ?? "—"], ["Квант", m?.quant ?? "—"], ["Контекст", `${ctx / 1024}K`],
    ["RPC-ноды", String(snapshot?.rpcServers.length ?? 0)], ["Сообщений", String(conv.messages.length)],
    ["Черновик", snapshot?.draftEnabled ? "вкл" : "выкл"], ["Кэш промпта", speed.promptCache ? "вкл" : "выкл"],
  ];
  return (
    <aside className="flex flex-col overflow-y-auto bg-pl-bg1">
      <div className="panel-head"><span>Свойства сессии</span></div>
      <dl className="divide-y divide-pl-line text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="flex justify-between gap-3 px-4 py-2"><dt className="shrink-0 text-pl-dim">{k}</dt><dd className="truncate font-mono" title={v}>{v}</dd></div>
        ))}
      </dl>
      <div className="panel-head mt-4 border-t border-pl-line"><span>Размещение слоёв (-ts)</span></div>
      <div className="space-y-2 p-4 font-mono text-2xs text-pl-mute">
        {snapshot?.split.map((e) => (
          <div key={e.device}>
            <div className="flex justify-between gap-2"><span className="truncate" title={e.name}>{e.name}</span><span className="text-pl-text">{(e.share * 100).toFixed(0)}%</span></div>
            <div className="mt-0.5 flex justify-between text-pl-dim"><span>{e.device}</span></div>
          </div>
        ))}
        {!snapshot?.split.length && <div>авто (llama.cpp --fit)</div>}
        <div className="pt-2 text-pl-dim">быстрые устройства заполняются первыми</div>
      </div>
      <HelpersMini />
    </aside>
  );
}

/** «Помощники» in the session panel (experiment «Роли»). */
function HelpersMini() {
  const { snapshot, setView } = useCluster();
  if (!snapshot?.rolesEnabled || !snapshot.helpers.length) return null;
  return (
    <>
      <div className="panel-head border-t border-pl-line"><span>Помощники</span></div>
      <div className="space-y-1.5 p-4 font-mono text-2xs">
        {snapshot.helpers.map((h) => (
          <div key={h.id} className="flex justify-between gap-2">
            <span className="truncate text-pl-mute" title={h.name}>{h.name}</span>
            <span className={"shrink-0 tabular-nums " + (h.ready ? "text-pl-green" : h.state === "error" ? "text-pl-red" : "text-pl-orange")}>{h.ready ? "готов" : h.state === "error" ? "ошибка" : helperPct(h) != null ? `${Math.round(helperPct(h)!)}%` : "готовится"}</span>
          </div>
        ))}
        <button className="mt-2 flex items-center gap-1 font-sans text-pl-blue hover:underline" onClick={() => setView("parallel")}><Columns3 size={11} /> Параллельный тест →</button>
      </div>
    </>
  );
}
