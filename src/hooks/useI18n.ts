import { useState, useEffect, useCallback } from 'react';

type Locale = 'zh-CN' | 'en-US';

interface I18nStore {
  locale: Locale;
  messages: Record<string, string>;
}

const STORAGE_KEY = 'pilotdesk-locale';

const loadedMessages: Record<Locale, Record<string, string> | null> = {
  'zh-CN': null,
  'en-US': null,
};

async function loadLocale(locale: Locale): Promise<Record<string, string>> {
  if (loadedMessages[locale]) return loadedMessages[locale]!;
  try {
    const mod = await import(`../locales/${locale}.json`);
    loadedMessages[locale] = mod.default;
    return mod.default;
  } catch {
    return {};
  }
}

/**
 * useI18n — 轻量国际化 hook。
 *
 * 使用 JSON 文件存储翻译，通过动态 import 按需加载。
 * 语言偏好持久化到 localStorage。
 */
export function useI18n() {
  const [store, setStore] = useState<I18nStore>({ locale: 'zh-CN', messages: {} });

  useEffect(() => {
    const saved = localStorage.getItem(STORAGE_KEY) as Locale | null;
    const locale = saved || 'zh-CN';
    loadLocale(locale).then((messages) => {
      setStore({ locale, messages });
    });
  }, []);

  const setLocale = useCallback(async (locale: Locale) => {
    localStorage.setItem(STORAGE_KEY, locale);
    const messages = await loadLocale(locale);
    setStore({ locale, messages });
  }, []);

  const t = useCallback((key: string, params?: Record<string, string | number>): string => {
    let msg = store.messages[key] || key;
    if (params) {
      Object.entries(params).forEach(([k, v]) => {
        msg = msg.replace(`{${k}}`, String(v));
      });
    }
    return msg;
  }, [store.messages]);

  return { locale: store.locale, setLocale, t };
}
