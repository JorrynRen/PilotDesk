/**
 * acpModeStore — ACP 宿主会话模式（`modes`）的缓存与用户选择。
 *
 * 背景：ACP 的权限档位由**宿主**定义（claude 5 档、codex 4 档、hermes 3 档），
 * 平台不再自带一套安全模式去映射它们，而是把宿主的模式列表原样拉过来供用户选择；
 * 只有宿主完全不提供模式时，才回退到本应用的安全模式（兜底审批策略）。
 *
 * 数据分两类：
 * - 模式列表与当前值：每轮建/恢复会话时由后端回传（`agent-session-modes`）。本身是运行时
 *   数据，但**按 Agent 再存一份展示缓存**：重启后没人回传，输入栏与工作流节点面板会退回
 *   "从没连过宿主"的置灰态，逼用户再开一次会话。缓存只用于渲染候选，id 合法性仍由后端在
 *   `session/set_mode` 时校验（失效只告警并回落跟随宿主），因此缓存过期不会造成错配；
 * - 用户选择（pin）：必须持久化——实测 codex-acp 的 `set_mode` 不跨进程持久，
 *   而 ACP 进程是每轮重建的，不记住用户选择就等于每轮都回到宿主默认。
 */
import { create } from 'zustand';

export interface AcpModeOption {
  id: string;
  name: string;
  description?: string | null;
}

export interface AcpModeSnapshot {
  modes: AcpModeOption[];
  currentModeId: string | null;
}

const PIN_STORAGE_KEY = 'pilotdesk.acp-mode.pins';

function loadPins(): Record<string, string> {
  try {
    const raw = localStorage.getItem(PIN_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, string>) : {};
  } catch {
    return {};
  }
}

function savePins(pins: Record<string, string>): void {
  try {
    localStorage.setItem(PIN_STORAGE_KEY, JSON.stringify(pins));
  } catch {
    /* 配额/隐私模式失败可忽略：只是下次要重选 */
  }
}

const SNAPSHOT_STORAGE_KEY = 'pilotdesk.acp-mode.snapshots';

/**
 * 读取按 Agent 缓存的宿主模式集（展示用）。
 *
 * 解析必须是防御式的：这份数据可能被旧版本、手工改动或半写入破坏，
 * 坏数据只能导致"回落成未得知"，绝不能让整个 store 初始化抛错。
 * 空数组是**有效**缓存（= 已确认该宿主不提供模式），与"未得知"区分开。
 */
function loadAgentSnapshots(): Record<string, AcpModeSnapshot> {
  try {
    const raw = localStorage.getItem(SNAPSHOT_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== 'object') return {};
    const out: Record<string, AcpModeSnapshot> = {};
    for (const [agentType, value] of Object.entries(parsed as Record<string, unknown>)) {
      const snap = value as { modes?: unknown; currentModeId?: unknown } | null;
      if (!snap || !Array.isArray(snap.modes)) continue;
      const modes = snap.modes
        .filter((m): m is AcpModeOption => !!m && typeof (m as AcpModeOption).id === 'string')
        .map((m) => ({ id: m.id, name: m.name || m.id, description: m.description ?? null }));
      out[agentType] = {
        modes,
        currentModeId: typeof snap.currentModeId === 'string' ? snap.currentModeId : null,
      };
    }
    return out;
  } catch {
    return {};
  }
}

function saveAgentSnapshots(byAgent: Record<string, AcpModeSnapshot>): void {
  try {
    localStorage.setItem(SNAPSHOT_STORAGE_KEY, JSON.stringify(byAgent));
  } catch {
    /* 配额/隐私模式失败可忽略：仅影响下次启动的首屏候选 */
  }
}

/** 会话级 key（用户在某条会话里选过） */
function sessionKey(sessionId: string): string {
  return `session:${sessionId}`;
}

/** Agent 级 key（草稿/新建会话在拿到会话 id 前使用） */
function agentKey(agentType: string): string {
  return `agent:${agentType}`;
}

interface AcpModeState {
  /** 会话 → 最近一次宿主返回的模式集与当前值 */
  bySession: Record<string, AcpModeSnapshot>;
  /** Agent → 最近见过的模式集（供新建会话首轮前渲染选择器；落一份展示缓存，重启后仍可渲染） */
  byAgent: Record<string, AcpModeSnapshot>;
  /** 用户固定的模式 id（`session:<id>` / `agent:<type>`）；不存在 = 跟随宿主 */
  pins: Record<string, string>;

  /** 收到一轮模式回传（空列表也要记：用于区分"宿主不提供模式"与"尚未得知"） */
  record: (sessionId: string, agentType: string, modes: AcpModeOption[], currentModeId: string | null) => void;
  /** 固定/取消固定（`modeId = null` → 跟随宿主） */
  pin: (sessionId: string | null, agentType: string, modeId: string | null) => void;
  /** 本会话应写回的固定模式：会话级优先，其次 Agent 级（草稿/新会话） */
  resolvePinned: (sessionId: string | null, agentType: string) => string | null;
  /** 渲染用快照：会话级优先，其次 Agent 级；`undefined` = 尚未得知 */
  snapshotFor: (sessionId: string | null, agentType: string) => AcpModeSnapshot | undefined;
}

export const useAcpModeStore = create<AcpModeState>((set, get) => ({
  bySession: {},
  byAgent: loadAgentSnapshots(),
  pins: loadPins(),

  record: (sessionId, agentType, modes, currentModeId) => {
    const snapshot: AcpModeSnapshot = { modes, currentModeId };
    const state = get();
    // 同步一份到 Agent 级：新建会话首轮前没有会话 id，只能靠它渲染列表；
    // 同时落展示缓存，使重启后无需再开一次会话就能看到宿主模式。
    const nextByAgent = agentType ? { ...state.byAgent, [agentType]: snapshot } : state.byAgent;
    if (agentType) saveAgentSnapshots(nextByAgent);
    set({
      bySession: sessionId ? { ...state.bySession, [sessionId]: snapshot } : state.bySession,
      byAgent: nextByAgent,
    });
    // 用户固定的 id 若已不在最新列表里（适配器升级导致改名），自动退回"跟随宿主"，
    // 否则每轮都会打一条"模式不存在"的告警，且 UI 会显示一个不存在的选项。
    const pins = get().pins;
    const keys = [sessionId ? sessionKey(sessionId) : '', agentType ? agentKey(agentType) : ''].filter(Boolean);
    if (modes.length > 0) {
      const valid = new Set(modes.map((m) => m.id));
      let changed = false;
      const nextPins = { ...pins };
      for (const key of keys) {
        const pinned = nextPins[key];
        if (pinned && !valid.has(pinned)) {
          delete nextPins[key];
          changed = true;
          console.warn('[acpMode] 用户选择的模式已不在宿主列表中，退回跟随宿主:', pinned);
        }
      }
      if (changed) {
        savePins(nextPins);
        set({ pins: nextPins });
      }
    }
  },

  pin: (sessionId, agentType, modeId) => {
    const key = sessionId ? sessionKey(sessionId) : agentType ? agentKey(agentType) : '';
    if (!key) return;
    const nextPins = { ...get().pins };
    if (modeId) nextPins[key] = modeId;
    else delete nextPins[key];
    savePins(nextPins);
    set({ pins: nextPins });
  },

  resolvePinned: (sessionId, agentType) => {
    const { pins } = get();
    if (sessionId && pins[sessionKey(sessionId)]) return pins[sessionKey(sessionId)];
    if (agentType && pins[agentKey(agentType)]) return pins[agentKey(agentType)];
    return null;
  },

  snapshotFor: (sessionId, agentType) => {
    const { bySession, byAgent } = get();
    if (sessionId && bySession[sessionId]) return bySession[sessionId];
    if (agentType && byAgent[agentType]) return byAgent[agentType];
    return undefined;
  },
}));
