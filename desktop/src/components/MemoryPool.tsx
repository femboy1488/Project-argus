import { useCluster } from "../store/cluster";
import { gb, pct } from "../lib/format";

export const NODE_COLORS = ["#4C90F0", "#BDADFF", "#68C1EE", "#32A467", "#EC9A3C", "#FF66A1"];

/** Segmented allocation strip: one block per node, proportional to its memory. */
export default function MemoryPool() {
  const snap = useCluster((s) => s.snapshot);
  const nodes = snap ? [...(snap.gpus.length ? snap.gpus : [snap.host]), ...snap.nodes].filter((n) => n.stage === "connected" && n.enabled) : [];
  // strip widths are shares of the *shown* nodes (snap.totalMb can include nodes filtered out here → segments summed past 100%)
  const shownMb = nodes.reduce((a, n) => a + Math.max(0, n.totalMb), 0);
  const total = snap?.totalMb ?? 0;
  const usable = snap?.usableMb ?? 0;

  return (
    <div className="panel">
      <div className="panel-head"><span>Memory Pool</span><span className="font-mono normal-case tracking-normal text-pl-dim">VRAM + RAM via RPC</span></div>
      <div className="grid grid-cols-1 gap-4 p-4 lg:grid-cols-[auto_minmax(0,1fr)] lg:gap-6">
        <div className="lg:pr-6">
          <div className="label">Total cluster memory</div>
          <div className="mt-1 flex items-baseline gap-1 font-mono">
            <span className="text-4xl font-semibold text-pl-text">{gb(total)}</span>
            <span className="text-sm text-pl-mute">GB</span>
          </div>
          <div className="mt-1 font-mono text-2xs text-pl-dim">
            <span className="text-pl-green">{gb(usable)} GB</span> allocatable · {nodes.length} nodes
          </div>
        </div>
        <div className="flex min-w-0 flex-col justify-center">
          <div className="flex h-6 w-full gap-px overflow-hidden bg-pl-bg0 p-px">
            {nodes.map((n, i) => (
              <div key={n.id} title={`${n.name}: ${gb(n.totalMb)} GB · ${gb(n.usableMb)} GB allocatable`} className="relative h-full min-w-[2px] shrink overflow-hidden transition-[width] duration-700 ease-out"
                style={{ width: `${pct(n.totalMb, shownMb)}%`, background: `${NODE_COLORS[i % 6]}26`, borderTop: `2px solid ${NODE_COLORS[i % 6]}` }}>
                <div className="absolute inset-y-0 left-0 transition-[width] duration-700 ease-out" style={{ width: `${pct(Math.max(0, n.usableMb), n.totalMb)}%`, background: `${NODE_COLORS[i % 6]}55` }} />
                <span className="absolute left-1.5 right-1 top-0.5 truncate font-mono text-2xs text-pl-text">{n.name}</span>
              </div>
            ))}
          </div>
          <div className="mt-2 flex flex-wrap gap-x-5 gap-y-1 font-mono text-2xs text-pl-mute">
            {nodes.map((n, i) => (
              <span key={n.id} className="flex items-center gap-1.5 whitespace-nowrap tabular-nums">
                <span className="h-2 w-2" style={{ background: NODE_COLORS[i % 6] }} />
                {n.name} <span className="text-pl-dim">{gb(n.totalMb)} / {gb(n.usableMb)} GB</span>
              </span>
            ))}
            <span className="ml-auto hidden text-pl-dim xl:inline">solid = allocatable · tint = reserved headroom</span>
          </div>
        </div>
      </div>
    </div>
  );
}
