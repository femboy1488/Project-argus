// 0.3.2 experiment «Роли»: small jobs for a «Помощник» (chat titles, «Сводка»).
// Every job has a fallback, so nothing breaks when no helper is ready.
import { useCluster } from "../store/cluster";
import { helperCancel, helperChat, onHelperStream, readyHelpers, streamChat, stripThink, type ApiMessage } from "./api";
import { uid } from "./format";
import type { ChatMessage } from "./types";

const TITLE_SYSTEM =
  "Придумай короткое название для этого чата: 2–5 слов на языке пользователя, без кавычек, без точки в конце. Ответь только названием.";

const SUMMARY_SYSTEM =
  "Сделай краткую сводку переписки: 3–7 пунктов по-русски, только суть — что спрашивали, к чему пришли, что осталось открытым. Без вступлений.";

function cleanTitle(t: string): string {
  const line = stripThink(t).split("\n").map((x) => x.trim()).find(Boolean) ?? "";
  return line.replace(/^(название|title)\s*[:：]\s*/i, "").replace(/^["«“'`*]+|["»”'`*.]+$/g, "").trim().slice(0, 60);
}

/** After the first answer of a chat: ask a ready «Помощник» for a short title. No helper — the first-line title stays. */
export async function autoTitle(convId: string) {
  const st = useCluster.getState();
  const c = st.conversations.find((x) => x.id === convId);
  if (!c || c.autoTitled || c.agent) return;
  if (!readyHelpers(st.snapshot).length) return; // fallback: keep the title from the first message
  const user = c.messages.find((m) => m.role === "user" && m.content.trim());
  const answer = c.messages.find((m) => m.role === "assistant" && m.content.trim() && !m.content.startsWith("**[ошибка]**"));
  if (!user || !answer) return;
  st.patchChat(convId, { autoTitled: true });
  try {
    const r = await helperChat([
      { role: "system", content: TITLE_SYSTEM },
      { role: "user", content: `Сообщение пользователя:\n${user.content.slice(0, 1500)}\n\nОтвет:\n${answer.content.slice(0, 800)}` },
    ], { maxTokens: 32, temperature: 0.2 });
    const title = cleanTitle(r.content);
    if (title) useCluster.getState().patchChat(convId, { title });
  } catch { /* helper busy or gone: keep the old title */ }
}

function transcript(msgs: ChatMessage[]): string {
  const lines = msgs.filter((m) => !m.hidden && (m.role === "user" || m.role === "assistant") && m.content.trim())
    .map((m) => `${m.role === "user" ? "Пользователь" : "Ассистент"}: ${m.content.trim()}`);
  let text = lines.join("\n\n");
  if (text.length > 12000) text = "…" + text.slice(-12000); // the most recent part matters most
  return text;
}

export interface SummaryJob { by: string; done: Promise<string>; cancel: () => void }

/** «Сводка»: a ready «Помощник» if there is one, otherwise the main model. `onText` gets the growing text. */
export function summarize(msgs: ChatMessage[], onText: (t: string) => void): SummaryJob | null {
  const st = useCluster.getState();
  const api: ApiMessage[] = [{ role: "system", content: SUMMARY_SYSTEM }, { role: "user", content: transcript(msgs) }];
  const helper = readyHelpers(st.snapshot)[0];
  if (helper) {
    const streamId = `sum-${uid()}`;
    let acc = "";
    let off: (() => void) | undefined;
    const sub = onHelperStream((d) => { if (d.streamId === streamId) { acc += d.delta; onText(stripThink(acc)); } }).then((f) => (off = f));
    const done = sub.then(() => helperChat(api, { maxTokens: 400, streamId, temperature: 0.3 }))
      .then((r) => { const t = stripThink(r.content); onText(t); return t; })
      .finally(() => off?.());
    return { by: `Помощник «${helper.name}» · ${helper.model}`, done, cancel: () => { helperCancel(streamId).catch(() => {}); } };
  }
  if (st.run.status !== "ready") return null;
  const ctrl = new AbortController();
  const model = st.run.model.name;
  const done = (async () => {
    let acc = "";
    for await (const chunk of streamChat(api, ctrl.signal)) { acc += chunk; onText(stripThink(acc)); }
    return stripThink(acc);
  })();
  return { by: `основная модель · ${model}`, done, cancel: () => ctrl.abort() };
}
