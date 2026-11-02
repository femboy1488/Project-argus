import { ChevronDown, ChevronUp, Copy, Cpu, ExternalLink, Loader2, MonitorSmartphone, PanelRight, Play, Search, Smartphone, Split, Square, Star, Zap } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { CTX_PRESETS, DRAFT_CHOICES, MODELS, baseOf, bestPick, draftFor, draftInfo, findModel, fitLevel, modelPoolMb, phoneVariantOf, requiredMb } from "../lib/models";
import { openExternal, planSplit, setDraftEnabled, setFastStart, setKeepLoaded, stopModel } from "../lib/api";
import type { ModelEntry, SplitPlan } from "../lib/types";
import { NODE_COLORS } from "./MemoryPool";
import { deploy } from "../lib/launch";
import EngineBanner from "./EngineBanner";
import { gb, pct } from "../lib/format";
import Meter, { Progress } from "./Meter";
import Drawer from "./Drawer";
import { useSteady, useWide } from "../lib/ui";
import HfBrowser from "./HfBrowser";
import { Toggle } from "./ChatView";

export const fitTag = {
  fits: ["FITS", "border-pl-green/50 text-pl-green", "green"],
  tight: ["TIGHT", "border-pl-orange/50 text-pl-orange", "orange"],
  "too-heavy": ["EXCEEDS POOL", "border-pl-red/50 text-pl-red", "red"],
} as const;

export const hfUrl = (repo: string) => `https://huggingface.co/${repo.split(":")[0]}`;

/** 0.3.3: curated models the user switched to their Q4_0 twin (faster on phone CPUs). */
const LS_Q40 = "argus.q40";
function useQ40() {
  const [set, setSet] = useState<string[]>(() => { try { return JSON.parse(localStorage.getItem(LS_Q40) ?? "[]"); } catch { return []; } });
  const toggle = (baseId: string, on: boolean) => setSet((cur) => {
    const next = on ? [...new Set([...cur, baseId])] : cur.filter((x) => x !== baseId);
    localStorage.setItem(LS_Q40, JSON.stringify(next));
    return next;
  });
  return { has: (id: string) => set.includes(id), toggle };
}

const plural = (n: number, one: string, few: string, many: string) => {
  const a = Math.abs(n) % 100, b = a % 10;
  return a > 10 && a < 20 ? many : b === 1 ? one : b >= 2 && b <= 4 ? few : many;
};

export const MoeBadge = () => <span className="tag border border-pl-gold/60 text-pl-gold" title="Смесь экспертов: на каждый токен работает лишь часть модели — быстро для своего размера">MoE</span>;

export default function ModelLibrary() {
  const { snapshot, run, setRun, ctx, setCtx, modelsTab: tab, setModelsTab: setTab } = useCluster();
  // steady: phone free-RAM wobble used to flip FITS/TIGHT and «лучший выбор» back and forth every poll
  const usable = useSteady(modelPoolMb(snapshot));
  const wide = useWide();
  const [side, setSide] = useState(false);
  const fitting = MODELS.filter((m) => fitLevel(m, usable, ctx) !== "too-heavy").length;
  const best = bestPick(usable, ctx);
  const [previewId, setPreviewId] = useState<string | null>(null);
  const q40 = useQ40();
  const shown = (b: ModelEntry) => (q40.has(b.id) ? phoneVariantOf(b) ?? b : b);
  const activeModel = run.status === "loading" || run.status === "ready" || run.status === "broken" ? run.model : undefined;
  const previewBase = findModel(previewId) ?? activeModel ?? best;
  const preview = previewBase ? (previewBase.baseId ? previewBase : MODELS.some((x) => x.id === previewBase.id) ? shown(previewBase) : previewBase) : undefined;
  const switchVariant = (m: ModelEntry, toQ40: boolean) => { const b = baseOf(m); q40.toggle(b.id, toQ40); setPreviewId(b.id); };

  return (
    <div className="flex h-full min-w-0 flex-col bg-pl-bg0 p-3">
      <div className="mb-3 grid grid-cols-4 gap-2 xl:gap-3">
        <Kpi k="Allocatable pool" v={`${gb(usable)} GB`} hint={snapshot && usable < snapshot.usableMb ? `без нод «Свободна» и «Помощник»: −${gb(snapshot.usableMb - usable)} GB` : undefined} />
        <Kpi k="Models that fit" v={`${fitting} / ${MODELS.length}`} tone="text-pl-green" />
        <div className="panel min-w-0 px-3 py-2 xl:px-4 xl:py-3">
          <div className="label truncate">Context window</div>
          <div className="mt-1.5 flex gap-px">
            {CTX_PRESETS.map((p) => (
              <button key={p.ctx} onClick={() => setCtx(p.ctx)} title={p.hint}
                className={clsx("min-w-0 flex-1 border px-1.5 py-1 text-left font-mono text-xs xl:px-2",
                  ctx === p.ctx ? "border-pl-blue bg-pl-blue2/25 text-pl-text" : "border-pl-line2 bg-pl-bg2 text-pl-mute hover:bg-pl-bg3")}>
                {p.label}<div className="hidden truncate text-2xs text-pl-dim xl:block">{p.hint}</div>
              </button>
            ))}
          </div>
        </div>
        <Kpi k="Engine" v={run.status === "idle" ? "idle" : run.status === "error" ? "error" : run.status} tone={run.status === "ready" ? "text-pl-green" : undefined} />
      </div>
      <div className="relative grid min-h-0 flex-1 grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_280px]">
        <div className="panel flex min-h-0 min-w-0 flex-col">
          <div className="flex min-w-0 items-center border-b border-pl-line bg-pl-bg2">
            {([["curated", "Рекомендуемые"], ["hf", "Hugging Face"]] as const).map(([id, label]) => (
              <button key={id} onClick={() => setTab(id)} className={clsx("relative h-8 shrink-0 whitespace-nowrap px-3 text-xs", tab === id ? "text-pl-text" : "text-pl-dim hover:text-pl-mute")}>
                {label}{tab === id && <span className="absolute inset-x-0 bottom-0 h-0.5 bg-pl-blue" />}
              </button>
            ))}
            <button data-tour="hf" className="btn ml-auto h-6" title="Найти модель на Hugging Face" onClick={() => setTab("hf")}><Search size={11} /> <span className="hidden lg:inline">Найти модель на Hugging Face</span><span className="lg:hidden">Найти</span></button>
            {!wide && <button className="btn ml-1.5 h-6" title="План запуска и ускорение" onClick={() => setSide(!side)}><PanelRight size={11} /> План</button>}
            <span className="w-2 shrink-0" />
          </div>
          {tab === "hf" ? <HfBrowser /> : (
            <div className="min-h-0 flex-1 overflow-auto">
              <table className="w-full min-w-[820px] text-xs">
                <thead className="sticky top-0 z-10 bg-pl-bg1">
                  <tr className="border-b border-pl-line text-left">
                    {["Model", "Params", "Quant", "File", "Required", "Pool usage", "Fit", ""].map((h) =>
                      <th key={h} className="label whitespace-nowrap px-3 py-1.5">{h}</th>)}
                  </tr>
                </thead>
                <tbody className="font-mono">
                  {MODELS.map((b) => {
                    const m = shown(b);
                    const fit = fitLevel(m, usable, ctx);
                    const [label, cls, tone] = fitTag[fit];
                    const need = requiredMb(m, ctx);
                    const active = (run.status === "loading" || run.status === "ready" || run.status === "broken") && run.model.id === m.id;
                    const isBest = best?.id === b.id;
                    return (
                      <tr key={m.id} onClick={() => { setPreviewId(b.id); if (!wide && tab === "curated") setSide(true); }} title="Показать план распределения справа"
                        className={clsx("cursor-pointer border-b border-pl-line", preview && baseOf(preview).id === b.id && !active && "outline outline-1 -outline-offset-1 outline-pl-blue/60", active ? "bg-pl-blue2/20" : isBest ? "bg-pl-gold/[0.07]" : "odd:bg-pl-bg1 even:bg-[#20252c] hover:bg-pl-bg3",
                        fit === "too-heavy" && "text-pl-dim", isBest && "shadow-[inset_2px_0_0_#FBD065]")}>
                        <td className="px-3 py-2 font-sans text-pl-text">
                          <div className="flex flex-wrap items-center gap-1.5">{isBest && <Star size={11} className="shrink-0 fill-pl-gold text-pl-gold" />}<span className="whitespace-nowrap">{m.name}</span>
                            {m.moe && <MoeBadge />}
                            {isBest && <span className="tag border border-pl-gold/60 text-pl-gold">лучший выбор</span>}</div>
                          <div className="mt-0.5 flex flex-wrap gap-1">{m.tags.filter((t) => t !== "MoE" && t !== "Q4_0").map((t) => <span key={t} className={clsx("tag", t === "vision" ? "bg-pl-violet/20 text-pl-violet" : t === "MoE" ? "bg-pl-gold/15 text-pl-gold" : "bg-pl-bg3 text-pl-mute")}>{t}</span>)}</div>
                          {m.note && <div className="mt-0.5 font-sans text-2xs text-pl-dim">{m.note}</div>}
                        </td>
                        <td className="whitespace-nowrap px-3 py-2">{m.params}</td>
                        <td className="whitespace-nowrap px-3 py-2 text-pl-mute">{m.quant}{m.baseId && <span className="ml-1 font-sans text-2xs text-pl-green" title="Q4_0 быстрее считается на процессорах телефонов">· для телефонов</span>}</td>
                        <td className="whitespace-nowrap px-3 py-2">{m.sizeGb} GB</td>
                        <td className="whitespace-nowrap px-3 py-2">{gb(need)} GB</td>
                        <td className="w-32 px-3 py-2">
                          <div className="mb-1 text-2xs tabular-nums text-pl-mute">{usable ? Math.round((need / usable) * 100) : 0}%</div>
                          <Meter value={pct(need, usable || 1)} tone={tone} />
                        </td>
                        <td className="whitespace-nowrap px-3 py-2"><span className={clsx("tag border", cls)}>{label}</span></td>
                        <td className="whitespace-nowrap px-3 py-2 text-right">
                          <button className="btn mr-1 px-1.5" title="Открыть на HF" onClick={(e) => { e.stopPropagation(); openExternal(hfUrl(m.hfRepo)); }}><ExternalLink size={11} /></button>
                          {active ? (
                            <button className="btn" onClick={(e) => { e.stopPropagation(); stopModel(); setRun({ status: "idle" }); }}>
                              {run.status === "loading" ? <><Loader2 size={12} className="animate-spin" /> Loading</> : run.status === "broken" ? <><Square size={11} /> Stop (остановлен)</> : <><Square size={11} /> Stop</>}
                            </button>
                          ) : (
                            <button className="btn-primary" disabled={fit === "too-heavy" || run.status === "loading"} onClick={(e) => { e.stopPropagation(); deploy(m); }} data-tour={isBest ? "deploy" : undefined}>
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
          {run.status === "broken" && <EngineBanner />}
        </div>
        <Drawer wide={wide} open={side} onClose={() => setSide(false)} title="План запуска" width={320}>
          <div className={clsx("flex min-h-0 flex-col gap-3", !wide && "flex-1 overflow-y-auto bg-pl-bg0 p-3 pt-9")}>
            {preview && tab === "curated" && <SplitPreview model={preview} onVariant={switchVariant} />}
            <SpeedPanel />
          </div>
        </Drawer>
      </div>
      <EngineLog />
    </div>
  );
}

/** 0.3.2: which devices would get layers for this model, which stay free, ≈ tok/s (plan_split). */
function SplitPreview({ model, onVariant }: { model: ModelEntry; onVariant: (m: ModelEntry, toQ40: boolean) => void }) {
  const { snapshot, ctx } = useCluster();
  const need = requiredMb(model, ctx);
  const [plan, setPlan] = useState<SplitPlan | null>(null);
  const [err, setErr] = useState<string | null>(null);
  // re-plan only when something the planner reads changes (not on every load-meter tick)
  const sig = snapshot ? [...snapshot.gpus, ...snapshot.nodes].map((n) =>
    `${n.id}:${n.stage}:${n.enabled}:${n.role}:${n.speed.toFixed(1)}:${Math.round(n.usableMb / 64)}`).join("|") + `|${snapshot.rolesEnabled}` : "";
  useEffect(() => {
    let live = true;
    planSplit(need).then((p) => { if (live) { setPlan(p); setErr(null); } }).catch((e) => live && setErr(String(e)));
    return () => { live = false; };
  }, [need, sig]);
  const name = (id: string) => [...(snapshot?.gpus ?? []), ...(snapshot?.nodes ?? [])].find((n) => n.id === id)?.name ?? id;
  const helpers = snapshot?.rolesEnabled ? snapshot.nodes.filter((n) => n.role === "helper" && n.stage === "connected") : [];
  const nodeOf = (id: string) => snapshot?.nodes.find((n) => n.id === id);
  const phones = (plan?.entries ?? []).filter((e) => e.share > 0 && ["phone", "tablet"].includes(nodeOf(e.id)?.kind ?? ""));
  const q40 = phoneVariantOf(baseOf(model));
  const moeOff = (plan?.moe?.offload ?? []).filter((o) => o.blocks.length > 0 && o.id !== "cpu" && !/^CPU/.test(o.target));
  const moeNodes = moeOff.map((o) => o.name);
  const cpuLayers = plan?.cpuLayers ?? 0;
  return (
    <div className="panel shrink-0">
      <div className="panel-head"><span className="flex shrink-0 items-center gap-1.5"><Split size={11} className="text-pl-blue" /> План запуска</span>
        <span className="min-w-0 truncate pl-2 normal-case tracking-normal text-pl-dim" title={model.name}>{model.name}</span></div>
      <div className="space-y-2.5 p-3 text-xs">
        {err ? <div className="text-2xs text-pl-red">{err}</div> : !plan ? <div className="flex items-center gap-1.5 text-2xs text-pl-dim"><Loader2 size={11} className="animate-spin" /> считаю…</div> : (
          <>
            <div className="flex items-baseline justify-between">
              <span className="text-pl-dim">нужно {gb(need)} ГБ</span>
              {plan.fits
                ? <span className="font-mono text-pl-gold" title="Грубая оценка по скорости устройств и задержке сети">{plan.estTps != null ? `≈ ${plan.estTps >= 10 ? plan.estTps.toFixed(0) : plan.estTps.toFixed(1)} ток/с` : ""}</span>
                : <span className="text-pl-red">не помещается</span>}
            </div>
            <div className="flex h-2 gap-px overflow-hidden bg-pl-bg2">
              {plan.entries.map((e, i) => <div key={e.device} className="shrink transition-[width] duration-500 ease-out" title={`${e.name}: ${(e.share * 100).toFixed(0)}%`} style={{ width: `${Math.max(0, Math.min(100, e.share * 100))}%`, background: NODE_COLORS[i % NODE_COLORS.length] }} />)}
            </div>
            <div className="space-y-1 font-mono text-2xs">
              {plan.entries.map((e, i) => (
                <div key={e.device} className="flex items-center gap-2">
                  <span className="h-2 w-2 shrink-0" style={{ background: NODE_COLORS[i % NODE_COLORS.length] }} />
                  <span className="min-w-0 flex-1 truncate font-sans text-pl-text" title={e.name}>{e.name}</span>
                  <span className="max-w-[64px] shrink-0 truncate text-pl-dim" title={e.device}>{e.device}</span>
                  <span className="w-16 shrink-0 whitespace-nowrap text-right text-pl-mute">{gb(e.share * need)} ГБ</span>
                  <span className="w-9 shrink-0 text-right text-pl-text">{(e.share * 100).toFixed(0)}%</span>
                </div>
              ))}
            </div>
            {plan.gpuOnly && (
              <div className="flex items-center gap-1.5 text-2xs text-pl-green"><MonitorSmartphone size={11} /> Только видеокарта — кабель и телефоны не нужны, так быстрее всего.</div>
            )}
            {cpuLayers > 0 && (
              <div className="flex items-center gap-1.5 text-2xs text-pl-mute" title="Эти слои считает процессор компьютера: в видеокарту и на устройства они уже не поместились">
                <Cpu size={11} className="text-pl-blue" /> Процессор ПК: {cpuLayers} {plural(cpuLayers, "слой", "слоя", "слоёв")}{(plan.nLayers ?? model.nLayers) ? ` из ${plan.nLayers ?? model.nLayers}` : ""}{plan.threads ? ` · ${plan.threads} ${plural(plan.threads, "поток", "потока", "потоков")}` : ""}
              </div>
            )}
            {model.moe && plan.moe && moeOff.length > 0 && (
              <div className="text-2xs leading-4 text-pl-dim">
                <MoeBadge /> <span className="text-pl-text">Эксперты на телефонах</span>: {moeOff.map((o) => `${o.name} — ${o.blocks.length} ${plural(o.blocks.length, "слой", "слоя", "слоёв")}, ${gb(o.mb)} ГБ`).join("; ")}.
                {" "}Внимание и контекст остаются в видеокарте, а на каждый токен работают лишь {plan.moe.expertUsed || model.expertsActive || 8} из {plan.moe.expertCount || model.experts || 128} экспертов.
              </div>
            )}
            {phones.length > 0 && model.quant !== "Q4_0" && q40 && (
              <div className="border-l-2 border-pl-green/60 bg-pl-green/5 p-2 text-2xs leading-4 text-pl-mute">
                <div className="flex items-center gap-1.5 text-pl-text"><Smartphone size={11} className="text-pl-green" /> Для телефонов быстрее Q4_0</div>
                <div className="mt-0.5">Часть слоёв уйдёт на {phones.map((e) => e.name).join(", ")}. Процессоры телефонов считают Q4_0 заметно быстрее, чем {model.quant}; качество почти то же.</div>
                <button className="btn mt-1.5 h-6" onClick={() => onVariant(model, true)}>Взять Q4_0 · {q40.sizeGb.toFixed(1)} ГБ</button>
              </div>
            )}
            {model.baseId && (
              <div className="text-2xs text-pl-dim">Вариант Q4_0 для телефонов. <button className="underline hover:text-pl-text" onClick={() => onVariant(model, false)}>Вернуть {baseOf(model).quant}</button></div>
            )}
            {plan.freeIds.length > 0 && (
              <div className="border-t border-pl-line pt-2 text-2xs leading-4 text-pl-dim">
                <span className="tag mr-1 border border-pl-gold/50 text-pl-gold">Свободна</span>
                {plan.freeIds.map(name).join(", ")} — модель помещается без {plan.freeIds.length > 1 ? "них" : "неё"}.
              </div>
            )}
            {helpers.length > 0 && (
              <div className="text-2xs leading-4 text-pl-dim"><span className="tag mr-1 border border-pl-violet/50 text-pl-violet">Помощник</span>{helpers.map((n) => n.name).join(", ")} — в модели не участвует.</div>
            )}
            {!plan.fits && <div className="text-2xs leading-4 text-pl-red">Даже все устройства вместе не вмещают модель с контекстом {ctx / 1024}K. Возьмите модель поменьше или уменьшите контекст.</div>}
          </>
        )}
        <div className="text-2xs text-pl-dim">Нажмите на строку таблицы, чтобы посмотреть план другой модели.</div>
      </div>
    </div>
  );
}

function SpeedPanel() {
  const { speed, setSpeed, run, snapshot } = useCluster();
  const target = run.status === "ready" || run.status === "loading" || run.status === "broken" ? run.model : bestPick(modelPoolMb(snapshot));
  const draft = target ? draftFor(target, speed.draft) : undefined;
  const di = draft ? draftInfo(draft) : undefined;
  const pushToast = useCluster((s) => s.pushToast);
  return (
    <div className="panel flex min-h-0 flex-col overflow-y-auto">
      <div className="panel-head"><span className="flex shrink-0 items-center gap-1.5"><Zap size={11} className="text-pl-gold" /> Ускорение</span><span className="min-w-0 truncate pl-2 normal-case tracking-normal text-pl-dim">при следующем Deploy</span></div>
      <div className="space-y-3 p-3 text-xs">
        <Toggle on={speed.speculative} set={(v) => { setSpeed({ speculative: v }); setDraftEnabled(v).catch((e) => pushToast("error", String(e))); }} label="Ускорение черновиком"
          sub={di ? `черновик: ${di.name} · ${di.sizeGb >= 1 ? di.sizeGb.toFixed(1) : di.sizeGb.toFixed(2)} ГБ — угадывает слова, большая модель проверяет пачкой` : "маленькая модель того же семейства угадывает слова, большая проверяет пачкой"} />
        {speed.speculative && (
          <div className="space-y-2 border-l-2 border-pl-line2 pl-2">
            <select value={speed.draft} onChange={(e) => setSpeed({ draft: e.target.value })} className="h-7 w-full border border-pl-line2 bg-pl-bg3 px-1 text-2xs text-pl-text">
              {DRAFT_CHOICES.map((c) => <option key={c.value} value={c.value}>{c.label}</option>)}
              <option value="none">Без черновой модели</option>
            </select>
            <div className="font-mono text-2xs text-pl-dim">{target ? (di ? `${target.name} ← ${di.name}${di.sizeGb ? ` (${di.sizeGb.toFixed(2)} ГБ)` : ""}` : `${target.name}: нет подходящего черновика`) : ""}</div>
            <label className="flex items-center justify-between font-mono text-2xs text-pl-mute">--spec-draft-n-max
              <input type="number" min={2} max={64} value={speed.draftMax} onChange={(e) => setSpeed({ draftMax: Math.max(2, Math.min(64, Number(e.target.value) || 16)) })}
                className="h-6 w-14 border border-pl-line2 bg-pl-bg0 px-1 text-right text-pl-text" />
            </label>
          </div>
        )}
        <Toggle on={speed.promptCache} set={(v) => setSpeed({ promptCache: v })} label="Кэш промпта" sub="--cache-reuse 256: длинная переписка не пересчитывается заново" />
        <Toggle on={speed.keepLoaded} set={(v) => { setSpeed({ keepLoaded: v }); setKeepLoaded(v); }} label="Держать модель загруженной" sub="между чатами и перезапусками Argus; повторный Deploy той же модели мгновенный" />
        <Toggle on={speed.fastStart} set={(v) => { setSpeed({ fastStart: v }); setFastStart(v); }} label="Быстрый старт нод" sub="не перезапускать rpc-server на устройстве, если он уже работает с той же версией" />
        <p className="border-t border-pl-line pt-2 text-2xs leading-4 text-pl-dim">Спекуляция помогает, когда основная модель большая и частично на телефонах: черновик (0.6–2B) считается локально, а дорогой проход по кабелю проверяет сразу пачку токенов.</p>
      </div>
    </div>
  );
}

const Kpi = ({ k, v, tone, hint }: { k: string; v: string; tone?: string; hint?: string }) => (
  <div className="panel min-w-0 px-3 py-2 xl:px-4 xl:py-3" title={hint}>
    <div className="label truncate">{k}</div>
    <div className={clsx("mt-1 truncate font-mono text-xl xl:text-2xl", tone ?? "text-pl-text")}>{v}</div>
    {hint && <div className="mt-0.5 line-clamp-2 text-2xs text-pl-dim">{hint}</div>}
  </div>
);

/** Live llama-server output: download progress, layer split per device, errors. */
function EngineLog() {
  const log = useCluster((s) => s.engineLog);
  const run = useCluster((s) => s.run);
  const dl = useCluster((s) => s.dlBytes);
  const box = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);
  // short windows: the log starts folded so the model table keeps its rows; it opens by itself while a model loads
  const [open, setOpen] = useState(() => window.innerHeight >= 760);
  const loading = run.status === "loading";
  useEffect(() => { if (loading) setOpen(true); }, [loading]);
  useEffect(() => { box.current?.scrollTo(0, box.current.scrollHeight); }, [log, open]);
  if (!log.length && run.status === "idle") return null;
  const tone = (l: string) =>
    /error|fail|abort|exception|cannot|unable/i.test(l) ? "text-pl-red"
    : /warn/i.test(l) ? "text-pl-orange"
    : /RPC|offload|model buffer|CUDA|Vulkan|server is listening|loaded|\[argus\]/i.test(l) ? "text-pl-blue" : "text-pl-mute";
  return (
    <div className="panel mt-3 flex min-h-0 shrink-0 flex-col">
      <div className="panel-head">
        <button className="flex min-w-0 items-center gap-1.5 uppercase hover:text-pl-text" onClick={() => setOpen(!open)} title={open ? "Свернуть журнал" : "Развернуть журнал"}>
          {open ? <ChevronDown size={12} /> : <ChevronUp size={12} />} Engine log {loading && <span className="ml-1 animate-blink text-pl-orange">loading</span>}
          {!open && log.length > 0 && <span className="min-w-0 truncate pl-2 font-mono normal-case tracking-normal text-pl-dim">{log[log.length - 1]}</span>}
        </button>
        <button className="flex shrink-0 items-center gap-1 normal-case tracking-normal text-pl-dim hover:text-pl-text"
          onClick={() => { navigator.clipboard.writeText(log.join("\n")); setCopied(true); setTimeout(() => setCopied(false), 1500); }}>
          <Copy size={12} /> {copied ? "Copied" : "Copy"}
        </button>
      </div>
      {loading && dl > 0 && <DownloadBar name={run.model.name} sizeGb={run.model.sizeGb} bytes={dl} />}
      <div ref={box} className={clsx("overflow-auto bg-pl-bg0 px-3 py-2 font-mono text-2xs leading-5", open ? "h-40 [@media(max-height:760px)]:h-28" : "hidden")}>
        {log.length ? log.map((l, i) => <div key={i} className={clsx("whitespace-pre-wrap break-all", tone(l))}>{l}</div>)
          : <div className="text-pl-mute">waiting for engine output…</div>}
      </div>
    </div>
  );
}

/**
 * Model download readout. `bytes` = what landed in the model cache (monotonic, see store.setDl). The cache can also
 * receive mmproj / draft-model files, so bytes may exceed the model size: then the total is unknown → indeterminate.
 */
function DownloadBar({ name, sizeGb, bytes }: { name: string; sizeGb: number; bytes: number }) {
  const total = sizeGb > 0 ? sizeGb * 1e9 : 0;
  const known = total > 0 && bytes <= total * 1.02;
  const p = known ? Math.min(100, (bytes / total) * 100) : null;
  return (
    <div className="border-b border-pl-line px-3 py-2 font-mono text-2xs text-pl-dim">
      <div className="mb-1 flex justify-between gap-3">
        <span className="min-w-0 truncate" title={name}>Downloading {name}</span>
        <span className="shrink-0 whitespace-nowrap tabular-nums text-pl-text">
          {(bytes / 1e9).toFixed(2)}{known ? ` / ${sizeGb.toFixed(1)}` : ""} GB{p != null ? <span className="inline-block w-[5ch] text-right">{p.toFixed(0)}%</span> : total > 0 ? " · доп. файлы" : ""}
        </span>
      </div>
      <Progress value={p} linear />
    </div>
  );
}
