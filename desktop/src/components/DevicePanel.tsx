import { useEffect, useState } from "react";
import { Columns3, Cpu, FlaskConical, Gauge, Loader2, Share2, Thermometer, PanelRight, Plug, Plus, Radar, RefreshCw, RotateCw, ShieldAlert, Trash2, Usb, Wifi } from "lucide-react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import {
  addDiscoveredNode, exportBenchResults, planSplit, rebenchNode, reconnectNode, rescan, removeLanNode, restartEngine, retryHelper, setAutoAdd, setHelperModel, setNodeEnabled,
  setNodeRole, setNodeWeight, setRolesEnabled, updateUsbNode, wifiNodeUpdate,
} from "../lib/api";
import { Toggle } from "./ChatView";
import EngineBanner from "./EngineBanner";
import NodeExtraRoles from "./NodeExtraRoles";
import { gb, pct } from "../lib/format";
import MemoryPool from "./MemoryPool";
import Topology from "./Topology";
import Meter, { loadTone, Progress } from "./Meter";
import Drawer from "./Drawer";
import { helperPct, isDownloadPhase, useWide } from "../lib/ui";
import type { ClusterNode, ClusterSnapshot, DiscoveredNode, HelperState, HelperView, NodeRole, NodeStage, SplitPlan } from "../lib/types";

export const stageTag: Record<NodeStage, [string, string]> = {
  connected: ["ONLINE", "border-pl-green/50 text-pl-green"],
  unauthorized: ["AUTH REQUIRED", "border-pl-orange/50 text-pl-orange animate-blink"],
  installing: ["INSTALLING", "border-pl-blue/50 text-pl-blue animate-blink"],
  updating: ["UPDATING", "border-pl-blue/50 text-pl-blue animate-blink"],
  forwarding: ["FORWARDING", "border-pl-blue/50 text-pl-blue animate-blink"],
  starting: ["STARTING", "border-pl-blue/50 text-pl-blue animate-blink"],
  syncing: ["SYNCING", "border-pl-blue/50 text-pl-blue animate-blink"],
  offline: ["OFFLINE", "border-pl-line2 text-pl-dim"],
  error: ["FAULT", "border-pl-red/50 text-pl-red"],
};

export const TransportBadge = ({ n }: { n: ClusterNode }) =>
  n.transport === "lan" ? <span className="tag border border-pl-cyan/60 text-pl-cyan">{n.beacon ? "Wi-Fi" : "LAN"}</span>
  : n.transport === "usb" ? <span className="tag border border-pl-line2 text-pl-mute">USB</span>
  : <span className="tag border border-pl-line2 text-pl-mute">{n.integrated ? "iGPU" : n.kind === "gpu" ? "GPU" : "local"}</span>;

// ---- 0.3.2 roles / speed
export const ROLE_LABEL: Record<NodeRole, string> = { auto: "Авто", model: "Часть модели", free: "Свободна", helper: "Помощник" };
export const ROLE_HINT: Record<NodeRole, string> = {
  auto: "Argus сам решает: нода берёт слои, только если модель без неё не помещается",
  model: "Всегда держит часть слоёв модели, даже если хватает остальных",
  free: "Не участвует в модели — устройство остаётся свободным",
  helper: "Запускает свою маленькую модель рядом с основной (эксперимент «Роли»)",
};
const roleChoices = (snap: ClusterSnapshot, cur: NodeRole): NodeRole[] =>
  (["auto", "model", "free", "helper"] as NodeRole[]).filter((r) => r !== "helper" || snap.rolesEnabled || cur === "helper");

export const HELPER_STATE: Record<HelperState, [string, string]> = {
  "": ["—", "border-pl-line2 text-pl-dim"],
  prepare: ["ГОТОВЛЮ", "border-pl-blue/50 text-pl-blue animate-blink"],
  "pc-download": ["КАЧАЮ НА ПК", "border-pl-blue/50 text-pl-blue animate-blink"],
  "node-download": ["ПЕРЕДАЮ НА НОДУ", "border-pl-blue/50 text-pl-blue animate-blink"],
  downloading: ["СКАЧИВАЕТСЯ", "border-pl-blue/50 text-pl-blue animate-blink"],
  starting: ["ЗАПУСКАЕТСЯ", "border-pl-orange/50 text-pl-orange animate-blink"],
  loading: ["ЗАГРУЖАЕТСЯ", "border-pl-orange/50 text-pl-orange animate-blink"],
  ready: ["ГОТОВ", "border-pl-green/50 text-pl-green"],
  error: ["ОШИБКА", "border-pl-red/50 text-pl-red"],
};

/** "18.2 ГБ/с" + замер/оценка */
export function SpeedCell({ n }: { n: ClusterNode }) {
  if (!(n.speed > 0)) return <span className="text-pl-dim">—</span>;
  return (
    <span className="whitespace-nowrap" title={n.speedMeasured
      ? `Измерено тестом на устройстве: ${n.benchGbps?.toFixed(1)} ГБ/с${n.weight !== 1 ? ` × вес ${n.weight.toFixed(2)}` : ""}`
      : "Тест не проводился — это оценка по типу устройства"}>
      <span className="text-pl-text">{n.speed.toFixed(1)}</span> <span className="text-pl-dim">ГБ/с</span>{" "}
      <span className={clsx("tag", n.speedMeasured ? "bg-pl-green/15 text-pl-green" : "bg-pl-bg3 text-pl-dim")}>{n.speedMeasured ? "замер" : "оценка"}</span>
    </span>
  );
}

const fmtTps = (t: number | null | undefined) => (t == null ? null : t >= 10 ? t.toFixed(0) : t.toFixed(1));

/** host row (when no llama GPU list), every local GPU, then USB + LAN nodes */
export const tableNodes = (s: ClusterSnapshot) => [...(s.gpus.length ? s.gpus : [s.host]), ...s.nodes];

export default function DevicePanel() {
  const { snapshot, setWizard, setLanDialog, pushToast } = useCluster();
  const [sel, setSel] = useState("host");
  const [restarting, setRestarting] = useState(false);
  const wide = useWide();
  const [insp, setInsp] = useState(false);
  const pick = (id: string) => { setSel(id); if (!wide) setInsp(true); };
  if (!snapshot) return <div className="p-6 font-mono text-xs text-pl-dim">AWAITING TELEMETRY…</div>;
  const nodes = tableNodes(snapshot);
  const current = nodes.find((n) => n.id === sel) ?? (sel === "host" ? snapshot.host : nodes[0]);

  return (
    <div className="relative grid h-full grid-cols-1 gap-px bg-pl-line xl:grid-cols-[minmax(0,1fr)_300px]">
      <div className="flex min-w-0 flex-col gap-3 overflow-y-auto bg-pl-bg0 p-3">
        <EngineBanner compact />
        <MemoryPool />
        <div className="panel">
          <div className="panel-head h-auto min-h-8 flex-wrap gap-x-3 gap-y-1 py-1">
            <span>Topology</span>
            <div className="ml-auto flex flex-wrap justify-end gap-1.5 normal-case tracking-normal">
              <button className="btn" title="Повторить подготовку упавших устройств, перезапустить adb при сбое" onClick={() => rescan()}><RefreshCw size={12} /> Rescan</button>
              {snapshot.engine.canRestart && (
                <button className="btn" disabled={restarting} title="Перезапустить llama-server с текущим набором нод"
                  onClick={async () => { setRestarting(true); try { await restartEngine(); pushToast("info", "Движок перезапускается…"); } catch (e) { pushToast("error", String(e)); } setRestarting(false); }}>
                  {restarting ? <Loader2 size={12} className="animate-spin" /> : <RotateCw size={12} />} Перезапустить движок
                </button>
              )}
              <button className="btn" data-tour="lan" onClick={() => setLanDialog(true)}><Wifi size={12} /> Добавить ноду по IP</button>
              <button className="btn-primary" data-tour="usb" onClick={() => setWizard(true)}><Usb size={12} /> Connect via USB</button>
              {!wide && <button className="btn" title="Inspector: свойства выбранного устройства" onClick={() => setInsp(true)}><PanelRight size={12} /> Inspector</button>}
            </div>
          </div>
          <div className="h-[240px] xl:h-[300px]"><Topology selected={sel} onSelect={pick} /></div>
        </div>
        <NodeTable nodes={nodes} sel={sel} onSel={pick} snap={snapshot} />
        <RolesPanel snap={snapshot} />
        <DiscoveredPanel snap={snapshot} />
      </div>
      <Drawer wide={wide} open={insp} onClose={() => setInsp(false)} title="Inspector" width={320}>
        <Inspector node={current} snap={snapshot} />
      </Drawer>
    </div>
  );
}

/** 0.3.3: node temperature + «греется» (the device slows itself down from heat). */
export function TempBadge({ n }: { n: ClusterNode }) {
  if (n.throttled) return <span className="tag border border-pl-orange/60 text-pl-orange" title={`Устройство нагрелось${n.tempC != null ? ` до ${n.tempC.toFixed(0)} °C` : ""} и сбавляет скорость. Помогут прохлада, снятый чехол или вентилятор рядом.`}><Thermometer size={10} className="mr-0.5 inline" />греется{n.tempC != null ? ` ${n.tempC.toFixed(0)}°` : ""}</span>;
  if (n.tempC == null) return null;
  return <span className="whitespace-nowrap text-2xs text-pl-dim" title="Температура устройства">{n.tempC.toFixed(0)} °C</span>;
}

/** «Поделиться результатами»: copies the speed-test report (export_bench_results). */
function ShareBench() {
  const pushToast = useCluster((s) => s.pushToast);
  const [busy, setBusy] = useState(false);
  return (
    <button className="btn h-6 shrink-0 normal-case tracking-normal" disabled={busy} title="Скопировать результаты теста скорости устройств — их можно отправить в чат или в обсуждение на GitHub"
      onClick={async (e) => {
        e.stopPropagation(); setBusy(true);
        try { await navigator.clipboard.writeText(await exportBenchResults()); pushToast("ok", "Результаты скопированы — вставьте их, куда нужно"); }
        catch (er) { pushToast("error", `Не получилось скопировать: ${String(er)}`); }
        setBusy(false);
      }}>
      {busy ? <Loader2 size={11} className="animate-spin" /> : <Share2 size={11} />} Поделиться результатами
    </button>
  );
}

const sloev = (n: number) => { const a = n % 100, b = n % 10; return a > 10 && a < 20 ? "слоёв" : b === 1 ? "слой" : b >= 2 && b <= 4 ? "слоя" : "слоёв"; };

/** Split plan for the last model (for the «Процессор ПК» row); re-planned only when the planner's inputs change. */
function usePlanFor(snap: ClusterSnapshot): SplitPlan | null {
  const [plan, setPlan] = useState<SplitPlan | null>(null);
  const sig = [...snap.gpus, ...snap.nodes].map((n) => `${n.id}:${n.stage}:${n.enabled}:${n.role}:${Math.round(n.usableMb / 64)}`).join("|") + `|${snap.needMb}`;
  useEffect(() => {
    let live = true;
    if (snap.needMb == null || snap.plan) { setPlan(null); return; }
    planSplit(snap.needMb).then((p) => live && setPlan(p)).catch(() => live && setPlan(null));
    return () => { live = false; };
  }, [sig]); // eslint-disable-line react-hooks/exhaustive-deps
  return plan;
}

function NodeTable({ nodes, sel, onSel, snap }: { nodes: ClusterNode[]; sel: string; onSel: (id: string) => void; snap: ClusterSnapshot }) {
  const pushToast = useCluster((s) => s.pushToast);
  const share = (id: string) => snap.split.filter((e) => e.id === id).reduce((a, e) => a + e.share, 0);
  const planned = usePlanFor(snap);
  const plan = snap.plan ?? planned;
  const cpuLayers = plan?.cpuLayers;
  return (
    <div className="panel">
      <div className="panel-head gap-3"><span className="shrink-0">Nodes</span>
        <span className="min-w-0 truncate font-mono normal-case tracking-normal text-pl-dim">
          {nodes.length} objects<span className="hidden xl:inline"> · быстрые устройства заполняются первыми</span>
          {snap.estTps != null && <span className="ml-2 text-pl-gold" title="Грубая оценка по скорости и RTT нод для последней модели">≈ {fmtTps(snap.estTps)} ток/с</span>}
          {snap.needMb != null && !snap.fits && <span className="ml-2 text-pl-red">модель не помещается</span>}
        </span>
        <span className="ml-auto" /><ShareBench />
      </div>
      <div className="overflow-x-auto">
      <table className="w-full min-w-[1000px] text-xs">
        <thead>
          <tr className="border-b border-pl-line text-left">
            {["Node", "Link", "Status", "Endpoint / device", "Memory", "Allocatable", "Скорость", "RTT", "Роль", "Split", "Load"].map((h) =>
              <th key={h} className="label whitespace-nowrap px-3 py-1.5 font-semibold">{h}</th>)}
          </tr>
        </thead>
        <tbody className="font-mono tabular-nums">
          {nodes.map((n) => {
            const used = n.totalMb - n.availableMb;
            const [t, c] = stageTag[n.stage];
            const sh = share(n.id);
            return (
              <tr key={n.id} onClick={() => onSel(n.id)}
                className={clsx("cursor-pointer border-b border-pl-line last:border-0",
                  sel === n.id ? "bg-pl-blue2/20" : "odd:bg-pl-bg1 even:bg-[#20252c] hover:bg-pl-bg3", !n.enabled && "opacity-50")}>
                <td className="max-w-[180px] px-3 py-2 font-sans text-pl-text" title={n.name}>{n.name}{!n.enabled && <span className="ml-1.5 text-2xs text-pl-dim">(выкл)</span>}</td>
                <td className="px-3 py-2"><TransportBadge n={n} /></td>
                <td className="min-w-[150px] px-3 py-2">
                  <div className="flex flex-wrap items-center gap-1">
                  <span className={clsx("tag border", c)}>{t}</span>
                  {n.inEngine && <span title="используется запущенной моделью" className="whitespace-nowrap text-2xs text-pl-green">● в модели</span>}
                  {n.assigned === "free" && <span title="Модели эта нода не нужна — устройство свободно" className="tag border border-pl-gold/50 text-pl-gold">Свободна</span>}
                  {n.assigned === "helper" && <HelperTag n={n} />}
                  <TempBadge n={n} />
                  {n.updateAvailable && <span title={`установлена ${n.appVersion ?? n.version ?? "?"}, в комплекте ${n.updateAvailable}`} className="tag border border-pl-orange/50 text-pl-orange">Нода устарела</span>}
                  </div>
                  {n.stage === "error" && n.retryIn != null && <div className="mt-0.5 text-2xs text-pl-dim">повтор через {n.retryIn} с · попытка {n.attempts + 1}</div>}
                </td>
                <td className="max-w-[170px] truncate px-3 py-2 text-pl-mute" title={n.device ?? n.endpoint ?? undefined}>{n.device ?? n.endpoint ?? (n.transport === "local" ? "local" : "—")}</td>
                <td className="w-36 px-3 py-2">
                  <div className="mb-1 whitespace-nowrap text-2xs text-pl-mute">{gb(Math.max(0, used))} / {gb(n.totalMb)} GB</div>
                  <Meter value={pct(used, n.totalMb)} tone="blue" />
                </td>
                <td className="whitespace-nowrap px-3 py-2 text-pl-green">{gb(n.usableMb)} GB</td>
                <td className="px-3 py-2"><SpeedCell n={n} /></td>
                <td className="whitespace-nowrap px-3 py-2 text-pl-mute">{n.rttMs != null ? `${n.rttMs.toFixed(1)} мс` : "—"}</td>
                <td className="px-3 py-2" onClick={(e) => e.stopPropagation()}>
                  {n.transport === "local" ? <span className="whitespace-nowrap font-sans text-2xs text-pl-dim">всегда в модели</span> : (
                    <select value={n.role} title={ROLE_HINT[n.role]} onChange={(e) => setNodeRole(n.id, e.target.value as NodeRole).catch((er) => pushToast("error", String(er)))}
                      className="h-6 border border-pl-line2 bg-pl-bg3 px-1 font-sans text-2xs text-pl-text">
                      {roleChoices(snap, n.role).map((r) => <option key={r} value={r}>{ROLE_LABEL[r]}</option>)}
                    </select>
                  )}
                </td>
                <td className="px-3 py-2 text-pl-text">{sh ? `${(sh * 100).toFixed(0)}%` : "—"}</td>
                <td className="w-28 px-3 py-2">
                  <div className="mb-1 text-2xs text-pl-mute">{(Number.isFinite(n.loadPct) ? n.loadPct : 0).toFixed(0)}%</div>
                  <Meter value={n.loadPct} tone={loadTone(n.loadPct)} />
                </td>
              </tr>
            );
          })}
          {cpuLayers != null && (
            <tr className="border-b border-pl-line bg-pl-bg1 last:border-0" title="Слои, которые не поместились в видеокарту и на устройства, считает процессор компьютера (медленнее видеокарты)">
              <td className="px-3 py-2 font-sans text-pl-text"><span className="flex items-center gap-1.5"><Cpu size={11} className="text-pl-blue" /> Процессор ПК</span></td>
              <td className="px-3 py-2 font-sans text-2xs text-pl-dim">local</td>
              <td className="px-3 py-2">{cpuLayers > 0
                ? <span className="tag border border-pl-blue/50 text-pl-blue">{cpuLayers} {sloev(cpuLayers)}</span>
                : <span className="font-sans text-2xs text-pl-dim">не нужен{plan?.gpuOnly ? " — всё в видеокарте" : ""}</span>}</td>
              <td className="max-w-[170px] truncate px-3 py-2 text-pl-mute" title={snap.cpu?.brand}>{plan?.threads ? `${plan.threads} потоков` : snap.cpu?.brand ?? "CPU"}</td>
              <td className="px-3 py-2 text-pl-dim">—</td>
              <td className="px-3 py-2 text-pl-dim">—</td>
              <td className="px-3 py-2 text-pl-dim">—</td>
              <td className="px-3 py-2 text-pl-dim">—</td>
              <td className="px-3 py-2 font-sans text-2xs text-pl-dim">по необходимости</td>
              <td className="px-3 py-2 text-pl-text">{cpuLayers > 0 ? `${cpuLayers} сл.` : "—"}</td>
              <td className="px-3 py-2 text-pl-dim">—</td>
            </tr>
          )}
        </tbody>
      </table>
      </div>
    </div>
  );
}

function Inspector({ node, snap }: { node: ClusterNode; snap: ClusterSnapshot }) {
  const [t, c] = stageTag[node.stage];
  const [w, setW] = useState(node.weight);
  useEffect(() => setW(node.weight), [node.id, node.weight]);
  const isHostAgg = node.id === "host";
  const rows: [string, string][] = [
    ["Object ID", node.id], ["Class", node.transport === "local" ? (node.kind === "gpu" ? "Local GPU" : "Host") : "RPC Node"],
    ["Device type", node.integrated ? "integrated gpu" : node.kind],
    ["Transport", node.transport === "local" ? `local${node.device ? ` (${node.device})` : ""}` : node.transport === "lan" ? "LAN · TCP" : "USB · adb forward"],
    ["Endpoint", node.endpoint ?? "—"], ["Version", node.version ?? (node.transport === "usb" ? "—" : "b12008")],
    ["Total memory", `${gb(node.totalMb, 2)} GB`], ["Free now", `${gb(node.availableMb, 2)} GB`],
    ["Allocatable", `${gb(node.usableMb, 2)} GB`], ["Load", `${node.loadPct.toFixed(1)} %`],
  ];
  if (node.transport === "usb" && node.version && node.version !== snap.bundledNodeVersion) rows.push(["Bundled node", snap.bundledNodeVersion]);
  if (node.transport === "lan") rows.push(["Discovery", node.beacon ? "Wi-Fi маяк (UDP 50053)" : "вручную по IP"]);
  if (node.transport === "lan" && node.appVersion) rows.push(["Argus Node", `${node.appVersion}${node.appVc != null ? ` (${node.appVc})` : ""}`]);
  if (node.updateAvailable) rows.push(["Доступна нода", node.updateAvailable]);
  if (node.variant) rows.push(["rpc-server build", node.variant]);
  if (node.transport !== "local") rows.push(["In running model", node.inEngine ? "да" : "нет"]);
  if (node.attempts > 0) rows.push(["Failed attempts", String(node.attempts)]);
  if (node.speed > 0) rows.push(["Скорость", `${node.speed.toFixed(1)} ГБ/с · ${node.speedMeasured ? "замер" : "оценка"}`]);
  if (node.rttMs != null) rows.push(["RTT", `${node.rttMs.toFixed(1)} мс`]);
  if (node.tempC != null) rows.push(["Температура", `${node.tempC.toFixed(0)} °C${node.throttled ? " · греется, сбавляет скорость" : ""}`]);
  if (node.transport !== "local" && node.assigned) rows.push(["Сейчас", node.assigned === "model" ? "часть модели" : node.assigned === "free" ? "свободна" : node.assigned === "helper" ? "помощник" : "выключена / не в сети"]);
  return (
    <aside className="flex flex-col overflow-y-auto bg-pl-bg1">
      <div className="panel-head border-t-0"><span>Inspector</span></div>
      <div className="border-b border-pl-line p-4">
        <div className="label">Selected object</div>
        <div className="mt-1 truncate text-sm font-medium text-pl-text" title={node.name}>{node.name}</div>
        <div className="mt-2 flex flex-wrap gap-1.5"><span className={clsx("tag border", c)}>{t}</span><TransportBadge n={node} /></div>
      </div>
      <dl className="divide-y divide-pl-line text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="flex justify-between gap-3 px-4 py-2">
            <dt className="shrink-0 text-pl-dim">{k}</dt><dd className="min-w-0 truncate font-mono text-pl-text" title={v}>{v}</dd>
          </div>
        ))}
      </dl>
      {!isHostAgg && (
        <div className="space-y-3 border-y border-pl-line p-4 text-xs">
          <div className="label">Распределение слоёв</div>
          {node.transport !== "local" && <RolePicker node={node} snap={snap} />}
          {node.helper && <HelperLine h={node.helper} />}
          <label className="flex cursor-pointer items-center justify-between">
            <span className="text-pl-text">Использовать в кластере</span>
            <input type="checkbox" className="accent-[#2D72D2]" checked={node.enabled} onChange={(e) => setNodeEnabled(node.id, e.target.checked)} />
          </label>
          <div>
            <div className="flex justify-between"><span className="text-pl-text">Поправка скорости</span><span className="font-mono text-pl-gold">×{w.toFixed(2)}</span></div>
            <input type="range" min={0.25} max={6} step={0.25} value={w} className="mt-1 w-full accent-[#FBD065]"
              onChange={(e) => setW(Number(e.target.value))} onMouseUp={() => setNodeWeight(node.id, w)} onKeyUp={() => setNodeWeight(node.id, w)} onTouchEnd={() => setNodeWeight(node.id, w)} />
            <div className="mt-1 flex justify-between gap-1 font-mono text-2xs text-pl-dim"><span>медленнее</span><span className="truncate">1.0 = как измерено</span><span>быстрее</span></div>
            <p className="mt-2 text-2xs leading-4 text-pl-dim">Скорость = результат теста (или оценка) × поправка. Самые быстрые устройства получают слои первыми, но не больше своей памяти. Применяется при следующем Deploy.</p>
          </div>
          {node.transport === "usb" && <RebenchButton node={node} />}
          {node.updateAvailable && <NodeUpdateButton node={node} />}
          {node.transport !== "local" && <ReconnectButton node={node} />}
          {node.transport !== "local" && <NodeExtraRoles node={node} />}
          {node.transport === "lan" && (
            <button className="btn w-full justify-center border-pl-red/50 text-pl-red" onClick={() => removeLanNode(node.id)}><Trash2 size={12} /> Удалить LAN-ноду</button>
          )}
        </div>
      )}
      {node.error && <div className="m-4 border-l-2 border-pl-red bg-pl-red/10 p-2 font-mono text-2xs text-pl-red">{node.error}</div>}
      <div className="mt-auto border-t border-pl-line p-4 font-mono text-2xs leading-5 text-pl-dim">
        {node.transport === "local"
          ? "llama-server runs here and drives every RPC node."
          : node.transport === "lan"
            ? "rpc-server в сети: без пароля и шифрования — только закрытая домашняя сеть."
            : "rpc-server слушает loopback :50052 устройства и доступен только через USB-проброс. Отключили кабель — нода OFFLINE; вернули — подготовка повторится сама на том же порту."}
      </div>
    </aside>
  );
}

function RolePicker({ node, snap }: { node: ClusterNode; snap: ClusterSnapshot }) {
  const pushToast = useCluster((s) => s.pushToast);
  const choices = roleChoices(snap, node.role);
  return (
    <div>
      <div className="mb-1 text-pl-text">Роль</div>
      <div className="grid grid-cols-2 gap-px">
        {choices.map((r) => (
          <button key={r} title={ROLE_HINT[r]} onClick={() => setNodeRole(node.id, r).catch((e) => pushToast("error", String(e)))}
            className={clsx("border px-2 py-1 text-left text-2xs",
              node.role === r ? (r === "helper" ? "border-pl-violet bg-pl-violet/15 text-pl-text" : "border-pl-blue bg-pl-blue2/25 text-pl-text") : "border-pl-line2 bg-pl-bg2 text-pl-mute hover:bg-pl-bg3")}>
            {ROLE_LABEL[r]}
          </button>
        ))}
      </div>
      <p className="mt-1.5 text-2xs leading-4 text-pl-dim">{ROLE_HINT[node.role]}.{node.role === "helper" && !snap.rolesEnabled ? " Эксперимент «Роли» выключен — пока работает как «Авто»." : ""}</p>
      {node.role === "auto" && node.assigned === "free" && <p className="mt-1 text-2xs leading-4 text-pl-gold">Сейчас модели хватает остальных устройств — нода свободна.</p>}
    </div>
  );
}

/** Table status chip for a «Помощник»; fixed-width percent so the column doesn't jitter on every poll. */
function HelperTag({ n }: { n: ClusterNode }) {
  const h = n.helper;
  const p = h ? helperPct(h) : null;
  return (
    <span title={h ? `${h.model}: ${h.detail}` : "Помощник"}
      className={clsx("tag border", h?.ready ? "border-pl-violet/60 text-pl-violet" : "border-pl-violet/40 text-pl-violet/80")}>
      Помощник{h && !h.ready && (p != null ? <span className="inline-block w-[4ch] text-right">{Math.round(p)}%</span> : <span className="animate-blink">…</span>)}
    </span>
  );
}

function HelperLine({ h, onRetry }: { h: HelperView; onRetry?: () => void }) {
  const [t, c] = HELPER_STATE[h.state] ?? HELPER_STATE[""];
  const p = helperPct(h);
  const bar = !h.ready && (p != null || isDownloadPhase(h.state));
  const step = h.state === "pc-download" ? "1/2 · " : h.state === "node-download" ? "2/2 · " : "";
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between gap-2">
        <span className="min-w-0 truncate text-pl-text" title={h.model}>{h.model || "Помощник"}</span>
        <span className={clsx("tag shrink-0 border tabular-nums", bar ? c.replace(" animate-blink", "") : c)} title={h.detail}>{t}{p != null && !h.ready ? <span className="inline-block w-[4.5ch] text-right">{Math.round(p)}%</span> : ""}</span>
      </div>
      {/* one bar per phase (keyed by state): ПК → нода starts from 0 instead of visibly sliding back */}
      {bar && <div className="flex items-center gap-2"><span className="shrink-0 font-mono text-2xs text-pl-dim">{step}</span><Progress key={h.state} value={p} tone="violet" className="h-1" /></div>}
      {!h.ready && h.detail && <div className={clsx("text-2xs leading-4", h.state === "error" ? "text-pl-red" : "text-pl-dim")}>{h.detail}</div>}
      {h.state === "error" && <RetryHelper id={h.id} onDone={onRetry} />}
    </div>
  );
}

function RetryHelper({ id, onDone }: { id: string; onDone?: () => void }) {
  const pushToast = useCluster((s) => s.pushToast);
  return (
    <button className="btn h-6" onClick={async () => { try { await retryHelper(id); onDone?.(); pushToast("info", "Пробую запустить Помощника снова…"); } catch (e) { pushToast("error", String(e)); } }}>
      <RotateCw size={11} /> Повторить
    </button>
  );
}

/** «Повторить тест» — autobench on the device (USB). */
function RebenchButton({ node }: { node: ClusterNode }) {
  const pushToast = useCluster((s) => s.pushToast);
  const [busy, setBusy] = useState(false);
  return (
    <button className="btn w-full justify-center" disabled={busy || node.stage !== "connected"}
      title="Заново измерить, сколько ГБ/с весов модели нода обрабатывает. Пока идёт тест, нода не должна считать модель."
      onClick={async () => {
        setBusy(true);
        try { const g = await rebenchNode(node.id); pushToast("ok", `«${node.name}»: ${g.toFixed(1)} ГБ/с — учту при следующем запуске модели`); }
        catch (e) { pushToast("error", String(e)); }
        setBusy(false);
      }}>
      {busy ? <><Loader2 size={12} className="animate-spin" /> Тест скорости…</> : <><Gauge size={12} /> Повторить тест скорости</>}
    </button>
  );
}

/** Settings block «Роли · Эксперимент»: switch, helper model, helper status per node. */
function RolesPanel({ snap }: { snap: ClusterSnapshot }) {
  const { pushToast, setView } = useCluster();
  const known = snap.helperModels.some((m) => m.id === snap.helperModel);
  const [custom, setCustom] = useState(known ? "" : snap.helperModel);
  const [customOpen, setCustomOpen] = useState(!known);
  const ready = snap.helpers.filter((h) => h.ready).length;
  async function pick(model: string) {
    try { await setHelperModel(model); pushToast("info", "Модель Помощника изменена — ноды перезапустят её сами"); }
    catch (e) { pushToast("error", String(e)); }
  }
  return (
    <div className="panel">
      <div className="panel-head">
        <span className="flex shrink-0 items-center gap-1.5"><FlaskConical size={12} className="text-pl-violet" /> Роли · Эксперимент</span>
        <span className="min-w-0 truncate pl-3 font-mono normal-case tracking-normal text-pl-dim">{snap.rolesEnabled ? `помощников: ${snap.helpers.length} · готово: ${ready}` : "выключено"}</span>
      </div>
      <div className="grid grid-cols-1 gap-px bg-pl-line text-xs lg:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]">
        <div className="space-y-3 bg-pl-bg1 p-3">
          <Toggle on={snap.rolesEnabled} set={(v) => setRolesEnabled(v).catch((e) => pushToast("error", String(e)))}
            label="Роль «Помощник»" sub="свободная нода запускает свою маленькую модель рядом с основной" />
          <p className="text-2xs leading-4 text-pl-dim">
            Помощник придумывает названия чатов, делает «Сводку» переписки и отвечает в «Параллельном тесте» — основная модель при этом не отвлекается.
            Модель один раз скачивается на ПК и передаётся на ноды по кабелю или по Wi-Fi. Нужна Argus Node 0.3.2.
          </p>
          {snap.rolesEnabled && (
            <div className="space-y-1.5">
              <div className="text-pl-text">Модель Помощника</div>
              <select value={customOpen ? "custom" : snap.helperModel}
                onChange={(e) => { if (e.target.value === "custom") setCustomOpen(true); else { setCustomOpen(false); pick(e.target.value); } }}
                className="h-7 w-full border border-pl-line2 bg-pl-bg3 px-1 text-2xs text-pl-text">
                {snap.helperModels.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
                <option value="custom">Своя модель с Hugging Face…</option>
              </select>
              {customOpen && (
                <div className="flex gap-1">
                  <input value={custom} onChange={(e) => setCustom(e.target.value)} placeholder="owner/repo/Файл-Q4_0.gguf" spellCheck={false}
                    className="h-7 min-w-0 flex-1 border border-pl-line2 bg-pl-bg0 px-2 font-mono text-2xs text-pl-text placeholder:text-pl-dim" />
                  <button className="btn h-7" disabled={!custom.trim()} onClick={() => pick(custom)}>Применить</button>
                </div>
              )}
              <div className="text-2xs text-pl-dim">Лучше Q4_0 — на процессорах телефонов это самый быстрый 4-битный формат.</div>
            </div>
          )}
        </div>
        <div className="bg-pl-bg1">
          {!snap.rolesEnabled ? (
            <div className="p-3 text-2xs leading-5 text-pl-dim">Включите эксперимент, затем выберите ноде роль «Помощник» в таблице или в инспекторе. Остальные роли — «Авто», «Часть модели», «Свободна» — работают всегда.</div>
          ) : snap.helpers.length === 0 ? (
            <div className="p-3 text-2xs leading-5 text-pl-dim">Пока ни одной ноды с ролью «Помощник». Выберите её в столбце «Роль» — лучше для устройства, которое модели и так не нужно (помечено «Свободна»).</div>
          ) : (
            <div className="divide-y divide-pl-line">
              {snap.helpers.map((h) => (
                <div key={h.id} className="p-3">
                  <div className="mb-1 truncate font-medium text-pl-text" title={h.name}>{h.name}</div>
                  <HelperLine h={h} />
                </div>
              ))}
            </div>
          )}
          {ready > 0 && (
            <div className="border-t border-pl-line p-3">
              <button className="btn" onClick={() => setView("parallel")}><Columns3 size={12} /> Открыть «Параллельный тест»</button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** USB: approve `adb install -r` (the user's click IS the answer). Wi-Fi: ask the node app to offer the update on the device. */
function NodeUpdateButton({ node }: { node: ClusterNode }) {
  const pushToast = useCluster((s) => s.pushToast);
  const [busy, setBusy] = useState(false);
  const wifi = node.transport === "lan";
  return (
    <button className="btn w-full justify-center border-pl-orange/50 text-pl-orange" disabled={busy || (wifi && !node.beacon)}
      title={wifi ? "Нода покажет запрос на обновление на самом устройстве (Android попросит подтвердить установку)" : "Установить Argus Node из комплекта по USB"}
      onClick={async () => {
        setBusy(true);
        try {
          if (wifi) pushToast("info", await wifiNodeUpdate(node.id));
          else { await updateUsbNode(node.id); pushToast("info", `«${node.name}»: обновление ноды до ${node.updateAvailable}…`); }
        } catch (e) { pushToast("error", String(e)); }
        setBusy(false);
      }}>
      {busy ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />} {wifi ? "Обновить по Wi-Fi" : `Обновить ноду до ${node.updateAvailable}`}
    </button>
  );
}

function ReconnectButton({ node }: { node: ClusterNode }) {
  const pushToast = useCluster((s) => s.pushToast);
  const [busy, setBusy] = useState(false);
  return (
    <button className="btn w-full justify-center" disabled={busy}
      title={node.transport === "usb" ? "Снять проброс порта, перезапустить RpcService на устройстве и подготовить ноду заново" : "Проверить ноду сейчас"}
      onClick={async () => {
        setBusy(true);
        try { await reconnectNode(node.id); pushToast("info", `«${node.name}»: переподключение…`); } catch (e) { pushToast("error", String(e)); }
        setBusy(false);
      }}>
      {busy ? <Loader2 size={12} className="animate-spin" /> : <Plug size={12} />} Переподключить
    </button>
  );
}

/** Nodes heard on UDP :50053 — Android "Wi-Fi режим", Linux PC / Raspberry Pi (argus-node-linux.sh). */
function DiscoveredPanel({ snap }: { snap: ClusterSnapshot }) {
  const pushToast = useCluster((s) => s.pushToast);
  const [busy, setBusy] = useState<string | null>(null);
  const list = snap.discovered ?? [];
  async function add(d: DiscoveredNode) {
    setBusy(d.id);
    try { await addDiscoveredNode(d.id); pushToast("ok", `Нода «${d.name}» добавлена`); } catch (e) { pushToast("error", String(e)); }
    setBusy(null);
  }
  return (
    <div className="panel">
      <div className="panel-head">
        <span className="flex items-center gap-1.5"><Radar size={12} /> Найдено в сети</span>
        <label className="flex shrink-0 cursor-pointer items-center gap-2 font-mono normal-case tracking-normal text-pl-mute" title="Добавлять найденные ноды без подтверждения">
          <input type="checkbox" className="accent-[#2D72D2]" checked={snap.autoAddDiscovered} onChange={(e) => setAutoAdd(e.target.checked)} /> автодобавление
        </label>
      </div>
      {snap.beaconError && <div className="border-b border-pl-line bg-pl-red/10 px-3 py-1.5 font-mono text-2xs text-pl-red">{snap.beaconError}</div>}
      {list.length === 0 ? (
        <div className="px-3 py-3 text-xs leading-5 text-pl-dim">
          Пока никого. Включите <span className="text-pl-text">«Wi-Fi режим»</span> в Argus Node на телефоне или запустите
          <span className="font-mono text-pl-text"> scripts/argus-node-linux.sh</span> на Linux-ПК / Raspberry Pi — нода появится здесь сама (маяк UDP 50053 каждые 2 с).
          Если не появляется: устройства должны быть в одной сети, а Argus — разрешён в брандмауэре для частных сетей.
        </div>
      ) : (
        <div className="overflow-x-auto">
        <table className="w-full min-w-[760px] text-xs">
          <thead><tr className="border-b border-pl-line text-left">
            {["Нода", "Адрес", "RAM / свободно", "Сборка", "Версия", "Маяк", ""].map((h) => <th key={h} className="label whitespace-nowrap px-3 py-1.5 font-semibold">{h}</th>)}
          </tr></thead>
          <tbody className="whitespace-nowrap font-mono">
            {list.map((d) => (
              <tr key={d.id} className="border-b border-pl-line last:border-0 odd:bg-pl-bg1 even:bg-[#20252c]">
                <td className="max-w-[200px] truncate px-3 py-2 font-sans text-pl-text" title={d.name}>{d.name}</td>
                <td className="px-3 py-2 text-pl-mute">{d.endpoint}</td>
                <td className="px-3 py-2 text-pl-mute">{d.ramMb ? `${gb(d.ramMb)} / ${gb(d.freeMb)} GB` : "—"}</td>
                <td className="px-3 py-2 text-pl-mute">{d.variant ?? "—"}</td>
                <td className="px-3 py-2 text-pl-mute">
                  {d.appVersion ?? "—"}
                  {d.outdated && <span title={`в комплекте ${snap.bundledNodeVersion}`} className="ml-1.5 tag border border-pl-orange/50 text-pl-orange">устарела</span>}
                </td>
                <td className={clsx("px-3 py-2", d.ageSec > 10 ? "text-pl-orange" : "text-pl-green")}>{d.ageSec > 10 ? `${d.ageSec} с назад` : "● сейчас"}</td>
                <td className="px-3 py-2 text-right">
                  {d.added ? <span className="text-2xs text-pl-dim">добавлена</span> : (
                    <button className="btn h-6" disabled={busy === d.id} onClick={() => add(d)}>
                      {busy === d.id ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />} Добавить
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
      )}
      <div className="flex items-start gap-2 border-t border-pl-line bg-pl-orange/5 px-3 py-2 text-2xs leading-4 text-pl-orange">
        <ShieldAlert size={12} className="mt-0.5 shrink-0" />
        <span>У RPC по сети нет пароля и шифрования: любой в этой сети может подключиться к ноде. Только закрытая домашняя сеть — не публичный Wi-Fi, порт 50052 не открывать в интернет. USB быстрее и стабильнее Wi-Fi.</span>
      </div>
    </div>
  );
}
