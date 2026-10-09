/**
 * WorkflowEditorPage — 工作流编辑器页面
 *
 * 独立路由页面，从 URL query 读取 definitionId。
 * 无 ID 时自动创建新工作流并跳转。
 * 使用统一 TitleBar + StatusBar 布局。
 */

import { useEffect, useState, useCallback, useRef } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { TitleBar, StatusBar } from '../components/layout';
import type { StatusHint } from '../components/layout';
import { WorkflowEditor } from '../components/workflow/WorkflowEditor';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';
import { useTerminal, type ViewMode } from '../TerminalManager';
import { errorMessage } from '../utils/errorMessage';

export function WorkflowEditorPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const definitionId = searchParams.get('id');
  const { createDefinition, loadDefinitions, updateDefinition, selectDefinition, definitions } = useWorkflowStore();
  const [readyId, setReadyId] = useState<string | null>(definitionId);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [workflowName, setWorkflowName] = useState('');
  const [statusHint, setStatusHint] = useState<StatusHint | null>(null);
  const statusTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 同步工作流名称：仓库里的定义名 → 本地输入框。用「渲染期修正」（React adjust-during-render）而不是 effect ——
  // 后者是「effect 体内同步 setState」，会多一轮级联渲染（`react-hooks/set-state-in-effect`），最终渲染结果一致。
  // 判据取「上次见过的 (definitions, readyId)」对，等价于原 effect 的依赖数组。
  const [nameSyncSource, setNameSyncSource] = useState<{ defs: typeof definitions; id: string | null }>({
    defs: definitions,
    id: readyId,
  });
  if (nameSyncSource.defs !== definitions || nameSyncSource.id !== readyId) {
    setNameSyncSource({ defs: definitions, id: readyId });
    const def = readyId ? definitions.find((d) => d.id === readyId) : undefined;
    if (def) setWorkflowName(def.name);
  }

  /** 更新状态提示（autoDismiss毫秒后自动清除） */
  const updateStatusHint = useCallback((hint: StatusHint) => {
    if (statusTimerRef.current) {
      clearTimeout(statusTimerRef.current);
      statusTimerRef.current = null;
    }
    setStatusHint(hint);
    if (hint.autoDismiss && hint.autoDismiss > 0) {
      statusTimerRef.current = setTimeout(() => {
        setStatusHint(null);
        statusTimerRef.current = null;
      }, hint.autoDismiss);
    }
  }, []);

  const handleNameChange = useCallback((name: string) => {
    setWorkflowName(name);
  }, []);

  // 无 ID 时自动创建新工作流
  useEffect(() => {
    if (definitionId) return;
    if (readyId) return;
    if (creating) return;

    let cancelled = false;

    (async () => {
      // `creating` 的置位放进异步体内：effect 体内同步 setState 会多一轮级联渲染
      //（`react-hooks/set-state-in-effect`），放到这里时序不变（异步体首句仍是同步执行）。
      setCreating(true);
      try {
        await loadDefinitions();
        const def = createDefaultWorkflow('新工作流');
        const id = await createDefinition(def);
        selectDefinition(id);
        if (!cancelled) {
          navigate(`/workflow/editor?id=${id}`, { replace: true });
          setReadyId(id);
          updateStatusHint({ state: 'ready' });
        }
      } catch (err) {
        if (!cancelled) {
          setError(errorMessage(err));
          updateStatusHint({ state: 'error' });
        }
      } finally {
        if (!cancelled) {
          setCreating(false);
        }
      }
    })();

    return () => { cancelled = true; };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // 组件卸载时清理定时器
  useEffect(() => {
    return () => {
      if (statusTimerRef.current) {
        clearTimeout(statusTimerRef.current);
        statusTimerRef.current = null;
      }
    };
  }, []);

  // 自动保存工作流名称修改（防抖500ms）
  const autoSaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!readyId || !workflowName || !definitions.length) return;
    const def = definitions.find(d => d.id === readyId);
    if (!def || def.name === workflowName) return;
    if (autoSaveTimerRef.current) clearTimeout(autoSaveTimerRef.current);
    autoSaveTimerRef.current = setTimeout(async () => {
      // 「保存中」提示随真正的保存开始出现（原先是按键时刻同步 set）；effect 体内同步 setState
      // 会多一轮级联渲染（`react-hooks/set-state-in-effect`）。
      updateStatusHint({ state: 'saving', text: '自动保存名称...' });
      try {
        await updateDefinition(readyId, { name: workflowName });
        updateStatusHint({ state: 'saved', text: '名称已保存', autoDismiss: 5000 });
      } catch {
        updateStatusHint({ state: 'save-error', text: '名称保存失败', autoDismiss: 5000 });
      }
      autoSaveTimerRef.current = null;
    }, 500);
    return () => {
      if (autoSaveTimerRef.current) {
        clearTimeout(autoSaveTimerRef.current);
        autoSaveTimerRef.current = null;
      }
    };
  }, [workflowName, readyId, definitions, updateDefinition, updateStatusHint]);

  // 有 ID 时加载工作流定义
  useEffect(() => {
    if (!definitionId) return;
    void (async () => {
      // `loading` 提示放进异步体：effect 体内同步 setState 会多一轮级联渲染
      //（`react-hooks/set-state-in-effect`），时序不变（异步体首句仍是同步执行）。
      updateStatusHint({ state: 'loading' });
      try {
        await loadDefinitions();
        updateStatusHint({ state: 'ready' });
      } catch {
        updateStatusHint({ state: 'error' });
      }
    })();
  }, [definitionId, loadDefinitions, updateStatusHint]);

  // 组合开关：切换模式并回到主布局（编辑器入口不再提供返回按钮，与设置页口径一致）
  const { viewMode, setMode } = useTerminal();
  const handleModeChange = useCallback((mode: ViewMode) => {
    if (statusTimerRef.current) {
      clearTimeout(statusTimerRef.current);
      statusTimerRef.current = null;
    }
    setMode(mode);
    navigate('/');
  }, [setMode, navigate]);

  // 加载中
  if (creating) {
    return (
      <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-canvas)' }}>
        <TitleBar
        showBackButton={false}
        mode={viewMode}
        onModeChange={handleModeChange}
        titleText="工作流任务编辑器"
        statusHint={statusHint}
        onOpenSettings={() => navigate('/settings')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenMarket={() => navigate('/market')} />
      <div className="flex-1 min-h-0 px-2">
        <div className="h-full rounded-lg overflow-hidden flex flex-col" style={{ backgroundColor: 'var(--bg-content)' }}>
        <div className="flex-1 flex items-center justify-center">
          <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>正在创建工作流...</span>
        </div>
        </div>
      </div>
        <StatusBar onOpenSettings={() => navigate('/settings')} onOpenEnvSettings={() => navigate('/settings?tab=environment')} />
      </div>
    );
  }

  // 创建失败
  if (error) {
    return (
      <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-canvas)' }}>
        <TitleBar
        showBackButton={false}
        mode={viewMode}
        onModeChange={handleModeChange}
        titleText="工作流任务编辑器"
        statusHint={statusHint}
        onOpenSettings={() => navigate('/settings')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenMarket={() => navigate('/market')} />
      <div className="flex-1 min-h-0 px-2">
        <div className="h-full rounded-lg overflow-hidden flex flex-col" style={{ backgroundColor: 'var(--bg-content)' }}>
        <div className="flex flex-col items-center justify-center flex-1 gap-3">
          <span className="text-xs" style={{ color: 'var(--status-danger)' }}>创建工作流失败: {error}</span>
          <button
            onClick={() => navigate('/workflow')}
            className="pd-btn px-3 py-1.5 text-xs rounded"
            style={{ color: 'var(--text-secondary)' }}
          >
            返回工作流列表
          </button>
        </div>
        </div>
      </div>
        <StatusBar onOpenSettings={() => navigate('/settings')} onOpenEnvSettings={() => navigate('/settings?tab=environment')} />
      </div>
    );
  }

  if (!readyId) return null;

  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      <TitleBar
        showBackButton={false}
        mode={viewMode}
        onModeChange={handleModeChange}
        titleText="工作流任务编辑器"
        statusHint={statusHint}
        onOpenSettings={() => navigate('/settings')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenMarket={() => navigate('/market')}
      />
      <div className="flex-1 min-h-0 px-2">
        <div className="h-full rounded-lg overflow-hidden flex flex-col" style={{ backgroundColor: 'var(--bg-content)' }}>
      <div className="flex-1 overflow-hidden">
        <WorkflowEditor
          definitionId={readyId}
          onNameChange={handleNameChange}
          onSaveResult={(success) => {
            updateStatusHint({
              state: success ? 'saved' : 'save-error',
              autoDismiss: 5000,
            });
          }}
          onImported={(newId) => {
            navigate(`/workflow/editor?id=${newId}`, { replace: true });
            setReadyId(newId);
          }}
        />
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
