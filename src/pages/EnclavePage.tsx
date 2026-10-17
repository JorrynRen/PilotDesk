import { useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { TitleBar, StatusBar } from '../components/layout';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { TaskWorkspace } from '../components/enclave/TaskWorkspace';

/**
 * 飞地（Enclave）任务工作台 —— 独立路由 `/enclave`。
 *
 * 主链：组织 → 任务与产出 → 自动沉淀进组织知识库 → Agent 可检索。
 * 组织与成员治理在 `/enclave/org`；数据出境闸门 / 应急 / 审计也在那里。
 */
export function EnclavePage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();

  // 顶栏组合菜单仍可切到其它模式：切模式后回到主布局（与知识库页同构）
  const handleModeChange = useCallback(
    (mode: ViewMode) => {
      setMode(mode);
      navigate('/');
    },
    [setMode, navigate],
  );

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        rightPanelOpen={false}
        showBackButton={false}
        enclaveOpen
        onOpenEnclave={() => {
          /* 已在飞地页，无需动作 */
        }}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenSettings={() => navigate('/settings')}
        onOpenMarket={() => navigate('/market')}
        onOpenInspirations={() => navigate('/inspirations')}
      />
      <div className="flex-1 min-h-0 px-2 pt-2 flex flex-col overflow-hidden">
        <div
          className="flex-1 min-h-0 rounded-lg overflow-hidden flex flex-col"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          <TaskWorkspace onOpenOrg={() => navigate('/enclave/org')} />
        </div>
      </div>
      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
