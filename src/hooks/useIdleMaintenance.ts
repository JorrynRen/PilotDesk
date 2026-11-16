import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useGeneratingStore } from '../stores/generatingStore';
import { useSessionStore } from '../stores/sessionStore';
import { isWorkflowSession } from '../utils/sessionType';
import { showToast } from '../utils/toast';

/**
 * 闲时自动维护：
 * - 知识：从普通会话的沉淀水位之后取增量，交给知识库模型整理成待确认候选。
 * - 记忆：每天最多触发一次冷记忆清理与配额驱逐。
 *
 * “闲时”不是只看窗口是否失焦：只要本地 Agent、圆桌、工作流或知识投喂队列仍在运行，
 * 就不发起额外的模型请求。每次触发最多处理一个会话，避免后台一次烧掉大量 token。
 */

const IDLE_AFTER_MS = 5 * 60_000;
const TICK_MS = 20_000;
const ATTEMPT_COOLDOWN_MS = 10 * 60_000;
const MEMORY_MAINTENANCE_INTERVAL_SECS = 24 * 60 * 60;
const MEMORY_LAST_RUN_KEY = 'idle_memory_maintenance_at';

interface SedimentOutcome {
  count: number;
  truncated: boolean;
  skipped: string;
}

interface RoomRuntimeRow {
  id: string;
  status: string;
}

/**
 * 这些跳过原因都发生在模型调用之前，可以安全地继续扫描下一个会话。
 * 其余结果（包括 skipped 为空且 count=0）表示本次已经做过一次实际尝试，
 * 必须结束本轮闲时维护，避免一次连续发起多次模型请求。
 */
const PRE_MODEL_SKIPS = new Set(['no_new', 'too_soon', 'busy', 'workflow', 'inactive']);

async function invokeOr<T>(command: string, fallback: T, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (err) {
    console.warn(`[IdleMaintenance] ${command} 检查失败，按未运行处理:`, err);
    return fallback;
  }
}

async function hasBusyRuntime(): Promise<boolean> {
  const generating = useGeneratingStore.getState().generatingMap;
  if (Object.keys(generating).length > 0) {
    return true;
  }

  const [agentSessions, workflowExecutions, knowledgeQueue] = await Promise.all([
    invokeOr<string[]>('agent_running_sessions', []),
    invokeOr<string[]>('list_live_executions', []),
    invokeOr<number>('kb_ingest_queue_count', 0),
  ]);
  if (agentSessions.length > 0 || workflowExecutions.length > 0 || knowledgeQueue > 0) {
    return true;
  }

  const rooms = await invokeOr<RoomRuntimeRow[]>('groupchat_list_rooms', []);
  const runningRooms = rooms.filter((room) => room.status === 'running');
  for (const room of runningRooms) {
    if (await invokeOr<boolean>('groupchat_room_actor_alive', false, { roomId: room.id })) {
      return true;
    }
  }
  return false;
}

async function maintainMemoryOnce(): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  const raw = await invokeOr<string | null>('get_app_setting', null, {
    key: MEMORY_LAST_RUN_KEY,
  });
  const lastRun = Number.parseInt(raw ?? '', 10);
  if (Number.isFinite(lastRun) && now - lastRun < MEMORY_MAINTENANCE_INTERVAL_SECS) {
    return;
  }

  try {
    await invoke<string[]>('run_memory_maintenance');
    await invoke('set_app_setting', {
      key: MEMORY_LAST_RUN_KEY,
      value: now.toString(),
    });
  } catch (err) {
    console.warn('[IdleMaintenance] 记忆维护失败，下次闲时重试:', err);
  }
}

async function sedimentOneSession(): Promise<SedimentOutcome | null> {
  await useSessionStore.getState().refreshSessions();
  const sessions = useSessionStore
    .getState()
    .sessions
    .filter(
      (session) =>
        session.status === 'active'
        && !isWorkflowSession(session.origin)
        && session.messageCount > 0,
    );

  for (const session of sessions) {
    // 模型调用前再确认：用户可能刚恢复操作，或别的入口已经开始生成。
    if (Object.keys(useGeneratingStore.getState().generatingMap).length > 0) {
      return null;
    }
    try {
      const outcome = await invoke<SedimentOutcome>('kb_auto_sediment_session', {
        sessionId: session.id,
      });
      if (!PRE_MODEL_SKIPS.has(outcome.skipped)) {
        return outcome;
      }
    } catch (err) {
      console.warn(`[IdleMaintenance] 会话 ${session.id} 自动沉淀失败:`, err);
      return { count: 0, truncated: false, skipped: 'error' };
    }
  }
  return null;
}

export function useIdleMaintenance(): void {
  useEffect(() => {
    let lastActivityAt = Date.now();
    let lastAttemptAt = 0;
    let busy = false;
    let disposed = false;

    const markActive = () => {
      lastActivityAt = Date.now();
    };
    const onVisibilityChange = () => {
      markActive();
    };

    window.addEventListener('pointerdown', markActive, { passive: true });
    window.addEventListener('keydown', markActive);
    window.addEventListener('wheel', markActive, { passive: true });
    window.addEventListener('touchstart', markActive, { passive: true });
    window.addEventListener('focus', markActive);
    document.addEventListener('visibilitychange', onVisibilityChange);

    const tick = async () => {
      if (disposed || busy) return;
      const now = Date.now();
      if (now - lastActivityAt < IDLE_AFTER_MS || now - lastAttemptAt < ATTEMPT_COOLDOWN_MS) {
        return;
      }

      busy = true;
      let claimedAttempt = false;
      try {
        if (await hasBusyRuntime()) return;
        claimedAttempt = true;
        lastAttemptAt = Date.now();

        await maintainMemoryOnce();

        // 记忆维护很快；若期间用户已回来，则把知识整理留到下一次闲时。
        if (Date.now() - lastActivityAt < IDLE_AFTER_MS) return;

        const outcome = await sedimentOneSession();
        if (outcome?.count) {
          const suffix = outcome.truncated ? '（内容较长，本次只整理了新增部分的前段）' : '';
          showToast(
            `闲时整理完成：${outcome.count} 条知识候选待确认${suffix}`,
            'success',
            12_000,
          );
        }
      } catch (err) {
        console.warn('[IdleMaintenance] 闲时维护失败:', err);
      } finally {
        // 检查运行态时若只是暂忙，不占用 10 分钟冷却；真正发起过一次维护后才进入冷却。
        if (!claimedAttempt) lastAttemptAt = 0;
        busy = false;
      }
    };

    const timer = window.setInterval(() => void tick(), TICK_MS);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      window.removeEventListener('pointerdown', markActive);
      window.removeEventListener('keydown', markActive);
      window.removeEventListener('wheel', markActive);
      window.removeEventListener('touchstart', markActive);
      window.removeEventListener('focus', markActive);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, []);
}
