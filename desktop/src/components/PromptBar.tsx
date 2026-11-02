import { useEffect, useRef, useState } from "react";
import { CornerDownLeft, ImagePlus, Loader2, Mic, Paperclip, Square, X } from "lucide-react";
import { transcribe } from "../lib/api";
import { useCluster } from "../store/cluster";

export type Attachment = { name: string; size: number; text: string };
export type ImageAttachment = { name: string; url: string }; // JPEG data URI, ≤1024px
const MAX_FILE = 256 * 1024;
const MAX_IMAGES = 4;
const IMG_MAX_SIDE = 1024;

/** Downscale any browser-decodable image to ≤1024px JPEG data URI. */
export async function toJpeg(file: Blob): Promise<string> {
  const src = await new Promise<string>((ok, bad) => { const r = new FileReader(); r.onload = () => ok(String(r.result)); r.onerror = () => bad(r.error); r.readAsDataURL(file); });
  const img = await new Promise<HTMLImageElement>((ok, bad) => { const i = new Image(); i.onload = () => ok(i); i.onerror = () => bad(new Error("не удалось открыть изображение")); i.src = src; });
  const k = Math.min(1, IMG_MAX_SIDE / Math.max(img.naturalWidth, img.naturalHeight));
  const c = document.createElement("canvas");
  c.width = Math.max(1, Math.round(img.naturalWidth * k)); c.height = Math.max(1, Math.round(img.naturalHeight * k));
  const g = c.getContext("2d")!;
  g.fillStyle = "#fff"; g.fillRect(0, 0, c.width, c.height); // transparent PNG → white, not black
  g.drawImage(img, 0, 0, c.width, c.height);
  return c.toDataURL("image/jpeg", 0.85);
}

/** Recorded audio (webm/opus …) → 16 kHz mono 16-bit PCM WAV, base64. */
async function toWav16k(blob: Blob): Promise<string> {
  const buf = await blob.arrayBuffer();
  const ctx = new AudioContext();
  let decoded: AudioBuffer;
  try { decoded = await ctx.decodeAudioData(buf); } finally { ctx.close().catch(() => {}); }
  const rate = 16000;
  const off = new OfflineAudioContext(1, Math.max(1, Math.ceil(decoded.duration * rate)), rate);
  const s = off.createBufferSource(); s.buffer = decoded; s.connect(off.destination); s.start();
  const pcm = (await off.startRendering()).getChannelData(0);
  const out = new DataView(new ArrayBuffer(44 + pcm.length * 2));
  const str = (o: number, t: string) => { for (let i = 0; i < t.length; i++) out.setUint8(o + i, t.charCodeAt(i)); };
  str(0, "RIFF"); out.setUint32(4, 36 + pcm.length * 2, true); str(8, "WAVE"); str(12, "fmt ");
  out.setUint32(16, 16, true); out.setUint16(20, 1, true); out.setUint16(22, 1, true); out.setUint32(24, rate, true);
  out.setUint32(28, rate * 2, true); out.setUint16(32, 2, true); out.setUint16(34, 16, true); str(36, "data"); out.setUint32(40, pcm.length * 2, true);
  for (let i = 0; i < pcm.length; i++) { const v = Math.max(-1, Math.min(1, pcm[i])); out.setInt16(44 + i * 2, v < 0 ? v * 0x8000 : v * 0x7fff, true); }
  const bytes = new Uint8Array(out.buffer);
  let bin = ""; for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

export default function PromptBar({ onSend, onStop, busy, disabled, placeholder, draft, onDraftUsed, hint, voiceUrl }: {
  onSend: (text: string, files: Attachment[], images: ImageAttachment[]) => void; onStop: () => void; busy: boolean; disabled?: boolean; placeholder?: string;
  draft?: string | null; onDraftUsed?: () => void; hint?: string; voiceUrl?: string;
}) {
  const pushToast = useCluster((s) => s.pushToast);
  const [text, setText] = useState("");
  const [files, setFiles] = useState<Attachment[]>([]);
  const [images, setImages] = useState<ImageAttachment[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [rec, setRec] = useState<"idle" | "rec" | "stt">("idle");
  const recorder = useRef<MediaRecorder | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const imgRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const el = ref.current; if (!el) return;
    el.style.height = "0px"; el.style.height = Math.min(el.scrollHeight, 200) + "px";
  }, [text]);
  useEffect(() => { if (draft != null) { setText(draft); onDraftUsed?.(); ref.current?.focus(); } }, [draft, onDraftUsed]);
  useEffect(() => () => { recorder.current?.state === "recording" && recorder.current.stop(); }, []);
  const empty = !text.trim() && !files.length && !images.length;
  const submit = () => { const t = text.trim(); if (empty || busy || disabled) return; onSend(t, files, images); setText(""); setFiles([]); setImages([]); };

  async function addImage(f: File) {
    try {
      const url = await toJpeg(f);
      setImages((x) => (x.length >= MAX_IMAGES ? (setErr(`не больше ${MAX_IMAGES} фото в сообщении`), x) : [...x.filter((y) => y.name !== f.name), { name: f.name || "фото.jpg", url }]));
    } catch (e) { setErr(`${f.name}: ${e instanceof Error ? e.message : e}`); }
  }

  async function attach(list: FileList | File[] | null) {
    setErr(null);
    for (const f of Array.from(list ?? [])) {
      if (f.type.startsWith("image/")) { await addImage(f); continue; }
      if (f.size > MAX_FILE) { setErr(`${f.name}: больше 256 KB — слишком много для контекста`); continue; }
      const t = await f.text();
      if (/\u0000/.test(t.slice(0, 2000))) { setErr(`${f.name}: похоже на бинарный файл`); continue; }
      setFiles((x) => [...x.filter((y) => y.name !== f.name), { name: f.name, size: f.size, text: t }]);
    }
    if (fileRef.current) fileRef.current.value = "";
    if (imgRef.current) imgRef.current.value = "";
  }

  function onPaste(e: React.ClipboardEvent) {
    const imgs = Array.from(e.clipboardData.files).filter((f) => f.type.startsWith("image/"));
    if (imgs.length) { e.preventDefault(); attach(imgs); }
  }

  async function toggleMic() {
    if (rec === "rec") { recorder.current?.stop(); return; }
    if (rec === "stt") return;
    setErr(null);
    let stream: MediaStream;
    try { stream = await navigator.mediaDevices.getUserMedia({ audio: true }); }
    catch (e) { pushToast("warn", `Нет доступа к микрофону: ${e instanceof Error ? e.message : e}`); return; }
    const chunks: Blob[] = [];
    const r = new MediaRecorder(stream);
    recorder.current = r;
    r.ondataavailable = (ev) => { if (ev.data.size) chunks.push(ev.data); };
    r.onstop = async () => {
      stream.getTracks().forEach((t) => t.stop());
      setRec("stt");
      try {
        const wav = await toWav16k(new Blob(chunks, { type: r.mimeType || "audio/webm" }));
        const said = (await transcribe(wav, voiceUrl)).trim();
        if (said) { setText((t) => (t.trim() ? `${t.trimEnd()} ${said}` : said)); ref.current?.focus(); }
        else pushToast("info", "Речь не распознана");
      } catch (e) { pushToast("warn", `Распознавание речи: ${e instanceof Error ? e.message : e}`); }
      finally { setRec("idle"); }
    };
    r.start();
    setRec("rec");
  }

  return (
    <div className="border-t border-pl-line bg-pl-bg1 p-3"
      onDragOver={(e) => e.preventDefault()} onDrop={(e) => { e.preventDefault(); attach(e.dataTransfer.files); }}>
      <div className="mx-auto max-w-4xl border border-pl-line2 bg-pl-bg0 focus-within:border-pl-blue focus-within:shadow-[0_0_0_1px_#4C90F0]">
        {(files.length > 0 || images.length > 0 || err) && (
          <div className="flex flex-wrap items-center gap-1.5 border-b border-pl-line px-2 py-1.5">
            {images.map((im) => (
              <span key={im.name} className="relative inline-block border border-pl-line2">
                <img src={im.url} alt={im.name} title={im.name} className="h-12 w-12 object-cover" />
                <button onClick={() => setImages((x) => x.filter((y) => y.name !== im.name))} title="Убрать"
                  className="absolute right-0 top-0 bg-pl-bg0/80 p-0.5 text-pl-mute hover:text-pl-text"><X size={10} /></button>
              </span>
            ))}
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
          onChange={(e) => setText(e.target.value)} onPaste={onPaste}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }}
          placeholder={rec === "rec" ? "Говорите… нажмите микрофон ещё раз, чтобы закончить" : placeholder}
          className="block max-h-[200px] w-full resize-none bg-transparent px-3 py-2.5 text-[14px] text-pl-text outline-none placeholder:text-pl-dim disabled:cursor-not-allowed" />
        <div className="flex items-center justify-between gap-2 border-t border-pl-line px-2 py-1.5">
          <div className="flex min-w-0 items-center gap-2">
            <input ref={fileRef} type="file" multiple className="hidden" onChange={(e) => attach(e.target.files)}
              accept=".txt,.md,.csv,.json,.log,.py,.js,.ts,.tsx,.rs,.go,.java,.c,.cpp,.h,.cs,.html,.css,.xml,.yaml,.yml,.toml,.ini,.sql,.sh,.ps1,text/*" />
            <input ref={imgRef} type="file" multiple accept="image/*" className="hidden" onChange={(e) => attach(e.target.files)} />
            <button className="btn h-6 px-1.5" title="Прикрепить текстовые файлы" disabled={disabled} onClick={() => fileRef.current?.click()}><Paperclip size={12} /></button>
            <button className="btn h-6 px-1.5" title="Прикрепить фото (нужна модель со зрением) — можно вставить Ctrl+V или перетащить" disabled={disabled} onClick={() => imgRef.current?.click()}><ImagePlus size={12} /></button>
            <button className={"btn h-6 px-1.5 " + (rec === "rec" ? "border-pl-red/70 text-pl-red" : "")} disabled={disabled && rec === "idle"}
              title={rec === "rec" ? "Остановить запись и распознать" : rec === "stt" ? "Распознаю…" : "Голосовой ввод (whisper)"} onClick={toggleMic}>
              {rec === "stt" ? <Loader2 size={12} className="animate-spin" /> : rec === "rec" ? <Square size={11} /> : <Mic size={12} />}
            </button>
            <span className="truncate font-mono text-2xs text-pl-dim">{rec === "rec" ? "● запись…" : hint ?? "↵ отправить · ⇧↵ новая строка · данные не покидают этот ПК"}</span>
          </div>
          {busy
            ? <button onClick={onStop} className="btn border-pl-red/60 text-pl-red"><Square size={11} /> Стоп</button>
            : <button onClick={submit} disabled={empty || disabled} className="btn-primary"><CornerDownLeft size={12} /> Run</button>}
        </div>
      </div>
    </div>
  );
}
