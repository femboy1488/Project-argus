import { useEffect, useState } from "react";
import { ChevronRight, CircleHelp, Hexagon } from "lucide-react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { gb } from "../lib/format";

const titles = { chat: "Chat", devices: "Cluster Topology", models: "Model Registry", api: "API Access" };

export default function TopBar() {
  const { view, snapshot, run, setTour } = useCluster();
  const [now, setNow] = useState(new Date());
  useEffect(() => { const t = setInterval(() => setNow(new Date()), 1000); return () => clearInterval(t); }, []);
  const online = (snapshot?.nodes.filter((n) => n.stage === "connected" && n.enabled).length ?? 0) + Math.max(1, snapshot?.gpus.filter((g) => g.enabled).length ?? 1);

  return (
    <header className="flex h-10 items-center border-b border-pl-line bg-pl-bg2 pl-3 pr-4">
      <div className="flex items-center gap-2">
        <Hexagon size={18} className="fill-pl-blue2 text-pl-blue" strokeWidth={1.5} />
        <span className="text-[13px] font-semibold tracking-[0.18em] text-pl-text">ARGUS</span>
        <span className="text-2xs text-pl-dim">LOCAL COMPUTE</span>
      </div>
      <div className="ml-6 flex items-center gap-1 text-xs text-pl-mute">
        <span>Workspace</span><ChevronRight size={12} className="text-pl-dim" />
        <span>usb-cluster-01</span><ChevronRight size={12} className="text-pl-dim" />
        <span className="text-pl-text">{titles[view]}</span>
      </div>

      <div className="ml-auto flex items-center gap-5 font-mono text-2xs">
        <Stat k="NODES" v={`${online}`} />
        <Stat k="POOL" v={`${gb(snapshot?.totalMb ?? 0)} GB`} />
        <Stat k="ALLOCATABLE" v={`${gb(snapshot?.usableMb ?? 0)} GB`} />
        <span className={clsx("tag border",
          run.status === "ready" ? "border-pl-green/50 text-pl-green" :
          run.status === "loading" ? "animate-blink border-pl-orange/50 text-pl-orange" :
          run.status === "error" ? "border-pl-red/50 text-pl-red" : "border-pl-line2 text-pl-dim")}>
          ● {run.status === "idle" ? "ENGINE IDLE" : run.status === "error" ? "ENGINE ERROR" : `${run.model.name}`}
        </span>
        <span className="text-pl-dim">{now.toISOString().slice(11, 19)}Z</span>
        <button title="Обучение" onClick={() => setTour(true)} className="flex h-6 w-6 items-center justify-center rounded border border-pl-line2 text-pl-mute hover:bg-pl-bg3 hover:text-pl-text">
          <CircleHelp size={13} />
        </button>
      </div>
    </header>
  );
}

const Stat = ({ k, v }: { k: string; v: string }) => (
  <span className="flex gap-1.5"><span className="text-pl-dim">{k}</span><span className="text-pl-text">{v}</span></span>
);
