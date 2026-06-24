/**
 * WorkflowEditorPage — 工作流编辑器页面
 *
 * 独立路由页面，从 URL query 读取 definitionId。
 * 无 ID 时自动创建新工作流并跳转。
 * 使用统一 TitleBar + StatusBar 布局。
 */

import { useEffect, useState, useCallback } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { TitleBar, StatusBar } from '../components/layout';
import { WorkflowEditor } from '../components/workflow/WorkflowEditor';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';

export function WorkflowEditorPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const definitionId = searchParams.get('id');
  const { createDefinition, loadDefinitions, selectDefinition, definitions } = useWorkflowStore();
  const [readyId, setReadyId] = useState<string | null>(definitionId);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [workflowName, setWorkflowName] = useState('');

  // 同步工作流名称
  useEffect(() => {
    if (readyId && definitions.length > 0) {
      const def = definitions.find((d) => d.id === readyId);
      if (def) setWorkflowName(def.name);
    }
  }, [readyId, definitions]);

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
        }
      } catch (err) {
        if (!cancelled) {
          setError(String(err));
        }
      } finally {
        if (!cancelled) {
          setCreating(false);
        }
      }
    })();

    return () => { cancelled = true; };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // 有 ID 时加载定义列表
  useEffect(() => {
    if (definitionId) {
      loadDefinitions();
    }
  }, [definitionId]);

  const handleBack = useCallback(() => {
    navigate('/workflow');
  }, [navigate]);

  // 加载中
  if (creating) {
    return (
      <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <TitleBar
        showBackButton={true}
        titleText="工作流任务编辑器"
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
        onBack={handleBack}
        onOpenSettings={() => navigate('/settings')}
      />
      <div className="flex-1 overflow-hidden">
        <WorkflowEditor
          definitionId={readyId}
          onClose={handleBack}
          onNameChange={handleNameChange}
        />
      </div>
      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
