// One place that turns "deploy this model" into start_model args (speed toggles, split hint, draft).
import { useCluster } from "../store/cluster";
import { startModel, waitHealthy, probeRunningEngine, isTauri, abortGenerations } from "./api";
import { draftFor, requiredMb, MODELS } from "./models";
import type { EngineView, ModelEntry } from "./types";

const LS_LAST = "argus.lastModel";

let deploying = false;

export async function deploy(m: ModelEntry, opts: { goChat?: boolean } = {}) {
  const st = useCluster.getState();
  const { ctx, speed, lan } = st;
  deploying = true;
  try {
    st.clearEngineLog();
    st.setRun({ status: "loading", model: m });
    const draftHf = speed.speculative ? draftFor(m, speed.draft) : undefined;
    const endpoint = await startModel(m, {
      ctx, lan, needMb: requiredMb(m, ctx),
      draftHf, draftMax: speed.draftMax,
      cacheReuse: speed.promptCache ? 256 : undefined,
      keepLoaded: speed.keepLoaded,
    });
    await waitHealthy();
    useCluster.getState().setRun({ status: "ready", model: m, endpoint });
    localStorage.setItem(LS_LAST, JSON.stringify(m));
    if (opts.goChat !== false) useCluster.getState().setView("chat");
  } catch (e) {
    useCluster.getState().setRun({ status: "error", message: String(e) });
  } finally {
    deploying = false;
  }
}

function modelForAlias(alias: string | null): ModelEntry | undefined {
  if (!alias) return undefined;
  let m: ModelEntry | undefined;
  try { m = JSON.parse(localStorage.getItem(LS_LAST) ?? "null") ?? undefined; } catch { /* */ }
  if (m && m.id === alias) return m;
  return MODELS.find((x) => x.id === alias)
    ?? { id: alias, name: alias, family: "?", params: "?", quant: "?", sizeGb: 0, hfRepo: "", kvMbPer1k: 0, tags: [] };
}

let lastEngine: EngineView["status"] | null = null;

/** Keeps the UI run state in sync with the Rust engine supervisor (events + every snapshot). */
export function applyEngine(e: EngineView, fromEvent = false) {
  const st = useCluster.getState();
  const run = st.run;
  if (fromEvent && e.toast) st.pushToast(e.status === "broken" ? "warn" : "ok", e.toast);
  const changed = e.status !== lastEngine;
  lastEngine = e.status;
  if (!changed && !fromEvent) return;
  const model = run.status === "ready" || run.status === "loading" || run.status === "broken" ? run.model : modelForAlias(e.alias);
  switch (e.status) {
    case "broken": {
      const reason = e.reason ?? "движок остановился";
      abortGenerations(reason); // a chat request must not hang on a dead engine
      if (model && (run.status === "ready" || run.status === "broken" || (run.status === "loading" && !deploying)))
        st.setRun({ status: "broken", model, reason });
      break;
    }
    case "loading":
      if (model && !deploying && run.status !== "loading") st.setRun({ status: "loading", model });
      break;
    case "ready":
      if (model && (run.status === "broken" || (run.status === "loading" && !deploying)))
        st.setRun({ status: "ready", model, endpoint: "http://127.0.0.1:8080" });
      break;
    case "idle":
      if (run.status === "ready" || run.status === "broken" || (run.status === "loading" && !deploying)) st.setRun({ status: "idle" });
      break;
  }
}

/** Keep-loaded: on startup re-attach to an engine left running by the previous session. */
export async function reattach() {
  if (!isTauri || !useCluster.getState().speed.keepLoaded) return;
  const alias = await probeRunningEngine();
  if (!alias) return;
  let m: ModelEntry | undefined;
  try { m = JSON.parse(localStorage.getItem(LS_LAST) ?? "null") ?? undefined; } catch { /* */ }
  m = m && m.id === alias ? m : MODELS.find((x) => x.id === alias) ?? m;
  if (!m) m = { id: alias, name: alias, family: "?", params: "?", quant: "?", sizeGb: 0, hfRepo: "", kvMbPer1k: 0, tags: [] };
  useCluster.getState().setRun({ status: "ready", model: m, endpoint: "http://127.0.0.1:8080" });
  useCluster.getState().pushEngineLog("[argus] подключились к уже запущенному движку (держать модель загруженной)");
}
