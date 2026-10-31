import { useEffect, useState } from "react";
import { RefreshCw, Trash2, Usb, Wifi } from "lucide-react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { rescan, removeLanNode, setNodeEnabled, setNodeWeight } from "../lib/api";
import { gb, pct } from "../lib/format";
import MemoryPool from "./MemoryPool";
import Topology from "./Topology";
import Meter, { loadTone } from "./Meter";
import type { ClusterNode, ClusterSnapshot, NodeStage } from "../lib/types";

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
  n.transport === "lan" ? <span className="tag border border-pl-cyan/60 text-pl-cyan">LAN</span>
  : n.transport === "usb" ? <span className="tag border border-pl-line2 text-pl-mute">USB</span>
  : <span className="tag border border-pl-line2 text-pl-mute">{n.integrated ? "iGPU" : n.kind === "gpu" ? "GPU" : "local"}</span>;

/** host row (when no llama GPU list), every local GPU, then USB + LAN nodes */
export const tableNodes = (s: ClusterSnapshot) => [...(s.gpus.length ? s.gpus : [s.host]), ...s.nodes];

export default function DevicePanel() {
  const { snapshot, setWizard, setLanDialog } = useCluster();
  const [sel, setSel] = useState("host");
  if (!snapshot) return <div className="p-6 font-mono text-xs text-pl-dim">AWAITING TELEMETRY…</div>;
  const nodes = tableNodes(snapshot);
  const current = nodes.find((n) => n.id === sel) ?? (sel === "host" ? snapshot.host : nodes[0]);

  return (
    <div className="grid h-full grid-cols-[1fr_300px] gap-px bg-pl-line">
      <div className="flex min-w-0 flex-col gap-3 overflow-y-auto bg-pl-bg0 p-3">
        <MemoryPool />
        <div className="panel">
          <div className="panel-head">
            <span>Topology</span>
            <div className="flex gap-1.5 normal-case tracking-normal">
              <button className="btn" onClick={() => rescan()}><RefreshCw size={12} /> Rescan</button>
              <button className="btn" data-tour="lan" onClick={() => setLanDialog(true)}><Wifi size={12} /> Добавить ноду по IP</button>
              <button className="btn-primary" data-tour="usb" onClick={() => setWizard(true)}><Usb size={12} /> Connect via USB</button>
            </div>
          </div>
          <div className="h-[300px]"><Topology selected={sel} onSelect={setSel} /></div>
        </div>
        <NodeTable nodes={nodes} sel={sel} onSel={setSel} snap={snapshot} />
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
                <td className="px-3 py-2"><span className={clsx("tag border", c)}>{t}</span></td>
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
            ? "rpc-server on the LAN: no auth, no encryption — closed home network only."
            : "rpc-server listens on device loopback :50052 and is reachable only through the USB forward."}
      </div>
    </aside>
  );
}
