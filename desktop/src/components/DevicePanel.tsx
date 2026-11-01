import { useEffect, useState } from "react";
import { Loader2, Plug, Plus, Radar, RefreshCw, RotateCw, ShieldAlert, Trash2, Usb, Wifi } from "lucide-react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { addDiscoveredNode, reconnectNode, rescan, removeLanNode, restartEngine, setAutoAdd, setNodeEnabled, setNodeWeight, updateUsbNode, wifiNodeUpdate } from "../lib/api";
import EngineBanner from "./EngineBanner";
import { gb, pct } from "../lib/format";
import MemoryPool from "./MemoryPool";
import Topology from "./Topology";
import Meter, { loadTone } from "./Meter";
import type { ClusterNode, ClusterSnapshot, DiscoveredNode, NodeStage } from "../lib/types";

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

/** host row (when no llama GPU list), every local GPU, then USB + LAN nodes */
export const tableNodes = (s: ClusterSnapshot) => [...(s.gpus.length ? s.gpus : [s.host]), ...s.nodes];

export default function DevicePanel() {
  const { snapshot, setWizard, setLanDialog, pushToast } = useCluster();
  const [sel, setSel] = useState("host");
  const [restarting, setRestarting] = useState(false);
  if (!snapshot) return <div className="p-6 font-mono text-xs text-pl-dim">AWAITING TELEMETRY…</div>;
  const nodes = tableNodes(snapshot);
  const current = nodes.find((n) => n.id === sel) ?? (sel === "host" ? snapshot.host : nodes[0]);

  return (
    <div className="grid h-full grid-cols-[1fr_300px] gap-px bg-pl-line">
      <div className="flex min-w-0 flex-col gap-3 overflow-y-auto bg-pl-bg0 p-3">
        <EngineBanner compact />
        <MemoryPool />
        <div className="panel">
          <div className="panel-head">
            <span>Topology</span>
            <div className="flex gap-1.5 normal-case tracking-normal">
              <button className="btn" title="Повторить подготовку упавших устройств, перезапустить adb при сбое" onClick={() => rescan()}><RefreshCw size={12} /> Rescan</button>
              {snapshot.engine.canRestart && (
                <button className="btn" disabled={restarting} title="Перезапустить llama-server с текущим набором нод"
                  onClick={async () => { setRestarting(true); try { await restartEngine(); pushToast("info", "Движок перезапускается…"); } catch (e) { pushToast("error", String(e)); } setRestarting(false); }}>
                  {restarting ? <Loader2 size={12} className="animate-spin" /> : <RotateCw size={12} />} Перезапустить движок
                </button>
              )}
              <button className="btn" data-tour="lan" onClick={() => setLanDialog(true)}><Wifi size={12} /> Добавить ноду по IP</button>
              <button className="btn-primary" data-tour="usb" onClick={() => setWizard(true)}><Usb size={12} /> Connect via USB</button>
            </div>
          </div>
          <div className="h-[300px]"><Topology selected={sel} onSelect={setSel} /></div>
        </div>
        <NodeTable nodes={nodes} sel={sel} onSel={setSel} snap={snapshot} />
        <DiscoveredPanel snap={snapshot} />
      </div>
      <Inspector node={current} snap={snapshot} />
    </div>
  );
}

function NodeTable({ nodes, sel, onSel, snap }: { nodes: ClusterNode[]; sel: string; onSel: (id: string) => void; snap: ClusterSnapshot }) {
  const share = (id: string) => snap.split.filter((e) => e.id === id).reduce((a, e) => a + e.share, 0);
  return (
    <div className="panel">
      <div className="panel-head"><span>Nodes</span><span className="font-mono normal-case tracking-normal text-pl-dim">{nodes.length} objects · -ts ∝ память × вес</span></div>
      <table className="w-full text-xs">
        <thead>
          <tr className="border-b border-pl-line text-left">
            {["Node", "Link", "Status", "Endpoint / device", "Memory", "Allocatable", "Weight", "Split", "Load"].map((h) =>
              <th key={h} className="label px-3 py-1.5 font-semibold">{h}</th>)}
          </tr>
        </thead>
        <tbody className="font-mono">
          {nodes.map((n) => {
            const used = n.totalMb - n.availableMb;
            const [t, c] = stageTag[n.stage];
            const sh = share(n.id);
            return (
              <tr key={n.id} onClick={() => onSel(n.id)}
                className={clsx("cursor-pointer border-b border-pl-line last:border-0",
                  sel === n.id ? "bg-pl-blue2/20" : "odd:bg-pl-bg1 even:bg-[#20252c] hover:bg-pl-bg3", !n.enabled && "opacity-50")}>
                <td className="px-3 py-2 font-sans text-pl-text">{n.name}{!n.enabled && <span className="ml-1.5 text-2xs text-pl-dim">(выкл)</span>}</td>
                <td className="px-3 py-2"><TransportBadge n={n} /></td>
                <td className="px-3 py-2">
                  <span className={clsx("tag border", c)}>{t}</span>
                  {n.inEngine && <span title="используется запущенной моделью" className="ml-1.5 text-2xs text-pl-green">● в модели</span>}
                  {n.updateAvailable && <span title={`установлена ${n.appVersion ?? n.version ?? "?"}, в комплекте ${n.updateAvailable}`} className="ml-1.5 tag border border-pl-orange/50 text-pl-orange">Нода устарела</span>}
                  {n.stage === "error" && n.retryIn != null && <div className="mt-0.5 text-2xs text-pl-dim">повтор через {n.retryIn} с · попытка {n.attempts + 1}</div>}
                </td>
                <td className="px-3 py-2 text-pl-mute">{n.device ?? n.endpoint ?? (n.transport === "local" ? "local" : "—")}</td>
                <td className="w-36 px-3 py-2">
                  <div className="mb-1 text-2xs text-pl-mute">{gb(used)} / {gb(n.totalMb)} GB</div>
                  <Meter value={pct(used, n.totalMb)} tone="blue" />
                </td>
                <td className="px-3 py-2 text-pl-green">{gb(n.usableMb)} GB</td>
                <td className="px-3 py-2 text-pl-mute">×{n.weight.toFixed(2)}</td>
                <td className="px-3 py-2 text-pl-text">{sh ? `${(sh * 100).toFixed(0)}%` : "—"}</td>
                <td className="w-28 px-3 py-2">
                  <div className="mb-1 text-2xs text-pl-mute">{n.loadPct.toFixed(0)}%</div>
                  <Meter value={n.loadPct} tone={loadTone(n.loadPct)} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
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
  return (
    <aside className="flex flex-col overflow-y-auto bg-pl-bg1">
      <div className="panel-head border-t-0"><span>Inspector</span></div>
      <div className="border-b border-pl-line p-4">
        <div className="label">Selected object</div>
        <div className="mt-1 text-sm font-medium text-pl-text">{node.name}</div>
        <div className="mt-2 flex gap-1.5"><span className={clsx("tag border", c)}>{t}</span><TransportBadge n={node} /></div>
      </div>
      <dl className="divide-y divide-pl-line text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="flex justify-between gap-3 px-4 py-2">
            <dt className="text-pl-dim">{k}</dt><dd className="truncate font-mono text-pl-text">{v}</dd>
          </div>
        ))}
      </dl>
      {!isHostAgg && (
        <div className="space-y-3 border-y border-pl-line p-4 text-xs">
          <div className="label">Распределение слоёв</div>
          <label className="flex cursor-pointer items-center justify-between">
            <span className="text-pl-text">Использовать в кластере</span>
            <input type="checkbox" className="accent-[#2D72D2]" checked={node.enabled} onChange={(e) => setNodeEnabled(node.id, e.target.checked)} />
          </label>
          <div>
            <div className="flex justify-between"><span className="text-pl-text">Вес скорости</span><span className="font-mono text-pl-gold">×{w.toFixed(2)}</span></div>
            <input type="range" min={0.25} max={6} step={0.25} value={w} className="mt-1 w-full accent-[#FBD065]"
              onChange={(e) => setW(Number(e.target.value))} onMouseUp={() => setNodeWeight(node.id, w)} onKeyUp={() => setNodeWeight(node.id, w)} onTouchEnd={() => setNodeWeight(node.id, w)} />
            <div className="mt-1 flex justify-between font-mono text-2xs text-pl-dim"><span>медленнее</span><span>1.0 = CPU телефона/Pi</span><span>быстрее</span></div>
            <p className="mt-2 text-2xs leading-4 text-pl-dim">Доля слоёв ∝ свободная память × вес. Быстрые устройства заполняются первыми, но не больше их памяти. Применяется при следующем Deploy.</p>
          </div>
          {node.updateAvailable && <NodeUpdateButton node={node} />}
          {node.transport !== "local" && <ReconnectButton node={node} />}
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
        <label className="flex cursor-pointer items-center gap-2 font-mono normal-case tracking-normal text-pl-mute" title="Добавлять найденные ноды без подтверждения">
          <input type="checkbox" className="accent-[#2D72D2]" checked={snap.autoAddDiscovered} onChange={(e) => setAutoAdd(e.target.checked)} /> автодобавление
        </label>
      </div>
      {snap.beaconError && <div className="border-b border-pl-line bg-pl-red/10 px-3 py-1.5 font-mono text-2xs text-pl-red">{snap.beaconError}</div>}
      {list.length === 0 ? (
        <div className="px-3 py-3 text-xs leading-5 text-pl-dim">
          Пока никого. Включите <span className="text-pl-text">«Wi-Fi режим»</span> в Argus Node на телефоне/планшете или запустите
          <span className="font-mono text-pl-text"> scripts/argus-node-linux.sh</span> на Linux-ПК / Raspberry Pi — нода появится здесь сама (маяк UDP 50053 каждые 2 с).
          Если не появляется: устройства должны быть в одной сети, а Argus — разрешён в брандмауэре для частных сетей.
        </div>
      ) : (
        <table className="w-full text-xs">
          <thead><tr className="border-b border-pl-line text-left">
            {["Нода", "Адрес", "RAM / свободно", "Сборка", "Версия", "Маяк", ""].map((h) => <th key={h} className="label px-3 py-1.5 font-semibold">{h}</th>)}
          </tr></thead>
          <tbody className="font-mono">
            {list.map((d) => (
              <tr key={d.id} className="border-b border-pl-line last:border-0 odd:bg-pl-bg1 even:bg-[#20252c]">
                <td className="px-3 py-2 font-sans text-pl-text">{d.name}</td>
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
      )}
      <div className="flex items-start gap-2 border-t border-pl-line bg-pl-orange/5 px-3 py-2 text-2xs leading-4 text-pl-orange">
        <ShieldAlert size={12} className="mt-0.5 shrink-0" />
        <span>У RPC по сети нет пароля и шифрования: любой в этой сети может подключиться к ноде. Только закрытая домашняя сеть — не публичный Wi-Fi, порт 50052 не открывать в интернет. USB быстрее и стабильнее Wi-Fi.</span>
      </div>
    </div>
  );
}
