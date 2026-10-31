import { Loader2, Rocket, Usb } from "lucide-react";
import { useCluster } from "../store/cluster";
import { bestPick, requiredMb } from "../lib/models";
import { deploy } from "../lib/launch";
import { gb } from "../lib/format";

/** Empty-state card: one button that picks the best curated model for the pool and deploys it. */
export default function QuickStart() {
  const { snapshot, ctx, run, setWizard } = useCluster();
  const usable = snapshot?.usableMb ?? 0;
  const pick = bestPick(usable, ctx);
  const loading = run.status === "loading";
  const nodes = (snapshot?.nodes.filter((n) => n.stage === "connected" && n.enabled).length ?? 0);
  return (
    <div data-tour="quickstart" className="w-[440px] border border-pl-line2 bg-pl-bg1 text-left shadow-xl">
      <div className="panel-head"><span>Быстрый старт</span><span className="font-mono normal-case tracking-normal text-pl-dim">пул {gb(usable)} GB · {nodes + 1} узл.</span></div>
      <div className="p-4">
        {pick ? (
          <>
            <div className="label">Лучший выбор для вашего пула</div>
            <div className="mt-1 text-base font-medium text-pl-text">{pick.name}</div>
            <div className="mt-0.5 font-mono text-2xs text-pl-dim">{pick.params} · {pick.quant} · нужно {gb(requiredMb(pick, ctx))} GB при {ctx / 1024}K контексте{pick.moe ? " · MoE" : ""}</div>
            {pick.note && <div className="mt-1 text-2xs text-pl-mute">{pick.note}</div>}
          </>
        ) : <div className="text-xs text-pl-mute">Пул пока пуст. Подключите планшет/телефон по USB или добавьте ноду по IP.</div>}
        <button className="btn-primary mt-4 h-10 w-full justify-center text-sm" disabled={!pick || loading} onClick={() => pick && deploy(pick)}>
          {loading ? <><Loader2 size={14} className="animate-spin" /> Загрузка {run.model.name}…</> : <><Rocket size={14} /> Подобрать модель и запустить</>}
        </button>
        {nodes === 0 && (
          <button className="mt-2 flex w-full items-center justify-center gap-1.5 text-2xs text-pl-blue hover:underline" onClick={() => setWizard(true)}>
            <Usb size={11} /> Добавить устройство, чтобы запускать модели крупнее
          </button>
        )}
        {run.status === "error" && <div className="mt-3 border-l-2 border-pl-red bg-pl-red/10 p-2 font-mono text-2xs text-pl-red">{run.message}</div>}
      </div>
    </div>
  );
}
