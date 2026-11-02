import clsx from "clsx";
import { clampPct } from "../lib/ui";

const colors = { blue: "bg-pl-blue", green: "bg-pl-green", orange: "bg-pl-orange", red: "bg-pl-red", violet: "bg-pl-violet", cyan: "bg-pl-cyan" };
export type Tone = keyof typeof colors;
export const loadTone = (p: number): Tone => (p > 85 ? "red" : p > 60 ? "orange" : "green");

/** Flat Blueprint-style meter with 10% tick marks. */
export default function Meter({ value, tone = "blue", className }: { value: number; tone?: Tone; className?: string }) {
  const v = clampPct(value);
  return (
    <div className={clsx("relative h-1.5 w-full min-w-0 overflow-hidden bg-pl-bg0", className)}>
      <div className={clsx("h-full transition-[width] duration-700 ease-out", colors[tone])} style={{ width: `${v}%` }} />
      <div className="pointer-events-none absolute inset-0 flex justify-between">
        {Array.from({ length: 11 }).map((_, i) => <span key={i} className="h-full w-px bg-pl-bg1/80" />)}
      </div>
    </div>
  );
}

/**
 * Progress bar: `value` 0–100, or `null` = unknown total → indeterminate sweep (never a fake 0% or a bar that jumps).
 * Only `width` animates; the element is never re-keyed by polls.
 */
export function Progress({ value, tone = "blue", className, linear }: { value: number | null; tone?: Tone; className?: string; linear?: boolean }) {
  return (
    <div className={clsx("relative h-1.5 w-full min-w-0 overflow-hidden bg-pl-bg2", className)}
      role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={value == null ? undefined : Math.round(clampPct(value))}>
      {value == null
        ? <div className={clsx("absolute inset-y-0 left-0 w-1/3 animate-sweep opacity-80", colors[tone])} />
        : <div className={clsx("h-full transition-[width] duration-700", linear ? "ease-linear" : "ease-out", colors[tone])} style={{ width: `${clampPct(value)}%` }} />}
    </div>
  );
}
