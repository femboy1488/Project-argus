import { useEffect, type ReactNode } from "react";
import { X } from "lucide-react";
import clsx from "clsx";

/**
 * Side panel that is a grid column on wide windows and a right-hand overlay drawer on narrow ones.
 * The parent must be `relative`. Children stay mounted while closed only in wide mode.
 */
export default function Drawer({ wide, open, onClose, title, width = 300, children }: {
  wide: boolean; open: boolean; onClose: () => void; title: string; width?: number; children: ReactNode;
}) {
  useEffect(() => {
    if (wide || !open) return;
    const k = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [wide, open, onClose]);
  if (wide) return <>{children}</>;
  if (!open) return null;
  return (
    <>
      <div className="absolute inset-0 z-30 bg-black/40" onClick={onClose} />
      <div className={clsx("absolute inset-y-0 right-0 z-40 flex min-h-0 max-w-[calc(100%-2rem)] flex-col border-l border-pl-line2 bg-pl-bg1 shadow-2xl")} style={{ width }}>
        <button className="absolute right-2 top-1.5 z-10 flex h-5 w-5 items-center justify-center text-pl-dim hover:text-pl-text" title={`Закрыть «${title}»`} onClick={onClose}>
          <X size={13} />
        </button>
        <div className="flex min-h-0 flex-1 flex-col [&>aside]:min-h-0 [&>aside]:flex-1 [&>aside>.panel-head:first-child]:pr-9">{children}</div>
      </div>
    </>
  );
}
