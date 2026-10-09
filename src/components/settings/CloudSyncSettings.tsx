/**
 * 设置 · 我的云同步：开关、同步范围、同步间隔、冲突与失败明细。
 *
 * 从「账号」设置页拆出为独立一级 tab。状态**懒加载**：只有本组件挂载时才请求
 * `cloud_sync_status`（打开设置页但未进入本 tab 时不会请求）。
 *
 * 业务语义保持与后端一致（前端只展示，不改后端行为）：开关默认关闭、仅专业版/团队版
 * 可用（能力门控）、冲突不自动选边、失败不静默。
 */
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChevronDown, ChevronRight, Cloud, Loader2, RefreshCw } from 'lucide-react';

import { CAP_CLOUD_SYNC, hasCapability, useAccountStore } from '../../stores/accountStore';
import { useCloudSyncStore, type CloudSyncResult } from '../../stores/cloudSyncStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { SettingsButton, SettingsCard, SettingsSection } from './index';

/** 自动同步间隔设置键（分钟；与后端 `cloud_sync.rs::SYNC_INTERVAL_KEY` 一致） */
const SYNC_INTERVAL_KEY = 'cloud_sync_interval_minutes';
/** 自动同步间隔可配范围（分钟） */
const SYNC_INTERVAL_MIN = 1;
const SYNC_INTERVAL_MAX = 1440;
const SYNC_INTERVAL_DEFAULT = 10;

/** 云同步结果摘要：有打包失败 / 冲突对象时带上数量与说明（不静默）。 */
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

export function CloudSyncSettings() {
  const account = useAccountStore((s) => s.account);
  const status = useCloudSyncStore((s) => s.status);
  const refresh = useCloudSyncStore((s) => s.refresh);

  // 云同步：开关（默认关闭）+ 立即同步 + 上次结果；仅专业版/团队版可用
  const canCloudSync = hasCapability(account, CAP_CLOUD_SYNC);
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

  const toggleCloudSync = async () => {
    const next = !cloudSyncEnabled;
    setCloudSyncEnabled(next);
    setCloudSyncing(true);
    try {
      // 后端在开启时会立即触发一轮同步，结果随返回值给出
      const r = await invoke<CloudSyncResult>('cloud_sync_set_enabled', { enabled: next });
      if (!next) {
        showToast('已关闭云同步', 'success');
      } else if (r.error) {
        showToast(r.error, 'error');
      } else {
        showToast(
          `已开启云同步（${cloudSyncSummary(r)}）`,
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

  return (
    <div className="space-y-6">
      {/* ① 状态概览：开关 / 上次同步 / 结果摘要 */}
      <SettingsSection
        title="云同步"
        description="同步不含设置与密钥，工作流跨设备可用。"
      >
        <SettingsCard>
          <div className="flex items-center gap-3">
            <button
              onClick={() => void toggleCloudSync()}
              disabled={!canCloudSync || cloudSyncing}
              className="relative rounded-full transition-colors shrink-0 disabled:opacity-50"
              style={{
                width: 36,
                height: 20,
                backgroundColor: cloudSyncEnabled ? '#22c55e' : 'var(--bg-field)',
                border: '1px solid var(--border)',
              }}
              title={!canCloudSync ? '云同步为专业版功能' : cloudSyncEnabled ? '点击关闭' : '点击开启'}
            >
              <div
                className="absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform shadow-sm"
                style={{ left: cloudSyncEnabled ? '18px' : '2px' }}
              />
            </button>
            <span
              className="text-xs"
              style={{ color: cloudSyncEnabled ? '#22c55e' : 'var(--text-tertiary)' }}
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
          <p className="mt-2 text-[11px]" style={{ color: '#F59E0B' }}>
            云同步为专业版功能，当前等级不含此能力
          </p>
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
              title={cloudSyncEnabled ? '立即同步一轮' : '已关闭云同步'}
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

      {/* ② 同步范围：当前仅工作流；插件与灵感待做 */}
      <SettingsSection title="同步范围">
        <SettingsCard>
          <div className="text-xs" style={{ color: 'var(--text-primary)' }}>
            当前支持：工作流
          </div>
          <div className="mt-1 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
            插件与灵感暂不支持（待后续版本）。
          </div>
        </SettingsCard>
        {/* 说明：子工作流不是独立同步对象，其内容随所属主工作流一并上传，避免对端重复导入 */}
        <p className="mt-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          子工作流不单独同步，其内容随所属主工作流一并上传（避免重复导入）。
          {status && status.subflowCount > 0
            ? `本机现有 ${status.rootCount} 个主工作流、${status.subflowCount} 个子工作流。`
            : ''}
        </p>
      </SettingsSection>

      {/* ③ 同步间隔（分钟）：1–1440，后端每轮读取 → 保存后下一轮生效（无需重启） */}
      <SettingsSection title="同步间隔">
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
      </SettingsSection>

      {/* ④ 冲突与失败明细：默认收起，可展开 */}
      <SettingsSection title="冲突与失败明细">
        <button
          onClick={() => setDetailsOpen((v) => !v)}
          className="flex items-center gap-1 text-[11px]"
          style={{ color: 'var(--text-secondary)' }}
        >
          {detailsOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
          {detailsOpen ? '收起明细' : '展开明细'}
        </button>

        {detailsOpen && (
          <SettingsCard>
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
      </SettingsSection>
    </div>
  );
}
