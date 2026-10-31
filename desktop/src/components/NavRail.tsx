import { Boxes, MessageSquareText, Network, Plug, Usb } from "lucide-react";
import clsx from "clsx";
import { useCluster, type View } from "../store/cluster";

const items: { id: View; label: string; icon: typeof Boxes }[] = [
  { id: "chat", label: "Chat", icon: MessageSquareText },
  { id: "devices", label: "Cluster", icon: Network },
  { id: "models", label: "Models", icon: Boxes },
  { id: "api", label: "API", icon: Plug },
];

export default function NavRail() {
  const { view, setView, setWizard } = useCluster();
  return (
    <aside className="flex w-14 flex-col items-center border-r border-pl-line bg-pl-bg1 py-2">
      {items.map(({ id, label, icon: Icon }) => (
        <button key={id} onClick={() => setView(id)} title={label}
          className={clsx("group relative flex h-12 w-full flex-col items-center justify-center gap-0.5 text-2xs",
            view === id ? "text-pl-text" : "text-pl-dim hover:text-pl-mute")}>
          {view === id && <span className="absolute left-0 top-2 h-8 w-0.5 bg-pl-blue" />}
          <Icon size={18} strokeWidth={1.6} />
          <span className="tracking-wide">{label}</span>
        </button>
      ))}
      <div className="mt-auto">
        <button onClick={() => setWizard(true)} title="Connect device"
          className="flex h-10 w-10 items-center justify-center rounded border border-pl-line2 text-pl-blue hover:bg-pl-bg3">
          <Usb size={17} />
        </button>
      </div>
    </aside>
  );
}
