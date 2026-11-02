import { useMemo, useState } from "react";
import { Check, Copy, RefreshCw, Smartphone } from "lucide-react";
import clsx from "clsx";
import qrcode from "qrcode-generator";
import type { FeaturesView } from "../lib/types";
import { useT } from "../lib/i18n";

export function Toggle({ on, onChange, title }: { on: boolean; onChange: (v: boolean) => void; title?: string }) {
  return (
    <button type="button" title={title} onClick={() => onChange(!on)} aria-pressed={on}
      className={clsx("relative h-4 w-8 shrink-0 rounded-full transition", on ? "bg-pl-blue2" : "bg-pl-bg4")}>
      <span className={clsx("absolute top-0.5 h-3 w-3 rounded-full bg-white transition-all", on ? "left-[18px]" : "left-0.5")} />
    </button>
  );
}

function newPin() {
  const a = new Uint32Array(1);
  crypto.getRandomValues(a);
  return String(a[0] % 1_000_000).padStart(6, "0");
}

/** Web chat for phones: toggle, PIN, URL + QR (http://<lan ip>:50080/#pin=…) */
export default function WebChatCard({ view, onChange }: { view: FeaturesView; onChange: (patch: Partial<FeaturesView["webchat"]>) => void }) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const wc = view.webchat;
  const url = view.lanIp ? `http://${view.lanIp}:${view.webchatPort}/#pin=${encodeURIComponent(wc.pin)}` : null;
  const qrSvg = useMemo(() => {
    if (!url) return "";
    const q = qrcode(0, "M");
    q.addData(url);
    q.make();
    return q.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
  }, [url]);

  return (
    <div className="panel">
      <div className="panel-head gap-2">
        <span className="flex items-center gap-2"><Smartphone size={14} /> {t("f.web.title")}</span>
        <span className="ml-auto flex items-center gap-2 text-2xs normal-case tracking-normal">
          {wc.enabled && <span className={view.webchatRunning ? "text-pl-green" : "text-pl-dim"}>● {view.webchatRunning ? t("f.web.running") : t("f.web.stopped")}</span>}
          <Toggle on={wc.enabled} onChange={(enabled) => onChange({ enabled })} />
        </span>
      </div>
      <div className="space-y-3 p-4 text-xs text-pl-mute">
        <p className="leading-5">{t("f.web.desc")}</p>
        <div className="flex flex-wrap items-center gap-2">
          <span className="w-16 text-pl-dim">{t("f.web.pin")}</span>
          <input value={wc.pin} onChange={(e) => onChange({ pin: e.target.value.replace(/\s/g, "").slice(0, 32) })}
            className="w-32 rounded border border-pl-line2 bg-pl-bg2 px-2 py-1 font-mono text-pl-text outline-none focus:border-pl-blue" />
          <button type="button" onClick={() => onChange({ pin: newPin() })}
            className="flex items-center gap-1 rounded border border-pl-line2 px-2 py-1 text-pl-mute hover:bg-pl-bg3">
            <RefreshCw size={12} /> {t("f.web.newPin")}
          </button>
        </div>
        {view.webchatError && <p className="text-pl-red">{view.webchatError}</p>}
        {!url && <p className="text-pl-orange">{t("f.web.noLan")}</p>}
        {url && wc.enabled && (
          <div className="flex flex-wrap items-start gap-4">
            <div className="h-40 w-40 shrink-0 rounded bg-white p-1" dangerouslySetInnerHTML={{ __html: qrSvg }} />
            <div className="min-w-0 flex-1 space-y-2">
              <div className="text-pl-dim">{t("f.web.url")}</div>
              <div className="flex items-center gap-2">
                <code className="min-w-0 truncate rounded bg-pl-bg2 px-2 py-1 font-mono text-pl-text">{url}</code>
                <button type="button" title={t("f.web.copy")} onClick={async () => { await navigator.clipboard.writeText(url); setCopied(true); setTimeout(() => setCopied(false), 1500); }}
                  className="rounded border border-pl-line2 p-1 hover:bg-pl-bg3">{copied ? <Check size={12} /> : <Copy size={12} />}</button>
              </div>
              <p className="text-2xs leading-4 text-pl-dim">{t("f.web.firewall")}</p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
