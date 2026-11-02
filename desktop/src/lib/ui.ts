// Small UI helpers for the responsive layout and steady progress readouts.
import { useEffect, useRef, useState } from "react";
import type { HelperView } from "./types";

/** Width at which side panels (inspector, session properties, plan) stop being columns and become drawers. */
export const WIDE_PX = 1280;

/** `true` while the window is at least `px` wide; follows resizes. */
export function useWide(px = WIDE_PX): boolean {
  const q = `(min-width: ${px}px)`;
  const [ok, setOk] = useState(() => typeof window === "undefined" || window.matchMedia(q).matches);
  useEffect(() => {
    const m = window.matchMedia(q);
    const on = () => setOk(m.matches);
    on();
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, [q]);
  return ok;
}

/** 0–100, never NaN/Infinity. */
export const clampPct = (v: number | null | undefined) => (v == null || !Number.isFinite(v) ? 0 : Math.max(0, Math.min(100, v)));

/**
 * Hold a number steady until it moves by more than `rel` (relative) — free RAM on phones wobbles every poll,
 * which used to flip «лучший выбор» / FITS↔TIGHT and make the pool-usage bars twitch.
 */
export function useSteady(v: number, rel = 0.02, abs = 96): number {
  const ref = useRef(v);
  if (Math.abs(v - ref.current) > Math.max(abs, Math.abs(ref.current) * rel)) ref.current = v;
  return ref.current;
}

// Helper download progress: the backend may report the same phase with pct=null for a tick (dumpsys / beacon gap),
// or a slightly lower number from a re-sent status. Keep the best value seen for (node, model, phase).
const helperBest = new Map<string, number>();
export function helperPct(h: Pick<HelperView, "id" | "model" | "state" | "pct" | "ready">): number | null {
  if (h.ready) return null;
  const key = `${h.id}|${h.model}|${h.state}`;
  const prev = helperBest.get(key);
  if (h.pct == null || !Number.isFinite(h.pct)) return prev ?? null;
  const v = Math.max(prev ?? 0, clampPct(h.pct));
  helperBest.set(key, v);
  if (helperBest.size > 64) helperBest.delete(helperBest.keys().next().value as string);
  return v;
}
/** Phases that move bytes: their progress bar stays mounted (indeterminate when pct is unknown). */
export const isDownloadPhase = (state: string) => state === "pc-download" || state === "node-download" || state === "downloading";
