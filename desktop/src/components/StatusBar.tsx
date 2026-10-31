import { useCluster } from "../store/cluster";

export default function StatusBar() {
  const { snapshot, run } = useCluster();
  const rpc = snapshot?.rpcServers ?? [];
  const log = run.status === "loading" ? run.log : undefined;
  return (
    <footer className="flex h-6 items-center gap-4 border-t border-pl-line bg-pl-bg2 px-3 font-mono text-2xs text-pl-dim">
      <span><span className="text-pl-green">●</span> adb bridge</span>
      {(snapshot?.nodes.some((n) => n.transport === "lan")) && <span><span className="text-pl-cyan">●</span> lan</span>}
      <span>rpc: {rpc.length ? rpc.join(", ") : "none"}</span>
      <span>engine: llama.cpp · 127.0.0.1:8080</span>
      {log && <span className="truncate text-pl-mute">› {log}</span>}
      <span className="ml-auto">v0.2.0</span>
    </footer>
  );
}
