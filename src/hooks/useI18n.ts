/**
 * i18n — 轻量国际化（Context + 中文内联兜底）
 *
 * 两条设计决定，都是为了"改一次界面不用再同步一次词条"：
 *
 * ① **中文原文就写在调用处**：`t('session.new', '新建会话')`。
 *    所以中文永远正确 —— en-US.json 缺词条时回落到调用处那段中文，而不是回落成
 *    `session.new` 这种原始 key（那比不翻译更糟）。改中文文案只改这一处，不用动任何 JSON。
 *    代价是英文词条要多带一个 key，这是刻意的取舍。
 *
 * ② **必须是 Context，不能是各自 useState 的 hook**：语言是全局状态。独立 hook 每调用一次
 *    就持有一份自己的 state，切换语言时只有"触发切换的那个组件实例"会更新，其余组件
 *    还停在旧语言 —— 这正是一个看似能用、实则错位的设计。
 *
 * 因此 `locales/zh-CN.json` 不再被读取（中文以内联原文为准），只保留 `en-US.json`。
 */

import { createContext, useContext } from 'react';

export type Locale = 'zh-CN' | 'en-US';

/** 语言偏好落 localStorage 的键（沿用既有键名，避免老用户偏好丢失） */
export const LOCALE_STORAGE_KEY = 'pilotdesk-locale';

/** 语言在设置页的展示名：**不翻译**，各语言用自身文字书写 */
export const LOCALE_LABEL: Record<Locale, string> = {
  'zh-CN': '简体中文',
  'en-US': 'English',
};

export const LOCALES: Locale[] = ['zh-CN', 'en-US'];

export interface I18nContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  /**
   * 取词条。
   *
   * @param key  词条键（en-US.json 里的键）
   * @param zh   **中文原文**，内联在调用处；英文缺词条时回落到它
   * @param params `{name}` 形式的插值参数
   */
  t: (key: string, zh: string, params?: Record<string, string | number>) => string;
}

/** 无 Provider 时的兜底（独立渲染某个组件、测试场景）：一律返回中文原文 */
const noop = () => {
  /* 无 Provider 时不持久化，仅保持默认中文 */
};

export const I18nContext = createContext<I18nContextValue>({
  locale: 'zh-CN',
  setLocale: noop,
  t: (_key, zh) => zh,
});

export function useI18n(): I18nContextValue {
  return useContext(I18nContext);
}
