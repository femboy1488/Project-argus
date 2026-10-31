import clsx from "clsx";

const colors = { blue: "bg-pl-blue", green: "bg-pl-green", orange: "bg-pl-orange", red: "bg-pl-red", violet: "bg-pl-violet", cyan: "bg-pl-cyan" };
export type Tone = keyof typeof colors;
export const loadTone = (p: number): Tone => (p > 85 ? "red" : p > 60 ? "orange" : "green");

/** Flat Blueprint-style meter with 10% tick marks. */
export default function Meter({ value, tone = "blue", className }: { value: number; tone?: Tone; className?: string }) {
  const v = Math.max(0, Math.min(100, value));
  return (
    <div className={clsx("relative h-1.5 w-full bg-pl-bg0", className)}>
      <div className={clsx("h-full transition-[width] duration-700", colors[tone])} style={{ width: `${v}%` }} />
      <div className="pointer-events-none absolute inset-0 flex justify-between">
        {Array.from({ length: 11 }).map((_, i) => <span key={i} className="h-full w-px bg-pl-bg1/80" />)}
      </div>
    </div>
  );
}
