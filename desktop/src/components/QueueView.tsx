// «Ночная очередь»: пакетные задания над папкой (перевод / пересказ / свой запрос) — по одному, через локальную модель.
import { useEffect, useState } from "react";
import clsx from "clsx";
import { ListTodo, Pause, Play, Plus, Trash2 } from "lucide-react";
import { useCluster } from "../store/cluster";
import { onQueueUpdate, queueAdd, queueList, queuePause, queueRemove, queueStart, type QueueJob, type QueueKind, type QueueState } from "../lib/api";

const KINDS: { id: QueueKind; label: string; hint: string }[] = [
  { id: "translate", label: "Перевести", hint: "каждый файл целиком на выбранный язык" },
  { id: "summarize", label: "Кратко пересказать", hint: "главное списком и вывод" },
  { id: "custom", label: "Свой запрос", hint: "что сделать с каждым файлом" },
];
const STATUS: Record<QueueJob["status"], string> = { pending: "в очереди", running: "идёт", done: "готово", error: "ошибка" };
const input = "mt-1 h-8 w-full border border-pl-line2 bg-pl-bg0 px-2 text-sm text-pl-text outline-none focus:border-pl-blue";

export default function QueueView() {
  const { pushToast } = useCluster();
  const [q, setQ] = useState<QueueState>({ running: false, jobs: [] });
  const [kind, setKind] = useState<QueueKind>("translate");
  const [lang, setLang] = useState("русский");
  const [prompt, setPrompt] = useState("");
  const [inDir, setInDir] = useState("");
  const [outDir, setOutDir] = useState("");
  const [startAt, setStartAt] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let off: (() => void) | undefined;
    queueList().then(setQ).catch(() => {});
    onQueueUpdate(setQ).then((f) => (off = f));
    return () => off?.();
  }, []);

  const act = (p: Promise<QueueState>) => p.then(setQ).catch((e) => pushToast("error", String(e)));

  async function add() {
    setBusy(true);
    try {
      const at = startAt ? Math.floor(new Date(startAt).getTime() / 1000) : null;
      setQ(await queueAdd({ kind, lang, prompt, inDir: inDir.trim(), outDir: outDir.trim(), startAt: at }));
      pushToast("ok", "Задание добавлено в очередь");
      setInDir(""); setOutDir("");
    } catch (e) { pushToast("error", String(e)); }
    setBusy(false);
  }

  return (
    <div className="grid h-full grid-cols-1 content-start gap-px overflow-y-auto bg-pl-line xl:grid-cols-[360px_minmax(0,1fr)] xl:content-stretch xl:overflow-hidden">
      <aside className="flex flex-col gap-3 bg-pl-bg1 p-4 text-xs xl:overflow-y-auto">
        <div className="label">Новое задание</div>
        <div className="flex flex-col gap-1">
          {KINDS.map((k) => (
            <button key={k.id} onClick={() => setKind(k.id)}
              className={clsx("rounded border px-3 py-2 text-left", kind === k.id ? "border-pl-blue bg-pl-bg3 text-pl-text" : "border-pl-line2 text-pl-mute hover:bg-pl-bg2")}>
              <div className="text-sm">{k.label}</div><div className="text-2xs text-pl-dim">{k.hint}</div>
            </button>
          ))}
        </div>
        {kind === "custom" && (
          <label className="block text-pl-mute">Что сделать с каждым файлом
            <textarea value={prompt} onChange={(e) => setPrompt(e.target.value)} rows={3} placeholder="Например: выпиши все даты и суммы из договора"
              className="mt-1 w-full border border-pl-line2 bg-pl-bg0 p-2 text-sm text-pl-text outline-none focus:border-pl-blue" />
          </label>
        )}
        <label className="block text-pl-mute">{kind === "translate" ? "На какой язык" : "Язык ответа"}
          <input value={lang} onChange={(e) => setLang(e.target.value)} className={input} />
        </label>
        <label className="block text-pl-mute">Папка с файлами
          <input value={inDir} onChange={(e) => setInDir(e.target.value)} placeholder="C:\Users\me\Documents\статьи" className={clsx(input, "font-mono")} />
        </label>
        <label className="block text-pl-mute">Куда сложить результат <span className="text-pl-dim">(пусто — в ту же папку)</span>
          <input value={outDir} onChange={(e) => setOutDir(e.target.value)} className={clsx(input, "font-mono")} />
        </label>
        <label className="block text-pl-mute">Начать не раньше <span className="text-pl-dim">(необязательно)</span>
          <input type="datetime-local" value={startAt} onChange={(e) => setStartAt(e.target.value)} className={input} />
        </label>
        <p className="text-2xs leading-4 text-pl-dim">Подходят .txt, .md, .docx и .pdf с текстом. Результат — рядом, файл вида «имя.{kind}.md». Если приложение закрыть, после запуска очередь продолжит с того файла, где остановилась.</p>
        <button className="btn-primary self-start" disabled={busy || !inDir.trim() || (kind === "custom" && !prompt.trim())} onClick={add}>
          <Plus size={14} /> Добавить в очередь
        </button>
      </aside>

      <section className="flex min-w-0 flex-col bg-pl-bg0 xl:overflow-hidden">
        <div className="panel-head gap-2">
          <span className="flex items-center gap-2"><ListTodo size={13} /> Ночная очередь {q.running ? "· работает" : "· на паузе"}</span>
          {q.running
            ? <button className="btn" onClick={() => act(queuePause())}><Pause size={13} /> Пауза</button>
            : <button className="btn-primary" disabled={!q.jobs.some((j) => j.status === "pending")} onClick={() => act(queueStart())}><Play size={13} /> Запустить</button>}
        </div>
        <div className="flex-1 space-y-2 overflow-y-auto p-3">
          {q.jobs.length === 0 && (
            <div className="mt-10 text-center text-sm text-pl-dim">Пока пусто. Добавьте задание слева и нажмите «Запустить» — можно на ночь.<br />Модель должна быть запущена во вкладке Chat.</div>
          )}
          {q.jobs.map((j) => (
            <div key={j.id} className="panel p-3 text-xs">
              <div className="flex items-start justify-between gap-2">
                <div className="min-w-0">
                  <div className="text-sm text-pl-text">{KINDS.find((k) => k.id === j.kind)?.label}{j.kind === "translate" && j.lang ? ` · ${j.lang}` : ""}</div>
                  <div className="truncate font-mono text-2xs text-pl-dim" title={j.inDir}>{j.inDir}</div>
                  {j.kind === "custom" && <div className="mt-0.5 line-clamp-2 text-pl-mute">{j.prompt}</div>}
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <span className={clsx("tag", j.status === "done" ? "bg-pl-bg3 text-pl-green" : j.status === "error" ? "bg-pl-bg3 text-pl-red" : j.status === "running" ? "bg-pl-bg3 text-pl-blue" : "bg-pl-bg3 text-pl-mute")}>{STATUS[j.status]}</span>
                  <button title="Убрать из очереди" className="text-pl-dim hover:text-pl-red" onClick={() => act(queueRemove(j.id))}><Trash2 size={14} /></button>
                </div>
              </div>
              <div className="mt-2 h-1.5 overflow-hidden rounded bg-pl-bg3"><div className="h-full bg-pl-blue2 transition-all" style={{ width: `${Math.round(j.progress * 100)}%` }} /></div>
              <div className="mt-1 flex justify-between gap-2 text-2xs text-pl-dim">
                <span className="truncate">
                  {j.current ? `${j.current}${j.message ? ` · ${j.message}` : ""}` : j.message || (j.startAt && j.status === "pending" ? `начнётся ${new Date(j.startAt * 1000).toLocaleString()}` : "")}
                </span>
                <span>{j.doneFiles.length} / {j.totalFiles || "?"} · {Math.round(j.progress * 100)}%</span>
              </div>
              {j.errors.length > 0 && (
                <details className="mt-1 text-2xs text-pl-red"><summary className="cursor-pointer">Не получилось: {j.errors.length}</summary>
                  {j.errors.map((e, i) => <div key={i} className="mt-0.5 break-all">{e}</div>)}</details>
              )}
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
