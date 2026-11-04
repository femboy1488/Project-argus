import { useEffect, useRef, useState } from "react";
import { AlertTriangle, Bot, GitBranch, Languages } from "lucide-react";
import clsx from "clsx";
import { featuresGet, featuresSet, isTauri } from "../lib/api";
import type { FeaturesConfig, FeaturesView as FView } from "../lib/types";
import { useLang, useT, type Lang } from "../lib/i18n";
import { useCluster } from "../store/cluster";
import WebChatCard, { Toggle } from "./WebChatCard";

const MOCK: FView = {
  webchat: { enabled: false, pin: "123456" }, telegram: { enabled: false, token: "", allowedIds: [] },
  routing: { simpleToHelper: false }, rag: { enabled: false, folders: [] }, library: { enabled: false, url: "" },
  voice: { enabled: false, url: "" }, lang: "ru", lanIp: "192.168.1.10", webchatPort: 50080, webchatUrl: null,
  webchatRunning: false, webchatError: "", telegramStatus: "",
};

const READONLY = ["lanIp", "webchatPort", "webchatUrl", "webchatRunning", "webchatError", "telegramStatus"] as const;

/** «Возможности»: web chat for phones, Telegram bot, simple-prompt routing, UI language */
export default function FeaturesView() {
  const t = useT();
  const setLang = useLang((s) => s.setLang);
  const pushToast = useCluster((s) => s.pushToast);
  const [view, setView] = useState<FView | null>(isTauri ? null : MOCK);
  const [idsText, setIdsText] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    let stop = false;
    const load = () => featuresGet().then((v) => {
      if (stop || !v) return;
      // keep local edits, refresh only the live status fields
      setView((cur) => cur ? { ...cur, ...Object.fromEntries(READONLY.map((k) => [k, v[k]])) } as FView : v);
      setIdsText((cur) => cur || v.telegram.allowedIds.join(", "));
    }).catch((e) => pushToast("error", String(e)));
    load();
    const iv = setInterval(load, 3000);
    return () => { stop = true; clearInterval(iv); };
  }, [pushToast]);

  // debounce-save only the sections this screen owns; features_set merges them into the file on the PC side
  // (other screens and the backend edit other fields, so nothing else is sent back)
  const save = (next: FView) => {
    setView(next);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(async () => {
      try {
        const patch = { webchat: next.webchat, telegram: next.telegram, routing: next.routing, lang: next.lang };
        const v = await featuresSet(patch as unknown as FeaturesConfig);
        if (v) setView((cur) => cur ? { ...cur, ...Object.fromEntries(READONLY.map((k) => [k, v[k]])), webchat: { ...cur.webchat, pin: v.webchat.pin } } as FView : v);
      } catch (e) { pushToast("error", String(e)); }
    }, 400);
  };

  if (!view) return <div className="p-6 text-xs text-pl-dim">…</div>;

  const setIds = (s: string) => {
    setIdsText(s);
    const ids = s.split(/[\s,;]+/).map((x) => x.trim()).filter((x) => /^-?\d+$/.test(x)).map(Number);
    save({ ...view, telegram: { ...view.telegram, allowedIds: ids } });
  };
  const tgErr = view.telegramStatus.startsWith("ошибка") || view.telegramStatus === "нет токена";

  return (
    <div className="h-full overflow-y-auto bg-pl-bg0">
      <div className="mx-auto flex max-w-3xl flex-col gap-3 p-3">
        <h1 className="px-1 pt-1 text-sm text-pl-text">{t("f.title")}</h1>

        <WebChatCard view={view} onChange={(p) => save({ ...view, webchat: { ...view.webchat, ...p } })} />

        <div className="panel">
          <div className="panel-head gap-2">
            <span className="flex items-center gap-2"><Bot size={14} /> {t("f.tg.title")}</span>
            <span className="ml-auto flex items-center gap-2 text-2xs normal-case tracking-normal">
              {view.telegram.enabled && view.telegramStatus && <span className={tgErr ? "text-pl-red" : "text-pl-green"}>● {view.telegramStatus}</span>}
              <Toggle on={view.telegram.enabled} onChange={(enabled) => save({ ...view, telegram: { ...view.telegram, enabled } })} />
            </span>
          </div>
          <div className="space-y-3 p-4 text-xs text-pl-mute">
            <p className="leading-5">{t("f.tg.desc")}</p>
            <label className="block space-y-1">
              <span className="text-pl-dim">{t("f.tg.token")}</span>
              <input type="password" autoComplete="off" value={view.telegram.token} placeholder="123456:ABC-DEF…"
                onChange={(e) => save({ ...view, telegram: { ...view.telegram, token: e.target.value.trim() } })}
                className="w-full rounded border border-pl-line2 bg-pl-bg2 px-2 py-1 font-mono text-pl-text outline-none focus:border-pl-blue" />
            </label>
            <label className="block space-y-1">
              <span className="text-pl-dim">{t("f.tg.ids")}</span>
              <input value={idsText} placeholder="123456789, 987654321" onChange={(e) => setIds(e.target.value)}
                className="w-full rounded border border-pl-line2 bg-pl-bg2 px-2 py-1 font-mono text-pl-text outline-none focus:border-pl-blue" />
              <span className="block text-2xs text-pl-dim">{t("f.tg.idsHint")}</span>
            </label>
            <p className="flex items-start gap-2 rounded border border-pl-orange/40 bg-pl-orange/10 px-2 py-1.5 text-2xs leading-4 text-pl-orange">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" /> {t("f.tg.warn")}
            </p>
          </div>
        </div>

        <div className="panel">
          <div className="panel-head gap-2">
            <span className="flex items-center gap-2"><GitBranch size={14} /> {t("f.route.title")}</span>
            <span className="ml-auto"><Toggle on={view.routing.simpleToHelper} onChange={(simpleToHelper) => save({ ...view, routing: { simpleToHelper } })} /></span>
          </div>
          <p className="p-4 text-xs leading-5 text-pl-mute">{t("f.route.desc")}</p>
        </div>

        <div className="panel">
          <div className="panel-head gap-2"><span className="flex items-center gap-2"><Languages size={14} /> {t("f.lang.title")}</span></div>
          <div className="flex items-center gap-2 p-4 text-xs">
            {(["ru", "en"] as Lang[]).map((l) => (
              <button key={l} type="button" onClick={() => { setLang(l); save({ ...view, lang: l }); }}
                className={clsx("rounded border px-3 py-1", view.lang === l ? "border-pl-blue text-pl-text" : "border-pl-line2 text-pl-dim hover:bg-pl-bg3")}>
                {l === "ru" ? "Русский" : "English"}
              </button>
            ))}
            <span className="ml-2 text-2xs text-pl-dim">{t("f.lang.desc")}</span>
          </div>
        </div>
      </div>
    </div>
  );
}
