/**
 * I18nProvider — 全局语言上下文（挂在 App 根部，全应用共用一个 locale）
 *
 * 与 `useI18n` 的分工：hook 里只有 context 与类型（无 JSX），Provider 作为组件单独放这里，
 * 免得一个文件同时导出组件与 hook 触发 react-refresh 的"只导出组件"规则。
 */

import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { I18nContext, LOCALE_STORAGE_KEY, type Locale } from '../../hooks/useI18n';

/**
 * 加载某语言的词条。
 *
 * 中文**不读词条** —— 中文原文内联在调用处，读 JSON 反而会造出第二个中文来源、
 * 两边迟早打架（旧词条盖住新文案）。所以只有英文才真的去拉 JSON。
 */
async function loadMessages(locale: Locale): Promise<Record<string, string>> {
  if (locale !== 'en-US') return {};
  try {
    const mod = await import('../../locales/en-US.json');
    return (mod.default ?? {}) as Record<string, string>;
  } catch {
    // 词条加载失败不该让界面挂掉：直接回落中文原文
    return {};
  }
}

function readSavedLocale(): Locale {
  const saved = localStorage.getItem(LOCALE_STORAGE_KEY);
  return saved === 'en-US' ? 'en-US' : 'zh-CN';
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(readSavedLocale);
  const [messages, setMessages] = useState<Record<string, string>>({});

  useEffect(() => {
    let alive = true;
    void loadMessages(locale).then((loaded) => {
      if (alive) setMessages(loaded);
    });
    // <html lang> 跟着语言走：影响字体回退与读屏软件的发音
    document.documentElement.lang = locale;
    return () => {
      alive = false;
    };
  }, [locale]);

  const setLocale = useCallback((next: Locale) => {
    localStorage.setItem(LOCALE_STORAGE_KEY, next);
    setLocaleState(next);
  }, []);

  const t = useCallback(
    (key: string, zh: string, params?: Record<string, string | number>) => {
      let msg = messages[key] || zh;
      if (params) {
        for (const [k, v] of Object.entries(params)) {
          msg = msg.replace(`{${k}}`, String(v));
        }
      }
      return msg;
    },
    [messages],
  );

  const value = useMemo(() => ({ locale, setLocale, t }), [locale, setLocale, t]);
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}
