import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { Users } from 'lucide-react';
import { TitleBar, StatusBar } from '../components/layout';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useAccountStore } from '../stores/accountStore';
import { OrgGovernanceSection } from '../components/enclave/OrgGovernancePanels';
import { EnclaveNavBar } from '../components/enclave/EnclaveNavBar';
import { EnclaveToolbarActions, EnclaveFilesRootPanel } from '../components/enclave/EnclaveToolbar';
import type { MemberView } from '../types/enclave';

/** 治理快照新鲜度上限（秒）：超过即视为「可能过期」，与后端 SNAPSHOT_STALE_MAX_SECS 对齐。 */
const SNAPSHOT_STALE_MAX_SECS = 600;

/** 成员列表每页条数 */
const MEMBER_PAGE_SIZE = 10;

type OrgTab = 'basic' | 'host' | 'security';

const ORG_TABS: { key: OrgTab; label: string }[] = [
  { key: 'basic', label: '基本信息与成员' },
  { key: 'host', label: '主机与连接' },
  { key: 'security', label: '安全与审计' },
];

const ROLE_LABEL: Record<string, string> = {
  owner: '所有者',
  admin: '管理员',
  member: '成员',
};

function roleLabel(role: string): string {
  return ROLE_LABEL[role] ?? role;
}

function seatLabel(status: string): string {
  return status === 'removed' ? '已移除' : status === 'active' ? '在用' : status;
}

/** 新鲜度文案：从未同步或距今超过上限 → 「可能过期（stale）」，否则「同步于 x 分钟前」。 */
function freshnessText(syncedAt: number): string {
  if (syncedAt <= 0) return '尚未同步（可能过期）';
  const age = Math.max(0, Math.floor(Date.now() / 1000) - syncedAt);
  if (age > SNAPSHOT_STALE_MAX_SECS) return '可能过期（stale）';
  const minutes = Math.floor(age / 60);
  return minutes < 1 ? '刚刚同步' : `同步于 ${minutes} 分钟前`;
}

/**
 * 飞地「组织与权限」页 —— 独立路由 `/enclave/org`。
 *
 * 只读展示：组织信息 + 成员表。策略开关 / 应急开关 / 审计流水 / 主机转移随后续里程碑接入。
 */
export function EnclaveOrgPage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();

  const orgs = useEnclaveOrgStore((s) => s.orgs);
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const detail = useEnclaveOrgStore((s) => s.detail);
  const loading = useEnclaveOrgStore((s) => s.loading);
  const error = useEnclaveOrgStore((s) => s.error);
  const lastRefresh = useEnclaveOrgStore((s) => s.lastRefresh);
  // 登录账号变化 → 必须先校验新身份，再重新加载本机团队数据
  const accountKey = useAccountStore((s) => s.account?.email ?? '');
  const account = useAccountStore((s) => s.account);
  const accountLoaded = useAccountStore((s) => s.loaded);
  /** 未登录（登录检查已完成但没有账号）：成员来自平台组织架构，此时不应展示空表 */
  const notLoggedIn = accountLoaded && !account;

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const st = useEnclaveOrgStore.getState();
      if (!(await st.ensureIdentity()) || cancelled) return;
      await st.loadOrgs();
      if (cancelled) return;
      const cur = useEnclaveOrgStore.getState();
      // 本机还没有当前账号的组织快照（新账号 / 刚升级）→ 主动拉一轮
      if (cur.orgs.length === 0 && !cur.lastRefresh) await cur.refresh();
      await useEnclaveOrgStore.getState().loadFreshness();
    })();
    return () => {
      cancelled = true;
    };
  }, [accountKey]);

  // 顶栏组合菜单仍可切到其它模式：切模式后回到主布局（与知识库页同构）
  const handleModeChange = useCallback(
    (mode: ViewMode) => {
      setMode(mode);
      navigate('/');
    },
    [setMode, navigate],
  );

  const org = detail?.org ?? orgs.find((o) => o.orgId === activeOrgId) ?? null;
  const members: MemberView[] = detail?.members ?? [];
  /**
   * 在线成员（主机侧订阅列表）：每 20s 拉一次。
   *
   * 团队数据本身不缓存，在线状态只是主机内存里的连接数；owner 由本机提供主机服务，
   * 因此能读到列表就说明 owner 在线，一并标成在线。
   */
  const [onlineIds, setOnlineIds] = useState<string[]>([]);
  useEffect(() => {
    if (!activeOrgId) {
      setOnlineIds([]);
      return;
    }
    let cancelled = false;
    const load = () => {
      void invoke<string[]>('enclave_online_members', { orgId: activeOrgId })
        .then((ids) => {
          if (!cancelled) setOnlineIds(ids);
        })
        .catch(() => {
          if (!cancelled) setOnlineIds([]);
        });
    };
    load();
    const timer = window.setInterval(load, 20_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [activeOrgId]);
  const onlineSet = new Set(onlineIds);
  // owner 就是主机：能读到在线列表说明主机在线。平台快照未带 ownerUserId 时回退到成员表里的 owner 行
  const ownerId = org?.ownerUserId || members.find((m) => m.role === 'owner')?.userId || '';
  if (ownerId) onlineSet.add(ownerId);
  const [orgTab, setOrgTab] = useState<OrgTab>('basic');
  const [memberPage, setMemberPage] = useState(0);
  /** 本机团队文件根目录面板（全局设置，与任务/知识库页共用） */
  const [rootOpen, setRootOpen] = useState(false);
  const memberPages = Math.max(1, Math.ceil(members.length / MEMBER_PAGE_SIZE));
  const currentMemberPage = Math.min(memberPage, memberPages - 1);
  const pagedMembers = members.slice(
    currentMemberPage * MEMBER_PAGE_SIZE,
    (currentMemberPage + 1) * MEMBER_PAGE_SIZE,
  );
  useEffect(() => {
    setMemberPage(0);
  }, [activeOrgId]);
  // 成员列表拉取失败的原因（平台成员接口属待对齐项）：只提示，不算整轮失败
  const memberWarning = lastRefresh?.memberError ?? null;

  /** 手动刷新：身份 → 平台快照 → 组织/成员/治理。 */
  const refreshOrg = async () => {
    const st = useEnclaveOrgStore.getState();
    if (!(await st.ensureIdentity())) return;
    await useEnclaveOrgStore.getState().refresh();
  };

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        rightPanelOpen={false}
        showBackButton={false}
        enclaveOpen
        onOpenEnclave={() => navigate('/enclave')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenSettings={() => navigate('/settings')}
        onOpenMarket={() => navigate('/market')}
        onOpenInspirations={() => navigate('/inspirations')}
      />

      <div className="flex-1 min-h-0 px-2 flex flex-col overflow-hidden">
        <div
          className="shrink-0 rounded-lg overflow-hidden"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          <EnclaveNavBar
            active="org"
            onRefresh={refreshOrg}
            refreshing={loading}
            right={
              <EnclaveToolbarActions
                rootOpen={rootOpen}
                onToggleRoot={() => setRootOpen((v) => !v)}
              />
            }
          />
        </div>

        {/* 本机团队文件根目录（全局设置；本页没有「新建」，只保留共用动作） */}
        <EnclaveFilesRootPanel open={rootOpen} />

        {/* 页内分组：把 8 个板块收敛为 3 组，避免单页过长
            与一级导航（实心强调色胶囊 + 图标）区分：这里用扁平下划线式 tab，
            选中项为强调色文字 + 2px 下划线，不依赖底色对比 */}
        <div
          role="tablist"
          className="shrink-0 flex items-stretch gap-4 mt-2"
          style={{ borderBottom: '1px solid var(--border)' }}
        >
          {ORG_TABS.map((t) => {
            const on = orgTab === t.key;
            return (
              <button
                key={t.key}
                role="tab"
                aria-selected={on}
                onClick={() => setOrgTab(t.key)}
                className="text-xs px-1 pb-1.5 -mb-px"
                style={{
                  backgroundColor: 'transparent',
                  border: 'none',
                  borderBottom: `2px solid ${on ? 'var(--accent)' : 'transparent'}`,
                  color: on ? 'var(--accent)' : 'var(--text-secondary)',
                  fontWeight: on ? 600 : 400,
                }}
              >
                {t.label}
              </button>
            );
          })}
        </div>

        <div className="flex-1 min-h-0 pt-2 pb-0 flex flex-col gap-2 overflow-auto">
        {error && (
          <div className="text-xs" style={{ color: 'var(--status-danger)' }}>
            {error}
          </div>
        )}

        {orgTab === 'basic' &&
          (notLoggedIn ? (
            <div
              className="rounded-lg p-6 flex flex-col items-center gap-2 text-center"
              style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
            >
              <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                登录后可查看组织与成员
              </span>
              <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                成员名单来自平台组织架构，登录后会同步到本机（只读快照）；未登录时本页不展示成员表。
              </span>
              <button
                onClick={() => navigate('/settings?tab=account')}
                className="pd-btn text-xs px-3 py-1 rounded-lg"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                去登录
              </button>
            </div>
          ) : (
          <>
        {/* 组织信息卡 */}
        {org && (
          <div
            className="shrink-0 rounded-lg p-3 flex flex-col gap-2"
            style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
          >
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
                {org.name || org.orgId}
              </span>
              <span
                className="text-[11px] px-1.5 py-0.5 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
              >
                {org.planKey || '未知档位'}
              </span>
              <span
                className="text-[11px] px-1.5 py-0.5 rounded"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: org.status === 'active' ? 'var(--text-secondary)' : 'var(--status-danger)',
                }}
              >
                {org.status || '未知状态'}
              </span>
              <span
                className="text-[11px] px-1.5 py-0.5 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
                title="当前登录用户在该组织中的角色（来自平台，主机侧只读）"
              >
                我的角色：{roleLabel(org.myRole)}
              </span>
              <span
                className="text-[11px] px-1.5 py-0.5 rounded"
                style={{
                  backgroundColor: 'var(--bg-tertiary)',
                  color: freshnessText(org.syncedAt).includes('stale') || org.syncedAt <= 0
                    ? 'var(--status-danger)'
                    : 'var(--text-tertiary)',
                }}
                title={`synced_at = ${org.syncedAt}`}
              >
                {freshnessText(org.syncedAt)}
              </span>
            </div>
            <div className="flex items-center gap-4 text-xs" style={{ color: 'var(--text-secondary)' }}>
              <span>席位 {org.seats}</span>
              <span>成员 {org.memberCount}</span>
              <span className="flex items-center gap-1">
                <span
                  className="w-1.5 h-1.5 rounded-full"
                  style={{ backgroundColor: 'var(--status-success, #2f9e44)' }}
                />
                在线 {onlineSet.size}
              </span>
            </div>
          </div>
        )}

        {/* 成员表 */}
        <div
          className="shrink-0 rounded-lg flex flex-col overflow-hidden"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          <div
            className="shrink-0 h-9 px-3 flex items-center gap-2"
            style={{ borderBottom: '1px solid var(--border)' }}
          >
            <Users size={13} style={{ color: 'var(--accent)' }} />
            <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
              成员
            </span>
            <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              {members.length}
            </span>
            {memberWarning && (
              <span className="text-[11px]" style={{ color: 'var(--status-danger)' }}>
                {memberWarning}
              </span>
            )}
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-xs" style={{ borderCollapse: 'collapse' }}>
              <thead>
                <tr style={{ color: 'var(--text-tertiary)' }}>
                  <th className="text-left px-3 py-1.5 font-normal">状态</th>
                  <th className="text-left px-3 py-1.5 font-normal">姓名</th>
                  <th className="text-left px-3 py-1.5 font-normal">邮箱</th>
                  <th className="text-left px-3 py-1.5 font-normal">角色</th>
                  <th className="text-left px-3 py-1.5 font-normal">席位状态</th>
                </tr>
              </thead>
              <tbody>
                {pagedMembers.map((m) => {
                  const removed = m.seatStatus === 'removed';
                  return (
                    <tr
                      key={m.userId}
                      style={{
                        color: removed ? 'var(--text-tertiary)' : 'var(--text-secondary)',
                        borderTop: '1px solid var(--border)',
                        opacity: removed ? 0.6 : 1,
                      }}
                    >
                      <td className="px-3 py-1.5">
                        {onlineSet.has(m.userId) ? (
                          <span
                            className="inline-flex items-center gap-1"
                            style={{ color: 'var(--status-success, #2f9e44)' }}
                          >
                            <span
                              className="w-1.5 h-1.5 rounded-full"
                              style={{ backgroundColor: 'var(--status-success, #2f9e44)' }}
                            />
                            在线
                          </span>
                        ) : (
                          <span style={{ color: 'var(--text-tertiary)' }}>—</span>
                        )}
                      </td>
                      <td className="px-3 py-1.5">{m.displayName || m.userId}</td>
                      <td className="px-3 py-1.5">{m.email || '-'}</td>
                      <td className="px-3 py-1.5">{roleLabel(m.role)}</td>
                      <td className="px-3 py-1.5">{seatLabel(m.seatStatus)}</td>
                    </tr>
                  );
                })}
                {members.length === 0 && (
                  <tr>
                    <td
                      colSpan={5}
                      className="px-3 py-4 text-center"
                      style={{ color: memberWarning ? 'var(--status-danger)' : 'var(--text-tertiary)' }}
                    >
                      {memberWarning ?? '暂无成员数据（点击「刷新」从平台拉取）'}
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
          <div
            className="shrink-0 h-8 px-3 flex items-center gap-2 text-[10px]"
            style={{ borderTop: '1px solid var(--border)', color: 'var(--text-tertiary)' }}
          >
            <span>
              {members.length === 0
                ? '0'
                : `${currentMemberPage * MEMBER_PAGE_SIZE + 1}-${Math.min(
                    (currentMemberPage + 1) * MEMBER_PAGE_SIZE,
                    members.length,
                  )}`}{' '}
              / {members.length}
            </span>
            <div className="flex-1" />
            <button
              onClick={() => setMemberPage(currentMemberPage - 1)}
              disabled={currentMemberPage <= 0}
              className="px-2 py-0.5 rounded"
              style={{
                border: '1px solid var(--border)',
                color: 'var(--text-secondary)',
                opacity: currentMemberPage <= 0 ? 0.4 : 1,
              }}
            >
              上一页
            </button>
            <button
              onClick={() => setMemberPage(currentMemberPage + 1)}
              disabled={currentMemberPage >= memberPages - 1}
              className="px-2 py-0.5 rounded"
              style={{
                border: '1px solid var(--border)',
                color: 'var(--text-secondary)',
                opacity: currentMemberPage >= memberPages - 1 ? 0.4 : 1,
              }}
            >
              下一页
            </button>
          </div>
        </div>
          </>
          ))}

        {orgTab === 'host' && <OrgGovernanceSection section="host" />}
        {orgTab === 'security' && <OrgGovernanceSection section="security" />}
        </div>
      </div>

      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
