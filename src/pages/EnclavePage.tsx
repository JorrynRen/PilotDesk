import { useCallback, useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { Boxes, Loader2 } from 'lucide-react';
import { TitleBar, StatusBar } from '../components/layout';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { errorMessage } from '../utils/errorMessage';
import type { EnclaveStatus } from '../types/enclave';

/**
 * 飞地（Enclave）工作台 —— 独立路由 `/enclave`。
 *
 * 骨架阶段：只确认模块已就绪（`enclave.db` 生成且表齐）。
 * 任务树 / 详情 / 组织与权限（`/enclave/org`）随后续里程碑补齐。
 */
export function EnclavePage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();

  const [status, setStatus] = useState<EnclaveStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const s = await invoke<EnclaveStatus>('enclave_status');
        if (alive) setStatus(s);
      } catch (e) {
        if (alive) setError(errorMessage(e));
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

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
      <div className="flex-1 min-h-0 px-2 flex items-center justify-center">
        <div
          className="w-[420px] rounded-lg p-6 flex flex-col items-center gap-3"
          style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
        >
          <Boxes size={22} style={{ color: 'var(--accent)' }} />
          <div className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
            飞地
          </div>
          <div className="text-xs text-center leading-5" style={{ color: 'var(--text-tertiary)' }}>
            内网小团队协作工作台：任务与产出自动沉淀进组织知识库，Agent 可检索。
          </div>

          {error && (
            <div className="text-xs text-center" style={{ color: 'var(--status-danger)' }}>
              读取模块状态失败：{error}
            </div>
          )}
          {!error && !status && (
            <div className="flex items-center gap-2 text-xs" style={{ color: 'var(--text-tertiary)' }}>
              <Loader2 size={12} className="animate-spin" />
              正在检查模块状态…
            </div>
          )}
          {status && (
            <div
              className="w-full rounded p-3 text-[11px] leading-5 font-mono break-all"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
            >
              <div>数据库：{status.dbPath}</div>
              <div>结构版本：v{status.schemaVersion}</div>
            </div>
          )}

          <div className="text-[11px] text-center" style={{ color: 'var(--text-tertiary)' }}>
            任务工作台与「组织与权限」将在后续版本接入
          </div>
        </div>
      </div>
      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
