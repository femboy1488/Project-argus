import { useEffect, useState } from "react";
import { ChevronRight, CircleHelp, Hexagon, Info } from "lucide-react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { gb } from "../lib/format";

const titles = { chat: "Chat", devices: "Cluster Topology", models: "Model Registry", api: "API Access", parallel: "Параллельный тест", features: "Возможности", queue: "Ночная очередь", knowledge: "Знания" };

export default function TopBar() {
  const { view, snapshot, run, setTour, setAbout, update, setUpdateDialog } = useCluster();
  const upd = update?.available && update.latest ? update.latest.version : null;
  const warming = run.status === "ready" && !!snapshot?.engine.warming;
  const [now, setNow] = useState(new Date());
  useEffect(() => { const t = setInterval(() => setNow(new Date()), 1000); return () => clearInterval(t); }, []);
  const online = (snapshot?.nodes.filter((n) => n.stage === "connected" && n.enabled).length ?? 0) + Math.max(1, snapshot?.gpus.filter((g) => g.enabled).length ?? 1);

  return (
    <header className="flex h-10 min-w-0 items-center gap-3 overflow-hidden whitespace-nowrap border-b border-pl-line bg-pl-bg2 pl-3 pr-3 xl:pr-4">
      <div className="flex shrink-0 items-center gap-2">
        <Hexagon size={18} className="fill-pl-blue2 text-pl-blue" strokeWidth={1.5} />
        <span className="text-[13px] font-semibold tracking-[0.18em] text-pl-text">ARGUS</span>
        <span className="hidden text-2xs text-pl-dim xl:inline">LOCAL COMPUTE</span>
      </div>
      <div className="ml-1 flex min-w-0 items-center gap-1 text-xs text-pl-mute xl:ml-3">
        <span className="hidden 2xl:inline">Workspace</span><ChevronRight size={12} className="hidden shrink-0 text-pl-dim 2xl:block" />
        <span className="hidden xl:inline">argus-cluster</span><ChevronRight size={12} className="hidden shrink-0 text-pl-dim xl:block" />
        <span className="truncate text-pl-text" title={titles[view]}>{titles[view]}</span>
      </div>

      <div className="ml-auto flex min-w-0 items-center gap-3 font-mono text-2xs xl:gap-5">
        <Stat k="NODES" v={`${online}`} className="hidden lg:flex" />
        <Stat k="POOL" v={`${gb(snapshot?.totalMb ?? 0)} GB`} className="hidden lg:flex" />
        <Stat k="ALLOCATABLE" v={`${gb(snapshot?.usableMb ?? 0)} GB`} className="hidden xl:flex" />
        <span className={clsx("tag block min-w-0 max-w-[260px] truncate border",
          warming ? "animate-blink border-pl-cyan/60 text-pl-cyan" :
          run.status === "ready" ? "border-pl-green/50 text-pl-green" :
          run.status === "loading" ? "animate-blink border-pl-orange/50 text-pl-orange" :
          run.status === "error" ? "border-pl-red/50 text-pl-red" :
          run.status === "broken" ? "animate-blink border-pl-orange/60 text-pl-orange" : "border-pl-line2 text-pl-dim")}
          title={run.status === "broken" ? run.reason : warming ? "Модель загружена и разогревает кэш системного промпта — первый ответ будет быстрее" : run.status === "idle" ? undefined : run.status === "error" ? run.message : run.model.name}>
          ● {run.status === "idle" ? "ENGINE IDLE" : run.status === "error" ? "ENGINE ERROR" : run.status === "broken" ? `${run.model.name} · ОСТАНОВЛЕН` : warming ? `${run.model.name} · Прогрев…` : `${run.model.name}`}
        </span>
        <span className="hidden text-pl-dim xl:inline">{now.toISOString().slice(11, 19)}Z</span>
        {upd && (
          <button title="Доступно обновление — нажмите, чтобы решить" onClick={() => setUpdateDialog(true)}
            className="tag shrink-0 border border-pl-green/60 text-pl-green hover:bg-pl-green/10">⬆ {upd}</button>
        )}
        <button title="О программе · обновления" onClick={() => setAbout(true)} className="flex h-6 w-6 shrink-0 items-center justify-center rounded border border-pl-line2 text-pl-mute hover:bg-pl-bg3 hover:text-pl-text">
          <Info size={13} />
        </button>
        <button title="Обучение" onClick={() => setTour(true)} className="flex h-6 w-6 shrink-0 items-center justify-center rounded border border-pl-line2 text-pl-mute hover:bg-pl-bg3 hover:text-pl-text">
          <CircleHelp size={13} />
        </button>
      </div>
    </header>
  );
}

const Stat = ({ k, v, className }: { k: string; v: string; className?: string }) => (
  <span className={clsx("shrink-0 gap-1.5", className ?? "flex")}><span className="text-pl-dim">{k}</span><span className="text-pl-text">{v}</span></span>
);
