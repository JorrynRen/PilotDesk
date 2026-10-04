/**
 * 状态栏（左下角）的账号状态入口。
 *
 * 未登录 → 「登录」；已登录 → 昵称 + 等级徽章（紧凑，**不显示天数**）。
 * 账户信息、等级与到期（含剩余天数）、已解锁能力、升级入口都收进**弹层**，
 * 避免把状态栏挤成一长条。
 */
import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Loader2, LogIn, LogOut } from 'lucide-react';

import { PlanBadge } from '../common/PlanBadge';
import { capabilityLabel, useAccountStore } from '../../stores/accountStore';
import { expiryText } from '../../utils/planText';

export function AccountStatusEntry() {
  const navigate = useNavigate();
  const { account, loaded, loggingIn, login, logout, openPlatform } = useAccountStore();
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

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
    return <span style={{ color: 'var(--text-tertiary)' }}>…</span>;
  }

  if (!account) {
    return (
      <button
        onClick={() => void login()}
        disabled={loggingIn}
        className="pd-btn flex items-center gap-1 rounded transition-colors hover:opacity-80 p-0.5 disabled:opacity-60"
        style={{ color: 'var(--text-secondary)', background: 'transparent' }}
        title={loggingIn ? '请在浏览器中完成登录与授权' : '登录平台账号'}
      >
        {loggingIn ? <Loader2 size={12} className="animate-spin" /> : <LogIn size={12} />}
        <span>{loggingIn ? '等待授权…' : '登录'}</span>
      </button>
    );
  }

  return (
    <div ref={rootRef} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        className="pd-btn flex items-center gap-1.5 rounded transition-colors hover:opacity-80 p-0.5"
        style={{ color: open ? 'var(--accent)' : 'var(--text-secondary)', background: 'transparent' }}
        title="账号与会员"
        aria-expanded={open}
      >
        <span className="truncate" style={{ maxWidth: 110 }}>
          {account.nickname || account.email}
        </span>
        <PlanBadge
          planKey={account.planKey}
          planName={account.planName}
          expired={account.expired}
          stale={account.stale}
        />
      </button>

      {/* 弹层挂在状态栏上方（状态栏在最底部，所以向上弹出） */}
      {open && (
        <div
          className="absolute left-0 bottom-full mb-1.5 w-64 rounded-lg shadow-lg z-50 p-3 text-[11px]"
          style={{
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
            color: 'var(--text-primary)',
          }}
        >
          <div className="flex items-center gap-2">
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
                void openPlatform('upgrade');
              }}
              className="pd-btn px-2 py-1 rounded text-[11px]"
              style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
            >
              升级 / 续费
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
