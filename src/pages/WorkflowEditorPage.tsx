/**
 * WorkflowEditorPage — 工作流编辑器页面
 *
 * 独立路由页面，从 URL query 读取 definitionId，
 * 未传 ID 时自动创建新工作流。
 */

import { useEffect } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { WorkflowEditor } from '../components/workflow/WorkflowEditor';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';

export function WorkflowEditorPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const definitionId = searchParams.get('id');
  const { createDefinition, definitions, loadDefinitions, selectDefinition } = useWorkflowStore();

  // 加载工作流定义列表
  useEffect(() => {
    loadDefinitions();
  }, []);

  // 新建工作流（无 ID 时）
  useEffect(() => {
    if (!definitionId && definitions.length >= 0) {
      // 如果没有传 ID，检查 store 中是否有选中的
      const selectedId = useWorkflowStore.getState().selectedDefinitionId;
      if (selectedId) return; // 已有选中的，不重复创建
    }
  }, [definitionId, definitions.length]);

  const handleNew = async () => {
    const def = createDefaultWorkflow('新工作流');
    const id = await createDefinition(def);
    selectDefinition(id);
    // 重新加载以触发 WorkflowEditor 获取新定义
    navigate(`/workflow/editor?id=${id}`, { replace: true });
  };

  // 如果没有 ID 且 store 中没有选中的，创建新工作流
  const effectiveId = definitionId || useWorkflowStore.getState().selectedDefinitionId;

  if (!effectiveId) {
    return (
      <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
        <header
          className="flex items-center gap-3 px-4 h-12 shrink-0 select-none"
          style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
        >
          <button onClick={() => navigate('/workflow')} className="pd-btn p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
            <ArrowLeft size={16} />
          </button>
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>工作流编辑器</span>
        </header>
        <div className="flex-1 flex items-center justify-center">
          <button
            onClick={handleNew}
            className="pd-btn px-4 py-2 text-xs rounded"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            创建新工作流
          </button>
        </div>
      </div>
    );
  }

  return (
    <WorkflowEditor
      definitionId={effectiveId}
      onClose={() => navigate('/workflow')}
    />
  );
}
