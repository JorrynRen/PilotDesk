/**
 * WorkflowPanel — 工作面板入口
 *
 * 工作流管理的主面板，包含定义列表、实例监控和执行控制。
 */

import React, { useState, useEffect } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { WorkflowEditor } from './WorkflowEditor';
import { WorkflowMonitor } from './WorkflowMonitor';
import { createEmptyDefinition } from '../../workflow/WorkflowDefinition';

type Tab = 'definitions' | 'instances';

export const WorkflowPanel: React.FC = () => {
  const {
    definitions, instances, selectedDefinitionId,
    loadDefinitions, loadInstances, createDefinition, deleteDefinition,
    selectDefinition, selectInstance, startWorkflow,
  } = useWorkflowStore();

  const [tab, setTab] = useState<Tab>('definitions');
  const [showEditor, setShowEditor] = useState(false);

  useEffect(() => {
    loadDefinitions();
    loadInstances();
  }, []);

  const handleCreate = async () => {
    const def = createEmptyDefinition('新工作流');
    const id = await createDefinition(def);
    selectDefinition(id);
    setShowEditor(true);
  };

  const handleStart = async (defId: string) => {
    const instanceId = await startWorkflow(defId);
    selectInstance(instanceId);
    setTab('instances');
  };

  return (
    <div className="workflow-panel">
      <div className="workflow-toolbar">
        <h2>工作流</h2>
        <div className="workflow-tabs">
          <button className={tab === 'definitions' ? 'active' : ''} onClick={() => setTab('definitions')}>
            定义 ({definitions.length})
          </button>
          <button className={tab === 'instances' ? 'active' : ''} onClick={() => setTab('instances')}>
            实例 ({instances.length})
          </button>
        </div>
        <div className="workflow-actions">
          {tab === 'definitions' && (
            <button onClick={handleCreate} className="btn-primary">新建工作流</button>
          )}
        </div>
      </div>

      {tab === 'definitions' && !showEditor && (
        <div className="workflow-def-list">
          {definitions.length === 0 ? (
            <div className="workflow-empty">
              <p>暂无工作流定义</p>
              <button onClick={handleCreate} className="btn-primary">创建第一个工作流</button>
            </div>
          ) : (
            definitions.map((def) => (
              <div key={def.id} className="workflow-def-card">
                <div className="def-card-header">
                  <h3>{def.name}</h3>
                  <span className={`def-status ${def.enabled ? 'enabled' : 'disabled'}`}>
                    {def.enabled ? '已启用' : '已禁用'}
                  </span>
                </div>
                <p className="def-description">{def.description || '无描述'}</p>
                <div className="def-meta">
                  <span>v{def.version}</span>
                  <span>{def.nodes.length} 个节点</span>
                  <span>{def.edges.length} 条连接</span>
                </div>
                <div className="def-actions">
                  <button onClick={() => { selectDefinition(def.id); setShowEditor(true); }}>编辑</button>
                  <button onClick={() => handleStart(def.id)} className="btn-primary">运行</button>
                  <button onClick={() => deleteDefinition(def.id)} className="btn-danger">删除</button>
                </div>
              </div>
            ))
          )}
        </div>
      )}

      {tab === 'definitions' && showEditor && selectedDefinitionId && (
        <WorkflowEditor
          definitionId={selectedDefinitionId}
          onClose={() => { setShowEditor(false); selectDefinition(null); }}
        />
      )}

      {tab === 'instances' && (
        <WorkflowMonitor
          onViewDefinition={(defId) => {
            selectDefinition(defId);
            setShowEditor(true);
            setTab('definitions');
          }}
        />
      )}
    </div>
  );
};
