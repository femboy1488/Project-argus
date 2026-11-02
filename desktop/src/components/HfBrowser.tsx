import { useEffect, useState } from "react";
import clsx from "clsx";
import { ChevronRight, Download, ExternalLink, Heart, Loader2, Rocket, Search } from "lucide-react";
import { useCluster } from "../store/cluster";
import { fitLevel, kvGuess, modelPoolMb, paramsFromName, quantOf, requiredMb } from "../lib/models";
import { openExternal } from "../lib/api";
import { deploy } from "../lib/launch";
import { gb } from "../lib/format";
import type { ModelEntry } from "../lib/types";
import { fitTag, hfUrl } from "./ModelLibrary";

interface HfModel { id: string; downloads: number; likes: number; tags?: string[]; lastModified?: string; createdAt?: string }
interface GgufFile { file: string; label: string; quant?: string; bytes: number; parts: number }

const HF = "https://huggingface.co/api";
const fmtN = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(1)}K` : String(n));

/** Hugging Face GGUF search -> file list -> fit check -> "Скачать и запустить" (-hf repo:QUANT or --hf-file). */
export default function HfBrowser() {
  const [q, setQ] = useState(() => localStorage.getItem("argus.hfQuery") ?? "qwen3");
  const [items, setItems] = useState<HfModel[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);

  async function search(query = q) {
    setBusy(true); setErr(null);
    localStorage.setItem("argus.hfQuery", query);
    try {
      const r = await fetch(`${HF}/models?search=${encodeURIComponent(query)}&filter=gguf&sort=downloads&limit=30`);
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      setItems(await r.json());
    } catch (e) { setErr(`Hugging Face недоступен: ${String(e)}`); setItems([]); }
    finally { setBusy(false); }
  }
  useEffect(() => { search(); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <form className="flex gap-2 border-b border-pl-line p-2" onSubmit={(e) => { e.preventDefault(); search(); }}>
        <div className="flex min-w-0 flex-1 items-center gap-2 border border-pl-line2 bg-pl-bg0 px-2 focus-within:border-pl-blue">
          <Search size={13} className="text-pl-dim" />
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Название модели: qwen3, llama 3.1 8b, gemma, mistral…"
            className="h-8 min-w-0 flex-1 bg-transparent text-sm text-pl-text outline-none placeholder:text-pl-dim" />
        </div>
        <button className="btn-primary h-8" disabled={busy}>{busy ? <Loader2 size={12} className="animate-spin" /> : <Search size={12} />} Искать</button>
        <button type="button" className="btn h-8" onClick={() => openExternal(`https://huggingface.co/models?library=gguf&sort=downloads&search=${encodeURIComponent(q)}`)}>
          <ExternalLink size={12} /> на сайте
        </button>
      </form>
      {err && <div className="m-2 border-l-2 border-pl-red bg-pl-red/10 p-2 font-mono text-2xs text-pl-red">{err}</div>}
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full text-xs">
          <thead className="sticky top-0 z-10 bg-pl-bg1">
            <tr className="border-b border-pl-line text-left">
              {["Репозиторий", "Загрузки", "Лайки", ""].map((h) => <th key={h} className="label px-3 py-1.5">{h}</th>)}
            </tr>
          </thead>
          <tbody>
            {items.map((m) => (
              <Row key={m.id} m={m} open={open === m.id} toggle={() => setOpen(open === m.id ? null : m.id)} />
            ))}
            {!busy && !items.length && !err && <tr><td colSpan={4} className="p-6 text-center font-mono text-2xs text-pl-dim">ничего не найдено</td></tr>}
          </tbody>
        </table>
      </div>
      <div className="border-t border-pl-line px-3 py-1.5 font-mono text-2xs text-pl-dim">
        huggingface.co/api/models?filter=gguf · веса скачиваются один раз в кэш моделей · проверка «влезет ли» — по размеру файла + KV-кэш
      </div>
    </div>
  );
}

function Row({ m, open, toggle }: { m: HfModel; open: boolean; toggle: () => void }) {
  const [owner, name] = m.id.split("/");
  const vision = m.tags?.some((t) => t === "vision" || t === "image-text-to-text");
  const moe = m.tags?.includes("moe") || /A\d+B/.test(m.id);
  return (
    <>
      <tr onClick={toggle} className={clsx("cursor-pointer border-b border-pl-line", open ? "bg-pl-blue2/15" : "odd:bg-pl-bg1 even:bg-[#20252c] hover:bg-pl-bg3")}>
        <td className="px-3 py-2">
          <div className="flex items-center gap-1.5">
            <ChevronRight size={12} className={clsx("text-pl-dim transition", open && "rotate-90")} />
            <span className="text-pl-dim">{owner}/</span><span className="text-pl-text">{name}</span>
            {vision && <span className="tag bg-pl-violet/20 text-pl-violet">vision</span>}
            {moe && <span className="tag bg-pl-gold/15 text-pl-gold">MoE</span>}
          </div>
        </td>
        <td className="px-3 py-2 font-mono text-pl-mute"><Download size={10} className="mr-1 inline" />{fmtN(m.downloads ?? 0)}</td>
        <td className="px-3 py-2 font-mono text-pl-mute"><Heart size={10} className="mr-1 inline" />{fmtN(m.likes ?? 0)}</td>
        <td className="px-3 py-2 text-right">
          <button className="btn h-6" onClick={(e) => { e.stopPropagation(); openExternal(hfUrl(m.id)); }}><ExternalLink size={11} /> Открыть на HF</button>
        </td>
      </tr>
      {open && <tr><td colSpan={4} className="border-b border-pl-line bg-pl-bg0 p-0"><Files repo={m.id} vision={!!vision} moe={!!moe} /></td></tr>}
    </>
  );
}

function Files({ repo, vision, moe }: { repo: string; vision: boolean; moe: boolean }) {
  const { snapshot, ctx, run } = useCluster();
  const usable = modelPoolMb(snapshot);
  const [files, setFiles] = useState<GgufFile[] | null>(null);
  const [mmproj, setMmproj] = useState(0);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let stop = false;
    (async () => {
      try {
        const r = await fetch(`${HF}/models/${repo}/tree/main?recursive=true`);
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const list = (await r.json()) as { type: string; path: string; size: number; lfs?: { size: number } }[];
        const gg = list.filter((f) => f.type === "file" && f.path.toLowerCase().endsWith(".gguf"));
        const mm = gg.filter((f) => /mmproj/i.test(f.path));
        const groups = new Map<string, GgufFile>();
        for (const f of gg.filter((f) => !/mmproj/i.test(f.path))) {
          const size = f.lfs?.size ?? f.size ?? 0;
          const split = f.path.match(/^(.*)-(\d{5})-of-(\d{5})\.gguf$/i);
          const key = split ? split[1] : f.path;
          const g = groups.get(key) ?? { file: f.path, label: (split ? split[1] : f.path.replace(/\.gguf$/i, "")).split("/").pop()!, quant: quantOf(f.path), bytes: 0, parts: 0 };
          g.bytes += size; g.parts += 1;
          if (split && split[2] === "00001") g.file = f.path;
          groups.set(key, g);
        }
        if (!stop) {
          setMmproj(mm.length ? Math.min(...mm.map((f) => f.lfs?.size ?? f.size ?? 0)) : 0);
          setFiles([...groups.values()].sort((a, b) => a.bytes - b.bytes));
        }
      } catch (e) { if (!stop) setErr(String(e)); }
    })();
    return () => { stop = true; };
  }, [repo]);

  if (err) return <div className="p-3 font-mono text-2xs text-pl-red">{err}</div>;
  if (!files) return <div className="flex items-center gap-2 p-3 font-mono text-2xs text-pl-dim"><Loader2 size={12} className="animate-spin" /> список файлов…</div>;
  if (!files.length) return <div className="p-3 font-mono text-2xs text-pl-dim">в репозитории нет .gguf файлов</div>;

  const paramsB = paramsFromName(repo) ?? undefined;
  const entry = (f: GgufFile): ModelEntry => {
    const sizeGb = (f.bytes + (vision ? mmproj : 0)) / 1e9;
    const id = `${repo.split("/")[1].replace(/-GGUF$/i, "").toLowerCase()}${f.quant ? "-" + f.quant.toLowerCase() : ""}`.slice(0, 60);
    // a known quant tag lets llama.cpp handle split files + mmproj itself (-hf repo:QUANT); otherwise pin the exact file
    const useTag = !!f.quant && files.filter((x) => x.quant?.toLowerCase() === f.quant!.toLowerCase()).length === 1;
    return {
      id, name: f.label, family: repo.split("/")[0], params: paramsB ? `${paramsB}B` : "?", quant: f.quant ?? "?", sizeGb,
      hfRepo: useTag ? `${repo}:${f.quant}` : repo, hfFile: useTag ? undefined : f.file,
      kvMbPer1k: kvGuess(moe ? Math.min(paramsB ?? 8, 8) : paramsB ?? sizeGb * 1.7), vision, moe, tags: [],
    };
  };

  return (
    <table className="w-full text-xs">
      <tbody className="font-mono">
        {files.map((f) => {
          const e = entry(f);
          const fit = fitLevel(e, usable, ctx);
          const [label, cls] = fitTag[fit];
          const loading = run.status === "loading" && run.model.id === e.id;
          return (
            <tr key={f.file} className="border-t border-pl-line/60">
              <td className="py-1.5 pl-9 pr-3 text-pl-text">{f.label}{f.parts > 1 && <span className="ml-1 text-pl-dim">({f.parts} частей)</span>}</td>
              <td className="px-3 py-1.5 text-pl-mute">{f.quant ?? "—"}</td>
              <td className="px-3 py-1.5">{(f.bytes / 1e9).toFixed(2)} GB{vision && mmproj ? <span className="text-pl-dim"> +{(mmproj / 1e9).toFixed(1)} mmproj</span> : null}</td>
              <td className="px-3 py-1.5 text-pl-mute">нужно {gb(requiredMb(e, ctx))} GB</td>
              <td className="px-3 py-1.5"><span className={clsx("tag border", cls)}>{label}</span></td>
              <td className="px-3 py-1.5 text-right">
                <button className="btn-primary h-6" disabled={fit === "too-heavy" || run.status === "loading"} onClick={() => deploy(e)}>
                  {loading ? <Loader2 size={11} className="animate-spin" /> : <Rocket size={11} />} Скачать и запустить
                </button>
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
