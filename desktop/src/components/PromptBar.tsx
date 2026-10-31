import { useEffect, useRef, useState } from "react";
import { CornerDownLeft, Paperclip, Square, X } from "lucide-react";

export type Attachment = { name: string; size: number; text: string };
const MAX_FILE = 256 * 1024;

export default function PromptBar({ onSend, onStop, busy, disabled, placeholder, draft, onDraftUsed, hint }: {
  onSend: (text: string, files: Attachment[]) => void; onStop: () => void; busy: boolean; disabled?: boolean; placeholder?: string;
  draft?: string | null; onDraftUsed?: () => void; hint?: string;
}) {
  const [text, setText] = useState("");
  const [files, setFiles] = useState<Attachment[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const el = ref.current; if (!el) return;
    el.style.height = "0px"; el.style.height = Math.min(el.scrollHeight, 200) + "px";
  }, [text]);
  useEffect(() => { if (draft != null) { setText(draft); onDraftUsed?.(); ref.current?.focus(); } }, [draft, onDraftUsed]);
  const submit = () => { const t = text.trim(); if ((!t && !files.length) || busy || disabled) return; onSend(t, files); setText(""); setFiles([]); };

  async function attach(list: FileList | null) {
    setErr(null);
    for (const f of Array.from(list ?? [])) {
      if (f.size > MAX_FILE) { setErr(`${f.name}: больше 256 KB — слишком много для контекста`); continue; }
      const t = await f.text();
      if (/\u0000/.test(t.slice(0, 2000))) { setErr(`${f.name}: похоже на бинарный файл`); continue; }
      setFiles((x) => [...x.filter((y) => y.name !== f.name), { name: f.name, size: f.size, text: t }]);
    }
    if (fileRef.current) fileRef.current.value = "";
  }

  return (
    <div className="border-t border-pl-line bg-pl-bg1 p-3"
      onDragOver={(e) => e.preventDefault()} onDrop={(e) => { e.preventDefault(); attach(e.dataTransfer.files); }}>
      <div className="mx-auto max-w-4xl border border-pl-line2 bg-pl-bg0 focus-within:border-pl-blue focus-within:shadow-[0_0_0_1px_#4C90F0]">
        {(files.length > 0 || err) && (
          <div className="flex flex-wrap items-center gap-1.5 border-b border-pl-line px-2 py-1.5">
            {files.map((f) => (
              <span key={f.name} className="tag border border-pl-line2 normal-case text-pl-mute">
                {f.name} · {(f.size / 1024).toFixed(1)} KB
                <button onClick={() => setFiles((x) => x.filter((y) => y.name !== f.name))} className="hover:text-pl-text"><X size={10} /></button>
              </span>
            ))}
            {err && <span className="font-mono text-2xs text-pl-red">{err}</span>}
          </div>
        )}
        <textarea ref={ref} rows={1} value={text} disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }}
          placeholder={placeholder}
          className="block max-h-[200px] w-full resize-none bg-transparent px-3 py-2.5 text-[14px] text-pl-text outline-none placeholder:text-pl-dim disabled:cursor-not-allowed" />
        <div className="flex items-center justify-between gap-2 border-t border-pl-line px-2 py-1.5">
          <div className="flex min-w-0 items-center gap-2">
            <input ref={fileRef} type="file" multiple className="hidden" onChange={(e) => attach(e.target.files)}
              accept=".txt,.md,.csv,.json,.log,.py,.js,.ts,.tsx,.rs,.go,.java,.c,.cpp,.h,.cs,.html,.css,.xml,.yaml,.yml,.toml,.ini,.sql,.sh,.ps1,text/*" />
            <button className="btn h-6 px-1.5" title="Прикрепить текстовые файлы" disabled={disabled} onClick={() => fileRef.current?.click()}><Paperclip size={12} /></button>
            <span className="truncate font-mono text-2xs text-pl-dim">{hint ?? "↵ отправить · ⇧↵ новая строка · данные не покидают этот ПК"}</span>
          </div>
          {busy
            ? <button onClick={onStop} className="btn border-pl-red/60 text-pl-red"><Square size={11} /> Стоп</button>
            : <button onClick={submit} disabled={(!text.trim() && !files.length) || disabled} className="btn-primary"><CornerDownLeft size={12} /> Run</button>}
        </div>
      </div>
    </div>
  );
}
