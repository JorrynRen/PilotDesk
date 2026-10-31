/**
 * useCrashReporter — 崩溃/未捕获异常的上报入口（App 级挂载一次）。
 *
 * 两件事：
 * 1. **启动采集**：后端在 panic 时已把现场写成 `logs/crashes/crash-*.log`，
 *    这里取走未提示过的记录，预填进全局反馈弹窗（`openFeedback`），
 *    由用户确认后提交 —— 不静默上传任何东西。
 * 2. **前端异常落盘**：`window.onerror` / `unhandledrejection` 写到应用日志，
 *    提交反馈时会被「最近错误日志」自动带上，不需要额外的存储。
 *
 * 只做阶段一：本地留存 + 引导提交，不接远端崩溃服务。
 */
import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';

import { useFeedbackStore } from '../stores/feedbackStore';

/** 待提示的崩溃（与后端 `crash::PendingCrash` 同构） */
interface PendingCrash {
  id: string;
  ts: number;
  summary: string;
  detail: string;
}

/** 同一 message 多久内只记一次：出错循环时避免刷爆日志 */
const DEDUPE_WINDOW_MS = 10_000;
const MAX_DEDUPE_ENTRIES = 200;

const lastReported = new Map<string, number>();

function safeStringify(value: unknown): string {
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function reportFrontendError(level: 'error' | 'warn', message: string, stack: string) {
  const key = `${level}:${message}`.slice(0, 200);
  const now = Date.now();
  const last = lastReported.get(key) ?? 0;
  if (now - last < DEDUPE_WINDOW_MS) return;
  if (lastReported.size >= MAX_DEDUPE_ENTRIES) lastReported.clear();
  lastReported.set(key, now);

  // 后端会再做脱敏与截断；这里只兜住 invoke 失败（非 Tauri 环境 / 页面正在卸载）
  void invoke('log_frontend_error', {
    level,
    message: message.slice(0, 1000),
    stack: stack.slice(0, 4000),
  }).catch(() => {});
}

/** 把崩溃记录拼成反馈正文（弹窗侧还会按 500 字上限截断） */
function buildCrashDetail(crash: PendingCrash, total: number): string {
  const lines = [
    '应用上次运行出现未处理的异常（Rust panic），现场已自动记录：',
    crash.summary.slice(0, 200),
    crash.detail,
    total > 1 ? `另有 ${total - 1} 条更早的崩溃记录保存在本地。` : '',
    '完整堆栈已保存在本地 logs/crashes 目录。',
  ];
  return lines.filter(Boolean).join('\n');
}

export function useCrashReporter() {
  // 启动采集：上一次运行留下的崩溃 → 预填反馈弹窗
  useEffect(() => {
    let alive = true;
    void invoke<PendingCrash[]>('crash_take_pending')
      .then((list) => {
        if (!alive || !Array.isArray(list) || list.length === 0) return;
        const newest = list[0];
        useFeedbackStore.getState().openFeedback({
          category: 'crash',
          title: `崩溃报告：${newest.summary}`.slice(0, 80),
          detail: buildCrashDetail(newest, list.length),
          // 日志聚焦到崩溃发生的那一刻附近
          focusTs: newest.ts,
        });
      })
      .catch(() => {
        /* 拿不到崩溃文件不影响正常使用 */
      });
    return () => {
      alive = false;
    };
  }, []);

  // 前端未捕获异常 → 应用日志
  useEffect(() => {
    const onError = (event: ErrorEvent) => {
      // 资源加载失败（<img>/<script> 等）也走 error 事件，但没有 message，跳过
      if (!event.message) return;
      const err: unknown = event.error;
      const stack =
        err instanceof Error && err.stack
          ? err.stack
          : `${event.filename || ''}:${event.lineno || 0}:${event.colno || 0}`;
      reportFrontendError('error', event.message, stack);
    };

    const onRejection = (event: PromiseRejectionEvent) => {
      const reason: unknown = event.reason;
      if (reason instanceof Error) {
        reportFrontendError('error', reason.message || String(reason), reason.stack ?? '');
      } else {
        reportFrontendError('error', safeStringify(reason), '');
      }
    };

    window.addEventListener('error', onError);
    window.addEventListener('unhandledrejection', onRejection);
    return () => {
      window.removeEventListener('error', onError);
      window.removeEventListener('unhandledrejection', onRejection);
    };
  }, []);
}
