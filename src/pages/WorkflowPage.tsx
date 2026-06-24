import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { ArrowLeft, Play, Plus, Trash2, Clock, CheckCircle, XCircle, AlertCircle } from 'lucide-react';
import { useWorkflowStore } from '../stores/workflowStore';
import { createDefaultWorkflow } from '../workflow/WorkflowDefinition';
import type { WorkflowDefinition, WorkflowInstance } from '../types/workflow';

interface WorkflowPageProps {
  onBack?: () => void;
}

export function WorkflowPage({ onBack }: WorkflowPageProps) {
  const navigate = useNavigate();
  const { definitions, instances, loading, error, loadDefinitions, loadInstances, createDefinition, deleteDefinition, startWorkflow, selectDefinition } = useWorkflowStore();
  const [activeTab, setActiveTab] = useState<'definitions' | 'instances'>('definitions');

  useEffect(() => {
    loadDefinitions();
    loadInstances();
  }, []);

  const handleCreateAndEdit = async () => {
    const def = createDefaultWorkflow('新工作流');
    try {
      const id = await createDefinition(def);
      selectDefinition(id);
      navigate(`/workflow/editor?id=${id}`);
    } catch (err) {
      console.error('创建工作流失败:', err);
    }
  };

  const handleStart = async (e: React.MouseEvent, id: string, name: string) => {
    e.stopPropagation();
    try {
      await startWorkflow(id);
    } catch (err) {
      console.error(`启动工作流「${name}」失败:`, err);
    }
  };

  const handleDelete = async (id: string, name: string) => {
    if (confirm(`确定删除工作流「${name}」？此操作不可撤销。`)) {
      await deleteDefinition(id);
    }
  };

  const statusIcon = (status: string) => {
    switch (status) {
      case 'success': return <CheckCircle size={14} className="text-green-500" />;
      case 'failed': return <XCircle size={14} className="text-red-500" />;
      case 'running': return <AlertCircle size={14} className="text-blue-500" />;
      case 'pending': return <Clock size={14} className="text-yellow-500" />;
      default: return <Clock size={14} className="text-gray-400" />;
    }
  };

  return (
    <div className="flex flex-col h-full" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* Header */}
      <header
        className="flex items-center justify-between px-4 h-12 shrink-0 select-none"
        style={{ borderBottom: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}
      >
        <div className="flex items-center gap-3">
          {onBack && (
            <button onClick={onBack} className="pd-btn p-1 rounded hover:opacity-80" style={{ color: 'var(--text-secondary)' }}>
              <ArrowLeft size={16} />
            </button>
          )}
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>工作流管理</span>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handleCreateAndEdit}
            className="pd-btn px-3 py-1.5 text-xs rounded flex items-center gap-1.5 transition-colors"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            <Plus size={14} /> 新建工作流
          </button>
        </div>
      </header>

      {/* Tabs */}
      <div className="flex px-4 pt-3 gap-4" style={{ borderBottom: '1px solid var(--border)' }}>
        <button
          onClick={() => setActiveTab('definitions')}
          className="pb-2 text-xs font-medium transition-colors relative"
          style={{
            color: activeTab === 'definitions' ? 'var(--accent)' : 'var(--text-secondary)',
            borderBottom: activeTab === 'definitions' ? '2px solid var(--accent)' : '2px solid transparent',
          }}
        >
          工作流定义 ({definitions.length})
        </button>
        <button
          onClick={() => setActiveTab('instances')}
          className="pb-2 text-xs font-medium transition-colors relative"
          style={{
            color: activeTab === 'instances' ? 'var(--accent)' : 'var(--text-secondary)',
            borderBottom: activeTab === 'instances' ? '2px solid var(--accent)' : '2px solid transparent',
          }}
        >
          执行记录 ({instances.length})
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {loading && (
          <div className="flex items-center justify-center h-32 text-xs" style={{ color: 'var(--text-tertiary)' }}>
            加载中...
          </div>
        )}

        {error && (
          <div className="p-3 mb-3 rounded text-xs" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#ef4444' }}>
            {error}
          </div>
        )}

        {!loading && activeTab === 'definitions' && (
          <>
            {definitions.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-48 gap-3">
                <div className="text-3xl opacity-30">🔀</div>
                <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无工作流定义</div>
                <button
                  onClick={handleCreateAndEdit}
                  className="pd-btn px-4 py-2 text-xs rounded transition-colors"
                  style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                >
                  创建第一个工作流
                </button>
              </div>
            ) : (
              <div className="grid gap-3">
                {definitions.map((def) => (
                  <div
                    key={def.id}
                    className="p-3 rounded-lg transition-colors cursor-pointer hover:opacity-90"
                    style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                    onClick={() => navigate(`/workflow/editor?id=${def.id}`)}
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>{def.name}</span>
                        <span className="text-[10px] px-1.5 py-0.5 rounded" style={{
                          backgroundColor: def.enabled ? 'rgba(34,197,94,0.15)' : 'rgba(107,114,128,0.15)',
                          color: def.enabled ? '#22c55e' : 'var(--text-tertiary)',
                        }}>
                          {def.enabled ? '已启用' : '已禁用'}
                        </span>
                      </div>
                      <div className="flex items-center gap-1">
                        <button
                          onClick={(e) => { handleStart(e, def.id, def.name); }}
                          className="pd-btn p-1.5 rounded hover:opacity-80"
                          style={{ color: 'var(--text-secondary)' }}
                          title="执行"
                        >
                          <Play size={14} />
                        </button>
                        <button
                          onClick={(e) => { e.stopPropagation(); handleDelete(def.id, def.name); }}
                          className="pd-btn p-1.5 rounded hover:opacity-80"
                          style={{ color: 'var(--text-secondary)' }}
                          title="删除"
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </div>
                    {def.description && (
                      <div className="mt-1 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>{def.description}</div>
                    )}
                    <div className="mt-2 flex items-center gap-3 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      <span>v{def.version}</span>
                      <span>阶段: {def.stages?.length || 0}</span>
                      <span>触发器: {def.trigger?.triggerType || 'manual'}</span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </>
        )}

        {!loading && activeTab === 'instances' && (
          <>
            {instances.length === 0 ? (
              <div className="flex items-center justify-center h-48 text-xs" style={{ color: 'var(--text-tertiary)' }}>
                暂无执行记录
              </div>
            ) : (
              <div className="grid gap-2">
                {instances.map((inst) => (
                  <div
                    key={inst.id}
                    className="p-3 rounded-lg"
                    style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center gap-2">
                        {statusIcon(inst.status)}
                        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>{inst.definitionName}</span>
                      </div>
                      <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                        {new Date(Number(inst.createdAt) * 1000).toLocaleString()}
                      </span>
                    </div>
                    <div className="mt-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      ID: {inst.id.slice(0, 12)}... | 触发器: {inst.trigger}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
