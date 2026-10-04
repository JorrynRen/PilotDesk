/**
 * 设置 · 账号：登录平台账号，查看会员等级与已解锁能力。
 *
 * 未登录 = 免费档：受限功能（如工作流导出）的入口会隐藏，这里给出登录入口。
 */
import { Loader2, LogIn, LogOut } from 'lucide-react';

import { PlanBadge } from '../common/PlanBadge';
import { capabilityLabel, useAccountStore } from '../../stores/accountStore';
import { expiryText } from '../../utils/planText';
import { SettingsButton, SettingsCard, SettingsSection } from './index';

export function AccountSettings() {
  const { account, loaded, loggingIn, error, login, logout, cancelLogin, openPlatform } =
    useAccountStore();

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
              <SettingsButton variant="primary" onClick={() => void openPlatform('upgrade')}>
                升级 / 续费
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
                暂无（免费档）。例如「工作流导出」为专业版功能。
              </div>
            )}
          </div>
        </SettingsSection>
      )}
    </div>
  );
}
