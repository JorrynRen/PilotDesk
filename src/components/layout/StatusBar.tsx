import { Loader2, Settings } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useEnvInfo, type AgentDetectStatus } from '../../hooks/useEnvInfo';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { useI18n } from '../../hooks/useI18n';
import type { AgentConfig } from '../../types';
import { AccountStatusEntry } from './AccountStatusEntry';

// 版本号单一来源：由 vite.config.ts 从根 package.json 注入（见该文件注释）
const APP_VERSION = import.meta.env.VITE_APP_VERSION as string;

interface StatusBarProps {
  onOpenSettings?: () => void;
  onOpenEnvSettings?: () => void;
}

export function StatusBar({ onOpenSettings, onOpenEnvSettings }: StatusBarProps) {
  const navigate = useNavigate();
  const { envInfo, agentStatus } = useEnvInfo();
  const { agents } = useAgentRegistry();
  const { t } = useI18n();

  // Show all enabled agents from DB, sorted by sortOrder
  const enabledAgents = agents
    .filter(a => a.isEnabled)
    .sort((a, b) => (a.sortOrder ?? 0) - (b.sortOrder ?? 0));

  // Determine per-agent display state using the agent's own color from DB
  const getAgentState = (agent: AgentConfig, version: string | null): {
    status: AgentDetectStatus;
    label: string;
    dotColor: string;
  } => {
    const status = agentStatus[agent.agentType];
    if (status === 'done' && version) {
      return { status: 'done', label: version, dotColor: agent.color || '#9CA3AF' };
    }
    if (status === 'error' || (status === 'done' && !version)) {
      return { status: 'error', label: t('statusBar.notInstalled', '未安装'), dotColor: '#9CA3AF' };
    }
    // pending or detecting — colon only, spinner indicates progress
    return { status: 'detecting', label: '', dotColor: '#9CA3AF' };
  };

  // 聚合展示所需的派生状态：逐个判定后汇总；版本号只进 tooltip，不常驻占位
  const agentRows = enabledAgents.map((agent) => ({
    displayName: agent.displayName || agent.agentType,
    ...getAgentState(agent, envInfo?.agentVersions?.[agent.agentType] ?? null),
  }));
  const detecting = agentRows.some((r) => r.status === 'detecting');
  const notReady = agentRows.filter((r) => r.status === 'error');
  const agentTooltip = [
    ...agentRows.map(
      (r) => `${r.displayName}：${r.status === 'detecting' ? '检测中' : r.label || '就绪'}`,
    ),
    t('statusBar.viewEnv', '点击查看环境检测'),
  ].join('　');

  return (
    <footer
      className="flex items-center justify-between px-4 h-8 text-[10px] shrink-0 select-none"
      // 壳层风格：状态栏同顶栏——去掉底色与上边框，并入壳层那片底
      style={{ color: 'var(--text-secondary)' }}
    >
      <div className="flex items-center gap-3">
        {/* 最左端：账号状态（未登录=登录入口；已登录=昵称+等级徽章，详情在弹层）。
            设置按钮已移到顶部 TitleBar 的功能按钮区。 */}
        <AccountStatusEntry />
        {/* 分隔符（账号 vs Agent 状态区） */}
        <div className="w-px h-3" style={{ backgroundColor: 'var(--border)' }} />
        <span className="flex items-center gap-1">{t('statusBar.agentLabel', 'Agent')}:</span>
        {enabledAgents.length === 0 && (
          <button
            onClick={() => navigate('/settings?tab=agents')}
            className="pd-btn flex items-center gap-1 transition-colors hover:opacity-80"
            title={t('statusBar.openAgentSettings', '点击前往设置页配置 Agent 集成')}
            style={{ color: 'var(--text-tertiary)' }}
          >
            <Settings size={10} />
            {t('statusBar.notIntegrated', '未安装或未集成配置')}
          </button>
        )}
        {/* 聚合展示：不逐个列 Agent（多了会横向挤压，且版本号常驻价值低），
            只在异常时用「N 个未安装」提示；名称与版本收进 hover，点击进环境检测。 */}
        {agentRows.length > 0 && (
          <button
            onClick={() => {
              (onOpenEnvSettings ?? onOpenSettings)?.();
            }}
            className="pd-btn flex items-center gap-1 transition-colors hover:opacity-80"
            title={agentTooltip}
            style={{ color: notReady.length > 0 ? '#F59E0B' : 'var(--text-tertiary)' }}
          >
            {detecting ? (
              <Loader2 size={10} className="animate-spin" style={{ color: '#9CA3AF' }} />
            ) : (
              <span
                className="w-1.5 h-1.5 rounded-full"
                style={{ backgroundColor: notReady.length > 0 ? '#F59E0B' : '#22C55E' }}
              />
            )}
            {detecting
              ? t('statusBar.detecting', '检测中…')
              : notReady.length > 0
                ? t('statusBar.notReady', `${notReady.length} 个未安装`, { count: notReady.length })
                : t('statusBar.agentsReady', '就绪')}
          </button>
        )}
      </div>
      <div className="flex items-center gap-2">
        <span style={{ color: 'var(--text-tertiary)' }}>PilotDesk v{APP_VERSION}</span>
      </div>
    </footer>
  );
}
