import { Copy, ExternalLink, Loader2, Play, Search, Square, Star, Zap } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { CTX_PRESETS, DRAFT_CHOICES, MODELS, bestPick, draftFor, fitLevel, requiredMb } from "../lib/models";
import { openExternal, setFastStart, stopModel } from "../lib/api";
import { deploy } from "../lib/launch";
import { gb, pct } from "../lib/format";
import Meter from "./Meter";
import HfBrowser from "./HfBrowser";
import { Toggle } from "./ChatView";

export const fitTag = {
  fits: ["FITS", "border-pl-green/50 text-pl-green", "green"],
  tight: ["TIGHT", "border-pl-orange/50 text-pl-orange", "orange"],
  "too-heavy": ["EXCEEDS POOL", "border-pl-red/50 text-pl-red", "red"],
} as const;

export const hfUrl = (repo: string) => `https://huggingface.co/${repo.split(":")[0]}`;

export default function ModelLibrary() {
  const { snapshot, run, setRun, ctx, setCtx, modelsTab: tab, setModelsTab: setTab } = useCluster();
  const usable = snapshot?.usableMb ?? 0;
  const fitting = MODELS.filter((m) => fitLevel(m, usable, ctx) !== "too-heavy").length;
  const best = bestPick(usable, ctx);

  return (
    <div className="flex h-full flex-col bg-pl-bg0 p-3">
      <div className="mb-3 grid grid-cols-4 gap-3">
        <Kpi k="Allocatable pool" v={`${gb(usable)} GB`} />
        <Kpi k="Models that fit" v={`${fitting} / ${MODELS.length}`} tone="text-pl-green" />
        <div className="panel px-4 py-3">
          <div className="label">Context window</div>
          <div className="mt-1.5 flex gap-px">
            {CTX_PRESETS.map((p) => (
              <button key={p.ctx} onClick={() => setCtx(p.ctx)} title={p.hint}
                className={clsx("flex-1 border px-2 py-1 text-left font-mono text-xs",
                  ctx === p.ctx ? "border-pl-blue bg-pl-blue2/25 text-pl-text" : "border-pl-line2 bg-pl-bg2 text-pl-mute hover:bg-pl-bg3")}>
                {p.label}<div className="text-2xs text-pl-dim">{p.hint}</div>
              </button>
            ))}
          </div>
        </div>
        <Kpi k="Engine" v={run.status === "idle" ? "idle" : run.status === "error" ? "error" : run.status} tone={run.status === "ready" ? "text-pl-green" : undefined} />
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-[1fr_280px] gap-3">
        <div className="panel flex min-h-0 flex-col">
          <div className="flex items-center border-b border-pl-line bg-pl-bg2">
            {([["curated", "Рекомендуемые"], ["hf", "Hugging Face"]] as const).map(([id, label]) => (
              <button key={id} onClick={() => setTab(id)} className={clsx("relative h-8 px-3 text-xs", tab === id ? "text-pl-text" : "text-pl-dim hover:text-pl-mute")}>
                {label}{tab === id && <span className="absolute inset-x-0 bottom-0 h-0.5 bg-pl-blue" />}
              </button>
            ))}
            <button data-tour="hf" className="btn ml-auto mr-2 h-6" onClick={() => setTab("hf")}><Search size={11} /> Найти модель на Hugging Face</button>
          </div>
          {tab === "hf" ? <HfBrowser /> : (
            <div className="min-h-0 flex-1 overflow-auto">
              <table className="w-full text-xs">
                <thead className="sticky top-0 z-10 bg-pl-bg1">
                  <tr className="border-b border-pl-line text-left">
                    {["Model", "Params", "Quant", "File", "Required", "Pool usage", "Fit", ""].map((h) =>
                      <th key={h} className="label px-3 py-1.5">{h}</th>)}
                  </tr>
                </thead>
                <tbody className="font-mono">
                  {MODELS.map((m) => {
                    const fit = fitLevel(m, usable, ctx);
                    const [label, cls, tone] = fitTag[fit];
                    const need = requiredMb(m, ctx);
                    const active = (run.status === "loading" || run.status === "ready") && run.model.id === m.id;
                    const isBest = best?.id === m.id;
                    return (
                      <tr key={m.id} className={clsx("border-b border-pl-line", active ? "bg-pl-blue2/20" : isBest ? "bg-pl-gold/[0.07]" : "odd:bg-pl-bg1 even:bg-[#20252c] hover:bg-pl-bg3",
                        fit === "too-heavy" && "text-pl-dim", isBest && "shadow-[inset_2px_0_0_#FBD065]")}>
                        <td className="px-3 py-2 font-sans text-pl-text">
                          <div className="flex items-center gap-1.5">{isBest && <Star size={11} className="fill-pl-gold text-pl-gold" />}{m.name}
                            {isBest && <span className="tag border border-pl-gold/60 text-pl-gold">лучший выбор</span>}</div>
                          <div className="mt-0.5 flex flex-wrap gap-1">{m.tags.map((t) => <span key={t} className={clsx("tag", t === "vision" ? "bg-pl-violet/20 text-pl-violet" : t === "MoE" ? "bg-pl-gold/15 text-pl-gold" : "bg-pl-bg3 text-pl-mute")}>{t}</span>)}</div>
                          {m.note && <div className="mt-0.5 font-sans text-2xs text-pl-dim">{m.note}</div>}
                        </td>
                        <td className="px-3 py-2">{m.params}</td>
                        <td className="px-3 py-2 text-pl-mute">{m.quant}</td>
                        <td className="whitespace-nowrap px-3 py-2">{m.sizeGb} GB</td>
                        <td className="whitespace-nowrap px-3 py-2">{gb(need)} GB</td>
                        <td className="w-32 px-3 py-2">
                          <div className="mb-1 text-2xs text-pl-mute">{usable ? Math.round((need / usable) * 100) : 0}%</div>
                          <Meter value={pct(need, usable || 1)} tone={tone} />
                        </td>
                        <td className="px-3 py-2"><span className={clsx("tag border", cls)}>{label}</span></td>
                        <td className="whitespace-nowrap px-3 py-2 text-right">
                          <button className="btn mr-1 px-1.5" title="Открыть на HF" onClick={() => openExternal(hfUrl(m.hfRepo))}><ExternalLink size={11} /></button>
                          {active ? (
                            <button className="btn" onClick={() => { stopModel(); setRun({ status: "idle" }); }}>
                              {run.status === "loading" ? <><Loader2 size={12} className="animate-spin" /> Loading</> : <><Square size={11} /> Stop</>}
                            </button>
                          ) : (
                            <button className="btn-primary" disabled={fit === "too-heavy" || run.status === "loading"} onClick={() => deploy(m)} data-tour={isBest ? "deploy" : undefined}>
                              <Play size={11} /> Deploy
                            </button>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
          {run.status === "error" && <div className="m-3 border-l-2 border-pl-red bg-pl-red/10 p-2 font-mono text-2xs text-pl-red">{run.message}</div>}
        </div>
        <SpeedPanel />
      </div>
      <EngineLog />
    </div>
  );
}

function SpeedPanel() {
  const { speed, setSpeed, run, snapshot } = useCluster();
  const target = run.status === "ready" || run.status === "loading" ? run.model : bestPick(snapshot?.usableMb ?? 0);
  const draft = target ? draftFor(target, speed.draft) : undefined;
  return (
    <div className="panel flex min-h-0 flex-col overflow-y-auto">
      <div className="panel-head"><span className="flex items-center gap-1.5"><Zap size={11} className="text-pl-gold" /> Ускорение</span><span className="normal-case tracking-normal text-pl-dim">при следующем Deploy</span></div>
      <div className="space-y-3 p-3 text-xs">
        <Toggle on={speed.speculative} set={(v) => setSpeed({ speculative: v })} label="Спекулятивное декодирование" sub="маленькая модель того же семейства угадывает токены на GPU" />
        {speed.speculative && (
          <div className="space-y-2 border-l-2 border-pl-line2 pl-2">
            <select value={speed.draft} onChange={(e) => setSpeed({ draft: e.target.value })} className="h-7 w-full border border-pl-line2 bg-pl-bg3 px-1 text-2xs text-pl-text">
              {DRAFT_CHOICES.map((c) => <option key={c.value} value={c.value}>{c.label}</option>)}
              <option value="none">Без черновой модели</option>
            </select>
            <div className="font-mono text-2xs text-pl-dim">{target ? (draft ? `${target.name} ← ${draft.split("/")[1]}` : `${target.name}: нет совместимой черновой модели`) : ""}</div>
            <label className="flex items-center justify-between font-mono text-2xs text-pl-mute">--spec-draft-n-max
              <input type="number" min={2} max={64} value={speed.draftMax} onChange={(e) => setSpeed({ draftMax: Math.max(2, Math.min(64, Number(e.target.value) || 16)) })}
                className="h-6 w-14 border border-pl-line2 bg-pl-bg0 px-1 text-right text-pl-text" />
            </label>
          </div>
        )}
        <Toggle on={speed.promptCache} set={(v) => setSpeed({ promptCache: v })} label="Кэш промпта" sub="--cache-reuse 256: длинная переписка не пересчитывается заново" />
        <Toggle on={speed.keepLoaded} set={(v) => setSpeed({ keepLoaded: v })} label="Держать модель загруженной" sub="между чатами и перезапусками Argus; повторный Deploy той же модели мгновенный" />
        <Toggle on={speed.fastStart} set={(v) => { setSpeed({ fastStart: v }); setFastStart(v); }} label="Быстрый старт нод" sub="не перезапускать rpc-server на устройстве, если он уже работает с той же версией" />
        <p className="border-t border-pl-line pt-2 text-2xs leading-4 text-pl-dim">Спекуляция помогает, когда основная модель большая и частично на телефонах: черновик (0.6–2B) считается локально, а дорогой проход по кабелю проверяет сразу пачку токенов.</p>
      </div>
    </div>
  );
}

const Kpi = ({ k, v, tone }: { k: string; v: string; tone?: string }) => (
  <div className="panel px-4 py-3">
    <div className="label">{k}</div>
    <div className={clsx("mt-1 font-mono text-2xl", tone ?? "text-pl-text")}>{v}</div>
  </div>
);

/** Live llama-server output: download progress, layer split per device, errors. */
function EngineLog() {
  const log = useCluster((s) => s.engineLog);
  const run = useCluster((s) => s.run);
  const dl = useCluster((s) => s.dlBytes);
  const box = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);
  useEffect(() => { box.current?.scrollTo(0, box.current.scrollHeight); }, [log]);
  if (!log.length && run.status === "idle") return null;
  const tone = (l: string) =>
    /error|fail|abort|exception|cannot|unable/i.test(l) ? "text-pl-red"
    : /warn/i.test(l) ? "text-pl-orange"
    : /RPC|offload|model buffer|CUDA|Vulkan|server is listening|loaded|\[argus\]/i.test(l) ? "text-pl-blue" : "text-pl-mute";
  return (
    <div className="panel mt-3 flex min-h-0 flex-col">
      <div className="panel-head">
        <span>Engine log {run.status === "loading" && <span className="ml-2 animate-blink text-pl-orange">loading</span>}</span>
        <button className="flex items-center gap-1 normal-case tracking-normal text-pl-dim hover:text-pl-text"
          onClick={() => { navigator.clipboard.writeText(log.join("\n")); setCopied(true); setTimeout(() => setCopied(false), 1500); }}>
          <Copy size={12} /> {copied ? "Copied" : "Copy"}
        </button>
      </div>
      {run.status === "loading" && dl > 0 && run.model.sizeGb > 0 && (() => {
        const total = run.model.sizeGb * 1e9, p = Math.min(100, (dl / total) * 100);
        return (
          <div className="border-b border-pl-line px-3 py-2 font-mono text-2xs text-pl-dim">
            <div className="mb-1 flex justify-between"><span>Downloading {run.model.name}</span>
              <span className="text-pl-text">{(dl / 1e9).toFixed(2)} / {run.model.sizeGb.toFixed(1)} GB · {p.toFixed(0)}%</span></div>
            <div className="h-1.5 bg-pl-bg2"><div className="h-full bg-pl-blue transition-all" style={{ width: `${p}%` }} /></div>
          </div>
        );
      })()}
      <div ref={box} className="h-40 overflow-auto bg-pl-bg0 px-3 py-2 font-mono text-2xs leading-5">
        {log.length ? log.map((l, i) => <div key={i} className={clsx("whitespace-pre-wrap break-all", tone(l))}>{l}</div>)
          : <div className="text-pl-mute">waiting for engine output…</div>}
      </div>
    </div>
  );
}
