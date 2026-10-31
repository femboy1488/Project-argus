import { Fragment, useState, type ReactNode } from "react";
import { Check, ChevronRight, Copy } from "lucide-react";
import { openExternal } from "../lib/api";

/** Small, dependency-free markdown renderer (no HTML injection): fenced code with copy, inline code,
 *  bold/italic, links, headings, lists, quotes, tables, <think> blocks. Tolerates unfinished streams. */
export default function Markdown({ text, streaming }: { text: string; streaming?: boolean }) {
  // <think>…</think> (Qwen3 / reasoning models) -> collapsible block
  const parts: ReactNode[] = [];
  let rest = text;
  let k = 0;
  while (true) {
    const a = rest.indexOf("<think>");
    if (a < 0) break;
    const b = rest.indexOf("</think>", a);
    if (a > 0) parts.push(<Blocks key={k++} src={rest.slice(0, a)} />);
    const inner = rest.slice(a + 7, b < 0 ? undefined : b);
    parts.push(<Think key={k++} text={inner} open={b < 0 && !!streaming} />);
    rest = b < 0 ? "" : rest.slice(b + 8);
  }
  if (rest) parts.push(<Blocks key={k++} src={rest} />);
  return <div className="md space-y-2 text-[14px] leading-6 text-pl-text">{parts}</div>;
}

function Think({ text, open }: { text: string; open: boolean }) {
  const [o, setO] = useState(false);
  const shown = o || open;
  return (
    <div className="border-l-2 border-pl-violet/50 bg-pl-violet/5 px-3 py-1 text-xs text-pl-mute">
      <button className="flex items-center gap-1 font-mono text-2xs uppercase tracking-wider text-pl-violet" onClick={() => setO(!o)}>
        <ChevronRight size={11} className={shown ? "rotate-90" : ""} /> размышления
      </button>
      {shown && <div className="mt-1 whitespace-pre-wrap">{text.trim()}</div>}
    </div>
  );
}

function Blocks({ src }: { src: string }) {
  const lines = src.replace(/\r/g, "").split("\n");
  const out: ReactNode[] = [];
  let i = 0, key = 0;
  while (i < lines.length) {
    const l = lines[i];
    const fence = l.match(/^\s*(```|~~~)\s*([\w+#.-]*)/);
    if (fence) {
      const lang = fence[2]; const body: string[] = []; i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) body.push(lines[i++]);
      i++; // closing fence (or EOF while streaming)
      out.push(<Code key={key++} lang={lang} code={body.join("\n")} />);
      continue;
    }
    if (!l.trim()) { i++; continue; }
    const h = l.match(/^(#{1,6})\s+(.*)/);
    if (h) {
      const size = ["text-lg", "text-base", "text-[15px]", "text-sm", "text-sm", "text-sm"][h[1].length - 1];
      out.push(<div key={key++} className={`${size} font-semibold text-pl-text`}>{inline(h[2])}</div>); i++; continue;
    }
    if (/^\s*(-{3,}|\*{3,}|_{3,})\s*$/.test(l)) { out.push(<hr key={key++} className="border-pl-line2" />); i++; continue; }
    if (/^\s*>/.test(l)) {
      const q: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) q.push(lines[i++].replace(/^\s*>\s?/, ""));
      out.push(<blockquote key={key++} className="border-l-2 border-pl-line2 pl-3 text-pl-mute">{q.map((x, j) => <div key={j}>{inline(x)}</div>)}</blockquote>);
      continue;
    }
    if (/^\s*\|.*\|\s*$/.test(l) && i + 1 < lines.length && /^\s*\|?\s*:?-{2,}/.test(lines[i + 1])) {
      const row = (s: string) => s.trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
      const head = row(l); i += 2; const rows: string[][] = [];
      while (i < lines.length && /^\s*\|.*\|\s*$/.test(lines[i])) rows.push(row(lines[i++]));
      out.push(
        <div key={key++} className="overflow-x-auto"><table className="text-xs">
          <thead><tr>{head.map((c, j) => <th key={j} className="border border-pl-line2 bg-pl-bg2 px-2 py-1 text-left">{inline(c)}</th>)}</tr></thead>
          <tbody>{rows.map((r, a) => <tr key={a}>{r.map((c, j) => <td key={j} className="border border-pl-line px-2 py-1">{inline(c)}</td>)}</tr>)}</tbody>
        </table></div>);
      continue;
    }
    const li = l.match(/^(\s*)([-*+]|\d+[.)])\s+(.*)/);
    if (li) {
      const ordered = /\d/.test(li[2]);
      const items: string[] = [];
      while (i < lines.length) {
        const m = lines[i].match(/^(\s*)([-*+]|\d+[.)])\s+(.*)/);
        if (m) { items.push(m[3]); i++; }
        else if (lines[i].trim() && /^\s{2,}/.test(lines[i]) && items.length) { items[items.length - 1] += " " + lines[i].trim(); i++; }
        else break;
      }
      const Tag = ordered ? "ol" : "ul";
      out.push(<Tag key={key++} className={`${ordered ? "list-decimal" : "list-disc"} space-y-0.5 pl-5 marker:text-pl-dim`}>{items.map((x, j) => <li key={j}>{inline(x)}</li>)}</Tag>);
      continue;
    }
    const para: string[] = [];
    while (i < lines.length && lines[i].trim() && !/^\s*(```|~~~|#{1,6}\s|>|([-*+]|\d+[.)])\s)/.test(lines[i])) para.push(lines[i++]);
    if (!para.length) para.push(lines[i++]);
    out.push(<p key={key++} className="whitespace-pre-wrap">{para.map((x, j) => <Fragment key={j}>{j > 0 && "\n"}{inline(x)}</Fragment>)}</p>);
  }
  return <>{out}</>;
}

/** inline: `code`, **bold**, *italic* / _italic_, [text](url), bare URLs */
export function inline(s: string): ReactNode[] {
  const re = /(`[^`]+`)|(\*\*[^*]+\*\*|__[^_]+__)|(\*[^*\s][^*]*\*|_[^_\s][^_]*_)|(\[[^\]]+\]\((https?:\/\/[^)\s]+)\))|(https?:\/\/[^\s)<>]+)/g;
  const out: ReactNode[] = [];
  let last = 0, m: RegExpExecArray | null, k = 0;
  while ((m = re.exec(s))) {
    if (m.index > last) out.push(s.slice(last, m.index));
    const t = m[0];
    if (m[1]) out.push(<code key={k++} className="rounded bg-pl-bg3 px-1 py-px font-mono text-[12.5px] text-pl-gold">{t.slice(1, -1)}</code>);
    else if (m[2]) out.push(<strong key={k++} className="font-semibold">{t.slice(2, -2)}</strong>);
    else if (m[3]) out.push(<em key={k++}>{t.slice(1, -1)}</em>);
    else if (m[4]) { const label = t.slice(1, t.indexOf("](")); out.push(<Link key={k++} href={m[5]}>{label}</Link>); }
    else if (m[6]) out.push(<Link key={k++} href={t}>{t}</Link>);
    last = m.index + t.length;
  }
  if (last < s.length) out.push(s.slice(last));
  return out;
}

const Link = ({ href, children }: { href: string; children: ReactNode }) => (
  <a href={href} onClick={(e) => { e.preventDefault(); openExternal(href); }} className="text-pl-blue underline decoration-pl-blue/40 hover:decoration-pl-blue">{children}</a>
);

function Code({ lang, code }: { lang: string; code: string }) {
  const [done, setDone] = useState(false);
  return (
    <div className="overflow-hidden border border-pl-line2 bg-pl-bg0">
      <div className="flex h-7 items-center justify-between border-b border-pl-line bg-pl-bg2 px-2 font-mono text-2xs text-pl-dim">
        <span>{lang || "code"}</span>
        <button className="flex items-center gap-1 hover:text-pl-text" onClick={() => { navigator.clipboard?.writeText(code); setDone(true); setTimeout(() => setDone(false), 1200); }}>
          {done ? <><Check size={11} className="text-pl-green" /> скопировано</> : <><Copy size={11} /> копировать</>}
        </button>
      </div>
      <pre className="overflow-x-auto p-3 font-mono text-[12.5px] leading-5 text-pl-text">{code}</pre>
    </div>
  );
}
