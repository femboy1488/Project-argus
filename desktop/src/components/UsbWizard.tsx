import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { gb } from "../lib/format";
import type { NodeStage } from "../lib/types";

type Step = 0 | 1 | 2 | 3; // searching, connected, syncing, ready
const STEPS = ["Searching", "Connected", "Syncing memory", "Ready"];

const stepOf = (s?: NodeStage): Step =>
  s === "offline" ? 0 :
  !s || s === "unauthorized" ? 0 : s === "syncing" ? 2 : s === "connected" ? 3 : 1;

export default function UsbWizard() {
  const { snapshot, setWizard } = useCluster();
  const baseline = useRef<Set<string> | null>(null);
  const [target, setTarget] = useState<string | null>(null);
  const [log, setLog] = useState<string[]>(["[adb] polling `adb devices -l` every 1500 ms"]);

  useEffect(() => {
    if (!snapshot) return;
    if (!baseline.current) baseline.current = new Set(snapshot.nodes.filter((n) => n.stage === "connected").map((n) => n.id));
    if (!target) { const f = snapshot.nodes.find((n) => n.transport === "usb" && !baseline.current!.has(n.id)); if (f) setTarget(f.id); }
  }, [snapshot, target]);

  const node = snapshot?.nodes.find((n) => n.id === target);
  const lastStage = useRef<string>();
  useEffect(() => {
    if (!node || node.stage === lastStage.current) return;
    lastStage.current = node.stage;
    const msg: Record<NodeStage, string> = {
      unauthorized: `[adb] ${node.id} unauthorized: waiting for RSA key approval on device`,
      installing: `[pm] checking com.harkcluster.node on ${node.id}`,
      updating: `[pm] node ${node.version ?? "old"} -> bundled: adb install -r`,
      offline: `[net] ${node.id} offline`,
      forwarding: `[adb] forward tcp:5005x -> tcp:50052`,
      starting: `[am] start-foreground-service .RpcService`,
      syncing: `[rpc] endpoint alive, reading /proc/meminfo`,
      connected: `[ok] ${node.name} joined: +${gb(node.totalMb)} GB`,
      error: `[fault] ${node.error}`,
    };
    setLog((l) => [...l, msg[node.stage]]);
  }, [node]);

  const step = stepOf(node?.stage);
  const err = node?.stage === "error";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="max-h-[92vh] w-[600px] max-w-[calc(100vw-2rem)] overflow-y-auto border border-pl-line2 bg-pl-bg1 shadow-2xl">
        <div className="flex h-9 items-center justify-between border-b border-pl-line bg-pl-bg2 px-3">
          <span className="text-xs font-semibold uppercase tracking-[0.12em] text-pl-mute">Add node · USB</span>
          <button onClick={() => setWizard(false)} className="text-pl-dim hover:text-pl-text"><X size={15} /></button>
        </div>

        <div className="grid grid-cols-4 border-b border-pl-line">
          {STEPS.map((s, i) => (
            <div key={s} className={clsx("relative border-r border-pl-line px-3 py-2.5 last:border-0",
              i < step ? "text-pl-green" : i === step ? (err ? "text-pl-red" : "text-pl-text") : "text-pl-dim")}>
              <div className="font-mono text-2xs">0{i + 1}</div>
              <div className="text-xs">{s}</div>
              {i === step && !err && step < 3 && <div className="absolute bottom-0 left-0 h-0.5 w-full overflow-hidden bg-pl-bg3">
                <div className="h-full w-1/3 animate-sweep bg-pl-blue" /></div>}
              {(i < step || step === 3) && <div className="absolute bottom-0 left-0 h-0.5 w-full bg-pl-green" />}
            </div>
          ))}
        </div>

        <div className="p-5">
          {step === 0 && (node?.stage === "unauthorized"
            ? <Msg title={`Authorize ${node.name}`} body="Unlock the device and tap Allow on “Allow USB debugging?”. Tick “Always allow from this computer”." tone="text-pl-orange" />
            : <Msg title="Waiting for a device" body="Connect an Android device by USB. Developer options → USB debugging must be enabled." />)}
          {(step === 1 || step === 2) && !err && <Msg title={node!.name} body={`Serial ${node!.id} · provisioning rpc-server node`} />}
          {err && <Msg title="Provisioning failed" body={node?.error ?? ""} tone="text-pl-red" />}
          {step === 3 && node && <Msg title={`${node.name} online`} body={`+${gb(node.totalMb)} GB · cluster pool is now ${gb(snapshot!.totalMb)} GB`} tone="text-pl-green" />}

          <div className="mt-4 h-28 overflow-y-auto border border-pl-line bg-pl-bg0 p-2 font-mono text-2xs leading-5 text-pl-mute">
            {log.map((l, i) => <div key={i}>{new Date().toISOString().slice(11, 19)} {l}</div>)}
          </div>
        </div>

        <div className="flex justify-end gap-2 border-t border-pl-line bg-pl-bg2 px-3 py-2">
          <button className="btn" onClick={() => setWizard(false)}>{step === 3 ? "Close" : "Cancel"}</button>
          {step === 3 && <button className="btn-primary" onClick={() => setWizard(false)}>Done</button>}
        </div>
      </div>
    </div>
  );
}

const Msg = ({ title, body, tone }: { title: string; body: string; tone?: string }) => (
  <div>
    <div className={clsx("text-sm font-medium", tone ?? "text-pl-text")}>{title}</div>
    <div className="mt-1 text-xs text-pl-mute">{body}</div>
  </div>
);
