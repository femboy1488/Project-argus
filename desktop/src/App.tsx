import { useEffect } from "react";
import { onClusterUpdate, onLlamaLog, onDownload, onEngineState, onToast, onUpdateAvailable, onUpdateProgress, platformInfo, setKeepLoaded, updateStatus } from "./lib/api";
import { AboutDialog, NodeUpdateDialog, UpdateDialog, useNodeToAsk } from "./components/Updates";
import type { UpdateStatus } from "./lib/types";
import { useCluster } from "./store/cluster";
import TopBar from "./components/TopBar";
import NavRail from "./components/NavRail";
import ChatView from "./components/ChatView";
import DevicePanel from "./components/DevicePanel";
import ModelLibrary from "./components/ModelLibrary";
import ApiPanel from "./components/ApiPanel";
import ParallelTest from "./components/ParallelTest";
import FeaturesView from "./components/FeaturesView";
import QueueView from "./components/QueueView";
import KnowledgeView from "./components/KnowledgeView";
import UsbWizard from "./components/UsbWizard";
import StatusBar from "./components/StatusBar";
import LanDialog from "./components/LanDialog";
import Tour from "./components/Tour";
import Toaster from "./components/Toaster";
import { reattach, applyEngine } from "./lib/launch";
import { setPlatform } from "./lib/agent";

export default function App() {
  const { view, setSnapshot, wizardOpen, lanDialog, tourOpen, setRun, updateDialog, aboutOpen } = useCluster();
  const nodeAsk = useNodeToAsk();

  // self-update: ASK (never install by itself). «Позже» = not again this session, «Пропустить» = never for that version.
  useEffect(() => {
    const offer = (s: UpdateStatus) => {
      const st = useCluster.getState();
      st.setUpdate(s);
      if (s.available && !s.skipped && s.latest && !st.updateLater.includes(s.latest.version)) st.setUpdateDialog(true);
    };
    updateStatus().then(offer).catch(() => {});
    let offA: (() => void) | undefined;
    let offP: (() => void) | undefined;
    onUpdateAvailable(offer).then((f) => (offA = f));
    onUpdateProgress((p) => useCluster.getState().setUpdProgress(p)).then((f) => (offP = f));
    return () => { offA?.(); offP?.(); };
  }, []);

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    let first = true;
    onClusterUpdate((snap) => {
      if (first) {
        first = false;
        useCluster.getState().setSpeed({ fastStart: snap.fastStart });
        setKeepLoaded(useCluster.getState().speed.keepLoaded).catch(() => {}); // Rust decides on exit whether to stop the engine
      }
      setSnapshot(snap);
      if (snap.engine) applyEngine(snap.engine);
    }).then((f) => (off1 = f));
    reattach();
    platformInfo().then(setPlatform).catch(() => {});
    let off4: (() => void) | undefined;
    let off5: (() => void) | undefined;
    onEngineState((e) => {
      // keep snapshot.engine fresh between cluster updates («Прогрев…» ends with an engine-state event)
      const snap = useCluster.getState().snapshot;
      if (snap) { const { toast: _t, ...engine } = e; setSnapshot({ ...snap, engine }); }
      applyEngine(e, true);
    }).then((f) => (off4 = f));
    onToast((t) => useCluster.getState().pushToast(t.kind, t.text)).then((f) => (off5 = f));
    onLlamaLog((line) => {
      useCluster.getState().pushEngineLog(line);
      const run = useCluster.getState().run;
      if (run.status === "loading") setRun({ ...run, log: line });
    }).then((f) => (off2 = f));
    let off3: (() => void) | undefined;
    onDownload((b) => useCluster.getState().setDl(b)).then((f) => (off3 = f));
    return () => { off1?.(); off2?.(); off3?.(); off4?.(); off5?.(); };
  }, [setSnapshot, setRun]);

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden text-pl-text">
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <NavRail />
        <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
          {view === "chat" && <ChatView />}
          {view === "devices" && <DevicePanel />}
          {view === "models" && <ModelLibrary />}
          {view === "api" && <ApiPanel />}
          {view === "parallel" && <ParallelTest />}
          {view === "features" && <FeaturesView />}
          {view === "queue" && <QueueView />}
          {view === "knowledge" && <KnowledgeView />}
        </main>
      </div>
      <StatusBar />
      {wizardOpen && <UsbWizard />}
      {lanDialog && <LanDialog />}
      {tourOpen && <Tour />}
      {aboutOpen && <AboutDialog />}
      {updateDialog ? <UpdateDialog /> : nodeAsk && !wizardOpen && !tourOpen && <NodeUpdateDialog key={nodeAsk[0].id} node={nodeAsk[0]} version={nodeAsk[1]} />}
      <Toaster />
    </div>
  );
}
