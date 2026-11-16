import { flushSync } from 'react-dom';
import { useNavigate } from 'react-router-dom';
import type { ReactNode } from 'react';
import type { ViewMode } from '../../TerminalManager';
import { useTerminal } from '../../TerminalManager';
import { PORTAL_TAB_ID, useCustomTabsStore } from '../../stores/customTabsStore';
import { StatusBar } from './StatusBar';
import { TitleBar } from './TitleBar';
import type { StatusHint } from './TitleBar';
import { WorkbenchRail, type WorkbenchAssetKey } from './WorkbenchRail';

interface WorkbenchAssetPageProps {
  children: ReactNode;
  contextLabel: string;
  activeAsset: WorkbenchAssetKey;
  statusHint?: StatusHint | null;
  /** 页面内部已有完整卡片边界时，去掉壳层最外层边框，避免出现双层描边 */
  frameless?: boolean;
}

/**
 * 独立资产页的工作台壳层。
 *
 * 资源市集/灵感库仍是独立路由，但视觉与交互必须与会话/工作流处于同一个工作台：
 * 左栏常驻、顶栏只显示上下文、内容以圆角面板浮在画布上。把这段壳层集中在这里，
 * 避免两个页面继续各自维护一套“独立应用”式布局。
 */
export function WorkbenchAssetPage({
  children,
  contextLabel,
  activeAsset,
  statusHint,
  frameless = false,
}: WorkbenchAssetPageProps) {
  const navigate = useNavigate();
  const { viewMode, setMode } = useTerminal();

  const handleModeChange = (mode: ViewMode) => {
    // 先同步落模式，再发路由 transition；否则低优先级导航可能被模式更新挤掉。
    flushSync(() => setMode(mode));
    navigate('/');
  };

  const openPortal = () => {
    useCustomTabsStore.getState().setActiveTab(PORTAL_TAB_ID);
    handleModeChange('custom');
  };

  return (
    <div className="pilotdesk-window-shell">
      <div className="pilotdesk-window-content flex flex-col h-full">
        <TitleBar
          mode={viewMode}
          onModeChange={handleModeChange}
          workbench
          workbenchContext={contextLabel}
          statusHint={statusHint}
          noActiveSegment
          onOpenKnowledge={() => navigate('/knowledge')}
          onOpenEnclave={() => navigate('/enclave')}
          onOpenSettings={() => navigate('/settings')}
        />

        <div className="workbench-body">
          <WorkbenchRail
            mode={viewMode}
            activeAsset={activeAsset}
            onModeChange={handleModeChange}
            onOpenKnowledge={() => navigate('/knowledge')}
            onOpenEnclave={() => navigate('/enclave')}
            onOpenMarket={() => navigate('/market')}
            onOpenInspirations={() => navigate('/inspirations')}
            onOpenPortal={openPortal}
            onOpenProjects={() => navigate('/projects')}
            onOpenSettings={() => navigate('/settings')}
            sessionListOpen={false}
            onToggleSessionList={() => undefined}
          />

          <div className="workbench-workspace">
            <div
              className={`workbench-surface workbench-page-surface${frameless ? ' is-frameless' : ''}`}
              data-page-asset={activeAsset}
              style={{ backgroundColor: 'var(--bg-content)' }}
            >
              {children}
            </div>
          </div>
        </div>

        <StatusBar
          onOpenSettings={() => navigate('/settings')}
          onOpenEnvSettings={() => navigate('/settings?tab=environment')}
        />
      </div>
    </div>
  );
}
