/**
 * WorkflowNodeConfig — 节点配置面板
 *
 * 适配 6 种实体节点类型 + 控制属性（延迟/超时/重试）。
 * 所有样式均使用 CSS 变量，无硬编码色值。
 */

import React, { useState, useEffect, useRef } from 'react';
import type { WorkflowNode, WorkflowNodeType } from '../../types/workflow';
import { getNodeTypeMeta, NODE_OUTPUT_SCHEMA } from '../../workflow/WorkflowDefinition';
import { useWorkflowStore } from '../../stores/workflowStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';

interface Props {
  node: WorkflowNode;
  onUpdate: (updates: Partial<WorkflowNode>) => void;
  onClose: () => void;
  onOpenSubflow?: (definitionId: string) => void;
}

const NODE_TYPE_CONFIG_MAP: Record<WorkflowNodeType, { fields: { key: string; label: string; type: string; placeholder?: string }[] }> = {
  agent: {
    fields: [
      { key: 'agent_type', label: 'Agent 类型', type: 'select', placeholder: 'claude' },
      { key: 'prompt_template', label: '提示词模板', type: 'textarea', placeholder: '请输入提示词模板，支持 {{variable}}' },
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
      { key: 'definitionId', label: '子工作流', type: 'subflow_select', placeholder: '选择子工作流' },
    ],
  },
  start: {
    fields: [],
  },
  end: {
    fields: [],
  },
};

/* ---------- 公共 style 对象（全部引用 CSS 变量） ---------- */

const S = {
  sectionGap: { marginBottom: 16, paddingTop: 16, borderTop: '1px solid var(--border)' } as React.CSSProperties,
  sectionTitle: {
    fontSize: 'var(--fs-11)',
    fontWeight: 600,
    color: 'var(--text-tertiary)',
    marginBottom: 8,
    textTransform: 'uppercase' as const,
    letterSpacing: 0.5,
  } as React.CSSProperties,
  label: (mb = 4) => ({
    fontSize: 'var(--fs-11)',
    color: 'var(--text-tertiary)',
    display: 'block',
    marginBottom: mb,
  }) as React.CSSProperties,
  labelSm: (mb = 2) => ({
    fontSize: 'var(--fs-10)',
    color: 'var(--text-tertiary)',
    display: 'block',
    marginBottom: mb,
  }) as React.CSSProperties,
  input: (fs = 'var(--fs-12)', py = '6px 10px') => ({
    width: '100%',
    padding: py,
    borderRadius: 'var(--radius-md)',
    border: '1px solid var(--border)',
    background: 'var(--bg-primary)',
    color: 'var(--text-primary)',
    fontSize: fs,
    outline: 'none',
  }) as React.CSSProperties,
  textarea: {
    width: '100%',
    padding: '6px 10px',
    borderRadius: 'var(--radius-md)',
    border: '1px solid var(--border)',
    background: 'var(--bg-primary)',
    color: 'var(--text-primary)',
    fontSize: 'var(--fs-12)',
    outline: 'none',
    resize: 'vertical' as const,
    fontFamily: 'inherit',
  } as React.CSSProperties,
  select: (extra?: React.CSSProperties) => ({
    width: '100%',
    padding: '6px 10px',
    borderRadius: 'var(--radius-md)',
    border: '1px solid var(--border)',
    background: 'var(--bg-primary)',
    color: 'var(--text-primary)',
    fontSize: 'var(--fs-12)',
    outline: 'none',
    ...extra,
  }) as React.CSSProperties,
  monoTextarea: {
    width: '100%',
    padding: '4px 8px',
    borderRadius: 'var(--radius-md)',
    border: '1px solid var(--border)',
    background: 'var(--bg-primary)',
    color: 'var(--text-primary)',
    fontSize: 'var(--fs-11)',
    outline: 'none',
    resize: 'vertical' as const,
    fontFamily: 'var(--font-mono)',
  } as React.CSSProperties,
  fieldGap: { marginBottom: 10 } as React.CSSProperties,
};

/* ---------- 映射键名生成辅助 ---------- */

/**
 * 为映射编辑器生成下一个新键名。
 * - currentKeys：当前已有的键名列表
 * - baseKeyRef：记录第一个手动输入的键名的 ref
 * - 首次添加返回 defaultKey（预填默认键名）
 * - 后续添加返回 baseKey2、baseKey3 …（自动递增避免冲突）
 */
function nextMappingKey(
  currentKeys: string[],
  baseKeyRef: React.MutableRefObject<string>,
  defaultKey: string,
): string {
  if (currentKeys.length === 0) return defaultKey;
  const base = baseKeyRef.current || currentKeys[0];
  let suffix = 2;
  const existing = new Set(currentKeys);
  while (existing.has(base + suffix)) suffix++;
  return base + suffix;
}

/* ---------- MappingEditor：键值对列表组件 ---------- */

/**
 * 非法键名正则：仅允许英文、数字、下划线，且不能以数字开头。
 */
const INVALID_KEY_RE = /[^a-zA-Z0-9_]/;
const STARTS_WITH_DIGIT_RE = /^\d/;

const MappingEditor: React.FC<{
  value?: Record<string, string>;
  onChange: (v: Record<string, string>) => void;
  keyPlaceholder: string;
  valuePlaceholder: string;
  baseKeyRef: React.MutableRefObject<string>;
  /** 可选：输出字段选项分组，提供时 key 列渲染为级联选择器 */
  keyOptions?: { group: string; options: { value: string; label: string; description?: string }[] }[];
}> = ({ value, onChange, keyPlaceholder, valuePlaceholder, baseKeyRef, keyOptions }) => {
  const entries = Object.entries(value || {});
  const [invalidKeys, setInvalidKeys] = useState<Set<string>>(new Set());
  const [dupKeys, setDupKeys] = useState<Set<string>>(new Set());

  const handleKeyChange = (oldKey: string, newKey: string) => {
    if (oldKey === newKey) return;
    const newMap: Record<string, string> = {};
    for (const [k, v] of Object.entries(value || {})) {
      newMap[k === oldKey ? newKey : k] = v;
    }
    onChange(newMap);
    const keys = Object.keys(newMap);
    if (keys.length === 1) baseKeyRef.current = keys[0];
  };

  const handleValueChange = (key: string, newValue: string) => {
    onChange({ ...value, [key]: newValue });
  };

  const handleRemove = (key: string) => {
    const newMap = { ...value };
    delete newMap[key];
    onChange(newMap);
    setInvalidKeys((prev) => { const n = new Set(prev); n.delete(key); return n; });
    setDupKeys((prev) => { const n = new Set(prev); n.delete(key); return n; });
  };

  const validateAllKeys = () => {
    const keys = Object.keys(value || {});
    const invalid = new Set<string>();
    const dup = new Set<string>();
    for (const k of keys) {
      if (INVALID_KEY_RE.test(k) || STARTS_WITH_DIGIT_RE.test(k) || k.trim() === '') {
        invalid.add(k);
      }
    }
    const seen = new Set<string>();
    for (const k of keys) {
      if (seen.has(k)) dup.add(k);
      seen.add(k);
    }
    setInvalidKeys(invalid);
    setDupKeys(dup);
    return invalid.size === 0 && dup.size === 0;
  };

  const getKeyInputStyle = (key: string): React.CSSProperties => {
    const hasError = invalidKeys.has(key) || dupKeys.has(key);
    return {
      flex: '0 0 100px',
      minWidth: 0,
      padding: '4px 8px',
      borderRadius: 'var(--radius-md)',
      border: hasError ? '1px solid var(--color-error, #e53935)' : '1px solid var(--border)',
      background: 'var(--bg-primary)',
      color: 'var(--text-primary)',
      fontSize: 'var(--fs-11)',
      outline: 'none',
      fontFamily: 'var(--font-mono)',
    };
  };

  const getValidationHint = (key: string): string | null => {
    if (invalidKeys.has(key)) return '仅允许英文/数字/下划线，不能以数字开头';
    if (dupKeys.has(key)) return '键名重复';
    return null;
  };

  /** 渲染 key 列：有 keyOptions 时渲染为级联选择器，否则为文本输入框 */
  const renderKeyColumn = (key: string) => {
    if (keyOptions && keyOptions.length > 0) {
      // 级联选择器模式
      const allOptions = keyOptions.flatMap(g => g.options);
      return (
        <select
          value={key}
          onChange={(e) => handleKeyChange(key, e.target.value)}
          style={{
            flex: '0 0 120px',
            minWidth: 0,
            padding: '4px 6px',
            borderRadius: 'var(--radius-md)',
            border: '1px solid var(--border)',
            background: 'var(--bg-primary)',
            color: 'var(--text-primary)',
            fontSize: 'var(--fs-11)',
            outline: 'none',
            fontFamily: 'var(--font-mono)',
            cursor: 'pointer',
          }}
        >
          {keyOptions.map((group) => (
            <optgroup key={group.group} label={group.group}>
              {group.options.map((opt) => (
                <option key={opt.value} value={opt.value} title={opt.description}>
                  {opt.label} ({opt.value})
                </option>
              ))}
            </optgroup>
          ))}
        </select>
      );
    }
    // 文本输入模式
    return (
      <input
        value={key}
        onChange={(e) => handleKeyChange(key, e.target.value)}
        onBlur={validateAllKeys}
        placeholder={keyPlaceholder}
        style={getKeyInputStyle(key)}
      />
    );
  };

  return (
    <div>
      {entries.length === 0 ? (
        <div
          style={{
            padding: '8px 10px',
            borderRadius: 'var(--radius-md)',
            border: '1px dashed var(--border)',
            fontSize: 'var(--fs-11)',
            color: 'var(--text-tertiary)',
            marginBottom: 6,
            textAlign: 'center',
          }}
        >
          暂无映射，点击右侧按钮添加
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4, marginBottom: 6 }}>
          {entries.map(([key, val]) => (
            <div key={key} className="flex items-center" style={{ gap: 4, minWidth: 0 }}>
              {renderKeyColumn(key)}
              <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--fs-11)', flexShrink: 0 }}>→</span>
              <input
                value={val}
                onChange={(e) => handleValueChange(key, e.target.value)}
                placeholder={valuePlaceholder}
                style={{
                  flex: 1,
                  minWidth: 0,
                  padding: '4px 8px',
                  borderRadius: 'var(--radius-md)',
                  border: '1px solid var(--border)',
                  background: 'var(--bg-primary)',
                  color: 'var(--text-primary)',
                  fontSize: 'var(--fs-11)',
                  outline: 'none',
                  fontFamily: 'var(--font-mono)',
                }}
              />
              <button
                onClick={() => handleRemove(key)}
                className="flex items-center justify-center"
                style={{
                  width: 24,
                  height: 24,
                  flexShrink: 0,
                  borderRadius: 'var(--radius-md)',
                  border: 'none',
                  background: 'transparent',
                  color: 'var(--text-tertiary)',
                  fontSize: 'var(--fs-14)',
                  cursor: 'pointer',
                }}
                title="删除"
              >
                ✕
              </button>
            </div>
          ))}
          {(invalidKeys.size > 0 || dupKeys.size > 0) && (
            <div style={{ fontSize: 'var(--fs-10)', color: 'var(--color-error, #e53935)', marginTop: 2 }}>
              {Array.from(new Set([...invalidKeys, ...dupKeys])).map((k) => (
                <div key={k}>{k}：{getValidationHint(k)}</div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
};
export const WorkflowNodeConfig: React.FC<Props> = ({ node, onUpdate, onClose, onOpenSubflow }) => {
  const meta = getNodeTypeMeta(node.type);
  const configFields = NODE_TYPE_CONFIG_MAP[node.type]?.fields || [];
  const [params, setParams] = useState<Record<string, any>>(node.params || {});
  const { getEnabledAgentTypes } = useAgentRegistry();
  const enabledAgentTypes = getEnabledAgentTypes();
  const { definitions, loadDefinitions } = useWorkflowStore();
  const inputBaseKeyRef = useRef<string>('');
  const outputBaseKeyRef = useRef<string>('');

  useEffect(() => {
    if (node.type === 'subflow' && definitions.length === 0) {
      loadDefinitions();
    }
  }, [node.type]);

  const handleParamChange = (key: string, value: any) => {
    const newParams = { ...params, [key]: value };
    setParams(newParams);
    onUpdate({ params: newParams });
  };

  return (
    <div>
      {/* ===== 顶部标题栏 ===== */}
      <div
        className="flex items-center"
        style={{
          height: 40,
          padding: '0 4px',
          marginBottom: 12,
          borderBottom: '1px solid var(--border)',
        }}
      >
        {/* 图标 */}
        <div
          style={{
            width: 32,
            height: 32,
            borderRadius: 'var(--radius-md)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            fontSize: 'var(--fs-14)',
            flexShrink: 0,
            background: `${meta.color}20`,
            color: meta.color,
          }}
        >
          {meta.icon}
        </div>
        {/* 标题 + 副标题（单行布局，flex items-center 对齐中线） */}
        <div
          className="flex flex-col justify-center"
          style={{ marginLeft: 8, minWidth: 0, flex: 1 }}
        >
          <div
            style={{
              fontSize: 'var(--fs-14)',
              fontWeight: 'var(--fw-semibold)',
              lineHeight: '18px',
              color: 'var(--text-primary)',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {node.label}
          </div>
          <div
            style={{
              fontSize: 'var(--fs-10)',
              lineHeight: '14px',
              color: 'var(--text-tertiary)',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            节点类型：{meta.label}
          </div>
        </div>
        {/* 关闭按钮 */}
        <button
          onClick={onClose}
          className="flex items-center justify-center"
          style={{
            width: 28,
            height: 28,
            flexShrink: 0,
            marginLeft: 4,
            borderRadius: 'var(--radius-md)',
            border: '1px solid var(--border)',
            background: 'var(--bg-tertiary)',
            color: 'var(--text-secondary)',
            fontSize: 'var(--fs-12)',
            cursor: 'pointer',
          }}
        >
          ✕
        </button>
      </div>

      {/* ===== 基本信息 ===== */}
      <div style={{ marginBottom: 16 }}>
        <label style={S.label()}>节点名称</label>
        <input
          value={node.label}
          onChange={(e) => onUpdate({ label: e.target.value })}
          style={S.input()}
        />
      </div>

      {/* ===== 类型特定配置 ===== */}
      {configFields.length > 0 && (
        <div style={S.sectionGap}>
          <div style={S.sectionTitle}>节点配置</div>
          {configFields.map((field) => (
            <div key={field.key} style={S.fieldGap}>
              <label style={S.label()}>{field.label}</label>
              {field.type === 'textarea' ? (
                <textarea
                  value={params[field.key] || ''}
                  onChange={(e) => handleParamChange(field.key, e.target.value)}
                  placeholder={field.placeholder}
                  rows={4}
                  style={S.textarea}
                />
              ) : field.type === 'subflow_select' ? (
                <div>
                  <select
                    value={params[field.key] || ''}
                    onChange={(e) => handleParamChange(field.key, e.target.value)}
                    style={S.select({ marginBottom: 6 })}
                  >
                    <option value="">选择子工作流...</option>
                    {definitions.map((d) => (
                      <option key={d.id} value={d.id}>{d.name} ({d.id.slice(0, 8)}...)</option>
                    ))}
                  </select>
                  <div className="flex" style={{ gap: 4 }}>
                    {params[field.key] && (
                      <button
                        onClick={() => onOpenSubflow?.(params[field.key])}
                        className="flex-1"
                        style={{
                          padding: '4px 8px',
                          borderRadius: 'var(--radius-md)',
                          border: '1px solid var(--accent)',
                          background: 'var(--accent-light)',
                          color: 'var(--accent)',
                          fontSize: 'var(--fs-11)',
                          cursor: 'pointer',
                        }}
                      >
                        打开子工作流
                      </button>
                    )}
                    <button
                      onClick={() => {
                        const newId = 'wf_' + Date.now().toString(36);
                        handleParamChange(field.key, newId);
                      }}
                      className="flex-1"
                      style={{
                        padding: '4px 8px',
                        borderRadius: 'var(--radius-md)',
                        border: '1px dashed var(--border)',
                        background: 'transparent',
                        color: 'var(--text-tertiary)',
                        fontSize: 'var(--fs-11)',
                        cursor: 'pointer',
                      }}
                    >
                      新建子工作流
                    </button>
                  </div>
                </div>
              ) : field.type === 'select' ? (
                <select
                  value={params[field.key] || field.placeholder || ''}
                  onChange={(e) => handleParamChange(field.key, e.target.value)}
                  style={S.select()}
                >
                  <option value="">选择...</option>
                  {field.key === 'agent_type' && enabledAgentTypes.map((v) => <option key={v} value={v}>{v}</option>)}
                  {field.key === 'method' && ['GET', 'POST', 'PUT', 'DELETE'].map((v) => <option key={v} value={v}>{v}</option>)}
                  {field.key === 'inputType' && ['text', 'select', 'confirm', 'file'].map((v) => <option key={v} value={v}>{v}</option>)}
                </select>
              ) : (
                <input
                  type={field.type}
                  value={params[field.key] || ''}
                  onChange={(e) => handleParamChange(field.key, field.type === 'number' ? Number(e.target.value) : e.target.value)}
                  placeholder={field.placeholder}
                  style={S.input()}
                />
              )}
            </div>
          ))}
        </div>
      )}

      {/* ===== 输入输出映射 ===== */}
      <div style={S.sectionGap}>
        <div style={S.sectionTitle}>输入输出映射</div>
        <div style={{ marginBottom: 12 }}>
          <div className="flex items-center justify-between" style={{ marginBottom: 4 }}>
            <label style={S.labelSm(0)}>输入映射</label>
            <button
              onClick={() => {
                const k = nextMappingKey(Object.keys(node.inputMapping || {}), inputBaseKeyRef, 'input');
                onUpdate({ inputMapping: { ...(node.inputMapping || {}), [k]: '' } });
              }}
              className="flex items-center justify-center"
              style={{
                padding: '1px 4px',
                borderRadius: 'var(--radius-md)',
                border: 'none',
                background: 'transparent',
                color: 'var(--accent)',
                fontSize: 'var(--fs-11)',
                cursor: 'pointer',
                flexShrink: 0,
                lineHeight: '18px',
              }}
            >
              添加
            </button>
          </div>
          <MappingEditor
            value={node.inputMapping}
            onChange={(v) => {
              onUpdate({ inputMapping: v });
              // 更新 baseKeyRef
              const keys = Object.keys(v || {});
              if (keys.length === 1) inputBaseKeyRef.current = keys[0];
            }}
            keyPlaceholder="参数名"
            valuePlaceholder={'{{nodes.nodeId.output.field}}'}
            baseKeyRef={inputBaseKeyRef}
          />
        </div>
        <div>
          <div className="flex items-center justify-between" style={{ marginBottom: 4 }}>
            <label style={S.labelSm(0)}>输出映射</label>
            <button
              onClick={() => {
                const k = nextMappingKey(Object.keys(node.outputMapping || {}), outputBaseKeyRef, 'output');
                onUpdate({ outputMapping: { ...(node.outputMapping || {}), [k]: '' } });
              }}
              className="flex items-center justify-center"
              style={{
                padding: '1px 4px',
                borderRadius: 'var(--radius-md)',
                border: 'none',
                background: 'transparent',
                color: 'var(--accent)',
                fontSize: 'var(--fs-11)',
                cursor: 'pointer',
                flexShrink: 0,
                lineHeight: '18px',
              }}
            >
              添加
            </button>
          </div>
          <MappingEditor
            value={node.outputMapping}
            onChange={(v) => {
              onUpdate({ outputMapping: v });
              const keys = Object.keys(v || {});
              if (keys.length === 1) outputBaseKeyRef.current = keys[0];
            }}
            keyPlaceholder="输出字段名"
            valuePlaceholder={'{{context.path}}'}
            baseKeyRef={outputBaseKeyRef}
            keyOptions={(() => {
              const schema = NODE_OUTPUT_SCHEMA[node.type];
              if (!schema || schema.length === 0) return undefined;
              return schema.map(g => ({
                group: g.group,
                options: g.fields.map(f => ({
                  value: f.name,
                  label: f.label,
                  description: f.description,
                })),
              }));
            })()}
          />
        </div>
      </div>

      {/* ===== 控制属性 ===== */}
      <div style={S.sectionGap}>
        <div style={S.sectionTitle}>控制属性</div>
        <div className="grid grid-cols-2" style={{ gap: 8 }}>
          {[
            { key: 'delayMs', label: '延迟 (ms)', ph: '0' },
            { key: 'timeoutMs', label: '超时 (ms)', ph: '60000' },
            { key: 'retryCount', label: '重试次数', ph: '0' },
            { key: 'retryDelayMs', label: '重试间隔 (ms)', ph: '1000' },
          ].map((item) => (
            <div key={item.key}>
              <label style={S.labelSm()}>{item.label}</label>
              <input
                type="number"
                value={(node as any)[item.key] || ''}
                onChange={(e) => onUpdate({ [item.key]: e.target.value ? Number(e.target.value) : undefined })}
                placeholder={item.ph}
                style={S.input('var(--fs-11)', '4px 8px')}
              />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
};
