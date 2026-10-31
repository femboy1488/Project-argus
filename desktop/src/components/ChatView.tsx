import { useCallback, useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Bot, Boxes, MessageSquarePlus, ShieldAlert, ShieldCheck, Trash2, X } from "lucide-react";
import { useCluster, activeConversation, SYSTEM_PRESETS } from "../store/cluster";
import { engineHasVision, streamChat } from "../lib/api";
import { uid } from "../lib/format";
import { MAX_STEPS, TOOL_LABEL, isDangerous, runAgent, toApi, AGENT_SYSTEM, type Decision } from "../lib/agent";
import { AGENT_MODELS, MODELS } from "../lib/models";
import type { ChatMessage, ToolCall } from "../lib/types";
import MessageBubble from "./MessageBubble";
import PromptBar, { type Attachment } from "./PromptBar";
import QuickStart from "./QuickStart";

export default function ChatView() {
  const s = useCluster();
  const conv = useCluster(activeConversation);
  const { run, setView, snapshot } = s;
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState<string | null>(null);
  const abort = useRef<AbortController | null>(null);
  const bottom = useRef<HTMLDivElement>(null);
  const ready = run.status === "ready";
  const messages = conv.messages.filter((m) => !m.hidden);

  // agent state
  const [dangerous, setDangerous] = useState(() => localStorage.getItem("argus.agentDangerous") === "1");
  const [allowAll, setAllowAll] = useState(false);
  const [pending, setPending] = useState<{ call: ToolCall; resolve: (d: Decision) => void } | null>(null);
  const [step, setStep] = useState(0);
  const [actionLog, setActionLog] = useState<string[]>([]);
  const allowAllRef = useRef(false);
  useEffect(() => { allowAllRef.current = allowAll; }, [allowAll]);
  useEffect(() => { setAllowAll(false); }, [conv.id]); // "allow all" is per session

  useEffect(() => { bottom.current?.scrollIntoView({ behavior: "smooth" }); }, [conv.messages.length, messages[messages.length - 1]?.content, pending]);

  const sys = (): ChatMessage[] => (conv.systemPrompt.trim() ? [{ id: "sys", role: "system", content: conv.systemPrompt }] : []);

  async function generate(history: ChatMessage[]) {
    const convId = conv.id;
    const st = useCluster.getState();
    abort.current = new AbortController();
    setBusy(true);
    try {
      if (conv.agent) {
        const vision = await engineHasVision();
        setStep(0);
        await runAgent(history, AGENT_SYSTEM(vision, dangerous) + (conv.systemPrompt ? `\n\n${conv.systemPrompt}` : ""),
          { dangerous, vision, signal: abort.current.signal }, {
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
      const aid = uid();
      st.pushMessage({ id: aid, role: "assistant", content: "", pending: true }, convId);
      const t0 = performance.now(); let first = 0, n = 0, server = false;
      const onStats = (x: { tps: number; promptTps?: number; tokens: number; ttftMs?: number }) => {
        server = true; st.patchMessage(aid, { tps: x.tps, promptTps: x.promptTps, tokens: x.tokens, ttftMs: x.ttftMs }, convId);
      };
      try {
        for await (const chunk of streamChat(toApi([...sys(), ...history], false), abort.current.signal, onStats)) {
          n++; const now = performance.now(); if (!first) first = now;
          if (!server && n > 1) st.patchMessage(aid, { tokens: n, ttftMs: first - t0, tps: (n - 1) / ((now - first) / 1000) }, convId);
          st.appendToMessage(aid, chunk, convId);
        }
      } finally { st.patchMessage(aid, { pending: false }, convId); }
    } catch (e) {
      if ((e as Error).name !== "AbortError")
        st.pushMessage({ id: uid(), role: "assistant", content: `**[ошибка]** ${(e as Error).message}` }, convId);
    } finally {
      setPending(null);
      setBusy(false);
      // mark any dangling pending assistant message as finished
      useCluster.getState().conversations.find((c) => c.id === convId)?.messages.filter((m) => m.pending)
        .forEach((m) => useCluster.getState().patchMessage(m.id, { pending: false }, convId));
    }
  }

  function send(text: string, files: Attachment[]) {
    const user: ChatMessage = { id: uid(), role: "user", content: text, files: files.length ? files : undefined };
    const history = [...conv.messages, user];
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

  return (
    <div className="grid h-full grid-cols-[220px_1fr_270px] gap-px bg-pl-line">
      <ChatList />
      <div className="flex min-h-0 min-w-0 flex-col bg-pl-bg0">
        <div className="panel-head gap-2">
          <span className="truncate">{conv.title} · {ready ? run.model.name : "нет модели"}</span>
          <div className="flex items-center gap-2 normal-case tracking-normal">
            <select value={SYSTEM_PRESETS.find((p) => p.prompt === conv.systemPrompt)?.id ?? "custom"}
              onChange={(e) => { const p = SYSTEM_PRESETS.find((x) => x.id === e.target.value); if (p) s.patchChat(conv.id, { systemPrompt: p.prompt }); }}
              className="h-6 border border-pl-line2 bg-pl-bg3 px-1 text-2xs text-pl-text" title="Системный промпт">
              {SYSTEM_PRESETS.map((p) => <option key={p.id} value={p.id}>{p.label}</option>)}
              <option value="custom" disabled>Свой промпт</option>
            </select>
            <button data-tour="agent" onClick={() => s.patchChat(conv.id, { agent: !conv.agent })} disabled={busy}
              className={clsx("flex h-6 shrink-0 items-center gap-1 whitespace-nowrap border px-2 text-2xs", conv.agent ? "border-pl-gold/70 bg-pl-gold/10 text-pl-gold" : "border-pl-line2 text-pl-dim hover:text-pl-text")}>
              <Bot size={12} /> Агент {conv.agent ? "ВКЛ" : "выкл"}
            </button>
            <button className="flex items-center gap-1 text-pl-dim hover:text-pl-text" onClick={() => s.clearChat()} disabled={busy}>
              <Trash2 size={12} /> Очистить
            </button>
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {messages.length === 0 ? (
            <div className="dot-grid flex h-full flex-col items-center justify-center p-6 text-center">
              {ready ? (
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
            <div className="mx-auto max-w-4xl space-y-6 px-6 py-6">
              {messages.map((m, i) => (
                <MessageBubble key={m.id} m={m} model={ready ? run.model.name : undefined}
                  onRegenerate={!busy && i === lastAssistant && i > lastUser ? regenerate : undefined}
                  onEdit={!busy && i === lastUser ? editLast : undefined} />
              ))}
              {pending && <ConfirmCard call={pending.call} onDecide={pending.resolve} />}
              <div ref={bottom} />
            </div>
          )}
        </div>
        <PromptBar busy={busy} disabled={!ready} onSend={send} onStop={stop} draft={draft} onDraftUsed={draftUsed}
          hint={conv.agent ? `агент · шаг ${step}/${MAX_STEPS} · каждое действие подтверждается` : undefined}
          placeholder={ready ? (conv.agent ? "Задача для агента: например, «найди в Загрузках самый большой файл»" : `Запрос к ${run.model.name}…`) : "Сначала загрузите модель"} />
      </div>
      {conv.agent
        ? <AgentPanel dangerous={dangerous} setDangerous={(v) => { setDangerous(v); localStorage.setItem("argus.agentDangerous", v ? "1" : "0"); }}
            allowAll={allowAll} setAllowAll={setAllowAll} step={step} log={actionLog} busy={busy} onStop={stop} />
        : <SessionPanel />}
    </div>
  );
}

function ChatList() {
  const { conversations, activeId, selectChat, newChat, deleteChat } = useCluster();
  const sorted = [...conversations].sort((a, b) => b.updatedAt - a.updatedAt);
  return (
    <aside className="flex min-h-0 flex-col bg-pl-bg1">
      <div className="panel-head"><span>Чаты</span>
        <button className="flex items-center gap-1 normal-case tracking-normal text-pl-blue hover:text-pl-text" onClick={newChat}><MessageSquarePlus size={12} /> Новый</button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {sorted.map((c) => (
          <div key={c.id} onClick={() => selectChat(c.id)}
            className={clsx("group flex cursor-pointer items-center gap-2 border-b border-pl-line px-3 py-2 text-xs",
              c.id === activeId ? "bg-pl-blue2/20 text-pl-text" : "text-pl-mute hover:bg-pl-bg3")}>
            {c.agent ? <Bot size={12} className="shrink-0 text-pl-gold" /> : <span className="h-1.5 w-1.5 shrink-0 bg-pl-line2" />}
            <div className="min-w-0 flex-1">
              <div className="truncate">{c.title}</div>
              <div className="font-mono text-2xs text-pl-dim">{c.messages.filter((m) => m.role === "user").length} сообщ. · {new Date(c.updatedAt).toLocaleDateString()}</div>
            </div>
            <button className="text-pl-dim opacity-0 hover:text-pl-red group-hover:opacity-100" title="Удалить" onClick={(e) => { e.stopPropagation(); deleteChat(c.id); }}><X size={12} /></button>
          </div>
        ))}
      </div>
    </aside>
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
        {call.name === "run_command" ? `PS> ${args.command}${args.cwd ? `\n(в папке ${args.cwd})` : ""}`
          : call.name === "write_file" ? `${args.path}\n──────\n${String(args.content ?? "").slice(0, 1500)}`
          : JSON.stringify(args, null, 2)}
      </pre>
      <div className="flex gap-2 border-t border-pl-line bg-pl-bg2 px-3 py-2">
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
          <div key={m.id} className="flex justify-between"><span className={run.status === "ready" && run.model.id === m.id ? "text-pl-green" : "text-pl-mute"}>{m.name}</span>
            <span>{m.vision ? "видит экран" : m.moe ? "MoE" : "tools"}</span></div>
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
    ["Спекулятивное", speed.speculative ? "вкл" : "выкл"], ["Кэш промпта", speed.promptCache ? "вкл" : "выкл"],
  ];
  return (
    <aside className="flex flex-col overflow-y-auto bg-pl-bg1">
      <div className="panel-head"><span>Свойства сессии</span></div>
      <dl className="divide-y divide-pl-line text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="flex justify-between px-4 py-2"><dt className="text-pl-dim">{k}</dt><dd className="font-mono">{v}</dd></div>
        ))}
      </dl>
      <div className="panel-head mt-4 border-t border-pl-line"><span>Размещение слоёв (-ts)</span></div>
      <div className="space-y-2 p-4 font-mono text-2xs text-pl-mute">
        {snapshot?.split.map((e) => (
          <div key={e.device}>
            <div className="flex justify-between"><span className="truncate">{e.name}</span><span className="text-pl-text">{(e.share * 100).toFixed(0)}%</span></div>
            <div className="mt-0.5 flex justify-between text-pl-dim"><span>{e.device}</span></div>
          </div>
        ))}
        {!snapshot?.split.length && <div>авто (llama.cpp --fit)</div>}
        <div className="pt-2 text-pl-dim">доля ∝ память × вес скорости</div>
      </div>
    </aside>
  );
}
