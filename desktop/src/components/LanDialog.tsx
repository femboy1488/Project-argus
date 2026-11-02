import { useState } from "react";
import { Loader2, Wifi, X } from "lucide-react";
import { useCluster } from "../store/cluster";
import { addLanNode, testLanNode } from "../lib/api";
import { Copy } from "lucide-react";

const CMDS: { os: string; cmd: string }[] = [
  { os: "Linux / Pi (авто, с маяком)", cmd: "bash scripts/argus-node-linux.sh" },
  { os: "Linux / Pi (вручную)", cmd: "./ggml-rpc-server -H 0.0.0.0 -p 50052 -c" },
  { os: "Windows (ноутбук/ПК)", cmd: "ggml-rpc-server.exe -H 0.0.0.0 -p 50052 -c" },
];

/** "Добавить ноду по IP": host:port of a machine running llama.cpp rpc-server (b12008). */
export default function LanDialog() {
  const setOpen = useCluster((s) => s.setLanDialog);
  const [addr, setAddr] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState<"test" | "add" | null>(null);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);

  const normalized = addr.trim() && !/:\d+$/.test(addr.trim()) && !addr.includes("]") ? `${addr.trim()}:50052` : addr.trim();
  async function test() {
    setBusy("test"); setMsg(null);
    try { setMsg({ ok: true, text: await testLanNode(normalized) }); } catch (e) { setMsg({ ok: false, text: String(e) }); }
    setBusy(null);
  }
  async function add() {
    setBusy("add"); setMsg(null);
    try { await addLanNode(normalized, name.trim() || undefined); setOpen(false); } catch (e) { setMsg({ ok: false, text: String(e) }); }
    setBusy(null);
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="max-h-[92vh] w-[620px] max-w-[calc(100vw-2rem)] overflow-y-auto border border-pl-line2 bg-pl-bg1 shadow-2xl">
        <div className="flex h-9 items-center justify-between border-b border-pl-line bg-pl-bg2 px-3">
          <span className="flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.12em] text-pl-mute"><Wifi size={13} /> Добавить ноду по IP · LAN</span>
          <button onClick={() => setOpen(false)} className="text-pl-dim hover:text-pl-text"><X size={15} /></button>
        </div>
        <div className="space-y-3 p-4">
          <div className="grid grid-cols-[1fr_180px] gap-2">
            <label className="text-xs"><span className="label">Адрес (host:port)</span>
              <input autoFocus value={addr} onChange={(e) => setAddr(e.target.value)} placeholder="192.168.1.50:50052"
                onKeyDown={(e) => { if (e.key === "Enter" && addr.trim()) add(); }}
                className="mt-1 h-8 w-full border border-pl-line2 bg-pl-bg0 px-2 font-mono text-sm text-pl-text outline-none focus:border-pl-blue" />
            </label>
            <label className="text-xs"><span className="label">Имя (необязательно)</span>
              <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Raspberry Pi 5"
                className="mt-1 h-8 w-full border border-pl-line2 bg-pl-bg0 px-2 text-sm text-pl-text outline-none focus:border-pl-blue" />
            </label>
          </div>
          <div className="font-mono text-2xs text-pl-dim">порт по умолчанию 50052 · будет: {normalized || "—"}</div>
          {msg && <div className={`border-l-2 p-2 font-mono text-2xs ${msg.ok ? "border-pl-green bg-pl-green/10 text-pl-green" : "border-pl-red bg-pl-red/10 text-pl-red"}`}>{msg.text}</div>}

          <div className="border border-pl-line bg-pl-bg0 p-3 text-xs leading-5 text-pl-mute">
            <div className="label mb-1">Как запустить ноду на Pi / ноутбуке</div>
            <ol className="list-decimal space-y-1 pl-4">
              <li>Проще всего: Android — «Wi-Fi режим» в Argus Node; Linux-ПК / Raspberry Pi — скрипт <span className="font-mono text-pl-text">scripts/argus-node-linux.sh</span> (скачает rpc-server b12008 и включит маяк). Такие ноды сами появятся в Cluster → «Найдено в сети».</li>
              <li>Вручную: <span className="text-pl-text">ggml-rpc-server</span> из релиза llama.cpp <span className="font-mono text-pl-gold">b12008</span> (та же версия, что у Argus — протокол RPC v8).</li>
              <li>Запустите на той машине:</li>
            </ol>
            {CMDS.map((c) => <Cmd key={c.os} os={c.os} cmd={c.cmd} />)}
            <div className="mt-2 border-l-2 border-pl-orange bg-pl-orange/10 p-2 text-2xs text-pl-orange">
              Только закрытая домашняя сеть! У rpc-server нет пароля и шифрования: любой в этой сети может подключиться и выполнять вычисления. Не открывайте порт 50052 в интернет и не запускайте в публичном Wi‑Fi.
            </div>
            <div className="mt-2 text-2xs text-pl-dim">Ключ <span className="font-mono">-c</span> кэширует веса на диске ноды — повторная загрузка модели быстрее. Сеть: Ethernet ≫ Wi‑Fi.</div>
          </div>
        </div>
        <div className="flex justify-end gap-2 border-t border-pl-line bg-pl-bg2 px-3 py-2">
          <button className="btn" onClick={() => setOpen(false)}>Отмена</button>
          <button className="btn" disabled={!addr.trim() || !!busy} onClick={test}>{busy === "test" && <Loader2 size={12} className="animate-spin" />} Проверить</button>
          <button className="btn-primary" disabled={!addr.trim() || !!busy} onClick={add}>{busy === "add" && <Loader2 size={12} className="animate-spin" />} Добавить</button>
        </div>
      </div>
    </div>
  );
}

function Cmd({ os, cmd }: { os: string; cmd: string }) {
  const [ok, setOk] = useState(false);
  return (
    <div className="mt-1.5 flex items-center gap-2">
      <span className="w-40 shrink-0 text-2xs text-pl-dim">{os}</span>
      <code className="flex-1 border border-pl-line2 bg-pl-bg1 px-2 py-1 font-mono text-2xs text-pl-text">{cmd}</code>
      <button className="btn h-6 px-1.5" onClick={() => { navigator.clipboard?.writeText(cmd); setOk(true); setTimeout(() => setOk(false), 1000); }}><Copy size={11} className={ok ? "text-pl-green" : ""} /></button>
    </div>
  );
}
