import { useCluster } from "../store/cluster";

export default function StatusBar() {
  const { snapshot, run } = useCluster();
  const rpc = snapshot?.rpcServers ?? [];
  const log = run.status === "loading" ? run.log : undefined;
  return (
    <footer className="flex h-6 items-center gap-4 border-t border-pl-line bg-pl-bg2 px-3 font-mono text-2xs text-pl-dim">
      <span title={snapshot && !snapshot.adbOk ? "adb не отвечает — будет перезапущен автоматически" : "adb работает"}>
        <span className={snapshot && !snapshot.adbOk ? "text-pl-red" : "text-pl-green"}>●</span> adb bridge</span>
      <span title={snapshot?.beaconError || "слушаем Wi-Fi маяки нод на UDP 50053"}>
        <span className={snapshot?.beaconError ? "text-pl-red" : "text-pl-cyan"}>●</span> wi-fi discovery{snapshot?.discovered.length ? ` (${snapshot.discovered.length})` : ""}</span>
      <span>rpc: {rpc.length ? rpc.join(", ") : "none"}</span>
      <span>engine: llama.cpp · 127.0.0.1:8080</span>
      {log && <span className="truncate text-pl-mute">› {log}</span>}
      <button className="ml-auto hover:text-pl-text" title="О программе · проверить обновления" onClick={() => useCluster.getState().setAbout(true)}>
        {snapshot?.platform ?? ""} · v{snapshot?.appVersion ?? "0.3.1"}
      </button>
    </footer>
  );
}
