/**
 * WorkflowEditorPage — 工作流编辑器页面
 *
 * 独立路由页面，从 URL query 读取 definitionId。
 * 无 ID 时自动创建新工作流并跳转。
 */

import { useEffect, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { WorkflowEditor } from '../components/workflow/WorkflowEditor';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';

export function WorkflowEditorPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const definitionId = searchParams.get('id');
  const { createDefinition, loadDefinitions, selectDefinition } = useWorkflowStore();
  const [readyId, setReadyId] = useState<string | null>(definitionId);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 无 ID 时自动创建新工作流
  useEffect(() => {
    if (definitionId) return; // 已有 ID，不需要创建
    if (readyId) return;     // 已有 readyId，不需要创建
    if (creating) return;     // 正在创建中

    let cancelled = false;
    setCreating(true);

    (async () => {
      try {
        await loadDefinitions();
        const def = createDefaultWorkflow('新工作流');
        const id = await createDefinition(def);
        selectDefinition(id);
        if (!cancelled) {
          // 用 replace 替换 URL，保留干净的浏览器历史
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

  // 有 ID 时加载定义列表确保 WorkflowEditor 能找到数据
  useEffect(() => {
    if (definitionId) {
      loadDefinitions();
    }
  }, [definitionId]);

  // 加载中
  if (creating) {
    return (
      <div className="flex items-center justify-center h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>正在创建工作流...</span>
      </div>
    );
  }

  // 创建失败
  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-3" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <span className="text-xs" style={{ color: 'var(--status-danger)' }}>创建工作流失败: {error}</span>
        <button
          onClick={() => navigate('/workflow')}
          className="pd-btn px-3 py-1.5 text-xs rounded"
          style={{ color: 'var(--text-secondary)' }}
        >
          返回工作流列表
        </button>
      </div>
    );
  }

  // 还没有 effectiveId（理论上不应该到这里，因为 creating 时已处理）
  if (!readyId) return null;

  return (
    <WorkflowEditor
      definitionId={readyId}
      onClose={() => navigate('/workflow')}
    />
  );
}
