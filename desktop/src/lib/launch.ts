// One place that turns "deploy this model" into start_model args (speed toggles, split hint, draft).
import { useCluster } from "../store/cluster";
import { startModel, waitHealthy, probeRunningEngine, isTauri } from "./api";
import { draftFor, requiredMb, MODELS } from "./models";
import type { ModelEntry } from "./types";

const LS_LAST = "argus.lastModel";

export async function deploy(m: ModelEntry, opts: { goChat?: boolean } = {}) {
  const st = useCluster.getState();
  const { ctx, speed, lan } = st;
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
