import { useEffect, useLayoutEffect, useState } from "react";
import { useCluster, type View } from "../store/cluster";

interface Step { view: View; target?: string; title: string; body: string }

const STEPS: Step[] = [
  { view: "devices", target: "usb", title: "1 · Подключите планшет или телефон",
    body: "На устройстве: Настройки → О телефоне → 7 раз нажмите «Номер сборки» → Для разработчиков → включите «Отладка по USB». Подключите кабель и нажмите «Разрешить» на экране устройства. Argus сам установит ноду и добавит её память в пул." },
  { view: "devices", target: "lan", title: "2 · Ноутбук или Raspberry Pi по сети",
    body: "Есть ещё компьютер? Запустите на нём rpc-server из llama.cpp b12008 и добавьте его кнопкой «Добавить ноду по IP». Только для закрытой домашней сети." },
  { view: "models", target: "deploy", title: "3 · Нажмите Deploy",
    body: "Звёздочкой отмечена лучшая модель для вашего пула. Deploy скачает веса один раз и разложит слои по всем устройствам — быстрые получают больше." },
  { view: "models", target: "hf", title: "4 · Любая модель с Hugging Face",
    body: "Найдите GGUF-модель по названию, Argus проверит, влезет ли она, и запустит одной кнопкой «Скачать и запустить»." },
  { view: "chat", target: "quickstart", title: "5 · Чат",
    body: "Несколько разговоров слева, markdown и код с копированием, остановка, перегенерация и прикреплённые текстовые файлы. Скорость (tok/s) видна у каждого ответа." },
  { view: "chat", target: "agent", title: "6 · Режим агента",
    body: "Модель может запускать команды, работать с файлами, открывать сайты, а модели со зрением — видеть экран и управлять мышью. Каждое действие вы подтверждаете сами." },
  { view: "api", target: "api", title: "7 · API для других программ",
    body: "Hermes, Open WebUI, Continue, Jan и любой OpenAI-совместимый клиент: base URL http://127.0.0.1:8080/v1. Готовые настройки — на этой вкладке." },
];

export default function Tour() {
  const { setTour, setView } = useCluster();
  const [i, setI] = useState(0);
  const [rect, setRect] = useState<DOMRect | null>(null);
  const step = STEPS[i];

  useEffect(() => { setView(step.view); }, [step.view, setView]);
  useLayoutEffect(() => {
    let raf = 0;
    const find = () => {
      const el = step.target ? document.querySelector(`[data-tour="${step.target}"]`) : null;
      setRect(el ? el.getBoundingClientRect() : null);
      raf = requestAnimationFrame(find);
    };
    const t = setTimeout(find, 60);
    return () => { clearTimeout(t); cancelAnimationFrame(raf); };
  }, [i, step.target]);
  useEffect(() => {
    const k = (e: KeyboardEvent) => { if (e.key === "Escape") setTour(false); if (e.key === "ArrowRight") setI((x) => Math.min(STEPS.length - 1, x + 1)); if (e.key === "ArrowLeft") setI((x) => Math.max(0, x - 1)); };
    window.addEventListener("keydown", k); return () => window.removeEventListener("keydown", k);
  }, [setTour]);

  const W = 360, pad = 8;
  let left = window.innerWidth / 2 - W / 2, top = window.innerHeight / 2 - 100;
  if (rect) {
    left = Math.min(window.innerWidth - W - 12, Math.max(12, rect.left + rect.width / 2 - W / 2));
    top = rect.bottom + 14 + 210 < window.innerHeight ? rect.bottom + 14 : Math.max(12, rect.top - 14 - 210);
  }
  return (
    <div className="fixed inset-0 z-[60]">
      <svg className="absolute inset-0 h-full w-full" onClick={() => setTour(false)}>
        <defs><mask id="tour-hole"><rect width="100%" height="100%" fill="white" />
          {rect && <rect x={rect.left - pad} y={rect.top - pad} width={rect.width + pad * 2} height={rect.height + pad * 2} fill="black" />}</mask></defs>
        <rect width="100%" height="100%" fill="rgba(0,0,0,0.62)" mask="url(#tour-hole)" />
        {rect && <rect x={rect.left - pad} y={rect.top - pad} width={rect.width + pad * 2} height={rect.height + pad * 2} fill="none" stroke="#4C90F0" strokeWidth="1.5" strokeDasharray="4 3" />}
      </svg>
      <div className="absolute border border-pl-blue/70 bg-pl-bg1 shadow-2xl" style={{ left, top, width: W }}>
        <div className="flex h-8 items-center justify-between border-b border-pl-line bg-pl-bg2 px-3">
          <span className="text-2xs font-semibold uppercase tracking-[0.12em] text-pl-blue">Обучение · {i + 1}/{STEPS.length}</span>
          <button className="text-2xs text-pl-dim hover:text-pl-text" onClick={() => setTour(false)}>Пропустить</button>
        </div>
        <div className="p-4">
          <div className="text-sm font-medium text-pl-text">{step.title}</div>
          <p className="mt-1.5 text-xs leading-5 text-pl-mute">{step.body}</p>
        </div>
        <div className="flex items-center gap-2 border-t border-pl-line px-3 py-2">
          <div className="flex gap-1">{STEPS.map((_, j) => <span key={j} className={`h-1 w-4 ${j <= i ? "bg-pl-blue" : "bg-pl-bg4"}`} />)}</div>
          <button className="btn ml-auto" disabled={i === 0} onClick={() => setI(i - 1)}>Назад</button>
          {i < STEPS.length - 1
            ? <button className="btn-primary" onClick={() => setI(i + 1)}>Далее</button>
            : <button className="btn-primary" onClick={() => { setTour(false); setView("models"); }}>Начать</button>}
        </div>
      </div>
    </div>
  );
}
