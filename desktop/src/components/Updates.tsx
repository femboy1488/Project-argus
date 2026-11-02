import { useState, type ReactNode } from "react";
import { Progress } from "./Meter";
import { Download, ExternalLink, Info, Loader2, RefreshCw, Smartphone, X } from "lucide-react";
import { useCluster } from "../store/cluster";
import { checkUpdates, installUpdate, openExternal, setUpdateCheck, skipNodeUpdate, skipUpdateVersion, updateUsbNode } from "../lib/api";
import Markdown from "./Markdown";
import type { ClusterNode, UpdateStatus } from "../lib/types";

const mb = (b: number) => (b / 1048576).toFixed(1);
const KIND: Record<UpdateStatus["installKind"], string> = {
  nsis: "Windows · установщик",
  portable: "Windows · без установщика",
  appimage: "Linux · AppImage",
  deb: "Linux · .deb",
  other: "другая сборка",
};
const PHASE: Record<string, string> = { download: "Загрузка", stopping: "Остановка движка", restarting: "Перезапуск…", error: "Ошибка" };

function Shell({ title, icon, onClose, children, footer, width = 640 }: {
  title: string; icon: ReactNode; onClose?: () => void; children: ReactNode; footer: ReactNode; width?: number;
}) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="flex max-h-[85vh] max-w-[calc(100vw-2rem)] flex-col border border-pl-line2 bg-pl-bg1 shadow-2xl" style={{ width }}>
        <div className="flex h-9 shrink-0 items-center justify-between border-b border-pl-line bg-pl-bg2 px-3">
          <span className="flex min-w-0 items-center gap-2 truncate text-xs font-semibold uppercase tracking-[0.12em] text-pl-mute">{icon} {title}</span>
          {onClose && <button onClick={onClose} className="text-pl-dim hover:text-pl-text"><X size={15} /></button>}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-4">{children}</div>
        <div className="flex shrink-0 flex-wrap justify-end gap-2 border-t border-pl-line bg-pl-bg2 px-3 py-2">{footer}</div>
      </div>
    </div>
  );
}

/** «Доступна версия X. Хотите обновить?» — nothing is downloaded or installed before «Обновить». */
export function UpdateDialog() {
  const { update, updProgress, laterUpdate, setUpdateDialog, setUpdate, pushToast } = useCluster();
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const rel = update?.latest;
  if (!update || !rel) return null;
  const opensPage = !rel.asset;

  async function go() {
    setBusy(true); setErr(null);
    try {
      const r = await installUpdate();
      if (r === "opened") { pushToast("info", `Открыта страница релиза ${rel!.tag} — скачайте и установите вручную`); setUpdateDialog(false); }
      // "installing": the app exits in a moment and the installer relaunches it
    } catch (e) { setErr(String(e)); }
    setBusy(false);
  }
  async function skip() {
    await skipUpdateVersion(rel!.version).catch(() => {});
    setUpdate({ ...update!, skipped: true });
    setUpdateDialog(false);
    pushToast("info", `Версия ${rel!.version} пропущена — напомню о следующей`);
  }
  const p = updProgress;
  const installing = busy || update.busy;

  return (
    <Shell title="Обновление Argus" icon={<Download size={13} />} onClose={installing ? undefined : () => laterUpdate(rel.version)}
      footer={<>
        <button className="btn" disabled={installing} onClick={skip}>Пропустить эту версию</button>
        <button className="btn" disabled={installing} onClick={() => laterUpdate(rel.version)}>Позже</button>
        <button className="btn-primary" disabled={installing} onClick={go}>
          {installing ? <Loader2 size={12} className="animate-spin" /> : opensPage ? <ExternalLink size={12} /> : <Download size={12} />}
          {opensPage ? "Открыть страницу релиза" : "Обновить"}
        </button>
      </>}>
      <div className="text-base font-medium text-pl-text">Доступна версия {rel.version}. Хотите обновить?</div>
      <div className="mt-1 font-mono text-2xs text-pl-dim">
        установлена {update.current} · {KIND[update.installKind]} · {rel.tag}{rel.publishedAt ? ` · ${rel.publishedAt.slice(0, 10)}` : ""}
      </div>
      {rel.notes.trim() ? (
        <div className="mt-3 max-h-[40vh] overflow-y-auto border border-pl-line bg-pl-bg0 p-3 text-sm"><Markdown text={rel.notes} /></div>
      ) : <div className="mt-3 text-xs text-pl-dim">Описание релиза пустое.</div>}
      <div className="mt-3 text-2xs leading-4 text-pl-dim">
        {opensPage
          ? update.installKind === "deb"
            ? "Сборка .deb обновляется пакетным менеджером: откроется страница релиза — скачайте новый .deb."
            : "Для этой сборки автоматическая установка недоступна — откроется страница релиза."
          : <>Файл {rel.asset!.name} ({mb(rel.asset!.size)} MB) будет проверен по SHA-256 из SHA256SUMS.txt. Затем Argus остановит модель,
            {update.installKind === "nsis" ? " запустит установщик в тихом режиме и откроется заново" : " заменит AppImage и перезапустится"}. Настройки, ноды и модели сохраняются.</>}
        {!opensPage && !rel.hasSums && <div className="mt-1 text-pl-orange">⚠ В релизе нет SHA256SUMS.txt — установка будет отклонена.</div>}
      </div>
      {p && p.phase && (
        <div className="mt-3">
          <div className="flex justify-between font-mono text-2xs text-pl-mute">
            <span>{PHASE[p.phase] ?? p.phase}</span>{p.total > 0 ? <span className="tabular-nums">{mb(Math.min(p.done, p.total))} / {mb(p.total)} MB</span> : p.done > 0 && <span className="tabular-nums">{mb(p.done)} MB</span>}
          </div>
          {/* unknown size (no Content-Length) or a non-download phase → indeterminate instead of a bar frozen at 0% */}
          <Progress className="mt-1" value={p.phase === "download" && p.total > 0 ? (p.done / p.total) * 100 : p.phase === "error" ? 100 : null} tone={p.phase === "error" ? "red" : "blue"} />
        </div>
      )}
      {err && <div className="mt-3 border-l-2 border-pl-red bg-pl-red/10 p-2 font-mono text-2xs text-pl-red">{err}</div>}
    </Shell>
  );
}

/** «Обновить ноду на <device>?» — the bundled APK is pushed over USB only after «Обновить». */
export function NodeUpdateDialog({ node, version }: { node: ClusterNode; version: string }) {
  const { laterNode, pushToast } = useCluster();
  const [busy, setBusy] = useState(false);
  const key = `${node.id}|${version}`;
  async function go() {
    setBusy(true);
    try { await updateUsbNode(node.id); pushToast("info", `«${node.name}»: обновление ноды до ${version}…`); laterNode(key); }
    catch (e) { pushToast("error", String(e)); }
    setBusy(false);
  }
  async function skip() {
    await skipNodeUpdate(version).catch(() => {});
    laterNode(key);
    pushToast("info", `Версия ноды ${version} пропущена`);
  }
  return (
    <Shell title="Обновление ноды" icon={<Smartphone size={13} />} width={520} onClose={() => laterNode(key)}
      footer={<>
        <button className="btn" disabled={busy} onClick={skip}>Пропустить эту версию</button>
        <button className="btn" disabled={busy} onClick={() => laterNode(key)}>Позже</button>
        <button className="btn-primary" disabled={busy} onClick={go}>{busy ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />} Обновить</button>
      </>}>
      <div className="text-base font-medium text-pl-text">Обновить ноду на «{node.name}»?</div>
      <div className="mt-2 font-mono text-xs text-pl-mute">установлена {node.version ?? "?"} → {version}</div>
      <p className="mt-3 text-xs leading-5 text-pl-mute">
        Argus установит Argus Node {version} по USB (<span className="font-mono">adb install -r</span>, данные и кэш весов сохраняются).
        Нода переподключится примерно за минуту{node.inEngine ? "; запущенная модель остановится и перезапустится сама, когда нода вернётся" : ""}.
        До обновления нода продолжает работать на текущей версии.
      </p>
    </Shell>
  );
}

/** USB nodes waiting for an answer (outdated, not skipped, not «Позже» this session). First one is asked. */
export function useNodeToAsk(): [ClusterNode, string] | null {
  const { snapshot, nodeLater } = useCluster();
  const n = snapshot?.nodes.find((x) => x.transport === "usb" && x.updateAvailable && !x.updateSkipped
    && x.stage === "connected" && !nodeLater.includes(`${x.id}|${x.updateAvailable}`));
  return n ? [n, n.updateAvailable!] : null;
}

/** «О программе»: current version, «Проверять обновления», «Проверить обновления». */
export function AboutDialog() {
  const { update, setUpdate, setAbout, setUpdateDialog, snapshot, pushToast } = useCluster();
  const [busy, setBusy] = useState(false);
  async function check() {
    setBusy(true);
    try {
      const s = await checkUpdates();
      setUpdate(s);
      if (s.available) { setAbout(false); setUpdateDialog(true); }   // manual check: ask even if skipped before
      else if (s.error) pushToast("warn", `Не удалось проверить: ${s.error}`);
      else pushToast("ok", `Установлена последняя версия (${s.current})`);
    } catch (e) { pushToast("error", String(e)); }
    setBusy(false);
  }
  async function toggle(on: boolean) {
    await setUpdateCheck(on).catch(() => {});
    if (update) setUpdate({ ...update, checkEnabled: on });
  }
  const last = update?.lastCheck ? new Date(update.lastCheck * 1000).toLocaleString("ru-RU") : "ещё не проверялось";
  return (
    <Shell title="О программе" icon={<Info size={13} />} width={520} onClose={() => setAbout(false)}
      footer={<>
        <button className="btn" onClick={() => openExternal("https://github.com/femboy1488/Project-argus/releases")}><ExternalLink size={12} /> Релизы на GitHub</button>
        <button className="btn-primary" disabled={busy} onClick={check}>{busy ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />} Проверить обновления</button>
      </>}>
      <div className="text-base font-medium text-pl-text">Argus Cluster {update?.current ?? snapshot?.appVersion ?? ""}</div>
      <dl className="mt-3 divide-y divide-pl-line border border-pl-line text-xs">
        {([
          ["Версия", update?.current ?? snapshot?.appVersion ?? "—"],
          ["Сборка", update ? KIND[update.installKind] : "—"],
          ["Нода в комплекте", snapshot?.bundledNodeVersion ?? "—"],
          ["Последний релиз", update?.latest ? `${update.latest.tag}${update.latest.version ? "" : " (не версия — пропущен)"}` : "—"],
          ["Последняя проверка", last],
        ] as [string, string][]).map(([k, v]) => (
          <div key={k} className="flex justify-between gap-3 px-3 py-1.5"><dt className="text-pl-dim">{k}</dt><dd className="font-mono text-pl-text">{v}</dd></div>
        ))}
      </dl>
      {update?.available && update.skipped && <div className="mt-2 text-2xs text-pl-dim">Версия {update.latest?.version} пропущена. «Проверить обновления» спросит снова.</div>}
      {update?.error && <div className="mt-2 font-mono text-2xs text-pl-orange">последняя проверка: {update.error}</div>}
      {update?.nodeServerError && <div className="mt-2 font-mono text-2xs text-pl-orange">{update.nodeServerError}</div>}
      <label className="mt-4 flex cursor-pointer items-center justify-between text-xs">
        <span className="text-pl-text">Проверять обновления <span className="text-pl-dim">(при запуске и каждые 6 ч)</span></span>
        <input type="checkbox" className="accent-[#2D72D2]" checked={update?.checkEnabled ?? true} onChange={(e) => toggle(e.target.checked)} />
      </label>
      <p className="mt-2 text-2xs leading-4 text-pl-dim">
        Argus никогда не обновляется сам: при новой версии он спросит «Обновить / Позже / Пропустить эту версию». То же для нод:
        USB-ноды обновляются только после вашего подтверждения, Wi-Fi ноды спрашивают на самом устройстве.
      </p>
    </Shell>
  );
}
