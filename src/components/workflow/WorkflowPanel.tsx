/**
 * WorkflowPanel — 工作面板入口
 *
 * 工作流管理的主面板，包含定义列表、实例监控和执行控制。
 */

import React, { useState, useEffect } from 'react';
import { useWorkflowStore } from '../../stores/workflowStore';
import { WorkflowEditor } from './WorkflowEditor';
import { WorkflowMonitor } from './WorkflowMonitor';
import { ExecutionStats } from './ExecutionStats';
import { createDefaultWorkflow } from '../../workflow/WorkflowDefinition';
import type { WorkflowDefinition, WorkflowNodeType } from '../../types/workflow';

type Tab = 'definitions' | 'instances' | 'stats' | 'templates';

// ── 内置模板 ──

interface WorkflowTemplate {
  id: string;
  name: string;
  description: string;
  category: string;
  icon: string;
  nodes: number;
  stages: number;
  create: () => WorkflowDefinition;
}

const WORKFLOW_TEMPLATES: WorkflowTemplate[] = [
  {
    id: 'data-pipeline',
    name: '数据处理流水线',
    description: '从 API 获取数据 → 转换处理 → 保存结果，包含错误处理和重试',
    category: '数据处理',
    icon: '📊',
    nodes: 4,
    stages: 3,
    create: () => {
      const def = createDefaultWorkflow('数据处理流水线');
      def.stages = [
        { id: 's1', name: '数据获取', order: 0, nodes: [
          { id: 'n1', type: 'api' as WorkflowNodeType, label: '获取数据', position: { x: 40, y: 40 }, params: { url: 'https://api.example.com/data', method: 'GET' }, timeoutMs: 30000 },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
        { id: 's2', name: '数据转换', order: 1, nodes: [
          { id: 'n2', type: 'transform' as WorkflowNodeType, label: '数据清洗', position: { x: 40, y: 40 }, params: { script: '// 数据清洗逻辑\nreturn input.map(item => ({ ...item, processed: true }));' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
        { id: 's3', name: '结果输出', order: 2, nodes: [
          { id: 'n3', type: 'agent' as WorkflowNodeType, label: '生成报告', position: { x: 40, y: 40 }, params: { prompt_template: '基于以下数据生成报告：{{input}}' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
      ];
      def.description = '从 API 获取数据，经过转换处理后生成报告';
      return def;
    },
  },
  {
    id: 'approval-flow',
    name: '审批工作流',
    description: '提交申请 → 人工审批 → 结果通知，支持驳回重审',
    category: '办公流程',
    icon: '✅',
    nodes: 3,
    stages: 2,
    create: () => {
      const def = createDefaultWorkflow('审批工作流');
      def.stages = [
        { id: 's1', name: '申请提交', order: 0, nodes: [
          { id: 'n1', type: 'interact' as WorkflowNodeType, label: '填写申请', position: { x: 40, y: 40 }, params: { prompt: '请填写申请内容', inputType: 'text' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
        { id: 's2', name: '审批处理', order: 1, nodes: [
          { id: 'n2', type: 'interact' as WorkflowNodeType, label: '审批', position: { x: 40, y: 40 }, params: { prompt: '请审批该申请', inputType: 'confirm' } },
          { id: 'n3', type: 'agent' as WorkflowNodeType, label: '通知结果', position: { x: 40, y: 120 }, params: { prompt_template: '审批结果：{{input}}' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
      ];
      def.description = '提交申请后由人工审批，自动通知结果';
      return def;
    },
  },
  {
    id: 'monitor-bot',
    name: '定时监控机器人',
    description: '定时检查服务状态 → 异常告警 → 自动修复或人工介入',
    category: '运维监控',
    icon: '🔍',
    nodes: 4,
    stages: 3,
    create: () => {
      const def = createDefaultWorkflow('定时监控机器人');
      def.trigger = { triggerType: 'cron', cron: '0 */30 * * * *', eventName: undefined };
      def.stages = [
        { id: 's1', name: '健康检查', order: 0, nodes: [
          { id: 'n1', type: 'api' as WorkflowNodeType, label: '检查服务状态', position: { x: 40, y: 40 }, params: { url: 'https://status.example.com/health', method: 'GET' }, timeoutMs: 10000 },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
        { id: 's2', name: '异常处理', order: 1, nodes: [
          { id: 'n2', type: 'transform' as WorkflowNodeType, label: '判断状态', position: { x: 40, y: 40 }, params: { script: 'return input.status === \"ok\" ? { healthy: true } : { healthy: false, error: input.error };' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
        { id: 's3', name: '告警通知', order: 2, nodes: [
          { id: 'n3', type: 'agent' as WorkflowNodeType, label: '发送告警', position: { x: 40, y: 40 }, params: { prompt_template: '服务异常告警：{{input.error}}' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
      ];
      def.description = '每30分钟检查服务状态，异常时发送告警';
      return def;
    },
  },
  {
    id: 'content-publish',
    name: '内容发布流程',
    description: '内容创作 → AI 审核 → 多平台发布',
    category: '内容管理',
    icon: '📝',
    nodes: 4,
    stages: 3,
    create: () => {
      const def = createDefaultWorkflow('内容发布流程');
      def.stages = [
        { id: 's1', name: '内容创作', order: 0, nodes: [
          { id: 'n1', type: 'agent' as WorkflowNodeType, label: '生成内容', position: { x: 40, y: 40 }, params: { prompt_template: '请根据以下主题创作内容：{{input.topic}}' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
        { id: 's2', name: '内容审核', order: 1, nodes: [
          { id: 'n2', type: 'interact' as WorkflowNodeType, label: '人工审核', position: { x: 40, y: 40 }, params: { prompt: '请审核以下内容是否适合发布', inputType: 'confirm' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
        { id: 's3', name: '发布', order: 2, nodes: [
          { id: 'n3', type: 'api' as WorkflowNodeType, label: '发布到平台', position: { x: 40, y: 40 }, params: { url: 'https://api.example.com/publish', method: 'POST' } },
        ], edges: [], gate: { strategy: 'all', mergeStrategy: 'merge' } },
      ];
      def.description = 'AI 生成内容 → 人工审核 → 自动发布';
      return def;
    },
  },
];

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
    const def = createDefaultWorkflow('新工作流');
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
          <button className={tab === 'stats' ? 'active' : ''} onClick={() => setTab('stats')}>
            统计
          </button>
          <button className={tab === 'templates' ? 'active' : ''} onClick={() => setTab('templates')}>
            模板市场
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
                  <span>{def.stages.reduce((sum, s) => sum + s.nodes.length, 0)} 个节点</span>
                  <span>{def.stages.reduce((sum, s) => sum + s.edges.length, 0)} 条连接</span>
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

      {tab === 'stats' && (
        <ExecutionStats workflowId={undefined} />
      )}

      {tab === 'templates' && (
        <div className="workflow-templates">
          <div style={{ marginBottom: 16 }}>
            <h3 style={{ fontSize: 15, color: '#f0f6fc', marginBottom: 4 }}>工作流模板市场</h3>
            <p style={{ fontSize: 12, color: '#8b949e' }}>从模板快速创建工作流，支持自定义修改</p>
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))', gap: 12 }}>
            {WORKFLOW_TEMPLATES.map((tmpl) => (
              <div key={tmpl.id} style={{ border: '1px solid #30363d', borderRadius: 8, background: '#161b22', padding: 16 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 10 }}>
                  <span style={{ fontSize: 24 }}>{tmpl.icon}</span>
                  <div>
                    <div style={{ fontSize: 14, fontWeight: 600, color: '#f0f6fc' }}>{tmpl.name}</div>
                    <div style={{ fontSize: 11, color: '#8b949e' }}>{tmpl.category}</div>
                  </div>
                </div>
                <p style={{ fontSize: 12, color: '#c9d1d9', marginBottom: 10, lineHeight: 1.5 }}>{tmpl.description}</p>
                <div style={{ display: 'flex', gap: 8, fontSize: 11, color: '#8b949e', marginBottom: 12 }}>
                  <span>{tmpl.stages} 阶段</span>
                  <span>{tmpl.nodes} 节点</span>
                </div>
                <button
                  onClick={async () => {
                    const def = tmpl.create();
                    await createDefinition(def);
                    loadDefinitions();
                    setTab('definitions');
                  }}
                  style={{ width: '100%', padding: '8px 0', borderRadius: 6, border: 'none', background: '#238636', color: '#fff', fontSize: 12, cursor: 'pointer' }}
                >
                  使用此模板
                </button>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};
