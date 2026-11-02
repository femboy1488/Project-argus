// «Голос» и «Библиотека» на ноде (roles_extra.rs). Показывается в инспекторе ноды.
import { useCallback, useEffect, useState } from "react";
import { Toggle } from "./ChatView";
import { Progress } from "./Meter";
import { useCluster } from "../store/cluster";
import { nodeRolesGet, nodesvcList, setNodeLibrary, setNodeVoice } from "../lib/api";
import type { NodeRoleReport, NodeRolesView, NodeService, VoiceModelId } from "../lib/api";
import type { ClusterNode } from "../lib/types";

const STATE_RU: Record<string, string> = {
  idle: "выключен", downloading: "получает модель", starting: "запускается", loading: "запускается", ready: "работает", error: "ошибка",
};

function stateText(r: NodeRoleReport | undefined, svc: NodeService | undefined): string | null {
  if (svc) {
    const base = STATE_RU[svc.state] ?? svc.state;
    return svc.state === "ready" ? `${base} · ПК :${svc.pc_port}` : base;
  }
  if (!r?.state) return null;
  let t = STATE_RU[r.state] ?? r.state;
  if (r.state === "downloading" && r.pct != null) t += ` ${Math.round(r.pct)}%`;
  if (r.error) t += `: ${r.error}`;
  return t;
}

export default function NodeExtraRoles({ node }: { node: ClusterNode }) {
  const pushToast = useCluster((s) => s.pushToast);
  const [v, setV] = useState<NodeRolesView | null>(null);
  const [svc, setSvc] = useState<NodeService[]>([]);

  const load = useCallback(() => {
    nodeRolesGet(node.id).then(setV).catch(() => {});
    nodesvcList().then((l) => setSvc(l.filter((s) => s.device === node.id))).catch(() => {});
  }, [node.id]);
  useEffect(() => { load(); const t = setInterval(load, 3000); return () => clearInterval(t); }, [load]);

  if (!v) return null;
  const whisper = svc.find((s) => s.service === "whisper");
  const kiwix = svc.find((s) => s.service === "kiwix");
  const err = (e: unknown) => pushToast("error", String(e));

  async function voice(on: boolean, model?: VoiceModelId) {
    try { await setNodeVoice(node.id, on, model); load(); } catch (e) { err(e); }
  }
  async function library(on: boolean) {
    try { await setNodeLibrary(node.id, on); load(); } catch (e) { err(e); }
  }

  const voiceState = stateText(v.node?.voice, whisper);
  const libState = stateText(v.node?.library, kiwix);
  const lan = node.transport === "lan";

  return (
    <div className="space-y-2 border-t border-pl-line pt-3">
      <div className="label">Голос и библиотека</div>

      <Toggle on={v.voice} set={(on) => voice(on)} label="Голос" sub="распознаёт речь для кнопки микрофона в чате" />
      {v.voice && (
        <div className="space-y-1.5 pl-1">
          <select className="w-full border border-pl-line2 bg-pl-bg2 px-2 py-1 text-2xs text-pl-text" value={v.voiceModel}
            onChange={(e) => voice(true, e.target.value as VoiceModelId)}>
            {v.models.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
          </select>
          {!v.modelOnPc && v.downloadError && <p className="text-2xs leading-4 text-pl-red">Не получилось скачать модель: {v.downloadError}</p>}
          {!v.modelOnPc && !v.downloadError && (
            <div>
              <div className="mb-1 text-2xs text-pl-dim">Качаю модель на компьютер{v.downloadPct != null ? ` · ${v.downloadPct}%` : "…"}</div>
              <Progress value={v.downloadPct} />
            </div>
          )}
          {v.modelOnPc && voiceState && <p className="text-2xs text-pl-dim">На телефоне: {voiceState}</p>}
          {v.modelOnPc && !voiceState && <p className="text-2xs text-pl-dim">{lan ? "Модель уйдёт на телефон с ответом на маяк (до 10 с)." : "Жду ответа телефона…"}</p>}
        </div>
      )}

      <Toggle on={v.library} set={library} label="Библиотека" sub=".zim выбирается на телефоне в Argus Node" />
      {v.library && libState && <p className="pl-1 text-2xs text-pl-dim">На телефоне: {libState}{v.node?.library?.zim ? ` · ${v.node.library.zim}` : ""}</p>}

      {v.note && <p className="text-2xs leading-4 text-pl-gold">{v.note}</p>}
      {lan && (v.voice || v.library) && <p className="text-2xs leading-4 text-pl-dim">По Wi-Fi состояние ролей видно в самом Argus Node.</p>}
    </div>
  );
}
