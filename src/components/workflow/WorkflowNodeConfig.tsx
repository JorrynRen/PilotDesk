/**
 * WorkflowNodeConfig — 节点配置面板
 *
 * 适配 6 种实体节点类型 + 控制属性（延迟/超时/重试）。
 * 所有样式均使用 CSS 变量，无硬编码色值。
 */

import React, { useState, useEffect, useRef } from 'react';
import type { WorkflowNode, WorkflowNodeType, Stage } from '../../types/workflow';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';
import { useWorkflowStore } from '../../stores/workflowStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';

interface Props {
  node: WorkflowNode;
  onUpdate: (updates: Partial<WorkflowNode>) => void;
  onClose: () => void;
  onOpenSubflow?: (definitionId: string) => void;
  /** 所有阶段（用于输入映射计算前序节点） */
  stages?: Stage[];
  /** 阶段间连线（用于阶段拓扑前序判断） */
  stageEdges?: WorkflowEdge[];
}

const NODE_TYPE_CONFIG_MAP: Record<WorkflowNodeType, { fields: { key: string; label: string; type: string; placeholder?: string }[] }> = {
  agent: {
    fields: [
      { key: 'agent_type', label: 'Agent 类型', type: 'select', placeholder: 'claude' },
      { key: 'prompt_template', label: '提示词模板', type: 'textarea', placeholder: '请输入提示词模板，支持 {{variable}}' },
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

/* ---------- 输出字段选项（按节点类型动态获取） ---------- */

interface OptionGroup {
  group: string;
  children?: OptionGroup[];
  options: { value: string; label: string }[];
}

function getOutputFieldOptions(nodeType: WorkflowNodeType): OptionGroup[] | undefined {
  // 新架构：所有节点类型的输出统一为 content 体系
  // 输出映射的 value（引用路径）可选项：
  //   content          → 节点执行结果（全部）
  //   session_id       → agent 节点会话ID
  //   xxx.content      → content 对象的 xxx 属性
  //   content[N]        → content 数组的第 N 个元素
  // 用户也可直接输入自定义路径
  if (nodeType === 'start') return undefined; // 开始节点无输出映射

  const baseOptions: { value: string; label: string }[] = [
    { value: 'content', label: 'content（执行结果）' },
  ];

  // agent 类型节点额外提供 session_id
  if (nodeType === 'agent') {
    baseOptions.push({ value: 'session_id', label: 'session_id（会话ID）' });
  }

  return [{
    group: '本节点输出',
    children: [],
    options: baseOptions,
  }];
}

/* ---------- 前序节点输出选项（输入映射用） ---------- */

/**
 * 计算前序阶段的门控合并输出键名
 * 根据阶段配置和节点 outputMapping 推导 gate merge 后的可用字段
 */
/**
 * 计算前序阶段的门控合并输出选项（作为阶段分组的二级子项）
 * 返回 Map: stageName -> gate child group（含 options）
 */
function getGateMergeOptions(
  predecessorNodes: { stageName: string; node: WorkflowNode }[],
  stageId: string,
  gateConfig: GateConfig | undefined,
): { value: string; label: string }[] | null {
  if (predecessorNodes.length === 0) return null;

  const mergeStrategy = gateConfig?.mergeStrategy || 'merge';
  const value = `gate_output.${stageId}`;

  if (mergeStrategy === 'custom') {
    return [{ value: '', label: '(自定义策略，暂不可引用)' }];
  }

  return [{
    value: `{{${value}}}`,
    label: `门控合并 (${mergeStrategy})`,
  }];
}



function getPredecessorOutputOptions(
  nodeId: string,
  stages: Stage[],
  stageEdges?: WorkflowEdge[],
): OptionGroup[] {
  // 1. 找到当前节点所在阶段索引
  let currentStageIdx = -1;
  for (let i = 0; i < stages.length; i++) {
    if (stages[i].nodes.some(n => n.id === nodeId)) {
      currentStageIdx = i;
      break;
    }
  }
  if (currentStageIdx === -1) return [];

  // 2. 按拓扑序收集前序节点（只能引用拓扑顺序在前面的节点）
  //    - 前序阶段的所有节点
  //    - 同阶段中通过边指向当前节点的源节点
  const predecessorNodes: { stageName: string; stageId: string; node: WorkflowNode }[] = [];
  const seenNodeIds = new Set<string>();

  // 前序阶段的所有节点
  for (let i = 0; i < currentStageIdx; i++) {
    for (const n of stages[i].nodes) {
      if (!seenNodeIds.has(n.id)) {
        seenNodeIds.add(n.id);
        predecessorNodes.push({ stageName: stages[i].name, stageId: stages[i].id, node: n });
      }
    }
  }

  // 同阶段中，通过边指向当前节点的拓扑前序节点（传递上游搜索）
  //    与 sanitizeMappingReferences 的 upstreamMap 逻辑一致
  const currentStage = stages[currentStageIdx];
  // BFS 传递上游搜索：从直接入边节点开始，递归收集所有上游节点
  const sameStageUpstream = new Set<string>();
  const queue: string[] = [];
  const directIncoming = currentStage.edges.filter(e => e.target === nodeId);
  for (const edge of directIncoming) {
    if (!sameStageUpstream.has(edge.source)) {
      sameStageUpstream.add(edge.source);
      queue.push(edge.source);
    }
  }
  while (queue.length > 0) {
    const current = queue.shift()!;
    const incomingToCurrent = currentStage.edges.filter(e => e.target === current);
    for (const edge of incomingToCurrent) {
      if (!sameStageUpstream.has(edge.source)) {
        sameStageUpstream.add(edge.source);
        queue.push(edge.source);
      }
    }
  }
  for (const upstreamId of sameStageUpstream) {
    const sourceNode = currentStage.nodes.find(n => n.id === upstreamId);
    if (sourceNode && !seenNodeIds.has(sourceNode.id)) {
      seenNodeIds.add(sourceNode.id);
      predecessorNodes.push({ stageName: currentStage.name, stageId: currentStage.id, node: sourceNode });
    }
  }

  if (predecessorNodes.length === 0) return [];

  // 3. 构建结果：阶段 → [node] 分组（节点字段） + gate_output（二级选项）
  //    新架构引用格式：{{key.节点ID.阶段ID}}
  //    outputMapping 的 key 是用户自定义参数名，value 是引用路径
  //    前序节点通过 outputMapping 的 key 暴露数据
  const result: OptionGroup[] = [];
  const stageGroups = new Map<string, OptionGroup>();

  // 3.1 收集所有前序阶段名（用于后续注入 gate_output）
  const allPredecessorStageNames = new Set<string>();
  for (const p of predecessorNodes) {
    allPredecessorStageNames.add(p.stageName);
  }

  // 3.2 按阶段分组收集节点输出
  for (const { stageName, stageId, node: pn } of predecessorNodes) {
    // 确保阶段分组存在
    if (!stageGroups.has(stageName)) {
      stageGroups.set(stageName, {
        group: stageName,
        children: [],
        options: [],
      });
    }
    const stageGroup = stageGroups.get(stageName)!;

    // 节点输出映射：key 是用户自定义参数名，value 是引用路径（如 content, session_id 等）
    const outputMapping = pn.outputMapping || {};
    const outputKeys = Object.keys(outputMapping);

    if (outputKeys.length === 0 && pn.type !== 'agent') continue;

    const fieldList: { value: string; label: string }[] = [];

    // outputMapping 中已声明的 key（用户自定义参数名）
    for (const key of outputKeys) {
      // 引用格式：{{用户参数名.节点ID.阶段ID}}
      fieldList.push({
        value: `{{${key}.${pn.id}.${stageId}}}`,
        label: key,
      });
    }

    // Agent 节点的 session_id 需通过 outputMapping 显式声明后才能被引用
    // 不再自动注入，确保遵循"变量通过输出映射显式声明"原则

    // 只有有有效字段的节点才添加到分组
    if (fieldList.length > 0) {
      stageGroup.children!.push({
        group: pn.label.startsWith('[') ? pn.label : `[node] ${pn.label}`,
        options: fieldList,
      });
    }
  }

  // 3.3 为每个前序阶段注入 gate_output 作为二级选项
  //    本阶段节点不能引用本阶段门控合并变量（门控合并在本阶段所有节点执行完后才产生）
  //    使用阶段拓扑前序关系（stageEdges）判断
  const stageUpstreamSet = new Set<string>();
  if (stages.length > 0 && currentStage && stageEdges && stageEdges.length > 0) {
    // 基于阶段连线构建上游集合（BFS 传递）
    const stageIdSet = new Set(stages.map(s => s.id));
    for (const edge of stageEdges) {
      if (stageIdSet.has(edge.source) && stageIdSet.has(edge.target)) {
        if (edge.target === currentStage.id) {
          stageUpstreamSet.add(edge.source);
        }
      }
    }
    // 传递上游
    let changed = true;
    while (changed) {
      changed = false;
      for (const edge of stageEdges) {
        if (stageUpstreamSet.has(edge.source) && !stageUpstreamSet.has(edge.target) && stageIdSet.has(edge.target)) {
          stageUpstreamSet.add(edge.target);
          changed = true;
        }
      }
    }
    // stageUpstreamSet 现在包含所有通过 stageEdges 可达 currentStage.id 的上游阶段
    // 但我们只需要 currentStage 的直接上游集合，所以取交集减去自身
    stageUpstreamSet.delete(currentStage.id);
  }

  for (const stageName of allPredecessorStageNames) {
    const stageObj = stages.find(s => s.name === stageName);
    if (!stageObj || !stageObj.gate) continue;
    // 跳过当前阶段（本阶段门控合并在本阶段所有节点执行完后才产生）
    if (stageObj.id === currentStage.id) continue;
    // 必须为阶段拓扑前序
    if (!stageUpstreamSet.has(stageObj.id)) continue;

    const stagePredecessors = predecessorNodes.filter(p => p.stageName === stageName);
    const gateOptions = getGateMergeOptions(stagePredecessors, stageObj.id, stageObj.gate);
    if (!gateOptions) continue;

    const resultStage = stageGroups.get(stageName);
    if (resultStage) {
      resultStage.options.push(...gateOptions);
    }
  }

  // 4. 将阶段分组按拓扑序排列
  for (const stageName of allPredecessorStageNames) {
    const stageObj = stages.find(s => s.name === stageName);
    if (stageObj && stageGroups.has(stageName)) {
      result.push(stageGroups.get(stageName)!);
    }
  }

  return result;
}

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
  valueOptions?: OptionGroup[];
}> = ({ value, onChange, keyPlaceholder, valuePlaceholder, baseKeyRef, valueOptions }) => {
  const entries = Object.entries(value || {});
  const [invalidKeys, setInvalidKeys] = useState<Set<string>>(new Set());
  const [dupKeys, setDupKeys] = useState<Set<string>>(new Set());
  const [activeDropdown, setActiveDropdown] = useState<string | null>(null);

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

  /** 渲染 key 列：始终为文本输入框 */
  const renderKeyColumn = (key: string) => {
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
          {entries.map(([key, val], idx) => (
            <div key={idx} className="flex items-center" style={{ gap: 4, minWidth: 0 }}>
              {renderKeyColumn(key)}
              <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--fs-11)', flexShrink: 0 }}>→</span>
              <div style={{ flex: 1, minWidth: 0, position: 'relative' }}>
                <div className="flex items-center" style={{ gap: 2 }}>
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
                  {valueOptions !== undefined && (
                    <button
                      onClick={() => setActiveDropdown(activeDropdown === key ? null : key)}
                      className="flex items-center justify-center"
                      style={{
                        width: 22,
                        height: 22,
                        flexShrink: 0,
                        borderRadius: 'var(--radius-md)',
                        border: '1px solid var(--border)',
                        background: 'var(--bg-tertiary)',
                        color: 'var(--text-secondary)',
                        fontSize: 'var(--fs-12)',
                        cursor: 'pointer',
                        lineHeight: 1,
                      }}
                      title="选择字段"
                    >
                      ▾
                    </button>
                  )}
                </div>
                {activeDropdown === key && valueOptions && (
                  <>
                    {/* 点击遮罩关闭下拉 */}
                    <div
                      onClick={() => setActiveDropdown(null)}
                      style={{
                        position: 'fixed',
                        inset: 0,
                        zIndex: 99,
                      }}
                    />
                    <div
                      style={{
                        position: 'absolute',
                        right: 0,
                        top: '100%',
                        zIndex: 100,
                        minWidth: 200,
                        maxHeight: 200,
                        overflow: 'auto',
                        borderRadius: 'var(--radius-md)',
                        border: '1px solid var(--border)',
                        background: 'var(--bg-primary)',
                        boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
                        fontSize: 'var(--fs-11)',
                      }}
                    >
                      <div
                        style={{
                          padding: '6px 8px',
                          fontSize: 'var(--fs-11)',
                          color: 'var(--text-secondary)',
                          fontWeight: 600,
                          borderBottom: '1px solid var(--border)',
                          textAlign: 'center',
                        }}
                      >
                        请选择…
                      </div>
                      {(() => {
                        const hasOptions = valueOptions.some(g => g.options.length > 0 || (g.children && g.children.some(c => c.options.length > 0)));
                        if (!hasOptions) {
                          return (
                            <div
                              style={{
                                padding: '12px 8px',
                                fontSize: 'var(--fs-11)',
                                color: 'var(--text-tertiary)',
                                textAlign: 'center',
                              }}
                            >
                              暂无可用字段
                            </div>
                          );
                        }
                        return valueOptions.map((stage, si) => (
                          <div key={si}>
                            {/* 第一级：[序号] 阶段名 */}
                            {stage.group && (
                              <div
                                style={{
                                  padding: '4px 8px',
                                  fontSize: 'var(--fs-10)',
                                  color: 'var(--text-tertiary)',
                                  fontWeight: 600,
                                  borderBottom: '1px solid var(--border)',
                                  background: 'var(--bg-tertiary)',
                                }}
                              >
                                [step{si + 1}] {stage.group}
                              </div>
                            )}
                            {/* 二级：gate_output 选项 + [node] 分组 */}
                            {/* gate_output 选项（直接可点，和[node]标题同级） */}
                            {stage.options.length > 0 && stage.options.map((opt) => (
                              <div
                                key={opt.value}
                                onClick={() => {
                                  handleValueChange(key, opt.value);
                                  setActiveDropdown(null);
                                }}
                                style={{
                                  padding: '6px 8px',
                                  cursor: 'pointer',
                                  color: 'var(--text-primary)',
                                  borderBottom: '1px solid var(--border)',
                                }}
                                onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--bg-tertiary)'; }}
                                onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                              >
                                {opt.label}
                              </div>
                            ))}
                            {/* [node] 分组（标题+嵌套字段） */}
                            {stage.children && stage.children.map((nodeGroup, ni) => (
                              <div key={ni}>
                                <div
                                  style={{
                                    padding: '6px 8px',
                                    fontSize: 'var(--fs-10)',
                                    color: 'var(--text-secondary)',
                                    fontWeight: 500,
                                    borderBottom: '1px solid var(--border)',
                                  }}
                                >
                                  {nodeGroup.group.startsWith('[') ? nodeGroup.group : `[node] ${nodeGroup.group}`}
                                </div>
                                {nodeGroup.options.map((opt) => (
                                  <div
                                    key={opt.value}
                                    onClick={() => {
                                      handleValueChange(key, opt.value);
                                      setActiveDropdown(null);
                                    }}
                                    style={{
                                      padding: '5px 8px 5px 20px',
                                      cursor: 'pointer',
                                      color: 'var(--text-primary)',
                                      borderBottom: '1px solid var(--border)',
                                    }}
                                    onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--bg-tertiary)'; }}
                                    onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                                  >
                                    {opt.label}
                                  </div>
                                ))}
                              </div>
                            ))}
                          </div>
                        ));
                      })()}
                    </div>
                  </>
                )}
              </div>
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
export const WorkflowNodeConfig: React.FC<Props> = ({ node, onUpdate, onClose, onOpenSubflow, stages, stageEdges }) => {
  const meta = getNodeTypeMeta(node.type);
  const configFields = NODE_TYPE_CONFIG_MAP[node.type]?.fields || [];
  const [params, setParams] = useState<Record<string, any>>(node.params || {});
  const { getEnabledAgentTypes } = useAgentRegistry();
  const enabledAgentTypes = getEnabledAgentTypes();
  const { definitions, loadDefinitions } = useWorkflowStore();
  const inputBaseKeyRef = useRef<string>('');
  const outputBaseKeyRef = useRef<string>('');
  const [fieldSelectorKey, setFieldSelectorKey] = useState<string | null>(null);
  const [resumeDropdownOpen, setResumeDropdownOpen] = useState(false);
  const [selectorPos, setSelectorPos] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const triggerCursorPosRef = useRef<number>(0);
  const panelRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (node.type === 'subflow' && definitions.length === 0) {
      loadDefinitions();
    }
  }, [node.type]);

  // 切换节点时同步 params 状态，避免状态串扰
  useEffect(() => {
    setParams(node.params || {});
  }, [node.id]);

  const handleParamChange = (key: string, value: any) => {
    const newParams = { ...params, [key]: value };
    setParams(newParams);
    onUpdate({ params: newParams });
  };

  /** 估算 textarea 中光标像素位置 */
  const getCursorPixelPosition = (textarea: HTMLTextAreaElement): { x: number; y: number } => {
    const rect = textarea.getBoundingClientRect();
    const text = textarea.value.slice(0, textarea.selectionStart);
    const lines = text.split('\n');
    const currentLine = lines.length - 1;
    const style = getComputedStyle(textarea);
    const lineHeight = parseInt(style.lineHeight) || 20;
    const paddingTop = parseInt(style.paddingTop) || 6;
    const panelRect = panelRef.current?.getBoundingClientRect();
    return {
      x: panelRect ? panelRect.left + panelRect.width / 2 : rect.left + rect.width / 2,
      y: rect.top + paddingTop + (currentLine + 1) * lineHeight,
    };
  };

  const handleTextareaChange = (key: string, value: string, cursorPos: number, textarea?: HTMLTextAreaElement) => {
    handleParamChange(key, value);
    // 检测光标前是否刚输入了 {{
    const textBefore = value.slice(0, cursorPos);
    if (textBefore.endsWith('{{') && stages) {
      setFieldSelectorKey(key);
      triggerCursorPosRef.current = cursorPos;
      if (textarea) {
        setSelectorPos(getCursorPixelPosition(textarea));
      }
    }
  };

  const insertFieldAtCursor = (key: string, fieldValue: string) => {
    const currentVal = (params[key] as string) || '';
    const pos = triggerCursorPosRef.current;
    if (pos <= 0) return;
    const newVal = currentVal.slice(0, pos - 2) + '{{' + fieldValue + '}}' + currentVal.slice(pos);
    handleParamChange(key, newVal);
    setFieldSelectorKey(null);
  };

  /** 输入映射参数选项（用于 textarea {{ 触发选择器） */
  const textareaFieldOptions: OptionGroup[] | undefined = (() => {
    const keys = Object.keys(node.inputMapping || {});
    if (keys.length === 0) return undefined;
    return [{
      group: '',
      children: [{ group: '输入参数', options: keys.map(k => ({ value: k, label: k })) }],
      options: [],
    }];
  })();

  return (
    <div ref={panelRef}>
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
        <div style={S.sectionTitle}>基本信息</div>
        <label style={S.label()}>节点名称</label>
        <input
          value={node.label}
          onChange={(e) => onUpdate({ label: e.target.value })}
          style={S.input()}
        />
        {configFields.length > 0 && (
          <div style={{ marginTop: 10 }}>
            <label style={S.label()}>{configFields[0].label}</label>
            {configFields[0].type === 'select' ? (
              <div style={{ display: 'flex', gap: 8, alignItems: 'flex-end' }}>
                <select
                  value={params[configFields[0].key] || configFields[0].placeholder || ''}
                  onChange={(e) => handleParamChange(configFields[0].key, e.target.value)}
                  style={S.select()}
                >
                  <option value="">选择...</option>
                  {configFields[0].key === 'agent_type' && enabledAgentTypes.map((v) => <option key={v} value={v}>{v}</option>)}
                  {configFields[0].key === 'method' && ['GET', 'POST', 'PUT', 'DELETE'].map((v) => <option key={v} value={v}>{v}</option>)}
                  {configFields[0].key === 'inputType' && ['text', 'select', 'confirm', 'file'].map((v) => <option key={v} value={v}>{v}</option>)}
                </select>
                {/* 会话方式 — 与 Agent 类型同行 */}
                {configFields[0].key === 'agent_type' && (() => {
                  const sessionMode = params['session_mode'] || 'new';
                  return (
                    <>
                      <select
                        value={sessionMode}
                        onChange={(e) => handleParamChange('session_mode', e.target.value)}
                        style={S.select({ minWidth: 90 })}
                      >
                        <option value="new">新会话</option>
                        <option value="resume">延续会话</option>
                      </select>
                      {sessionMode === 'resume' && (() => {
                        const resumeRef = params['resume_session_ref'] || '';
                        const groups = stages ? getPredecessorOutputOptions(node.id, stages, stageEdges) : [];
                        const hasAny = groups.some(g => g.options.length > 0 || (g.children && g.children.some(c => c.options.length > 0)));
                        // 从 value 反查可读 label
                        const displayLabel = (() => {
                          if (!resumeRef) return '';
                          for (const g of groups) {
                            if (g.options) for (const opt of g.options) if (opt.value === resumeRef) return opt.label;
                            if (g.children) for (const c of g.children) for (const opt of c.options) if (opt.value === resumeRef) return opt.label;
                          }
                          return resumeRef;
                        })();
                        return (
                          <div style={{ position: 'relative' }}>
                            <div
                              onClick={() => setResumeDropdownOpen(v => !v)}
                              style={{
                                ...S.select({ minWidth: 140 }),
                                cursor: 'pointer',
                                display: 'flex',
                                alignItems: 'center',
                                justifyContent: 'space-between',
                                userSelect: 'none',
                              }}
                            >
                              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{displayLabel || '请选择会话变量'}</span>
                              <span style={{ fontSize: 'var(--fs-9)', opacity: 0.5 }}>{resumeDropdownOpen ? '▲' : '▼'}</span>
                            </div>
                            {resumeDropdownOpen && (
                              <div
                                style={{
                                  position: 'absolute',
                                  left: 0,
                                  top: '100%',
                                  width: '100%',
                                  zIndex: 100,
                                  maxHeight: 200,
                                  overflow: 'auto',
                                  borderRadius: 'var(--radius-md)',
                                  border: '1px solid var(--border)',
                                  background: 'var(--bg-primary)',
                                  boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
                                  fontSize: 'var(--fs-11)',
                                }}
                                onClick={(e) => e.stopPropagation()}
                              >
                                {!hasAny && (
                                  <div style={{ padding: '12px 8px', color: 'var(--text-tertiary)', textAlign: 'center' }}>暂无可用字段</div>
                                )}
                                {groups.map((stage, si) => (
                                  <div key={si}>
                                    {stage.group && (
                                      <div style={{ padding: '4px 8px', fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', fontWeight: 600, borderBottom: '1px solid var(--border)', background: 'var(--bg-tertiary)' }}>
                                        [step{si + 1}] {stage.group}
                                      </div>
                                    )}
                                    {stage.options.length > 0 && stage.options.map((opt) => (
                                      <div key={opt.value} onClick={() => { handleParamChange('resume_session_ref', opt.value); setResumeDropdownOpen(false); }} style={{ padding: '6px 8px', cursor: 'pointer', color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }} onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--bg-tertiary)'; }} onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}>
                                        {opt.label}
                                      </div>
                                    ))}
                                    {stage.children && stage.children.map((nodeGroup, ni) => (
                                      <div key={ni}>
                                        <div style={{ padding: '6px 8px', fontSize: 'var(--fs-10)', color: 'var(--text-secondary)', fontWeight: 500, borderBottom: '1px solid var(--border)' }}>
                                          {nodeGroup.group.startsWith('[') ? nodeGroup.group : `[node] ${nodeGroup.group}`}
                                        </div>
                                        {nodeGroup.options.map((opt) => (
                                          <div key={opt.value} onClick={() => { handleParamChange('resume_session_ref', opt.value); setResumeDropdownOpen(false); }} style={{ padding: '6px 8px 6px 20px', cursor: 'pointer', color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }} onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--bg-tertiary)'; }} onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}>
                                            {opt.label}
                                          </div>
                                        ))}
                                      </div>
                                    ))}
                                  </div>
                                ))}
                              </div>
                            )}
                          </div>
                          <div style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', marginTop: 4, flexBasis: '100%' }}>
                            选择前序节点通过输出映射声明的会话变量，用于延续该节点的 Agent 会话上下文
                          </div>
                        );
                      })()}
                    </>
                  );
                })()}
              </div>
            ) : (
              <input
                type={configFields[0].type || 'text'}
                value={params[configFields[0].key] || ''}
                onChange={(e) => handleParamChange(configFields[0].key, configFields[0].type === 'number' ? Number(e.target.value) : e.target.value)}
                placeholder={configFields[0].placeholder}
                style={S.input()}
              />
            )}
            {/* 延续会话空状态提示 — 仅 agent + resume 模式 */}
            {configFields[0].key === 'agent_type' && params['session_mode'] === 'resume' && (() => {
              if (!stages) return null;
              // 复用 getPredecessorOutputOptions 检查是否有可用选项
              const groups = getPredecessorOutputOptions(node.id, stages, stageEdges);
              const hasOptions = groups.length > 0;
              if (hasOptions) return null;
              return (
                <div style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', marginTop: 4 }}>
                  提示：前序节点暂无可引用的输出变量，请确保前序节点已配置输出映射
                </div>
              );
            })()}
          </div>
        )}
      </div>

      {/* ===== 输入映射 ===== */}
      {node.type !== 'start' && (
      <div style={S.sectionGap}>
        <div className="flex items-center justify-between" style={{ marginBottom: 8 }}>
          <div style={S.sectionTitle}>输入映射</div>
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
              const keys = Object.keys(v || {});
              if (keys.length === 1) inputBaseKeyRef.current = keys[0];
            }}
            keyPlaceholder="参数名"
            valuePlaceholder={'输入文本或选择前序字段'}
            baseKeyRef={inputBaseKeyRef}
            valueOptions={stages ? getPredecessorOutputOptions(node.id, stages, stageEdges) : undefined}
          />
      </div>
      )}

      {/* ===== 节点配置 ===== */}
      {configFields.length > 0 && (
        <div style={S.sectionGap}>
          <div style={S.sectionTitle}>节点配置</div>

          {/* 其余配置字段（跳过第一个已在基本信息中） */}
          {configFields.slice(1).map((field) => (
            <div key={field.key} style={S.fieldGap}>
              <label style={S.label()}>{field.label}</label>
              {field.type === 'textarea' ? (
                <div style={{ position: 'relative' }}>
                  <textarea
                    value={params[field.key] || ''}
                    onChange={(e) => handleTextareaChange(field.key, e.target.value, e.target.selectionStart || 0, e.target)}
                    placeholder={field.placeholder}
                    rows={4}
                    style={S.textarea}
                  />
                  {/* {{ 触发选择器 — 跟随光标 + 可拖动 */}
                  {fieldSelectorKey === field.key && textareaFieldOptions && textareaFieldOptions.length > 0 && (
                    <>
                      <div
                        onClick={() => setFieldSelectorKey(null)}
                        style={{ position: 'fixed', inset: 0, zIndex: 99 }}
                      />
                      <div
                        style={{
                          position: 'fixed',
                          left: selectorPos.x,
                          top: selectorPos.y,
                          transform: 'translateX(-50%)',
                          zIndex: 100,
                          minWidth: 200,
                          maxHeight: 250,
                          display: 'flex',
                          flexDirection: 'column',
                          borderRadius: 'var(--radius-md)',
                          border: '1px solid var(--border)',
                          background: 'var(--bg-primary)',
                          boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
                          fontSize: 'var(--fs-11)',
                        }}
                      >
                        {/* 可拖动标题栏 */}
                        <div
                          onMouseDown={(e) => {
                            e.stopPropagation();
                            const startX = e.clientX;
                            const startY = e.clientY;
                            const startPos = { ...selectorPos };
                            const handleMouseMove = (ev: MouseEvent) => {
                              setSelectorPos({
                                x: startPos.x + (ev.clientX - startX),
                                y: startPos.y + (ev.clientY - startY),
                              });
                            };
                            const handleMouseUp = () => {
                              document.removeEventListener('mousemove', handleMouseMove);
                              document.removeEventListener('mouseup', handleMouseUp);
                            };
                            document.addEventListener('mousemove', handleMouseMove);
                            document.addEventListener('mouseup', handleMouseUp);
                          }}
                          style={{
                            padding: '6px 8px',
                            fontSize: 'var(--fs-11)',
                            color: 'var(--text-secondary)',
                            fontWeight: 600,
                            borderBottom: '1px solid var(--border)',
                            textAlign: 'center',
                            cursor: 'grab',
                            userSelect: 'none',
                            flexShrink: 0,
                          }}
                        >
                          请选择…
                        </div>
                        {/* 可滚动内容区 */}
                        <div style={{ overflow: 'auto', flex: 1 }}>
                          {(() => {
                            const hasOptions = textareaFieldOptions.some(g => g.options.length > 0 || (g.children && g.children.some(c => c.options.length > 0)));
                            if (!hasOptions) {
                              return <div style={{ padding: '12px 8px', fontSize: 'var(--fs-11)', color: 'var(--text-tertiary)', textAlign: 'center' }}>暂无可用字段</div>;
                            }
                            return textareaFieldOptions.map((stage, si) => (
                              <div key={si}>
                                {stage.children && stage.children.map((nodeGroup, ni) => (
                                  <div key={ni}>
                                    <div style={{ padding: '3px 8px', fontSize: 'var(--fs-10)', color: 'var(--text-secondary)', fontWeight: 500, borderBottom: '1px solid var(--border)' }}>
                                      {nodeGroup.group.startsWith('[') ? nodeGroup.group : `[node] ${nodeGroup.group}`}
                                    </div>
                                    {nodeGroup.options.map((opt) => (
                                      <div
                                        key={opt.value}
                                        onClick={() => { insertFieldAtCursor(field.key, opt.value); }}
                                        style={{ padding: '5px 8px 5px 20px', cursor: 'pointer', color: 'var(--text-primary)', borderBottom: '1px solid var(--border)' }}
                                        onMouseEnter={(e) => { e.currentTarget.style.background = 'var(--bg-tertiary)'; }}
                                        onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                                      >
                                        {opt.label}
                                      </div>
                                    ))}
                                  </div>
                                ))}
                              </div>
                            ));
                          })()}
                        </div>
                      </div>
                    </>
                  )}
                </div>
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

      {/* ===== 输出映射 ===== */}
      <div style={S.sectionGap}>
        <div className="flex items-center justify-between" style={{ marginBottom: 8 }}>
          <div style={S.sectionTitle}>输出映射</div>
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
            valuePlaceholder={'输入文本或选择输出字段'}
            baseKeyRef={outputBaseKeyRef}
            valueOptions={getOutputFieldOptions(node.type)} />
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
