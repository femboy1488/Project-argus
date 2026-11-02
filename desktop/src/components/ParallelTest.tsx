// 0.3.2 «Параллельный тест»: one prompt to the main model and every ready «Помощник» at once,
// answers side by side with tok/s each. Experiment «Роли».
import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Columns3, CornerDownLeft, FlaskConical, Loader2, Network, Square, Trophy } from "lucide-react";
import { useCluster } from "../store/cluster";
import { chatErrorText, helperCancel, helperChat, onHelperStream, readyHelpers, streamChat, stripThink, trackGeneration } from "../lib/api";
import { uid } from "../lib/format";
import Markdown from "./Markdown";

type Status = "run" | "done" | "stopped" | "error";
interface Col {
  key: string;
  kind: "main" | "helper";
  name: string;    // device / role
  model: string;
  node?: string;
  streamId?: string;
  text: string;
  status: Status;
  tps: number | null;
  live: boolean;   // tps is a live estimate
  tokens: number | null;
  ms: number | null;
  ttftMs: number | null;
  error?: string;
}

const MAX_TOKENS = [128, 256, 512, 1024];
const EXAMPLES = [
  "Объясни простыми словами, как модель делится между видеокартой и телефонами.",
  "Напиши функцию на Python, которая проверяет, является ли строка палиндромом.",
  "Придумай три названия для домашнего кластера из старых телефонов.",
];

export default function ParallelTest() {
  const { snapshot, run, setView } = useCluster();
  const helpers = readyHelpers(snapshot);
  const mainReady = run.status === "ready";
  const [prompt, setPrompt] = useState("");
  const [maxTokens, setMaxTokens] = useState(256);
  const [cols, setCols] = useState<Col[]>([]);
  const [asked, setAsked] = useState("");
  const streams = useRef(new Map<string, { key: string; t0: number; first: number; n: number }>());
  const mainCtrl = useRef<AbortController | null>(null);
  const stopping = useRef(false);
  const running = cols.some((c) => c.status === "run");

  const patch = (key: string, p: Partial<Col> | ((c: Col) => Partial<Col>)) =>
    setCols((cs) => cs.map((c) => (c.key === key ? { ...c, ...(typeof p === "function" ? p(c) : p) } : c)));

  // helper answers arrive as `helper-stream` events
  useEffect(() => {
    let off: (() => void) | undefined;
    onHelperStream((d) => {
      const s = streams.current.get(d.streamId);
      if (!s) return;
      const now = performance.now();
      if (!s.first) s.first = now;
      s.n++;
      const est = s.n > 1 ? (s.n - 1) / ((now - s.first) / 1000) : null;
      patch(s.key, (c) => ({ text: c.text + d.delta, ...(c.live ? { tps: est, tokens: s.n, ttftMs: s.first - s.t0 } : {}) }));
    }).then((f) => (off = f));
    return () => off?.();
  }, []);

  // leaving the view stops everything
  useEffect(() => () => { mainCtrl.current?.abort(); streams.current.forEach((_, id) => helperCancel(id).catch(() => {})); }, []);

  function start() {
    const q = prompt.trim();
    if (!q || running) return;
    stopping.current = false;
    streams.current.clear();
    const next: Col[] = [];
    if (mainReady) next.push({ key: "main", kind: "main", name: "Основная модель", model: run.model.name, text: "", status: "run", tps: null, live: true, tokens: null, ms: null, ttftMs: null });
    for (const h of helpers) {
      const streamId = `par-${uid()}`;
      next.push({ key: h.id, kind: "helper", name: h.name, model: h.model, node: h.id, streamId, text: "", status: "run", tps: null, live: true, tokens: null, ms: null, ttftMs: null });
    }
    setCols(next);
    setAsked(q);
    const messages = [{ role: "user", content: q }];
    for (const c of next) {
      if (c.kind === "main") runMain(messages);
      else runHelper(c, messages);
    }
  }

  async function runMain(messages: { role: string; content: string }[]) {
    const ctrl = new AbortController();
    mainCtrl.current = ctrl;
    const untrack = trackGeneration(ctrl);
    const t0 = performance.now();
    let first = 0, n = 0, server = false;
    try {
      for await (const chunk of streamChat(messages, ctrl.signal, (s) => {
        server = true;
        patch("main", { tps: s.tps, tokens: s.tokens, ttftMs: s.ttftMs ?? null, live: !s.final });
      }, maxTokens)) {
        n++;
        const now = performance.now();
        if (!first) first = now;
        patch("main", (c) => ({ text: c.text + chunk, ...(!server && n > 1 ? { tps: (n - 1) / ((now - first) / 1000), tokens: n, ttftMs: first - t0 } : {}) }));
      }
      patch("main", { status: ctrl.signal.aborted && stopping.current ? "stopped" : "done", ms: performance.now() - t0, live: false });
    } catch (e) {
      const text = chatErrorText(e, ctrl.signal);
      patch("main", { status: text ? "error" : "stopped", error: text ?? undefined, ms: performance.now() - t0, live: false });
    } finally {
      untrack();
    }
  }

  async function runHelper(c: Col, messages: { role: string; content: string }[]) {
    const id = c.streamId!;
    streams.current.set(id, { key: c.key, t0: performance.now(), first: 0, n: 0 });
    try {
      const r = await helperChat(messages, { node: c.node, maxTokens, streamId: id });
      patch(c.key, (x) => ({
        text: stripThink(r.content) || stripThink(x.text), status: stopping.current ? "stopped" : "done", live: false,
        tps: r.tps ?? x.tps, tokens: r.tokens ?? x.tokens, ms: r.ms,
      }));
    } catch (e) {
      patch(c.key, { status: "error", error: String(e), live: false });
    } finally {
      streams.current.delete(id);
    }
  }

  function stop() {
    stopping.current = true;
    mainCtrl.current?.abort();
    for (const c of cols) if (c.status === "run" && c.streamId) helperCancel(c.streamId).catch(() => {});
  }

  const finished = cols.filter((c) => c.status === "done" && c.tps);
  const fastest = finished.length > 1 ? finished.reduce((a, b) => ((b.tps ?? 0) > (a.tps ?? 0) ? b : a)).key : null;
  const nothing = !mainReady && helpers.length === 0;

  return (
    <div className="flex h-full flex-col gap-3 overflow-hidden bg-pl-bg0 p-3">
      <div className="panel shrink-0">
        <div className="panel-head">
          <span className="flex shrink-0 items-center gap-1.5"><Columns3 size={12} className="text-pl-violet" /> Параллельный тест</span>
          <span className="min-w-0 truncate pl-3 font-mono normal-case tracking-normal text-pl-dim">
            {mainReady ? `основная: ${run.model.name}` : "основная модель не загружена"} · помощников готово: {helpers.length}
          </span>
        </div>
        <div className="flex gap-3 p-3">
          <div className="min-w-0 flex-1">
            <textarea value={prompt} onChange={(e) => setPrompt(e.target.value)} rows={2} disabled={nothing}
              onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); start(); } }}
              placeholder={nothing ? "Некого спрашивать: загрузите модель или включите Помощника" : "Один вопрос — всем сразу. Enter — отправить"}
              className="w-full resize-none border border-pl-line2 bg-pl-bg1 px-3 py-2 text-sm text-pl-text placeholder:text-pl-dim focus:border-pl-blue focus:outline-none" />
            <div className="mt-1.5 flex flex-wrap gap-1.5">
              {EXAMPLES.map((x) => (
                <button key={x} title={x} disabled={running || nothing} onClick={() => setPrompt(x)}
                  className="min-w-0 max-w-[300px] truncate border border-pl-line2 bg-pl-bg2 px-2 py-0.5 text-2xs text-pl-mute hover:bg-pl-bg3 hover:text-pl-text disabled:opacity-40">{x}</button>
              ))}
            </div>
          </div>
          <div className="flex w-36 shrink-0 flex-col gap-2 text-xs xl:w-44">
            <label className="flex items-center justify-between gap-2 whitespace-nowrap text-pl-mute">Макс. токенов
              <select value={maxTokens} onChange={(e) => setMaxTokens(Number(e.target.value))} disabled={running}
                className="h-6 border border-pl-line2 bg-pl-bg3 px-1 font-mono text-2xs text-pl-text">
                {MAX_TOKENS.map((t) => <option key={t} value={t}>{t}</option>)}
              </select>
            </label>
            {running ? (
              <button className="btn h-8 justify-center border-pl-red/60 text-pl-red" onClick={stop}><Square size={12} /> Стоп</button>
            ) : (
              <button className="btn-primary h-8 justify-center" disabled={!prompt.trim() || nothing} onClick={start}><CornerDownLeft size={12} /> Спросить всех</button>
            )}
          </div>
        </div>
      </div>

      {nothing ? (
        <div className="dot-grid flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
          <FlaskConical size={22} className="text-pl-violet" />
          <div className="max-w-md text-xs leading-5 text-pl-mute">
            Здесь один вопрос уходит сразу основной модели и всем готовым Помощникам — ответы видно рядом, со скоростью каждого.
            Включите эксперимент «Роли» в Cluster и дайте свободной ноде роль «Помощник», или загрузите модель в Models.
          </div>
          <div className="flex gap-2">
            <button className="btn" onClick={() => setView("devices")}><Network size={12} /> Открыть Cluster</button>
            <button className="btn" onClick={() => setView("models")}>Открыть Models</button>
          </div>
        </div>
      ) : cols.length === 0 ? (
        <div className="dot-grid flex flex-1 items-center justify-center p-6 text-center text-xs text-pl-dim">
          Ответят: {[mainReady ? `основная модель (${run.model.name})` : null, ...helpers.map((h) => `«${h.name}» (${h.model})`)].filter(Boolean).join(", ")}
        </div>
      ) : (
        <>
          <div className="shrink-0 truncate px-1 text-xs text-pl-dim" title={asked}>Вопрос: <span className="text-pl-text">{asked}</span></div>
          <div className="grid min-h-0 flex-1 gap-3 overflow-x-auto" style={{ gridTemplateColumns: `repeat(${cols.length}, minmax(280px, 1fr))` }}>
            {cols.map((c) => <Column key={c.key} c={c} fastest={fastest === c.key} />)}
          </div>
        </>
      )}
    </div>
  );
}

function Column({ c, fastest }: { c: Col; fastest: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => { if (c.status === "run") box.current?.scrollTo(0, box.current.scrollHeight); }, [c.text, c.status]);
  const tps = c.tps != null ? (c.tps >= 10 ? c.tps.toFixed(1) : c.tps.toFixed(2)) : "—";
  return (
    <div className={clsx("panel flex min-h-0 flex-col", fastest && "shadow-[inset_0_2px_0_#FBD065]")}>
      <div className="panel-head gap-2">
        <span className="flex min-w-0 items-center gap-1.5 normal-case tracking-normal">
          <span className={clsx("tag shrink-0 border", c.kind === "main" ? "border-pl-blue/60 text-pl-blue" : "border-pl-violet/60 text-pl-violet")}>{c.kind === "main" ? "ОСНОВНАЯ" : "ПОМОЩНИК"}</span>
          <span className="truncate text-pl-text" title={c.kind === "main" ? c.model : `${c.name} · ${c.model}`}>{c.kind === "main" ? c.model : c.name}</span>
        </span>
        {fastest && <span className="flex shrink-0 items-center gap-1 normal-case tracking-normal text-pl-gold"><Trophy size={11} /> быстрее всех</span>}
      </div>
      <div className="grid shrink-0 grid-cols-3 gap-px border-b border-pl-line bg-pl-line font-mono">
        <Metric k="ток/с" v={`${c.live && c.tps != null ? "≈" : ""}${tps}`} tone={c.status === "done" ? "text-pl-green" : "text-pl-text"} big />
        <Metric k="токенов" v={c.tokens != null ? String(c.tokens) : "—"} />
        <Metric k={c.ms != null ? "всего" : "первый токен"} v={c.ms != null ? `${(c.ms / 1000).toFixed(1)} с` : c.ttftMs != null ? `${(c.ttftMs / 1000).toFixed(1)} с` : "—"} />
      </div>
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-pl-line px-3 py-1 font-mono text-2xs text-pl-dim">
        <span className="min-w-0 truncate">{c.kind === "main" ? "кластер · llama-server" : `${c.model} · на ноде`}</span>
        <span className="shrink-0">
          {c.status === "run" ? <span className="flex items-center gap-1 text-pl-orange"><Loader2 size={10} className="animate-spin" /> отвечает</span>
            : c.status === "stopped" ? <span className="text-pl-orange">остановлено</span>
            : c.status === "error" ? <span className="text-pl-red">ошибка</span> : <span className="text-pl-green">готово</span>}
        </span>
      </div>
      <div ref={box} className="min-h-0 flex-1 overflow-y-auto px-3 py-2 text-sm">
        {c.text ? <Markdown text={c.text} streaming={c.status === "run"} /> : c.status === "run" ? <span className="text-xs text-pl-dim">ждём первый токен…</span> : null}
        {c.error && <div className="mt-1 text-xs text-pl-red">{c.error}</div>}
      </div>
    </div>
  );
}

const Metric = ({ k, v, tone, big }: { k: string; v: string; tone?: string; big?: boolean }) => (
  <div className="min-w-0 bg-pl-bg1 px-3 py-1.5">
    <div className="label truncate">{k}</div>
    <div className={clsx("truncate tabular-nums", big ? "text-lg" : "text-sm", tone ?? "text-pl-text")}>{v}</div>
  </div>
);
