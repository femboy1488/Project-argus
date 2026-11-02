import { useEffect } from "react";
import { BookOpen, Boxes, ListTodo, Columns3, MessageSquareText, Network, Plug, Sparkles, Usb } from "lucide-react";
import clsx from "clsx";
import { useCluster, type View } from "../store/cluster";
import { featuresGet } from "../lib/api";
import { useLang, useT, type I18nKey } from "../lib/i18n";

const items: { id: View; label: I18nKey; icon: typeof Boxes }[] = [
  { id: "chat", label: "nav.chat", icon: MessageSquareText },
  { id: "devices", label: "nav.devices", icon: Network },
  { id: "models", label: "nav.models", icon: Boxes },
  { id: "api", label: "nav.api", icon: Plug },
  { id: "parallel", label: "nav.parallel", icon: Columns3 },
  { id: "features", label: "nav.features", icon: Sparkles },
  { id: "queue", label: "nav.queue", icon: ListTodo },
  { id: "knowledge", label: "nav.knowledge", icon: BookOpen },
];

export default function NavRail() {
  const { view, setView, setWizard } = useCluster();
  const t = useT();
  // UI language lives in argus-features.json
  useEffect(() => { featuresGet().then((f) => { if (f && (f.lang === "ru" || f.lang === "en")) useLang.getState().setLang(f.lang); }).catch(() => {}); }, []);
  return (
    <aside className="flex w-12 shrink-0 flex-col items-center border-r border-pl-line bg-pl-bg1 py-2 lg:w-14">
      {items.map(({ id, label: key, icon: Icon }) => { const label = t(key); return (
        <button key={id} data-nav={id} aria-label={label} onClick={() => setView(id)} title={id === "parallel" ? t("nav.parallelHint") : label}
          className={clsx("group relative flex h-11 w-full lg:h-12 flex-col items-center justify-center gap-0.5 text-2xs",
            view === id ? "text-pl-text" : "text-pl-dim hover:text-pl-mute")}>
          {view === id && <span className="absolute left-0 top-2 h-8 w-0.5 bg-pl-blue" />}
          <Icon size={18} strokeWidth={1.6} />
          <span className="hidden tracking-wide lg:block">{label}</span>
        </button>
      ); })}
      <div className="mt-auto">
        <button onClick={() => setWizard(true)} title={t("nav.connect")}
          className="flex h-9 w-9 items-center lg:h-10 lg:w-10 justify-center rounded border border-pl-line2 text-pl-blue hover:bg-pl-bg3">
          <Usb size={17} />
        </button>
      </div>
    </aside>
  );
}
