/**
 * 设置 · 个人云同步：开关、同步范围、同步间隔、冲突与失败明细。
 *
 * 从「账号」设置页拆出为独立一级 tab。状态**懒加载**：只有本组件挂载时才请求
 * `cloud_sync_status`（打开设置页但未进入本 tab 时不会请求）。
 *
 * 业务语义保持与后端一致（前端只展示，不改后端行为）：开关默认关闭、仅专业版/团队版
 * 可用（能力门控）、冲突不自动选边、失败不静默。
 */
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  ChevronDown,
  ChevronRight,
  Cloud,
  Laptop,
  Loader2,
  Pencil,
  RefreshCw,
  Trash2,
  Wifi,
  WifiOff,
} from 'lucide-react';

import {
  CAP_CLOUD_SYNC,
  CAP_WORKFLOW_REMOTE_RUN,
  hasCapability,
  useAccountStore,
} from '../../stores/accountStore';
import { UpgradePrompt } from '../common/UpgradePrompt';
import { useCloudSyncStore, type CloudSyncResult } from '../../stores/cloudSyncStore';
import { CLOUD_SYNC_UPGRADE, REMOTE_RUN_UPGRADE } from '../../utils/upgradeOffers';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { confirmDialog } from '../../stores/confirmStore';
import { cloudSyncQuotaSummary } from '../../utils/planText';
import { SettingsButton, SettingsCard, SettingsSection, SettingsToggle } from './index';

/** 自动同步间隔设置键（分钟；与后端 `cloud_sync.rs::SYNC_INTERVAL_KEY` 一致） */
const SYNC_INTERVAL_KEY = 'cloud_sync_interval_minutes';
/** 自动同步间隔可配范围（分钟） */
const SYNC_INTERVAL_MIN = 1;
const SYNC_INTERVAL_MAX = 1440;
const SYNC_INTERVAL_DEFAULT = 10;

/** 同步范围设置键（CSV；与后端 `cloud_sync.rs::SYNC_KINDS_KEY` 一致） */
const SYNC_KINDS_KEY = 'cloud_sync_kinds';
/** 同步范围选项：kind 与展示名（与后端 `KIND_WORKFLOW` / `KIND_INSPIRATION` 一致，顺序固定） */
const SYNC_KIND_OPTIONS: { kind: string; label: string }[] = [
  { kind: 'workflow', label: '工作流' },
  { kind: 'inspiration', label: '灵感' },
];
/** 全部 kind（缺省全开时用） */
const SYNC_KIND_ALL = SYNC_KIND_OPTIONS.map((o) => o.kind);

/**
 * 解析同步范围 CSV：只保留已支持的 kind、去重；缺省 / 空 / 全部非法 → 两项全开。
 * 判据与后端 `cloud_sync.rs::sync_kinds` 保持一致。
 */
function parseSyncKinds(raw: string | null): string[] {
  const picked = new Set(
    (raw ?? '')
      .split(',')
      .map((s) => s.trim())
      .filter((s) => SYNC_KIND_ALL.includes(s)),
  );
  if (picked.size === 0) return [...SYNC_KIND_ALL];
  return SYNC_KIND_ALL.filter((k) => picked.has(k));
}


/** 个人云同步结果摘要：有打包失败 / 冲突对象时带上数量与说明（不静默）。 */
function cloudSyncSummary(r: CloudSyncResult): string {
  let text = `拉取 ${r.pulled} / 推送 ${r.pushed} / 冲突 ${r.conflicts}`;
  if (r.failed && r.failed > 0) {
    const first = r.failures?.[0];
    text += `；${r.failed} 个失败${first ? `（${first}）` : ''}`;
  }
  if (r.conflicts > 0) {
    // 冲突不自动选边：本地改动已留档为本地版本、主对象跟随远端；提示用户可在「版本」中查看并回滚
    text += `。检测到 ${r.conflicts} 个冲突：你的改动已保存为本地版本，可在工作流的「版本」中查看并回滚`;
  }
  if (r.updatedItems && r.updatedItems.length > 0) {
    // 被云端更新（覆盖本地）：本地无改动、直接跟随远端；如实告知本地原内容已备份为本地版本
    text += `；已跟随云端更新 ${r.updatedItems.length} 个工作流（本地原内容已备份为本地版本，可在「版本」中回滚）`;
  }
  return text;
}

interface RemoteDevice {
  registered: boolean;
  id: number | null;
  name: string;
  platform: string;
  appVersion: string;
  online: boolean;
  lastSeenAt: string | null;
  revokedAt: string | null;
  currentDevice: boolean;
}

interface RemoteRun {
  id: number;
  deviceId: number;
  workflowKey: string;
  status: string;
  createdAt: string;
  expiresAt: string;
  claimedAt: string | null;
  finishedAt: string | null;
  resultCode: string | null;
}

const REMOTE_STATUS_LABEL: Record<string, string> = {
  pending: '等待设备领取',
  claimed: '设备已领取',
  running: '执行中',
  succeeded: '已完成',
  failed: '失败',
  expired: '已过期',
  cancelled: '已取消',
};

export function CloudSyncSettings() {
  const account = useAccountStore((s) => s.account);
  const status = useCloudSyncStore((s) => s.status);
  const refresh = useCloudSyncStore((s) => s.refresh);

  // 个人云同步：开关（默认关闭）+ 立即同步 + 上次结果；仅专业版/团队版可用
  const canCloudSync = hasCapability(account, CAP_CLOUD_SYNC);
  const canRemoteRun = hasCapability(account, CAP_WORKFLOW_REMOTE_RUN);
  const syncQuotaSummary = cloudSyncQuotaSummary(account?.quotas);
  const [cloudSyncEnabled, setCloudSyncEnabled] = useState(false);
  const [cloudSyncing, setCloudSyncing] = useState(false);
  // 本次会话最近一轮同步结果（用于「冲突与失败明细」；仅展示，不落库）
  const [lastRun, setLastRun] = useState<CloudSyncResult | null>(null);
  // 冲突与失败明细：默认收起
  const [detailsOpen, setDetailsOpen] = useState(false);

  // 挂载即拉取状态（懒加载：仅本 tab 被激活时才请求）
  useEffect(() => {
    let alive = true;
    void (async () => {
      const s = await refresh();
      if (alive && s) setCloudSyncEnabled(s.enabled);
    })();
    return () => {
      alive = false;
    };
  }, [refresh]);

  // 自动同步间隔（分钟）：读/写主库设置 `cloud_sync_interval_minutes`（1–1440，后端每轮读取即时生效）
  const [syncIntervalMin, setSyncIntervalMin] = useState(SYNC_INTERVAL_DEFAULT);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const raw = await invoke<string | null>('get_app_setting', { key: SYNC_INTERVAL_KEY });
        if (!alive) return;
        const n = raw ? parseInt(raw, 10) : NaN;
        setSyncIntervalMin(
          Number.isFinite(n)
            ? Math.max(SYNC_INTERVAL_MIN, Math.min(SYNC_INTERVAL_MAX, n))
            : SYNC_INTERVAL_DEFAULT,
        );
      } catch {
        /* 忽略：读不到就用默认值展示 */
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  const changeSyncInterval = async (raw: string) => {
    const n = parseInt(raw, 10);
    if (Number.isNaN(n)) return;
    const clamped = Math.max(SYNC_INTERVAL_MIN, Math.min(SYNC_INTERVAL_MAX, n));
    setSyncIntervalMin(clamped);
    try {
      await invoke('set_app_setting', { key: SYNC_INTERVAL_KEY, value: String(clamped) });
    } catch (e) {
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    }
  };

  // 同步范围（逐类开关）：读/写主库设置 `cloud_sync_kinds`（CSV），缺省视为两项全开
  const [syncKinds, setSyncKinds] = useState<string[]>(SYNC_KIND_ALL);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const raw = await invoke<string | null>('get_app_setting', { key: SYNC_KINDS_KEY });
        if (!alive) return;
        setSyncKinds(parseSyncKinds(raw));
      } catch {
        /* 忽略：读不到就按默认全开展示 */
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  const toggleSyncKind = async (kind: string) => {
    const on = syncKinds.includes(kind);
    // 至少留一项：静默的「全关」会被后端按「全部非法」读回全开，故这里直接拦下并回滚
    if (on && syncKinds.length <= 1) {
      showToast('至少保留一项同步范围', 'warning');
      return;
    }
    const next = on ? syncKinds.filter((k) => k !== kind) : [...syncKinds, kind];
    // 写回完整 CSV，且按固定顺序归一（与后端一致）
    const ordered = SYNC_KIND_OPTIONS.map((o) => o.kind).filter((k) => next.includes(k));
    const prev = syncKinds;
    setSyncKinds(ordered);
    try {
      await invoke('set_app_setting', { key: SYNC_KINDS_KEY, value: ordered.join(',') });
    } catch (e) {
      setSyncKinds(prev); // 回滚
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    }
  };

  const toggleCloudSync = async () => {
    const next = !cloudSyncEnabled;
    if (next) {
      const confirmed = await confirmDialog({
        title: '开启个人云同步',
        message:
          '将同步完整工作流定义与灵感。工作流定义可能包含请求地址、脚本和提示内容。\n\n不会同步设置、API Key、Provider 凭据、插件、技能、执行记录、日志或产物。',
        confirmText: '确认开启',
        cancelText: '取消',
        danger: false,
      });
      if (!confirmed) return;
    }
    setCloudSyncEnabled(next);
    setCloudSyncing(true);
    try {
      // 后端在开启时会立即触发一轮同步，结果随返回值给出
      const r = await invoke<CloudSyncResult>('cloud_sync_set_enabled', { enabled: next });
      if (!next) {
        showToast('已关闭个人云同步', 'success');
      } else if (r.error) {
        showToast(r.error, 'error');
      } else {
        showToast(
          `已开启个人云同步（${cloudSyncSummary(r)}）`,
          r.failed && r.failed > 0 ? 'warning' : 'success',
        );
      }
      if (!r.error) setLastRun(r);
      const s = await refresh();
      if (s) setCloudSyncEnabled(s.enabled);
    } catch (e) {
      setCloudSyncEnabled(!next); // 回滚
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    } finally {
      setCloudSyncing(false);
    }
  };

  const syncNow = async () => {
    setCloudSyncing(true);
    try {
      const r = await invoke<CloudSyncResult>('cloud_sync_now');
      // 未登录 / 非付费等级 / 网络失败都不抛异常，错误在 r.error 文案里
      if (r.error) {
        showToast(r.error, 'error');
      } else {
        setLastRun(r);
        showToast(
          `同步完成：${cloudSyncSummary(r)}`,
          r.failed && r.failed > 0 ? 'warning' : 'success',
        );
      }
      await refresh();
    } catch (e) {
      showToast(`同步失败: ${errorMessage(e)}`, 'error');
    } finally {
      setCloudSyncing(false);
    }
  };

  // 明细里是否有可展开的内容（会话最近一轮的冲突 / 失败 / 被云端覆盖，或后端记录的上次结果）
  const hasDetails =
    Boolean(status?.lastResult) ||
    Boolean(lastRun?.conflictItems?.length) ||
    Boolean(lastRun?.failures?.length) ||
    Boolean(lastRun?.updatedItems?.length);

  // ── 远程发起：设备与请求 ───────────────────────────────────────
  const [remoteDevice, setRemoteDevice] = useState<RemoteDevice | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<RemoteDevice[]>([]);
  const [remoteRuns, setRemoteRuns] = useState<RemoteRun[]>([]);
  const [remoteLoading, setRemoteLoading] = useState(false);
  /** 正在改名的设备 id（null = 无）与输入值 */
  const [editingDeviceId, setEditingDeviceId] = useState<number | null>(null);
  const [editingName, setEditingName] = useState('');

  const refreshRemote = async () => {
    if (!canRemoteRun) return;
    setRemoteLoading(true);
    try {
      // 设备列表已包含「当前设备」标记，不再额外调一次状态接口（少一次并发请求）
      const [devices, runs] = await Promise.all([
        invoke<RemoteDevice[]>('remote_devices_list'),
        invoke<RemoteRun[]>('remote_run_list'),
      ]);
      setRemoteDevice(devices.find((d) => d.currentDevice && !d.revokedAt) ?? null);
      setRemoteDevices(devices);
      setRemoteRuns(runs.slice(0, 12));
    } catch (e) {
      showToast(`读取远程设备失败：${errorMessage(e)}`, 'error');
    } finally {
      setRemoteLoading(false);
    }
  };

  useEffect(() => {
    if (canRemoteRun) void refreshRemote();
  }, [canRemoteRun]);

  const registerCurrentDevice = async () => {
    setRemoteLoading(true);
    try {
      const device = await invoke<RemoteDevice>('remote_device_register');
      setRemoteDevice(device);
      await refreshRemote();
      showToast('当前设备已登记，可接收远程工作流', 'success');
    } catch (e) {
      showToast(`登记失败：${errorMessage(e)}`, 'error');
    } finally {
      setRemoteLoading(false);
    }
  };

  /** 保存设备别名：只改云端展示名，令牌与在线状态不变 */
  const saveDeviceName = async (device: RemoteDevice) => {
    if (device.id == null) return;
    const name = editingName.trim();
    if (!name || name === device.name) {
      setEditingDeviceId(null);
      return;
    }
    setRemoteLoading(true);
    try {
      await invoke('remote_device_rename', { deviceId: device.id, name });
      setEditingDeviceId(null);
      await refreshRemote();
      showToast('设备别名已更新', 'success');
    } catch (e) {
      showToast(`改名失败：${errorMessage(e)}`, 'error');
    } finally {
      setRemoteLoading(false);
    }
  };

  /** 删除设备登记记录：删除后该设备停止领取远程工作流，需要时可重新登记 */
  const deleteDevice = async (device: RemoteDevice) => {
    if (device.id == null) return;
    const confirmed = await confirmDialog({
      title: '删除远程设备',
      message: device.currentDevice
        ? `删除「${device.name}」后，本机将停止领取远程工作流，需要时再重新登记。`
        : `删除「${device.name}」后，该设备将不能再接收新的远程工作流。`,
      confirmText: '删除设备',
    });
    if (!confirmed) return;
    setRemoteLoading(true);
    try {
      await invoke('remote_device_delete', { deviceId: device.id });
      await refreshRemote();
      showToast('设备已删除', 'success');
    } catch (e) {
      showToast(`删除失败：${errorMessage(e)}`, 'error');
    } finally {
      setRemoteLoading(false);
    }
  };

  const cancelRemoteRun = async (run: RemoteRun) => {
    try {
      await invoke('remote_run_cancel', { requestId: run.id });
      await refreshRemote();
      showToast('远程请求已取消', 'success');
    } catch (e) {
      showToast(`取消失败：${errorMessage(e)}`, 'error');
    }
  };

  return (
    <div className="settings-page-stack">
      {/* ① 状态概览：开关 / 上次同步 / 结果摘要 */}
      <SettingsSection
        title="个人云同步"
        description={`跨设备同步工作流定义与灵感的最新状态${
          syncQuotaSummary ? `（${syncQuotaSummary}）` : ''
        }。版本历史仅保留在本地；配置、凭据、插件、技能与执行数据不上云。`}
      >
        <SettingsCard>
          <div className="flex items-center gap-3">
            <SettingsToggle
              checked={cloudSyncEnabled}
              onChange={() => void toggleCloudSync()}
              disabled={!canCloudSync || cloudSyncing}
              title={
                !canCloudSync
                  ? '个人云同步为专业版功能'
                  : cloudSyncEnabled
                    ? '点击关闭'
                    : '点击开启'
              }
            />
            <span
              className="text-xs"
              style={{
                color: cloudSyncEnabled ? 'var(--status-success)' : 'var(--text-tertiary)',
              }}
            >
              {cloudSyncEnabled ? '已开启' : '已关闭'}
            </span>
            <span
              className="text-[11px] flex items-center gap-1"
              style={{ color: 'var(--text-secondary)' }}
            >
              <Cloud size={12} />
              已跟踪 {status?.objectCount ?? 0} 个对象
            </span>
          </div>
        </SettingsCard>

        {!canCloudSync ? (
          <UpgradePrompt
            className="mt-2"
            capability={CLOUD_SYNC_UPGRADE.capability}
            plan={CLOUD_SYNC_UPGRADE.plan}
            title={CLOUD_SYNC_UPGRADE.title}
            description={CLOUD_SYNC_UPGRADE.description}
            source="cloud-sync-settings"
          />
        ) : (
          <div className="mt-3 flex items-center gap-3">
            <SettingsButton
              variant="primary"
              icon={
                cloudSyncing ? (
                  <Loader2 size={13} className="animate-spin" />
                ) : (
                  <RefreshCw size={13} />
                )
              }
              onClick={() => void syncNow()}
              disabled={cloudSyncing || !cloudSyncEnabled}
              title={cloudSyncEnabled ? '立即同步一轮' : '已关闭个人云同步'}
            >
              立即同步
            </SettingsButton>
            <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
              {status?.lastAt ? `上次同步：${status.lastAt}` : '尚未同步'}
              {status?.lastResult ? ` · ${status.lastResult}` : ''}
            </span>
          </div>
        )}
      </SettingsSection>

      {/* ② 同步范围：逐类开关（工作流 / 灵感） */}
      <SettingsSection title="同步范围">
        <SettingsCard>
          <div className="space-y-3">
            {SYNC_KIND_OPTIONS.map((opt) => {
              const on = syncKinds.includes(opt.kind);
              return (
                <div key={opt.kind} className="flex items-center gap-3">
                  <SettingsToggle
                    checked={on}
                    onChange={() => void toggleSyncKind(opt.kind)}
                    disabled={!canCloudSync || !cloudSyncEnabled}
                    title={
                      !canCloudSync || !cloudSyncEnabled
                        ? '个人云同步已关闭（或当前等级不支持）'
                        : on
                          ? `点击关闭${opt.label}`
                          : `点击开启${opt.label}`
                    }
                  />
                  <span className="text-xs" style={{ color: 'var(--text-primary)' }}>
                    {opt.label}
                  </span>
                  <span
                    className="text-[11px]"
                    style={{ color: on ? 'var(--status-success)' : 'var(--text-tertiary)' }}
                  >
                    {on ? '已开启' : '已关闭'}
                  </span>
                </div>
              );
            })}
          </div>
          <div className="mt-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
            插件、技能、Agent 配置、API Key、Provider 凭据、执行记录与产物不会同步；至少保留一项。
          </div>
        </SettingsCard>
        {/* 说明：子工作流不是独立同步对象，其内容随所属主工作流一并上传，避免对端重复导入 */}
        <p className="mt-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          子工作流不单独同步，其内容随所属主工作流一并上传（避免重复导入）。
          {status && status.subflowCount > 0
            ? `本机现有 ${status.rootCount} 个主工作流、${status.subflowCount} 个子工作流。`
            : ''}
        </p>

        {/* ③ 同步间隔（分钟）：1–1440，后端每轮读取 → 保存后下一轮生效（无需重启） */}
        <div className="mt-4 pt-3" style={{ borderTop: '1px solid var(--border)' }}>
          <div className="mb-1.5 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
            同步间隔
          </div>
          <div className="flex items-center gap-2">
            <input
              type="number"
              min={SYNC_INTERVAL_MIN}
              max={SYNC_INTERVAL_MAX}
              value={syncIntervalMin}
              disabled={!canCloudSync}
              onChange={(e) => void changeSyncInterval(e.target.value)}
              className="px-2 py-1 rounded-lg text-xs outline-none disabled:opacity-50"
              style={{
                width: 72,
                backgroundColor: 'var(--bg-secondary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
              title="自动同步间隔（分钟），范围 1–1440"
            />
            <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              分钟（1–1440，保存后下一轮生效）
            </span>
          </div>
        </div>

        {/* ④ 冲突与失败明细：默认收起，可展开 */}
        <div className="mt-4 pt-3" style={{ borderTop: '1px solid var(--border)' }}>
          <button
            onClick={() => setDetailsOpen((v) => !v)}
            className="flex items-center gap-1 text-[11px]"
            style={{ color: 'var(--text-secondary)' }}
          >
            {detailsOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            {detailsOpen ? '收起冲突与失败明细' : '展开冲突与失败明细'}
          </button>

          {detailsOpen && (
            <SettingsCard className="mt-2">
              <div className="space-y-2 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                <div>
                  上次结果：{status?.lastResult ? status.lastResult : '暂无记录'}
                </div>

                {lastRun?.conflictItems && lastRun.conflictItems.length > 0 && (
                  <div>
                    本次冲突对象（{lastRun.conflictItems.length}）：你的改动已保存为本地版本，可在工作流「版本」中回滚
                    <ul className="mt-1 list-disc pl-4" style={{ color: 'var(--text-primary)' }}>
                      {lastRun.conflictItems.map((it) => (
                        <li key={it.id}>{it.name}</li>
                      ))}
                    </ul>
                  </div>
                )}

                {lastRun?.failures && lastRun.failures.length > 0 && (
                  <div style={{ color: '#F59E0B' }}>
                    本次失败对象（{lastRun.failures.length}）
                    <ul className="mt-1 list-disc pl-4">
                      {lastRun.failures.map((f, i) => (
                        <li key={i}>{f}</li>
                      ))}
                    </ul>
                  </div>
                )}

                {lastRun?.updatedItems && lastRun.updatedItems.length > 0 && (
                  <div>
                    本次被云端更新（{lastRun.updatedItems.length}）：本地原内容已备份为本地版本
                    <ul className="mt-1 list-disc pl-4" style={{ color: 'var(--text-primary)' }}>
                      {lastRun.updatedItems.map((it) => (
                        <li key={it.id}>{it.name}</li>
                      ))}
                    </ul>
                  </div>
                )}

                {!hasDetails && <div>本次会话暂无冲突或失败记录。</div>}
              </div>
            </SettingsCard>
          )}
        </div>
      </SettingsSection>

      {/* 远程发起：设备登记与请求状态 */}
      <SettingsSection
        title="远程发起工作流"
        description="从其他设备发起工作流。云端只保存设备、工作流标识、状态与结果码，执行过程、日志和产物留在目标设备。"
      >
        {!canRemoteRun ? (
          <UpgradePrompt
            capability={REMOTE_RUN_UPGRADE.capability}
            plan={REMOTE_RUN_UPGRADE.plan}
            title={REMOTE_RUN_UPGRADE.title}
            description={REMOTE_RUN_UPGRADE.description}
            source="remote-run-settings"
          />
        ) : (
          <>
            <SettingsCard>
              <div className="flex min-w-0 flex-1 items-center gap-2">
                <Laptop size={15} style={{ color: 'var(--text-secondary)' }} />
                <div className="min-w-0">
                  <div className="truncate text-xs" style={{ color: 'var(--text-primary)' }}>
                    {remoteDevice?.registered
                      ? remoteDevice.name
                      : '当前设备尚未登记为远程执行节点'}
                  </div>
                  <div className="mt-0.5 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    {remoteDevice?.registered
                      ? `${remoteDevice.platform} · PilotDesk ${remoteDevice.appVersion}`
                      : '登记后可在其他设备选择此设备发起工作流'}
                  </div>
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {remoteDevice?.registered && (
                  <span
                    className="flex items-center gap-1 text-[11px]"
                    style={{
                      color: remoteDevice.online
                        ? 'var(--status-success)'
                        : 'var(--text-tertiary)',
                    }}
                  >
                    {remoteDevice.online ? <Wifi size={12} /> : <WifiOff size={12} />}
                    {remoteDevice.online ? '在线' : '离线'}
                  </span>
                )}
                <SettingsButton
                  icon={
                    remoteLoading ? (
                      <Loader2 size={13} className="animate-spin" />
                    ) : remoteDevice?.registered ? (
                      <RefreshCw size={13} />
                    ) : (
                      <Laptop size={13} />
                    )
                  }
                  onClick={() =>
                    void (remoteDevice?.registered
                      ? refreshRemote()
                      : registerCurrentDevice())
                  }
                  disabled={remoteLoading}
                >
                  {remoteDevice?.registered ? '刷新状态' : '登记当前设备'}
                </SettingsButton>
              </div>
            </SettingsCard>

            {remoteDevices.length > 0 && (
              <SettingsCard className="mt-2">
                <div className="w-full">
                  <div className="mb-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    已登记设备
                  </div>
                  <div className="space-y-1.5">
                    {remoteDevices.map((device) => (
                      <div
                        key={device.id}
                        className="flex items-center gap-2 rounded-md px-2 py-1.5"
                        style={{ backgroundColor: 'var(--bg-tertiary)' }}
                      >
                        <span
                          className="flex h-6 w-6 shrink-0 items-center justify-center rounded"
                          style={{
                            color: 'var(--text-secondary)',
                            backgroundColor: 'var(--bg-secondary)',
                          }}
                        >
                          <Laptop size={13} />
                        </span>
                        <div className="min-w-0 flex-1">
                          {editingDeviceId === device.id ? (
                            <div className="flex items-center gap-1">
                              <input
                                value={editingName}
                                onChange={(e) => setEditingName(e.target.value)}
                                onKeyDown={(e) => {
                                  if (e.key === 'Enter') void saveDeviceName(device);
                                  if (e.key === 'Escape') setEditingDeviceId(null);
                                }}
                                autoFocus
                                maxLength={80}
                                aria-label="设备别名"
                                className="min-w-0 flex-1 rounded px-1.5 py-0.5 text-xs outline-none"
                                style={{
                                  background: 'var(--bg-secondary)',
                                  border: '1px solid var(--border)',
                                  color: 'var(--text-primary)',
                                }}
                              />
                              <button
                                type="button"
                                className="text-[10px]"
                                style={{ color: 'var(--accent)' }}
                                onClick={() => void saveDeviceName(device)}
                              >
                                保存
                              </button>
                              <button
                                type="button"
                                className="text-[10px]"
                                style={{ color: 'var(--text-tertiary)' }}
                                onClick={() => setEditingDeviceId(null)}
                              >
                                取消
                              </button>
                            </div>
                          ) : (
                            <div className="truncate text-xs" style={{ color: 'var(--text-primary)' }}>
                              {device.name}
                              {device.currentDevice ? '（当前设备）' : ''}
                            </div>
                          )}
                          <div
                            className="text-[10px]"
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            {device.platform} · PilotDesk {device.appVersion}
                          </div>
                        </div>
                        <span
                          className="flex items-center gap-1 text-[10px]"
                          style={{
                            color: device.online
                              ? 'var(--status-success)'
                              : 'var(--text-tertiary)',
                          }}
                        >
                          {device.online ? <Wifi size={11} /> : <WifiOff size={11} />}
                          {device.revokedAt ? '已撤销' : device.online ? '在线' : '离线'}
                        </span>
                        {editingDeviceId !== device.id && (
                          <button
                            type="button"
                            className="pd-icon-btn p-1"
                            title="更改设备别名"
                            onClick={() => {
                              setEditingDeviceId(device.id);
                              setEditingName(device.name);
                            }}
                          >
                            <Pencil size={12} />
                          </button>
                        )}
                        <button
                          type="button"
                          className="pd-icon-btn p-1"
                          title="删除设备"
                          onClick={() => void deleteDevice(device)}
                        >
                          <Trash2 size={12} />
                        </button>
                      </div>
                    ))}
                  </div>
                </div>
              </SettingsCard>
            )}

            <SettingsCard className="mt-2">
              <div className="w-full">
                <div className="mb-2 flex items-center justify-between gap-3">
                  <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    最近远程请求
                  </span>
                  <button
                    type="button"
                    className="pd-icon-btn p-1"
                    title="刷新远程请求"
                    onClick={() => void refreshRemote()}
                    disabled={remoteLoading}
                  >
                    <RefreshCw size={12} className={remoteLoading ? 'animate-spin' : ''} />
                  </button>
                </div>
                {remoteRuns.length === 0 ? (
                  <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                    暂无远程请求。工作流列表中的“远程发起”会在这里显示状态。
                  </div>
                ) : (
                  <div className="space-y-1.5">
                    {remoteRuns.map((run) => (
                      <div
                        key={run.id}
                        className="flex items-center gap-2 rounded-md px-2 py-1.5"
                        style={{ backgroundColor: 'var(--bg-tertiary)' }}
                      >
                        <div className="min-w-0 flex-1">
                          <div
                            className="truncate text-xs"
                            style={{ color: 'var(--text-primary)' }}
                          >
                            {run.workflowKey}
                          </div>
                          <div
                            className="text-[10px]"
                            style={{ color: 'var(--text-tertiary)' }}
                          >
                            {REMOTE_STATUS_LABEL[run.status] ?? run.status}
                            {run.resultCode ? ` · ${run.resultCode}` : ''}
                          </div>
                        </div>
                        {run.status === 'pending' && (
                          <button
                            type="button"
                            className="pd-btn px-2 py-1 text-[10px]"
                            style={{
                              border: '1px solid var(--border)',
                              color: 'var(--text-secondary)',
                            }}
                            onClick={() => void cancelRemoteRun(run)}
                          >
                            取消
                          </button>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </SettingsCard>
          </>
        )}
      </SettingsSection>
    </div>
  );
}
