import { useEffect, useState } from "react";
import { Check, Copy } from "lucide-react";
import clsx from "clsx";
import { useCluster } from "../store/cluster";
import { LLAMA_URL } from "../lib/api";

/** Copy-ready settings for any OpenAI-compatible client: Hermes Agent, Open WebUI, Jan, Continue, AnythingLLM… */
export default function ApiPanel() {
  const { run, lan, setLan, ctx } = useCluster();
  const live = run.status === "ready";
  const model = live || run.status === "loading" ? run.model.id : "<load a model first>";
  const base = `${lan ? "http://<this-PC-LAN-IP>:8080" : LLAMA_URL}/v1`;
  const [tab, setTab] = useState(0);

  const clients: { name: string; lang: string; body: string }[] = [
    { name: "Hermes Agent", lang: "~/.hermes/config.yaml", body:
`model:
  default: ${model}
  provider: custom
  base_url: ${base}
  api_key: argus          # any string, the server has no auth
# Hermes needs >= 64K context: pick 64K in Models before deploying.
# Or interactively: hermes model -> Custom endpoint -> paste the URL above.` },
    { name: "Open WebUI", lang: "Admin → Settings → Connections → OpenAI API", body:
`URL:     ${base.replace("127.0.0.1", "host.docker.internal")}   (from Docker)
         ${base}   (native install)
API Key: argus
Model:   auto-detected from /v1/models (${model})` },
    { name: "Jan / AnythingLLM / Msty", lang: "Remote provider → OpenAI-compatible", body:
`Base URL: ${base}
API key:  argus
Model:    ${model}` },
    { name: "Continue (VS Code)", lang: "~/.continue/config.yaml", body:
`models:
  - name: Argus ${model}
    provider: openai
    model: ${model}
    apiBase: ${base}
    apiKey: argus
    roles: [chat, edit, apply]` },
    { name: "Python / curl", lang: "openai SDK", body:
`from openai import OpenAI
client = OpenAI(base_url="${base}", api_key="argus")
r = client.chat.completions.create(model="${model}", messages=[{"role":"user","content":"hi"}])
print(r.choices[0].message.content)

# curl ${base}/models` },
  ];

  return (
    <div className="grid h-full grid-cols-[1fr_320px] gap-px bg-pl-line">
      <div className="flex min-w-0 flex-col gap-3 overflow-y-auto bg-pl-bg0 p-3">
        <div className="panel" data-tour="api">
          <div className="panel-head"><span>OpenAI-compatible endpoint</span><Health live={live} /></div>
          <div className="grid grid-cols-3 divide-x divide-pl-line">
            <Field k="Base URL" v={base} />
            <Field k="Model ID" v={model} />
            <Field k="API key" v="argus (any value)" />
          </div>
          <div className="border-t border-pl-line px-4 py-2 font-mono text-2xs text-pl-dim">
            /v1/chat/completions · /v1/completions · /v1/embeddings · /v1/models · streaming · tool calling (--jinja) · ctx {ctx / 1024}K
          </div>
        </div>

        <div className="panel min-h-0">
          <div className="flex border-b border-pl-line bg-pl-bg2">
            {clients.map((c, i) => (
              <button key={c.name} onClick={() => setTab(i)}
                className={clsx("relative h-8 px-3 text-xs", tab === i ? "text-pl-text" : "text-pl-dim hover:text-pl-mute")}>
                {c.name}{tab === i && <span className="absolute inset-x-0 bottom-0 h-0.5 bg-pl-blue" />}
              </button>
            ))}
          </div>
          <Snippet title={clients[tab].lang} body={clients[tab].body} />
        </div>
      </div>

      <aside className="flex flex-col bg-pl-bg1">
        <div className="panel-head"><span>Access policy</span></div>
        <label className="flex cursor-pointer items-center justify-between border-b border-pl-line px-4 py-3 text-xs">
          <span>
            <span className="text-pl-text">Expose on local network</span>
            <span className="mt-0.5 block text-2xs text-pl-dim">binds 0.0.0.0:8080 · for WSL, Docker, other PCs</span>
          </span>
          <button onClick={() => setLan(!lan)} className={clsx("relative h-4 w-8 rounded-full transition", lan ? "bg-pl-blue2" : "bg-pl-bg4")}>
            <span className={clsx("absolute top-0.5 h-3 w-3 rounded-full bg-white transition-all", lan ? "left-[18px]" : "left-0.5")} />
          </button>
        </label>
        <div className="space-y-3 p-4 text-xs leading-5 text-pl-mute">
          <p>Applies on next Deploy. Off by default: only apps on this PC can reach the model.</p>
          <p>Agents (Hermes, OpenHands, Cline) need tool calling and a long context. Use a model tagged <span className="tag bg-pl-bg3 text-pl-mute">agent</span> with the 64K preset.</p>
          <p className="text-pl-dim">No auth on the server: keep LAN mode off on public Wi‑Fi.</p>
        </div>
      </aside>
    </div>
  );
}

function Health({ live }: { live: boolean }) {
  const [ok, setOk] = useState<boolean | null>(null);
  useEffect(() => {
    if (!live) { setOk(null); return; }
    let stop = false;
    const tick = () => fetch(`${LLAMA_URL}/v1/models`).then((r) => !stop && setOk(r.ok)).catch(() => !stop && setOk(false));
    tick(); const t = setInterval(tick, 5000);
    return () => { stop = true; clearInterval(t); };
  }, [live]);
  const [t, c] = ok ? ["SERVING", "border-pl-green/50 text-pl-green"] : live ? ["CHECKING", "border-pl-orange/50 text-pl-orange"] : ["OFFLINE", "border-pl-line2 text-pl-dim"];
  return <span className={clsx("tag border normal-case tracking-normal", c)}>● {t}</span>;
}

function Field({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex items-center justify-between gap-2 px-4 py-3">
      <div className="min-w-0"><div className="label">{k}</div><div className="mt-1 truncate font-mono text-sm text-pl-text">{v}</div></div>
      <CopyBtn text={v} />
    </div>
  );
}

function Snippet({ title, body }: { title: string; body: string }) {
  return (
    <div>
      <div className="flex items-center justify-between border-b border-pl-line px-4 py-1.5">
        <span className="font-mono text-2xs text-pl-dim">{title}</span><CopyBtn text={body} />
      </div>
      <pre className="overflow-x-auto bg-pl-bg0 p-4 font-mono text-xs leading-5 text-pl-text">{body}</pre>
    </div>
  );
}

function CopyBtn({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  return (
    <button className="btn shrink-0" onClick={() => { navigator.clipboard?.writeText(text); setDone(true); setTimeout(() => setDone(false), 1200); }}>
      {done ? <Check size={12} className="text-pl-green" /> : <Copy size={12} />}
    </button>
  );
}
