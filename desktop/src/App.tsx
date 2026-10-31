import { useEffect } from "react";
import { onClusterUpdate, onLlamaLog , onDownload } from "./lib/api";
import { useCluster } from "./store/cluster";
import TopBar from "./components/TopBar";
import NavRail from "./components/NavRail";
import ChatView from "./components/ChatView";
import DevicePanel from "./components/DevicePanel";
import ModelLibrary from "./components/ModelLibrary";
import ApiPanel from "./components/ApiPanel";
import UsbWizard from "./components/UsbWizard";
import StatusBar from "./components/StatusBar";
import LanDialog from "./components/LanDialog";
import Tour from "./components/Tour";
import { reattach } from "./lib/launch";

export default function App() {
  const { view, setSnapshot, wizardOpen, lanDialog, tourOpen, setRun } = useCluster();

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    let first = true;
    onClusterUpdate((snap) => {
      if (first) { first = false; useCluster.getState().setSpeed({ fastStart: snap.fastStart }); }
      setSnapshot(snap);
    }).then((f) => (off1 = f));
    reattach();
    onLlamaLog((line) => {
      useCluster.getState().pushEngineLog(line);
      const run = useCluster.getState().run;
      if (run.status === "loading") setRun({ ...run, log: line });
    }).then((f) => (off2 = f));
    let off3: (() => void) | undefined;
    onDownload((b) => useCluster.getState().setDl(b)).then((f) => (off3 = f));
    return () => { off1?.(); off2?.(); off3?.(); };
  }, [setSnapshot, setRun]);

  return (
    <div className="flex h-full flex-col text-pl-text">
      <TopBar />
      <div className="flex min-h-0 flex-1">
        <NavRail />
        <main className="min-w-0 flex-1">
          {view === "chat" && <ChatView />}
          {view === "devices" && <DevicePanel />}
          {view === "models" && <ModelLibrary />}
          {view === "api" && <ApiPanel />}
        </main>
      </div>
      <StatusBar />
      {wizardOpen && <UsbWizard />}
      {lanDialog && <LanDialog />}
      {tourOpen && <Tour />}
    </div>
  );
}
