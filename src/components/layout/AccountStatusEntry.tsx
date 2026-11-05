/**
 * 顶栏右侧的账号入口（头像）。
 *
 * 未登录 → 登录图标按钮；已登录 → 32px 圆角方形首字母头像（昵称/等级/到期/能力/升级入口全收进弹层）。
 * 尺寸与形状对齐顶栏其余图标按钮（32px、8px 圆角），避免头像比旁边控件小一圈。
 * 注意圆角要显式写 8：本项目 `--radius-lg` 是 12px，顶栏同排按钮都是内联 8px。
 */
import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Loader2, LogIn, LogOut } from 'lucide-react';

import { AccountAvatar } from '../common/AccountAvatar';
import { PlanBadge } from '../common/PlanBadge';
import { capabilityLabel, useAccountStore } from '../../stores/accountStore';
import { expiryText } from '../../utils/planText';

export function AccountStatusEntry() {
  const navigate = useNavigate();
  const { account, loaded, loggingIn, login, logout, openPlatform } = useAccountStore();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  // 组织来源：升级入口指到平台的组织页
  const fromOrganization = account?.source === 'organization';

  // 点击外部 / Esc 收起弹层
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  if (!loaded) {
    return (
      <span
        className="shrink-0 rounded-lg"
        style={{ width: 32, height: 32, alignSelf: 'center', borderRadius: 8, backgroundColor: 'var(--bg-tertiary)' }}
        aria-hidden
      />
    );
  }

  if (!account) {
    return (
      <button
        onClick={() => void login()}
        disabled={loggingIn}
        className="flex items-center justify-center shrink-0 rounded-lg transition-colors hover:bg-[var(--bg-tertiary)] disabled:opacity-60"
        style={{
          width: 32,
          height: 32,
          alignSelf: 'center',
          borderRadius: 8,
          color: 'var(--text-secondary)',
          background: 'var(--bg-tertiary)',
          border: '1px solid var(--border)',
        }}
        title={loggingIn ? '请在浏览器中完成登录与授权' : '登录平台账号'}
      >
        {loggingIn ? <Loader2 size={16} className="animate-spin" /> : <LogIn size={16} />}
      </button>
    );
  }

  return (
    // 外层必须是 flex 容器：全局 `button { display: inline-flex }` 会让直接放在普通 div 里的按钮
    // 变成行内元素，div 被行框撑高约 5px，头像就会比旁边的图标按钮偏高。
    <div ref={rootRef} className="relative flex" style={{ alignSelf: 'center' }}>
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex items-center justify-center shrink-0 rounded-lg transition-opacity hover:opacity-80"
        style={{
          width: 32,
          height: 32,
          alignSelf: 'center',
          borderRadius: 8,
          padding: 0,
          overflow: 'hidden',
          background: 'transparent',
          border: `1px solid ${open ? 'var(--accent)' : 'var(--border)'}`,
        }}
        title="账号与会员"
        aria-label={`账号：${account.nickname || account.email}`}
        aria-expanded={open}
      >
        <AccountAvatar nickname={account.nickname} email={account.email} avatarId={account.avatarId} size={30} radius={7} />
      </button>

      {/* 弹层挂在顶栏下方，右对齐（顶栏在最上方，所以向下弹出） */}
      {open && (
        <div
          className="absolute right-0 top-full mt-1.5 w-64 rounded-lg shadow-lg z-50 p-3 text-[11px]"
          style={{
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
            color: 'var(--text-primary)',
          }}
        >
          <div className="flex items-center gap-2">
            <AccountAvatar nickname={account.nickname} email={account.email} avatarId={account.avatarId} size={28} radius={7} />
            <span className="truncate font-medium">{account.nickname || '（未设昵称）'}</span>
            <PlanBadge
              planKey={account.planKey}
              planName={account.planName}
              expired={account.expired}
              stale={account.stale}
            />
          </div>
          <div className="mt-1 truncate" style={{ color: 'var(--text-secondary)' }}>
            {account.email}
          </div>

          <div className="mt-2 pt-2" style={{ borderTop: '1px solid var(--border)' }}>
            <div style={{ color: 'var(--text-secondary)' }}>
              到期时间：
              {account.expiresAt ? expiryText(account.expiresAt) : '长期有效'}
            </div>
            {fromOrganization && account.organization && (
              <div className="mt-1 truncate" style={{ color: 'var(--text-secondary)' }}>
                来源：{account.organization.name}
              </div>
            )}
            <div className="mt-1" style={{ color: 'var(--text-secondary)' }}>
              已解锁：
              {account.capabilities.length > 0
                ? account.capabilities.map(capabilityLabel).join('、')
                : '无（免费档）'}
            </div>
            {account.stale && (
              <div className="mt-1" style={{ color: '#F59E0B' }}>
                离线：平台不可达，按上次已知权益显示。
              </div>
            )}
          </div>

          <div className="mt-2 pt-2 flex items-center gap-2" style={{ borderTop: '1px solid var(--border)' }}>
            <button
              onClick={() => {
                setOpen(false);
                void openPlatform(fromOrganization ? 'organizations' : 'upgrade');
              }}
              className="pd-btn px-2 py-1 rounded text-[11px]"
              style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
            >
              {fromOrganization ? '组织与订阅' : '升级 / 续费'}
            </button>
            <button
              onClick={() => {
                setOpen(false);
                navigate('/settings?tab=account');
              }}
              className="pd-btn px-2 py-1 rounded text-[11px]"
              style={{ border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
            >
              账号设置
            </button>
            <button
              onClick={() => {
                setOpen(false);
                void logout();
              }}
              className="pd-btn ml-auto flex items-center gap-1 px-2 py-1 rounded text-[11px]"
              style={{ color: 'var(--text-secondary)' }}
            >
              <LogOut size={11} />
              退出
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
