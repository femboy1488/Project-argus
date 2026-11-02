// «Знания»: поиск по своим файлам (индекс папки) и офлайн-библиотека (Kiwix: Википедия и др.).
// Агент пользуется этим через инструменты search_files, wiki_search, wiki_read.
import { useEffect, useState } from "react";
import { BookOpen, FolderSearch, Loader2, Search } from "lucide-react";
import { useCluster } from "../store/cluster";
import {
  kiwixBooks, kiwixInfo, kiwixRead, kiwixSearch, kiwixSetUrl, onRagProgress, ragIndex, ragSearch, ragStatus,
  type KiwixBook, type KiwixInfo, type RagHit, type RagStatus, type WikiHit,
} from "../lib/api";

const input = "h-8 w-full border border-pl-line2 bg-pl-bg0 px-2 text-sm text-pl-text outline-none focus:border-pl-blue";

export default function KnowledgeView() {
  return (
    <div className="grid h-full grid-cols-1 content-start gap-px overflow-y-auto bg-pl-line xl:grid-cols-2 xl:content-stretch xl:overflow-hidden">
      <FilesPanel />
      <LibraryPanel />
    </div>
  );
}

function FilesPanel() {
  const { pushToast } = useCluster();
  const [st, setSt] = useState<RagStatus | null>(null);
  const [folder, setFolder] = useState("");
  const [url, setUrl] = useState("");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<RagHit[] | null>(null);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    let off: (() => void) | undefined;
    ragStatus().then((s) => { setSt(s); setFolder((f) => f || s.folder); setUrl((u) => u || s.customUrl); }).catch(() => {});
    onRagProgress(setSt).then((f) => (off = f));
    return () => off?.();
  }, []);

  async function index() {
    try { setSt(await ragIndex(folder.trim(), url.trim() || null)); } catch (e) { pushToast("error", String(e)); }
  }
  async function search() {
    setSearching(true); setHits(null);
    try { setHits(await ragSearch(query.trim(), 5)); } catch (e) { pushToast("error", String(e)); }
    setSearching(false);
  }

  return (
    <section className="flex min-w-0 flex-col gap-3 bg-pl-bg0 p-4 text-xs xl:overflow-y-auto">
      <div className="flex items-center gap-2 text-sm text-pl-text"><FolderSearch size={16} /> Поиск по моим файлам</div>
      <p className="text-pl-mute">Укажите папку с документами — я разберу их на кусочки и смогу находить нужное по смыслу, а не только по словам. Подходят .txt, .md, .docx и .pdf с текстом. Агент тоже сможет искать здесь.</p>
      <label className="block text-pl-mute">Папка
        <div className="mt-1 flex gap-2">
          <input value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="C:\Users\me\Documents" className={`${input} font-mono`} />
          <button className="btn-primary" disabled={!folder.trim() || st?.indexing} onClick={index}>
            {st?.indexing ? <Loader2 size={13} className="animate-spin" /> : null}{st?.files ? "Обновить" : "Проиндексировать"}
          </button>
        </div>
      </label>
      <details className="text-pl-dim">
        <summary className="cursor-pointer">Сервис поиска</summary>
        <p className="mt-1">По умолчанию — телефон с сервисом поиска, если подключён, иначе небольшая модель прямо на ПК (скачается один раз, ~140 МБ). Можно указать свой адрес llama-server с эмбеддингами:</p>
        <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://192.168.1.20:8081" className={`${input} mt-1 font-mono`} />
      </details>
      {st && (
        <div className="panel p-3 text-pl-mute">
          {st.indexing ? (
            <>
              <div className="flex justify-between"><span>{st.message}</span><span>{st.total ? Math.round((st.done / st.total) * 100) : 0}%</span></div>
              <div className="mt-1 h-1.5 overflow-hidden rounded bg-pl-bg3"><div className="h-full bg-pl-blue2 transition-all" style={{ width: `${st.total ? (st.done / st.total) * 100 : 3}%` }} /></div>
              {st.current && <div className="mt-1 truncate text-2xs text-pl-dim">{st.current}</div>}
            </>
          ) : st.files ? (
            <div>В индексе {st.files} файл(ов), {st.chunks} фрагментов{st.model ? ` · модель ${st.model}` : ""}{st.message ? <div className="mt-1 text-2xs text-pl-dim">{st.message}</div> : null}</div>
          ) : <div>{st.message || "Индекса пока нет."}</div>}
          {st.errors.length > 0 && (
            <details className="mt-1 text-2xs text-pl-orange"><summary className="cursor-pointer">Не прочитано: {st.errors.length}</summary>
              {st.errors.slice(0, 50).map((e, i) => <div key={i} className="mt-0.5 break-all">{e}</div>)}</details>
          )}
        </div>
      )}
      <div className="label mt-2">Проверить поиск</div>
      <div className="flex gap-2">
        <input value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && query.trim()) search(); }}
          placeholder="Например: сроки оплаты по договору" className={input} />
        <button className="btn" disabled={!query.trim() || searching || !st?.chunks} onClick={search}>
          {searching ? <Loader2 size={13} className="animate-spin" /> : <Search size={13} />} Найти
        </button>
      </div>
      {hits && hits.length === 0 && <div className="text-pl-dim">Ничего похожего не нашлось.</div>}
      {hits?.map((h, i) => (
        <div key={i} className="panel p-3">
          <div className="flex justify-between gap-2 text-2xs text-pl-dim"><span className="truncate font-mono" title={h.source}>{h.source}</span><span>{h.score.toFixed(2)}</span></div>
          <div className="mt-1 whitespace-pre-wrap text-pl-text">{h.text}</div>
        </div>
      ))}
    </section>
  );
}

function LibraryPanel() {
  const { pushToast } = useCluster();
  const [info, setInfo] = useState<KiwixInfo | null>(null);
  const [url, setUrl] = useState("");
  const [books, setBooks] = useState<KiwixBook[]>([]);
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<WikiHit[] | null>(null);
  const [article, setArticle] = useState<{ title: string; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  async function refresh() {
    try {
      const i = await kiwixInfo();
      setInfo(i); setUrl((u) => u || i.url);
      setBooks(i.base ? await kiwixBooks().catch(() => []) : []);
    } catch { /* not in the app */ }
  }
  useEffect(() => { refresh(); }, []);

  async function saveUrl() {
    try { await kiwixSetUrl(url.trim()); await refresh(); } catch (e) { pushToast("error", String(e)); }
  }
  async function search() {
    setBusy(true); setHits(null); setArticle(null);
    try { setHits(await kiwixSearch(q.trim())); } catch (e) { pushToast("error", String(e)); }
    setBusy(false);
  }
  async function open(h: WikiHit) {
    setBusy(true);
    try { setArticle({ title: h.title, text: await kiwixRead(h.path) }); } catch (e) { pushToast("error", String(e)); }
    setBusy(false);
  }

  return (
    <section className="flex min-w-0 flex-col gap-3 bg-pl-bg0 p-4 text-xs xl:overflow-y-auto">
      <div className="flex items-center gap-2 text-sm text-pl-text"><BookOpen size={16} /> Библиотека</div>
      <p className="text-pl-mute">Офлайн-Википедия и другие книги Kiwix — с телефона или с компьютера, где запущен kiwix-serve. Работает без интернета; агент сможет искать и читать статьи.</p>
      <div className="panel p-3 text-pl-mute">
        {info === null ? "Проверяю…" : info.base
          ? <>Подключено: <span className="font-mono text-pl-text">{info.base}</span>{info.phone ? " (телефон)" : ""} · книг: {books.length}</>
          : "Библиотека не найдена. Подключите телефон с библиотекой или укажите адрес kiwix-serve ниже."}
        {books.length > 0 && <div className="mt-1 text-2xs text-pl-dim">{books.slice(0, 8).map((b) => b.title || b.name).join(" · ")}{books.length > 8 ? " …" : ""}</div>}
      </div>
      <label className="block text-pl-mute">Адрес kiwix-serve <span className="text-pl-dim">(пусто — искать на телефоне)</span>
        <div className="mt-1 flex gap-2">
          <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://127.0.0.1:8090" className={`${input} font-mono`} />
          <button className="btn" onClick={saveUrl}>Сохранить</button>
        </div>
      </label>
      <div className="label mt-2">Проверить поиск</div>
      <div className="flex gap-2">
        <input value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && q.trim()) search(); }} placeholder="Например: Байкал" className={input} />
        <button className="btn" disabled={!q.trim() || busy || !info?.base} onClick={search}>
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Search size={13} />} Найти
        </button>
      </div>
      {hits && hits.length === 0 && <div className="text-pl-dim">Ничего не нашлось.</div>}
      {article ? (
        <div className="panel p-3">
          <div className="flex items-center justify-between gap-2"><span className="text-sm text-pl-text">{article.title}</span>
            <button className="btn" onClick={() => setArticle(null)}>К результатам</button></div>
          <div className="mt-2 whitespace-pre-wrap leading-5 text-pl-mute">{article.text}</div>
        </div>
      ) : hits?.map((h, i) => (
        <button key={i} onClick={() => open(h)} className="panel block w-full p-3 text-left hover:bg-pl-bg2">
          <div className="text-sm text-pl-text">{h.title}</div>
          {h.snippet && <div className="mt-0.5 line-clamp-2 text-pl-mute">{h.snippet}</div>}
          <div className="mt-0.5 text-2xs text-pl-dim">{h.book}</div>
        </button>
      ))}
    </section>
  );
}
