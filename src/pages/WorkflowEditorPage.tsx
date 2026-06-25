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

  // 同步工作流名称
  useEffect(() => {
    if (readyId && definitions.length > 0) {
      const def = definitions.find((d) => d.id === readyId);
      if (def) setWorkflowName(def.name);
    }
  }, [readyId, definitions]);

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
    setCreating(true);

    (async () => {
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
          setError(String(err));
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
    updateStatusHint({ state: 'saving', text: '自动保存名称...' });
    autoSaveTimerRef.current = setTimeout(async () => {
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
  }, [workflowName, readyId, definitions]);

  // 有 ID 时加载工作流定义
  useEffect(() => {
    if (definitionId) {
      updateStatusHint({ state: 'loading' });
      loadDefinitions().then(() => {
        updateStatusHint({ state: 'ready' });
      }).catch(() => {
        updateStatusHint({ state: 'error' });
      });
    }
  }, [definitionId]);

  const handleBack = useCallback(() => {
    if (statusTimerRef.current) {
      clearTimeout(statusTimerRef.current);
      statusTimerRef.current = null;
    }
    navigate('/workflow');
  }, [navigate]);

  // 加载中
  if (creating) {
    return (
      <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <TitleBar
        showBackButton={true}
        titleText="工作流任务编辑器"
        statusHint={statusHint}
        onBack={() => navigate('/workflow')}
        onOpenSettings={() => navigate('/settings')} />
        <div className="flex-1 flex items-center justify-center">
          <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>正在创建工作流...</span>
        </div>
        <StatusBar onOpenSettings={() => navigate('/settings')} onOpenEnvSettings={() => navigate('/settings?tab=environment')} />
      </div>
    );
  }

  // 创建失败
  if (error) {
    return (
      <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <TitleBar
        showBackButton={true}
        titleText="工作流任务编辑器"
        statusHint={statusHint}
        onBack={() => navigate('/workflow')}
        onOpenSettings={() => navigate('/settings')} />
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
        <StatusBar onOpenSettings={() => navigate('/settings')} onOpenEnvSettings={() => navigate('/settings?tab=environment')} />
      </div>
    );
  }

  if (!readyId) return null;

  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
      <TitleBar
        showBackButton={true}
        titleText="工作流任务编辑器"
        statusHint={statusHint}
        onBack={handleBack}
        onOpenSettings={() => navigate('/settings')}
      />
      <div className="flex-1 overflow-hidden">
        <WorkflowEditor
          definitionId={readyId}
          onClose={handleBack}
          onNameChange={handleNameChange}
          onSaveResult={(success) => {
            updateStatusHint({
              state: success ? 'saved' : 'save-error',
              autoDismiss: 5000,
            });
          }}
        />
      </div>
      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
