import { memo, useState } from "react";
import clsx from "clsx";
import { Check, Copy, FileText, Pencil, RotateCcw, Wrench } from "lucide-react";
import type { ChatMessage } from "../lib/types";
import Markdown from "./Markdown";
import { TOOL_LABEL, short } from "../lib/agent";

/** Log-style message block, as in Palantir AIP: role header + content, no bubbles. */
export default memo(MessageBubble);

function MessageBubble({ m, model, onRegenerate, onEdit }: {
  m: ChatMessage; model?: string; onRegenerate?: () => void; onEdit?: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const user = m.role === "user";
  if (m.role === "tool") return <ToolResult m={m} />;
  return (
    <div className={clsx("group border-l-2 pl-4", user ? "border-pl-line2" : "border-pl-blue")}>
      <div className="mb-1.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-2xs uppercase tracking-wider">
        <span className={clsx("min-w-0 truncate", user ? "text-pl-mute" : "text-pl-blue")}>{user ? "Оператор" : m.model ?? model ?? "Model"}</span>
        {m.pending && <span className="animate-blink text-pl-orange">генерация</span>}
        <span className="flex gap-2 normal-case tracking-normal text-pl-dim opacity-0 transition group-hover:opacity-100">
          {!m.pending && m.content && (
            <button title="Копировать" className="hover:text-pl-text" onClick={() => { navigator.clipboard?.writeText(m.content); setCopied(true); setTimeout(() => setCopied(false), 1200); }}>
              {copied ? <Check size={11} className="text-pl-green" /> : <Copy size={11} />}
            </button>)}
          {onEdit && <button title="Редактировать" className="hover:text-pl-text" onClick={onEdit}><Pencil size={11} /></button>}
          {onRegenerate && <button title="Сгенерировать заново" className="hover:text-pl-text" onClick={onRegenerate}><RotateCcw size={11} /></button>}
        </span>
        {!user && m.tps != null && m.tps > 0 && (
          <span className="ml-auto flex flex-wrap justify-end gap-x-3 whitespace-nowrap normal-case tracking-normal text-pl-dim tabular-nums">
            <span className="text-pl-green">{m.tps.toFixed(1)} tok/s</span>
            {m.promptTps != null && m.promptTps > 0 && <span>prompt {m.promptTps.toFixed(0)} tok/s</span>}
            {m.tokens != null && <span>{m.tokens} tok</span>}
            {m.ttftMs != null && <span>TTFT {(m.ttftMs / 1000).toFixed(1)}s</span>}
          </span>
        )}
      </div>
      {m.files?.length ? (
        <div className="mb-2 flex flex-wrap gap-1.5">
          {m.files.map((f) => <span key={f.name} className="tag border border-pl-line2 normal-case text-pl-mute"><FileText size={10} /> {f.name} · {(f.size / 1024).toFixed(1)} KB</span>)}
        </div>
      ) : null}
      {m.images?.length ? (
        <div className="mb-2 flex flex-wrap gap-1.5">
          {m.images.map((u, i) => <img key={i} src={u} alt={`фото ${i + 1}`} className="max-h-40 max-w-[240px] border border-pl-line2 object-contain" />)}
        </div>
      ) : null}
      {user ? <div className="whitespace-pre-wrap text-[14px] leading-6 text-pl-text">{m.content}</div>
            : <Markdown text={m.content} streaming={m.pending} />}
      {m.sources?.length ? (
        <div className="mt-2 flex flex-wrap items-center gap-1.5 font-mono text-2xs text-pl-dim">
          <span>источники:</span>
          {m.sources.map((src) => <span key={src} title={src} className="tag max-w-[260px] truncate border border-pl-line2 normal-case text-pl-mute">{src.split(/[\\/]/).pop()}</span>)}
        </div>
      ) : null}
      {m.pending && !m.content && <span className="ml-0.5 inline-block h-3.5 w-1.5 animate-blink bg-pl-blue align-middle" />}
      {m.toolCalls?.map((c) => (
        <div key={c.id} className="mt-2 flex min-w-0 items-center gap-2 border border-pl-line bg-pl-bg1 px-2 py-1 font-mono text-2xs text-pl-mute">
          <Wrench size={11} className="shrink-0 text-pl-gold" /><span className="shrink-0 text-pl-gold">{TOOL_LABEL[c.name] ?? c.name}</span>
          <span className="truncate">{(() => { try { return short(JSON.parse(c.arguments || "{}")); } catch { return c.arguments; } })()}</span>
        </div>
      ))}
    </div>
  );
}

function ToolResult({ m }: { m: ChatMessage }) {
  const [open, setOpen] = useState(false);
  const long = m.content.length > 400;
  return (
    <div className="ml-4 border-l-2 border-pl-gold/50 pl-4">
      <div className="mb-1 font-mono text-2xs uppercase tracking-wider text-pl-gold">↳ {TOOL_LABEL[m.toolName ?? ""] ?? m.toolName} · результат</div>
      {m.image && <img src={m.image} alt="screenshot" className="mb-2 max-h-72 max-w-full border border-pl-line2" />}
      <pre className="overflow-x-auto whitespace-pre-wrap bg-pl-bg0 p-2 font-mono text-2xs leading-4 text-pl-mute">{long && !open ? m.content.slice(0, 400) + "…" : m.content}</pre>
      {long && <button className="font-mono text-2xs text-pl-dim hover:text-pl-text" onClick={() => setOpen(!open)}>{open ? "свернуть" : "показать всё"}</button>}
    </div>
  );
}
