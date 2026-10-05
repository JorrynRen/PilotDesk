/**
 * 设置 · 账号：登录平台账号，查看会员等级与已解锁能力。
 *
 * 未登录 = 免费档：受限功能（如工作流批量导出）的入口会隐藏，这里给出登录入口。
 */
import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Cloud, Loader2, LogIn, LogOut, RefreshCw, UploadCloud } from 'lucide-react';

import { PlanBadge } from '../common/PlanBadge';
import { CAP_CLOUD_SYNC, capabilityLabel, hasCapability, useAccountStore } from '../../stores/accountStore';
import { expiryText } from '../../utils/planText';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { SettingsButton, SettingsCard, SettingsSection } from './index';

/** 「向组织上报用量」开关设置键（与后端 `commands/usage.rs::USAGE_REPORT_ENABLED_KEY` 一致） */
const USAGE_REPORT_ENABLED_KEY = 'usage_report_enabled';
/** 上次上报时间的本地设置键（明文、非敏感，仅用于界面提示） */
const USAGE_REPORT_LAST_KEY = 'usage_report_last_at';

/** 单条上报条目（后端 `UsageItem`） */
interface UsageItem {
  model: string;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  costFen: number;
}

/** 预览结果（后端 `UsageReportPreview`） */
interface UsageReportPreview {
  day: string;
  items: UsageItem[];
}

/** 上报结果（后端 `UsageReportResult`） */
interface UsageReportResult {
  day: string;
  itemCount: number;
  wrote?: boolean;
  ownerType?: string;
  error?: string;
}

/** 云同步一轮结果（后端 `CloudSyncResult`） */
interface CloudSyncResult {
  pulled: number;
  pushed: number;
  conflicts: number;
  error?: string;
}

/** 云同步状态（后端 `CloudSyncStatus`） */
interface CloudSyncStatus {
  enabled: boolean;
  capabilityOk: boolean;
  lastAt: string | null;
  lastResult: string | null;
  objectCount: number;
  cursor: number;
}

/** 组织内角色的中文名；未知角色回退原值 */
function roleLabel(role: string): string {
  const map: Record<string, string> = { owner: '所有者', admin: '管理员', member: '成员' };
  return map[role] ?? role;
}

export function AccountSettings() {
  const { account, loaded, loggingIn, error, login, logout, cancelLogin, openPlatform } =
    useAccountStore();

  // 组织来源：升级 / 续费与账户信息都指到平台的组织页
  const fromOrganization = account?.source === 'organization';

  // 向组织上报用量：开关（默认开启）+ 立即上报 + 上次上报时间
  const [reportEnabled, setReportEnabled] = useState(true);
  const [loadingReportEnabled, setLoadingReportEnabled] = useState(true);
  const [reporting, setReporting] = useState(false);
  const [lastReport, setLastReport] = useState('');

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const [enabledRaw, lastRaw] = await Promise.all([
          invoke<string | null>('get_app_setting', { key: USAGE_REPORT_ENABLED_KEY }),
          invoke<string | null>('get_app_setting', { key: USAGE_REPORT_LAST_KEY }),
        ]);
        if (!alive) return;
        // 缺省 / 非 "0"/"false" = 开启（与后端同一判据）
        const t = (enabledRaw ?? '').trim();
        setReportEnabled(!(t === '0' || t.toLowerCase() === 'false'));
        setLastReport(lastRaw ?? '');
      } catch {
        /* 忽略：读不到就保持默认开启，不让设置页整体失败 */
      } finally {
        if (alive) setLoadingReportEnabled(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  const toggleReport = async () => {
    const next = !reportEnabled;
    setReportEnabled(next);
    try {
      await invoke('set_app_setting', { key: USAGE_REPORT_ENABLED_KEY, value: next ? '1' : '0' });
      showToast(next ? '已开启向组织上报用量' : '已关闭向组织上报用量', 'success');
    } catch (e) {
      setReportEnabled(!next); // 回滚
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    }
  };

  const reportNow = async () => {
    setReporting(true);
    try {
      // 先预览（只读）：确认本次有哪些聚合条目，再上报
      const preview = await invoke<UsageReportPreview>('usage_report_preview');
      const result = await invoke<UsageReportResult>('usage_report_now');
      // 未登录 / 已关闭 / 网络失败都不抛异常，错误在 result.error 文案里
      if (result.error) {
        showToast(result.error, 'error');
        return;
      }
      const when = new Date().toLocaleString();
      setLastReport(when);
      try {
        await invoke('set_app_setting', { key: USAGE_REPORT_LAST_KEY, value: when });
      } catch {
        /* 上次时间只作提示，写失败不影响成功结果 */
      }
      showToast(
        `已上报 ${result.itemCount} 个模型（${preview.day}）`,
        'success',
      );
    } catch (e) {
      showToast(`上报失败: ${errorMessage(e)}`, 'error');
    } finally {
      setReporting(false);
    }
  };

  // 云同步：开关（默认关闭）+ 立即同步 + 上次结果；仅专业版/团队版可用
  const canCloudSync = hasCapability(account, CAP_CLOUD_SYNC);
  const [cloudSyncEnabled, setCloudSyncEnabled] = useState(false);
  const [cloudSyncing, setCloudSyncing] = useState(false);
  const [cloudStatus, setCloudStatus] = useState<CloudSyncStatus | null>(null);

  const refreshCloudStatus = useCallback(async () => {
    try {
      const s = await invoke<CloudSyncStatus>('cloud_sync_status');
      setCloudStatus(s);
      setCloudSyncEnabled(s.enabled);
    } catch {
      /* 忽略：读不到就保持默认关闭，不让设置页整体失败 */
    }
  }, []);

  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const s = await invoke<CloudSyncStatus>('cloud_sync_status');
        if (!alive) return;
        setCloudStatus(s);
        setCloudSyncEnabled(s.enabled);
      } catch {
        /* 忽略：读不到就保持默认关闭，不让设置页整体失败 */
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

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
        showToast(`已开启云同步（拉取 ${r.pulled} / 推送 ${r.pushed} / 冲突 ${r.conflicts}）`, 'success');
      }
      await refreshCloudStatus();
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
        showToast(`同步完成：拉取 ${r.pulled} / 推送 ${r.pushed} / 冲突 ${r.conflicts}`, 'success');
      }
      await refreshCloudStatus();
    } catch (e) {
      showToast(`同步失败: ${errorMessage(e)}`, 'error');
    } finally {
      setCloudSyncing(false);
    }
  };

  return (
    <div className="space-y-6">
      <SettingsSection
        title="平台账号"
        description="登录后可使用会员功能。登录会打开系统浏览器，密码只在平台页面输入，本软件不保存你的密码。"
      >
        {!loaded ? (
          <SettingsCard>
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              正在检查登录状态…
            </span>
          </SettingsCard>
        ) : account ? (
          <SettingsCard>
            <div className="min-w-0">
              <div className="text-xs truncate" style={{ color: 'var(--text-primary)' }}>
                {account.nickname || account.email}
              </div>
              <div className="text-[11px] truncate" style={{ color: 'var(--text-secondary)' }}>
                {account.email}
              </div>
            </div>
            <SettingsButton variant="secondary" icon={<LogOut size={13} />} onClick={() => void logout()}>
              退出登录
            </SettingsButton>
          </SettingsCard>
        ) : (
          <SettingsCard>
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
              {loggingIn ? '请在浏览器中完成登录与授权…' : '未登录'}
            </span>
            {loggingIn ? (
              <SettingsButton
                variant="secondary"
                icon={<Loader2 size={13} className="animate-spin" />}
                onClick={cancelLogin}
              >
                取消
              </SettingsButton>
            ) : (
              <SettingsButton
                variant="primary"
                icon={<LogIn size={13} />}
                onClick={() => void login()}
              >
                登录
              </SettingsButton>
            )}
          </SettingsCard>
        )}

        {error && (
          <p className="mt-2 text-[11px]" style={{ color: '#EF4444' }}>
            {error}
          </p>
        )}
      </SettingsSection>

      {/* 向组织上报用量：默认开启；仅上报聚合数字，不含任何对话内容或提示词 */}
      <SettingsSection
        title="向组织上报用量"
        description="仅上报聚合数字（模型、调用次数、token 数、金额），不包含对话内容或提示词；同一组织内成员的上报汇总为团队用量看板。"
      >
        <SettingsCard>
          <div className="flex items-center gap-3">
            <button
              onClick={() => void toggleReport()}
              disabled={loadingReportEnabled}
              className="relative rounded-full transition-colors shrink-0 disabled:opacity-50"
              style={{
                width: 36,
                height: 20,
                backgroundColor: reportEnabled ? '#22c55e' : 'var(--bg-tertiary)',
                border: '1px solid var(--border)',
              }}
              title={reportEnabled ? '点击关闭' : '点击开启'}
            >
              <div
                className="absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform shadow-sm"
                style={{ left: reportEnabled ? '18px' : '2px' }}
              />
            </button>
            <span
              className="text-xs"
              style={{ color: reportEnabled ? '#22c55e' : 'var(--text-tertiary)' }}
            >
              {loadingReportEnabled ? '读取中…' : reportEnabled ? '已开启' : '已关闭'}
            </span>
          </div>
        </SettingsCard>

        <div className="mt-3 flex items-center gap-3">
          <SettingsButton
            variant="primary"
            icon={
              reporting ? (
                <Loader2 size={13} className="animate-spin" />
              ) : (
                <UploadCloud size={13} />
              )
            }
            onClick={() => void reportNow()}
            disabled={reporting}
            title={reportEnabled ? '立即上报当天用量' : '已关闭上报'}
          >
            立即上报
          </SettingsButton>
          <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
            {lastReport ? `上次上报：${lastReport}` : '尚未上报'}
          </span>
        </div>
      </SettingsSection>

      {/* 云同步：默认关闭；仅同步工作流，不含设置与密钥 */}
      <SettingsSection
        title="云同步"
        description="同步工作流到你的账号，跨设备可用；不含设置与密钥，插件与灵感暂不支持。"
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
                backgroundColor: cloudSyncEnabled ? '#22c55e' : 'var(--bg-tertiary)',
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
              已跟踪 {cloudStatus?.objectCount ?? 0} 个对象
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
              {cloudStatus?.lastAt ? `上次同步：${cloudStatus.lastAt}` : '尚未同步'}
              {cloudStatus?.lastResult ? ` · ${cloudStatus.lastResult}` : ''}
            </span>
          </div>
        )}
      </SettingsSection>

      {account && (
        <SettingsSection
          title="会员等级"
          description="等级决定哪些功能入口可用；到期后自动回到免费档。"
          actions={
            <div className="flex items-center gap-2">
              <SettingsButton
                onClick={() => void openPlatform('portal')}
                title="在浏览器打开官网会员中心（改昵称 / 改密码 / 注销账户）"
              >
                修改账户信息
              </SettingsButton>
              <SettingsButton
                variant="primary"
                onClick={() => void openPlatform(fromOrganization ? 'organizations' : 'upgrade')}
              >
                {fromOrganization ? '组织与订阅' : '升级 / 续费'}
              </SettingsButton>
            </div>
          }
        >
          <SettingsCard>
            <div className="min-w-0 flex items-center gap-2">
              <PlanBadge
                planKey={account.planKey}
                planName={account.planName}
                expired={account.expired}
                stale={account.stale}
              />
              <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                {account.expiresAt ? `到期：${expiryText(account.expiresAt)}` : '长期有效'}
              </span>
            </div>
            {fromOrganization && account.organization && (
              <span className="text-[11px] shrink-0 ml-2" style={{ color: 'var(--text-secondary)' }}>
                权益来源：{account.organization.name}（{roleLabel(account.organization.role)}）
              </span>
            )}
          </SettingsCard>

          {account.stale && (
            <p className="mt-2 text-[11px]" style={{ color: '#F59E0B' }}>
              离线：平台不可达，按上次已知权益显示（7 天宽限内）。
            </p>
          )}

          <div className="mt-2 px-3 py-2.5 rounded-lg" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
            <div className="text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
              已解锁功能
            </div>
            {account.capabilities.length > 0 ? (
              <div className="flex flex-wrap gap-1.5">
                {account.capabilities.map((key) => (
                  <span
                    key={key}
                    className="px-2 py-0.5 rounded text-[11px]"
                    style={{ backgroundColor: 'var(--bg-secondary)', color: 'var(--text-primary)' }}
                  >
                    {capabilityLabel(key)}
                  </span>
                ))}
              </div>
            ) : (
              <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                暂无（免费档）。例如「工作流批量导出」为专业版功能。
              </div>
            )}
          </div>
        </SettingsSection>
      )}
    </div>
  );
}
