/**
 * WorkflowNodeConfig — 节点配置面板
 *
 * 适配 6 种实体节点类型 + 控制属性（延迟/超时/重试）。
 */

import React, { useState } from 'react';
import type { WorkflowNode, WorkflowNodeType } from '../../types/workflow';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';

interface Props {
  node: WorkflowNode;
  onUpdate: (updates: Partial<WorkflowNode>) => void;
  onClose: () => void;
}

const NODE_TYPE_CONFIG_MAP: Record<WorkflowNodeType, { fields: { key: string; label: string; type: string; placeholder?: string }[] }> = {
  agent: {
    fields: [
      { key: 'agent_type', label: 'Agent 类型', type: 'select', placeholder: 'claude' },
      { key: 'prompt_template', label: '提示词模板', type: 'textarea', placeholder: '请输入提示词模板，支持 ${variable}' },
      { key: 'system_prompt', label: '系统提示词', type: 'textarea', placeholder: '可选系统提示词' },
    ],
  },
  api: {
    fields: [
      { key: 'url', label: '请求 URL', type: 'text', placeholder: 'https://api.example.com/data' },
      { key: 'method', label: '请求方法', type: 'select', placeholder: 'GET' },
      { key: 'body_template', label: '请求体模板', type: 'textarea', placeholder: '可选 JSON 模板' },
    ],
  },
  transform: {
    fields: [
      { key: 'script', label: '转换脚本', type: 'textarea', placeholder: 'JavaScript 转换脚本\n参数: context, input\n返回值作为输出' },
    ],
  },
  interact: {
    fields: [
      { key: 'prompt', label: '提示文案', type: 'text', placeholder: '请输入提示用户的内容' },
      { key: 'inputType', label: '输入类型', type: 'select', placeholder: 'text' },
      { key: 'timeoutMinutes', label: '超时时间(分钟)', type: 'number', placeholder: '1440' },
    ],
  },
  plugin: {
    fields: [
      { key: 'pluginId', label: '插件 ID', type: 'text', placeholder: '插件 ID' },
      { key: 'commandId', label: '命令 ID', type: 'text', placeholder: '命令 ID' },
    ],
  },
  subflow: {
    fields: [
      { key: 'definitionId', label: '子工作流 ID', type: 'text', placeholder: '工作流定义 ID' },
    ],
  },
};

export const WorkflowNodeConfig: React.FC<Props> = ({ node, onUpdate, onClose }) => {
  const meta = getNodeTypeMeta(node.type);
  const configFields = NODE_TYPE_CONFIG_MAP[node.type]?.fields || [];
  const [params, setParams] = useState<Record<string, any>>(node.params || {});

  const handleParamChange = (key: string, value: any) => {
    const newParams = { ...params, [key]: value };
    setParams(newParams);
    onUpdate({ params: newParams });
  };

  return (
    <div>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 20 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <div style={{ width: 28, height: 28, borderRadius: 6, display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 14, background: `${meta.color}22`, color: meta.color }}>{meta.icon}</div>
          <div>
            <div style={{ fontSize: 14, fontWeight: 600, color: '#f0f6fc' }}>{node.label}</div>
            <div style={{ fontSize: 11, color: '#8b949e' }}>{meta.label}</div>
          </div>
        </div>
        <button onClick={onClose} style={{ padding: '4px 10px', borderRadius: 4, border: '1px solid #30363d', background: '#21262d', color: '#c9d1d9', fontSize: 11, cursor: 'pointer' }}>关闭</button>
      </div>

      {/* 基本信息 */}
      <div style={{ marginBottom: 16 }}>
        <label style={{ fontSize: 11, color: '#8b949e', display: 'block', marginBottom: 4 }}>节点名称</label>
        <input
          value={node.label}
          onChange={(e) => onUpdate({ label: e.target.value })}
          style={{ width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none' }}
        />
      </div>

      {/* 类型特定配置 */}
      {configFields.length > 0 && (
        <div style={{ marginBottom: 16 }}>
          <div style={{ fontSize: 11, fontWeight: 600, color: '#8b949e', marginBottom: 8, textTransform: 'uppercase', letterSpacing: 0.5 }}>节点配置</div>
          {configFields.map((field) => (
            <div key={field.key} style={{ marginBottom: 10 }}>
              <label style={{ fontSize: 11, color: '#8b949e', display: 'block', marginBottom: 4 }}>{field.label}</label>
              {field.type === 'textarea' ? (
                <textarea
                  value={params[field.key] || ''}
                  onChange={(e) => handleParamChange(field.key, e.target.value)}
                  placeholder={field.placeholder}
                  rows={4}
                  style={{ width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none', resize: 'vertical', fontFamily: 'inherit' }}
                />
              ) : field.type === 'select' ? (
                <select
                  value={params[field.key] || field.placeholder || ''}
                  onChange={(e) => handleParamChange(field.key, e.target.value)}
                  style={{ width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none' }}
                >
                  <option value="">选择...</option>
                  {field.key === 'agent_type' && ['claude', 'hermes', 'codex'].map((v) => <option key={v} value={v}>{v}</option>)}
                  {field.key === 'method' && ['GET', 'POST', 'PUT', 'DELETE'].map((v) => <option key={v} value={v}>{v}</option>)}
                  {field.key === 'inputType' && ['text', 'select', 'confirm', 'file'].map((v) => <option key={v} value={v}>{v}</option>)}
                </select>
              ) : (
                <input
                  type={field.type}
                  value={params[field.key] || ''}
                  onChange={(e) => handleParamChange(field.key, field.type === 'number' ? Number(e.target.value) : e.target.value)}
                  placeholder={field.placeholder}
                  style={{ width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none' }}
                />
              )}
            </div>
          ))}
        </div>
      )}

      {/* 控制属性 */}
      <div style={{ marginBottom: 16 }}>
        <div style={{ fontSize: 11, fontWeight: 600, color: '#8b949e', marginBottom: 8, textTransform: 'uppercase', letterSpacing: 0.5 }}>控制属性</div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
          <div>
            <label style={{ fontSize: 10, color: '#8b949e', display: 'block', marginBottom: 2 }}>延迟 (ms)</label>
            <input
              type="number"
              value={node.delayMs || ''}
              onChange={(e) => onUpdate({ delayMs: e.target.value ? Number(e.target.value) : undefined })}
              placeholder="0"
              style={{ width: '100%', padding: '4px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 11, outline: 'none' }}
            />
          </div>
          <div>
            <label style={{ fontSize: 10, color: '#8b949e', display: 'block', marginBottom: 2 }}>超时 (ms)</label>
            <input
              type="number"
              value={node.timeoutMs || ''}
              onChange={(e) => onUpdate({ timeoutMs: e.target.value ? Number(e.target.value) : undefined })}
              placeholder="60000"
              style={{ width: '100%', padding: '4px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 11, outline: 'none' }}
            />
          </div>
          <div>
            <label style={{ fontSize: 10, color: '#8b949e', display: 'block', marginBottom: 2 }}>重试次数</label>
            <input
              type="number"
              value={node.retryCount || ''}
              onChange={(e) => onUpdate({ retryCount: e.target.value ? Number(e.target.value) : undefined })}
              placeholder="0"
              style={{ width: '100%', padding: '4px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 11, outline: 'none' }}
            />
          </div>
          <div>
            <label style={{ fontSize: 10, color: '#8b949e', display: 'block', marginBottom: 2 }}>重试间隔 (ms)</label>
            <input
              type="number"
              value={node.retryDelayMs || ''}
              onChange={(e) => onUpdate({ retryDelayMs: e.target.value ? Number(e.target.value) : undefined })}
              placeholder="1000"
              style={{ width: '100%', padding: '4px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 11, outline: 'none' }}
            />
          </div>
        </div>
      </div>

      {/* 输入输出映射 */}
      <div>
        <div style={{ fontSize: 11, fontWeight: 600, color: '#8b949e', marginBottom: 8, textTransform: 'uppercase', letterSpacing: 0.5 }}>输入输出映射</div>
        <div style={{ marginBottom: 8 }}>
          <label style={{ fontSize: 10, color: '#8b949e', display: 'block', marginBottom: 2 }}>输入映射 (JSON)</label>
          <textarea
            value={node.inputMapping ? JSON.stringify(node.inputMapping, null, 2) : ''}
            onChange={(e) => {
              try { onUpdate({ inputMapping: JSON.parse(e.target.value) }); } catch {}
            }}
            placeholder='{"key": "${context.path}"}'
            rows={3}
            style={{ width: '100%', padding: '4px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 11, outline: 'none', resize: 'vertical', fontFamily: 'monospace' }}
          />
        </div>
        <div>
          <label style={{ fontSize: 10, color: '#8b949e', display: 'block', marginBottom: 2 }}>输出映射 (JSON)</label>
          <textarea
            value={node.outputMapping ? JSON.stringify(node.outputMapping, null, 2) : ''}
            onChange={(e) => {
              try { onUpdate({ outputMapping: JSON.parse(e.target.value) }); } catch {}
            }}
            placeholder='{"result": "context.path"}'
            rows={3}
            style={{ width: '100%', padding: '4px 8px', borderRadius: 4, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 11, outline: 'none', resize: 'vertical', fontFamily: 'monospace' }}
          />
        </div>
      </div>
    </div>
  );
};
