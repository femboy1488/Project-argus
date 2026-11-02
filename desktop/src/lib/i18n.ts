// Minimal i18n: ru (default) / en. `t(key)` outside React, `useT()` inside (re-renders on language change).
// The language is stored in argus-features.json (features.lang) and mirrored to localStorage for a fast first paint.
import { create } from "zustand";

export type Lang = "ru" | "en";

const ru = {
  "nav.chat": "Чат",
  "nav.devices": "Кластер",
  "nav.models": "Модели",
  "nav.api": "API",
  "nav.parallel": "Тест",
  "nav.features": "Возможности",
  "nav.queue": "Очередь",
  "nav.knowledge": "Знания",
  "nav.connect": "Подключить устройство",
  "nav.parallelHint": "Параллельный тест: основная модель и Помощники отвечают на один вопрос",
  "f.title": "Возможности",
  "f.save": "Сохранить",
  "f.saved": "Сохранено",
  "f.on": "Вкл",
  "f.off": "Выкл",
  "f.web.title": "Веб-чат для телефона",
  "f.web.desc": "Чат с моделью кластера с телефона в той же Wi‑Fi сети. Откройте ссылку или наведите камеру на QR-код, затем «Добавить на главный экран».",
  "f.web.pin": "PIN",
  "f.web.newPin": "Новый PIN",
  "f.web.url": "Адрес",
  "f.web.noLan": "Не найден адрес в локальной сети — подключите ПК к Wi‑Fi/LAN.",
  "f.web.running": "работает",
  "f.web.stopped": "остановлен",
  "f.web.copy": "Копировать",
  "f.web.firewall": "Если телефон не открывает страницу — разрешите Argus в брандмауэре Windows (частные сети).",
  "f.tg.title": "Telegram-бот",
  "f.tg.desc": "Бот отвечает моделью кластера. Создайте бота у @BotFather и вставьте токен. /new — новый диалог.",
  "f.tg.token": "Токен бота",
  "f.tg.ids": "Разрешённые id (через запятую)",
  "f.tg.idsHint": "Напишите боту — он ответит вашим id, если его нет в списке.",
  "f.tg.warn": "Токен хранится открытым текстом в argus-features.json на этом ПК. Не делитесь этим файлом.",
  "f.tg.status": "Статус",
  "f.route.title": "Маршрутизация запросов",
  "f.route.desc": "Короткие вопросы (до 200 символов, без кода и вложений) отвечает готовый Помощник, остальные — модель кластера. Нужен эксперимент «Роли» и узел с ролью «Помощник».",
  "f.lang.title": "Язык интерфейса",
  "f.lang.desc": "Пока переведены меню и этот экран.",
} as const;

export type I18nKey = keyof typeof ru;

const en: Record<I18nKey, string> = {
  "nav.chat": "Chat",
  "nav.devices": "Cluster",
  "nav.models": "Models",
  "nav.api": "API",
  "nav.parallel": "Test",
  "nav.features": "Features",
  "nav.queue": "Queue",
  "nav.knowledge": "Knowledge",
  "nav.connect": "Connect device",
  "nav.parallelHint": "Parallel test: the main model and Helpers answer the same question",
  "f.title": "Features",
  "f.save": "Save",
  "f.saved": "Saved",
  "f.on": "On",
  "f.off": "Off",
  "f.web.title": "Web chat for your phone",
  "f.web.desc": "Chat with the cluster model from a phone on the same Wi‑Fi. Open the link or scan the QR code, then “Add to Home screen”.",
  "f.web.pin": "PIN",
  "f.web.newPin": "New PIN",
  "f.web.url": "Address",
  "f.web.noLan": "No local network address found — connect this PC to Wi‑Fi/LAN.",
  "f.web.running": "running",
  "f.web.stopped": "stopped",
  "f.web.copy": "Copy",
  "f.web.firewall": "If the phone can't open the page, allow Argus in Windows Firewall (private networks).",
  "f.tg.title": "Telegram bot",
  "f.tg.desc": "The bot answers with the cluster model. Create a bot with @BotFather and paste its token. /new starts a new dialog.",
  "f.tg.token": "Bot token",
  "f.tg.ids": "Allowed ids (comma separated)",
  "f.tg.idsHint": "Message the bot — it replies with your id if you're not on the list.",
  "f.tg.warn": "The token is stored in plain text in argus-features.json on this PC. Don't share that file.",
  "f.tg.status": "Status",
  "f.route.title": "Request routing",
  "f.route.desc": "Short questions (under 200 characters, no code or attachments) go to a ready Helper, the rest to the cluster model. Needs the «Roles» experiment and a node with the Helper role.",
  "f.lang.title": "Interface language",
  "f.lang.desc": "Menus and this screen are translated so far.",
};

const dicts: Record<Lang, Record<I18nKey, string>> = { ru, en };

const initial: Lang = (typeof localStorage !== "undefined" && localStorage.getItem("argus.lang") === "en") ? "en" : "ru";

export const useLang = create<{ lang: Lang; setLang: (l: Lang) => void }>((set) => ({
  lang: initial,
  setLang: (lang) => { try { localStorage.setItem("argus.lang", lang); } catch { /* ignore */ } set({ lang }); },
}));

export function t(key: I18nKey, lang: Lang = useLang.getState().lang): string {
  return dicts[lang][key] ?? ru[key] ?? key;
}

export function useT() {
  const lang = useLang((s) => s.lang);
  return (key: I18nKey) => t(key, lang);
}
