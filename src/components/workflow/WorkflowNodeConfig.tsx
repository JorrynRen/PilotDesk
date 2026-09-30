/**
 * WorkflowNodeConfig — 节点配置面板
 *
 * 适配 6 种实体节点类型 + 控制属性（延迟/超时/重试）。
 * 所有样式均使用 CSS 变量，无硬编码色值。
 */

import React, { useState, useEffect, useRef, useMemo } from 'react';
import type { WorkflowNode, WorkflowNodeType, Stage, WorkflowEdge, GateConfig } from '../../types/workflow';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';
import { invoke } from '@tauri-apps/api/core';
import { useWorkflowStore } from '../../stores/workflowStore';
import { useSessionStore } from '../../stores/sessionStore';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { usePluginStore, pluginCommands, findPluginCommand, commandParamKeys, pluginSupportsWorkflow, applyCommandParamDefaults } from '../../stores/pluginStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { isWorkflowSession } from '../../utils/sessionType';
import { pluginRegistry } from '../../plugin/PluginRegistry';
import { SchemaParamForm } from '../plugin/SchemaParamForm';
import { TemplateField } from './TemplateField';
import { TemplateVariablesContext } from './templateVariables';
import type { TemplateVariableGroup } from './templateVariables';
import type { SecurityModeValue } from '../security/SecurityModeSelector';
import { Select } from '../common/Select';

/**
 * API Agent 节点的「工具授权」（`security_mode`，节点级）。
 *
 * 审批尺度**只由这个模式决定**（`approval_floor`，见 agent_loop.rs）：模式越严，
 * 越低风险的工具调用也要确认；命令与路径另按各自的子策略矩阵分级。
 * 命中审批的调用会暂停并等待用户在工作流页/通知中心确认（超时按拒绝）。
 */
const SECURITY_MODE_OPTIONS: { value: SecurityModeValue; label: string; workflowEffect: string }[] = [
  { value: 'strict', label: '严格', workflowEffect: '所有工具调用（含只读、工作区内读写）都会暂停等待确认（最保守）' },
  { value: 'standard', label: '标准', workflowEffect: '只读与工作区内读写免确认；风险/未知命令与路径、中高风险工具会暂停等待确认 ← 默认' },
  { value: 'relaxed', label: '宽松', workflowEffect: '工作区内读写与命令默认放行，仅高风险操作会暂停等待确认' },
  { value: 'unrestricted', label: '无限制', workflowEffect: '全部放行、不做任何确认（高风险，慎用）' },
];

/**
 * 人工交互节点的「输入类型」选项：`value` 是后端契约（保持英文，不能改），
 * 显示文案补中文备注，避免只看到 text/select/confirm/file 不知所指。
 */
const INPUT_TYPE_OPTIONS: { value: string; label: string }[] = [
  { value: 'text', label: 'text（单行文本）' },
  { value: 'select', label: 'select（下拉选择）' },
  { value: 'confirm', label: 'confirm（确认 / 取消）' },
  { value: 'file', label: 'file（选择文件，读取其内容）' },
];

/**
 * 「已有会话」标签里标题可占的**显示字数**（全角/中日韩按 1 计，ASCII 按 0.55 计）。
 *
 * 面板内容宽 320px、选项内边距 16+8（可用约 296px），尾部时间戳 `· MM-DD HH:mm` 约 70px，
 * 11px 字号下一个全角字约 11px —— 18 个全角字的预算 + 时间戳 ≈ 268px，
 * 保证「标题 · 时间」整条在一行内显示完（标题超出即截断加省略号，时间戳始终完整）。
 */
const SESSION_TITLE_DISPLAY_BUDGET = 18;

/** 按显示宽度截断会话标题：超出预算即截断并加省略号，使标题与时间戳能落在同一行 */
function truncateSessionTitle(title: string): string {
  let used = 0;
  let out = '';
  for (const ch of title) {
    // 全角（中日韩/全角标点）按 1 字宽，半角（ASCII 等）按 0.55 字宽
    const w = /[\u2e80-\u9fff\uac00-\ud7ff\uf900-\ufaff\uff00-\uff60\uffe0-\uffe6]/.test(ch) ? 1 : 0.55;
    if (used + w > SESSION_TITLE_DISPLAY_BUDGET) return out + '…';
    used += w;
    out += ch;
  }
  return title;
}

/**
 * 延续会话的目标是否为**前序节点暴露的会话 ID**（`{{session_id.<节点ID>.<阶段ID>}}`）。
 *
 * 这种会话是那个 Agent 节点运行时创建的**内部会话**，会话行里的模型就是该节点当次的模型；
 * 因此"跟随会话"等于静默沿用一个本节点看不见的模型、并忽略本节点自己的配置 —— 不提供该选项。
 */
function isNodeSessionRef(sessionMode: unknown, resumeRef: unknown): boolean {
  if (sessionMode !== 'resume') return false;
  return /^\{\{session_id\.[^.}]+\./.test(String(resumeRef || '').trim());
}

/**
 * 本轮是否**跟随会话的模型**（= 节点上的 厂商/模型 运行期不生效，需置灰）。
 *
 * 三个条件同时成立才算：延续会话 + 未选"跟随节点" + 目标不是前序节点暴露的会话。
 * **新会话模式不适用**：没有会话可跟随，厂商/模型就是本节点自己的配置，必须可编辑。
 */
function followsSessionModel(sessionMode: unknown, resumeRef: unknown, modelSource: unknown): boolean {
  if (sessionMode !== 'resume') return false;
  if (isNodeSessionRef(sessionMode, resumeRef)) return false;
  return ((modelSource as string) || 'session') !== 'node';
}

interface Props {
  node: WorkflowNode;
  onUpdate: (updates: Partial<WorkflowNode>) => void;
  onClose: () => void;
  onOpenSubflow?: (definitionId: string) => void;
  /** 所有阶段（用于输入映射计算前序节点） */
  stages?: Stage[];
  /** 阶段间连线（用于阶段拓扑前序判断） */
  stageEdges?: WorkflowEdge[];
  /** 当前工作流 ID（用于子工作流选择过滤） */
  definitionId?: string;
}

const NODE_TYPE_CONFIG_MAP: Record<WorkflowNodeType, { fields: { key: string; label: string; type: string; placeholder?: string; rows?: number }[] }> = {
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
      { key: 'script', label: '转换脚本', type: 'textarea', rows: 14, placeholder: '// JavaScript 转换脚本\n// ⚠️ 不支持 {{}} 模板语法，必须用 JS 语法访问变量\n//\n// 可访问变量：输入映射里配置的每个字段名，直接当变量用\n//   （如映射配了 input1，脚本里就直接写 input1；多字段对象可继续下钻 input1.result）\n//   注意：与 JS 保留字/内置对象同名（如 JSON、Math、for）的字段名无法直接使用，请改名\n//   没有全局上下文变量：引用其它节点的输出请在本节点「输入映射」里配置\n//\n//   例如输入映射配了 { name: "{{result.node1.stage1}}", age: "{{age.node1.stage1}}" }\n//   则脚本中 name = "alice", age = 30\n//\n//   类型自动推断（从输入映射值智能判断）：\n//     "6"     → 数字 6     (typeof = number)\n//     "3.14"  → 数字 3.14  (typeof = number)\n//     "true"  → 布尔 true  (typeof = boolean)\n//     "false" → 布尔 false (typeof = boolean)\n//     ""      → null       (typeof = object)\n//     "hello" → 字符串     (typeof = string)\n//\n//   若需强制转换，可用 Number() / String() / Boolean()\n//\n// 返回值（二选一）：\n//   1. 使用 return 语句\n//   2. 最后一个表达式的值\n\n// ===== 示例 =====\n\n// 示例 1：重组字段\nreturn {\n  label: name,\n  value: age  // 已自动推断为数字，无需 Number()\n};\n\n// 示例 2：简单计算（数字类型自动支持，直接计算）\nage * 2\n\n// 示例 3：条件分支（布尔类型直接用于判断）\nif (is_admin === true) {\n  return { role: "管理员" };\n}\nreturn { role: "普通用户" };\n\n// 示例 4：数组转换（字符串用 split）\nreturn String(tags ?? "").split(",").map(t => t.trim());' },
    ],
  },
  interact: {
    fields: [
      { key: 'prompt', label: '提示文案', type: 'textarea', placeholder: '请输入提示用户的内容' },
      { key: 'inputType', label: '输入类型', type: 'select', placeholder: 'text' },
      { key: 'timeoutMinutes', label: '超时时间(分钟)', type: 'number', placeholder: '30' },
    ],
  },
  plugin: {
    fields: [
      { key: 'plugin_id', label: '选择插件', type: 'plugin_select', placeholder: '选择插件' },
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

/* ---------- JSON Schema → 配置字段转换 ---------- */

interface SchemaConfigField {
  key: string;
  label: string;
  type: string;
  placeholder?: string;
  /** select 类型的选项列表 */
  options?: Array<{ value: string; label: string }>;
  /** 字段描述 */
  description?: string;
}

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
  /** `title`（可选）：标签被截断时的悬浮全文（如「已有会话」的完整标题 + 时间） */
  options: { value: string; label: string; title?: string }[];
  /** 非"工作流阶段"的分组（如延续会话里的「已有会话」）：标题不显示 [stepN] 前缀 */
  kind?: 'sessions';
}

function getOutputFieldOptions(nodeType: WorkflowNodeType): OptionGroup[] | undefined {
  // 新架构：所有节点类型的输出统一为 content 体系
  // 输出映射的 value（引用路径）支持：
  //   content            → 节点执行结果（全部）
  //   session_id         → agent 节点会话ID
  //   content.<字段>      → 结果里的字段（结果是 JSON 文本时自动解析，无需转换节点）
  //   content[N].<字段>   → 结果的第 N 个元素再取字段
  // 用户也可直接输入自定义路径
  if (nodeType === 'start') return undefined; // 开始节点无输出映射

  const baseOptions: { value: string; label: string }[] = [
    { value: '{{content}}', label: 'content（执行结果）' },
  ];

  // agent 类型节点额外提供 session_id
  if (nodeType === 'agent') {
    baseOptions.push({ value: '{{session_id}}', label: 'session_id（会话ID）' });
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
        if (stageUpstreamSet.has(edge.target) && !stageUpstreamSet.has(edge.source) && stageIdSet.has(edge.source)) {
          stageUpstreamSet.add(edge.source);
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
  const [expandedStages, setExpandedStages] = useState<Set<string>>(new Set());
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [searchQuery, setSearchQuery] = useState('');
  /** 下拉滚动容器：打开时把选中项滚进视野 */
  const dropdownRef = useRef<HTMLDivElement | null>(null);
  /** 当前选中项节点（高亮项），用于滚动定位 */
  const selectedOptionRef = useRef<HTMLDivElement | null>(null);
  /** 本次打开是否还需要滚动（只在打开那一刻滚一次，之后用户手动展开/折叠不再抢滚动） */
  const pendingScrollRef = useRef(false);

  /**
   * 展开包含指定值的阶段组与节点组（默认展开选中组，让用户看到当前值在哪一层）。
   *
   * 分组 key 与渲染时一致：阶段 `s:<阶段名>`、节点 `n:<阶段名>:<子项序号>`；
   * 值不在候选列表里（手写 3 段式引用、历史值、纯常量）时不做任何展开。
   */
  const expandGroupsForValue = (selected: string | undefined) => {
    if (!selected || !valueOptions) return;
    const stageKeys: string[] = [];
    const nodeKeys: string[] = [];
    valueOptions.forEach((stage) => {
      const inStageOptions = stage.options.some((opt) => opt.value === selected);
      const nodeIdx = (stage.children || []).findIndex((node) => node.options.some((opt) => opt.value === selected));
      if (inStageOptions || nodeIdx >= 0) {
        stageKeys.push('s:' + stage.group);
        if (nodeIdx >= 0) nodeKeys.push('n:' + stage.group + ':' + nodeIdx);
      }
    });
    if (stageKeys.length > 0) setExpandedStages((prev) => new Set([...prev, ...stageKeys]));
    if (nodeKeys.length > 0) setExpandedNodes((prev) => new Set([...prev, ...nodeKeys]));
  };

  /** 打开某一行的下拉：默认展开选中组，并标记待滚动 */
  const openDropdown = (key: string) => {
    if (activeDropdown === key) {
      setActiveDropdown(null);
      return;
    }
    expandGroupsForValue(value?.[key]);
    pendingScrollRef.current = true;
    setActiveDropdown(key);
  };

  // 打开下拉且选中组已展开后，把高亮项滚进视野（容器内部滚动，不带动页面）
  useEffect(() => {
    if (!activeDropdown || !pendingScrollRef.current) return;
    pendingScrollRef.current = false;
    const raf = requestAnimationFrame(() => {
      const container = dropdownRef.current;
      const el = selectedOptionRef.current;
      if (!container || !el) return;
      const elTop = el.offsetTop;
      const elBottom = elTop + el.offsetHeight;
      if (elTop < container.scrollTop) {
        container.scrollTop = Math.max(elTop - 8, 0);
      } else if (elBottom > container.scrollTop + container.clientHeight) {
        container.scrollTop = elBottom - container.clientHeight + 8;
      }
    });
    return () => cancelAnimationFrame(raf);
  }, [activeDropdown, expandedStages, expandedNodes]);

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
                      onClick={() => openDropdown(key)}
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
                      ref={dropdownRef}
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
                      <input
                        autoFocus
                        value={searchQuery}
                        onChange={(e) => { e.stopPropagation(); setSearchQuery(e.target.value); }}
                        placeholder="搜索字段…"
                        onClick={(e) => e.stopPropagation()}
                        onKeyDown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); setActiveDropdown(null); } }}
                        style={{
                          width: '100%',
                          padding: '6px 8px',
                          fontSize: 'var(--fs-11)',
                          border: 'none',
                          borderBottom: '1px solid var(--border)',
                          outline: 'none',
                          background: 'var(--bg-primary)',
                          color: 'var(--text-primary)',
                          boxSizing: 'border-box',
                        }}
                      />
                      {(() => {
                        const showAll = searchQuery.length > 0;
                        const q = searchQuery.toLowerCase();
                        const filteredGroups = showAll ? valueOptions.reduce((acc, stage) => {
                          const stageMatch = stage.group?.toLowerCase().includes(q);
                          const filteredOptions = stage.options.filter(opt => opt.label.toLowerCase().includes(q));
                          const filteredChildren = (stage.children || []).reduce((ca, node) => {
                            const nodeMatch = node.group.toLowerCase().includes(q);
                            const filteredNodeOpts = node.options.filter(opt => opt.label.toLowerCase().includes(q));
                            if (nodeMatch || filteredNodeOpts.length > 0) {
                              ca.push({ ...node, options: nodeMatch ? node.options : filteredNodeOpts });
                            }
                            return ca;
                          }, [] as OptionGroup[]);
                          if (stageMatch || filteredOptions.length > 0 || filteredChildren.length > 0) {
                            acc.push({ ...stage, options: stageMatch ? stage.options : filteredOptions, children: filteredChildren });
                          }
                          return acc;
                        }, [] as OptionGroup[]) : valueOptions;
                        const hasOptions = filteredGroups.some(g => g.options.length > 0 || (g.children && g.children.some(c => c.options.length > 0)));
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
                              {searchQuery ? '无匹配字段' : '暂无可用字段'}
                            </div>
                          );
                        }
                        return filteredGroups.map((stage, si) => (
                          <div key={si}>
                            {/* 第一级：[序号] 阶段名 */}
                            {stage.group && (
                              <div
                                onClick={() => {
                                  const k = 's:' + stage.group;
                                  setExpandedStages(prev => {
                                    const next = new Set(prev);
                                    if (next.has(k)) next.delete(k); else next.add(k);
                                    return next;
                                  });
                                }}
                                style={{
                                  padding: '4px 8px',
                                  fontSize: 'var(--fs-10)',
                                  color: 'var(--text-tertiary)',
                                  fontWeight: 600,
                                  borderBottom: '1px solid var(--border)',
                                  background: 'var(--bg-tertiary)',
                                  cursor: 'pointer',
                                  userSelect: 'none',
                                }}
                              >
                                {expandedStages.has('s:' + stage.group) ? '\u25bc' : '\u25b6'} [step{si + 1}] {stage.group}
                              </div>
                            )}
                            {(showAll || expandedStages.has('s:' + stage.group)) && (
                            <>
                            {/* gate_output 选项 */}
                            {stage.options.length > 0 && stage.options.map((opt) => (
                              <div
                                key={opt.value}
                                ref={opt.value === value?.[key] ? selectedOptionRef : undefined}
                                onClick={() => {
                                  handleValueChange(key, opt.value);
                                  setActiveDropdown(null);
                                }}
                                className={opt.value === value?.[key] ? 'pd-option pd-option-selected' : 'pd-option'}
                                style={{
                                  padding: '6px 8px 6px 16px',
                                  borderBottom: '1px solid var(--border)',
                                }}
                              >
                                {opt.label}
                              </div>
                            ))}
                            {/* [node] 分组 */}
                            {stage.children && stage.children.map((nodeGroup, ni) => (
                              <div key={ni}>
                                <div
                                  onClick={() => {
                                    const k = 'n:' + stage.group + ':' + ni;
                                    setExpandedNodes(prev => {
                                      const next = new Set(prev);
                                      if (next.has(k)) next.delete(k); else next.add(k);
                                      return next;
                                    });
                                  }}
                                  style={{
                                    padding: '6px 8px 6px 16px',
                                    fontSize: 'var(--fs-10)',
                                    color: 'var(--text-secondary)',
                                    fontWeight: 500,
                                    borderBottom: '1px solid var(--border)',
                                    cursor: 'pointer',
                                    userSelect: 'none',
                                  }}
                                >
                                  {expandedNodes.has('n:' + stage.group + ':' + ni) ? '\u25bc' : '\u25b6'} {nodeGroup.group.startsWith('[') ? nodeGroup.group : `[node] ${nodeGroup.group}`}
                                </div>
                                {(showAll || expandedNodes.has('n:' + stage.group + ':' + ni)) && nodeGroup.options.map((opt) => (
                                  <div
                                    key={opt.value}
                                    ref={opt.value === value?.[key] ? selectedOptionRef : undefined}
                                    onClick={() => {
                                      handleValueChange(key, opt.value);
                                      setActiveDropdown(null);
                                    }}
                                    className={opt.value === value?.[key] ? 'pd-option pd-option-selected' : 'pd-option'}
                                    style={{
                                      padding: '5px 8px 5px 32px',
                                      borderBottom: '1px solid var(--border)',
                                    }}
                                  >
                                    {opt.label}
                                  </div>
                                ))}
                              </div>
                            ))}
                            </>
                            )}
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
/** 子工作流选择器组件：可搜索、合法性过滤 */
const SubflowSelector: React.FC<{
  definitions: { id: string; name: string }[];
  currentDefinitionId?: string;
  value: string;
  onChange: (v: string) => void;
  onOpenSubflow?: (id: string) => void;
  onCreateNew: () => void;
  loadDefinitions?: () => Promise<void>;
}> = ({ definitions, currentDefinitionId, value, onChange, onOpenSubflow: _onOpenSubflow, onCreateNew: _onCreateNew, loadDefinitions }) => {
  const [searchQuery, setSearchQuery] = useState('');
  const [invalidIds, setInvalidIds] = useState<Set<string>>(new Set());
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // 确保 definitions 已加载
  useEffect(() => {
    if (definitions.length === 0 && loadDefinitions) {
      loadDefinitions();
    }
  }, [definitions.length, loadDefinitions]);

  // 加载时异步检测所有候选工作流的合法性
  useEffect(() => {
    if (!currentDefinitionId) return;
    const checkAll = async () => {
      const invalid = new Set<string>();
      for (const d of definitions) {
        if (d.id === currentDefinitionId) {
          invalid.add(d.id); // 排除自身
          continue;
        }
        try {
          const hasCycle = await invoke<boolean>('check_subflow_cycle', {
            parentId: currentDefinitionId,
            candidateId: d.id,
          });
          if (hasCycle) invalid.add(d.id);
        } catch {
          // 查询失败时不阻塞，标记为合法
        }
      }
      setInvalidIds(invalid);
    };
    checkAll();
  }, [currentDefinitionId, definitions]);

  // 点击外部关闭下拉
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  const filtered = definitions.filter(d => {
    // 搜索过滤
    if (searchQuery && !d.name.toLowerCase().includes(searchQuery.toLowerCase()) &&
        !d.id.toLowerCase().includes(searchQuery.toLowerCase())) {
      return false;
    }
    return true;
  });

  const selectedDef = definitions.find(d => d.id === value);

  return (
    <div ref={dropdownRef} style={{ position: 'relative' }}>
      {/* 搜索输入框（兼做选择显示） */}
      <input
        type="text"
        value={isOpen ? searchQuery : (selectedDef ? selectedDef.name : '')}
        placeholder="搜索工作流..."
        onFocus={() => { setIsOpen(true); setSearchQuery(''); }}
        onChange={(e) => { setSearchQuery(e.target.value); setIsOpen(true); }}
        style={{
          width: '100%',
          padding: '6px 8px',
          borderRadius: 'var(--radius-md)',
          border: '1px solid var(--border)',
          background: 'var(--bg-primary)',
          color: 'var(--text-primary)',
          fontSize: 'var(--fs-12)',
          boxSizing: 'border-box',
          marginBottom: 4,
        }}
      />
      {/* 下拉选项列表 */}
      {isOpen && (
        <div
          style={{
            position: 'absolute',
            top: '100%',
            left: 0,
            right: 0,
            maxHeight: 200,
            overflowY: 'auto',
            background: 'var(--bg-primary)',
            border: '1px solid var(--border)',
            borderRadius: 'var(--radius-md)',
            zIndex: 1000,
            boxShadow: '0 4px 12px rgba(0,0,0,0.15)',
          }}
        >
          {filtered.length === 0 && (
            <div style={{ padding: '8px 12px', color: 'var(--text-tertiary)', fontSize: 'var(--fs-11)' }}>
              无匹配工作流
            </div>
          )}
          {filtered.map((d) => {
            const isSelf = d.id === currentDefinitionId;
            const hasCycle = invalidIds.has(d.id);
            const disabled = isSelf || hasCycle;
            return (
              <div
                key={d.id}
                onClick={() => {
                  if (!disabled) {
                    onChange(d.id);
                    setIsOpen(false);
                    setSearchQuery('');
                  }
                }}
                style={{
                  padding: '6px 12px',
                  cursor: disabled ? 'not-allowed' : 'pointer',
                  opacity: disabled ? 0.4 : 1,
                  color: 'var(--text-primary)',
                  background: d.id === value ? 'var(--accent-light)' : 'transparent',
                  fontSize: 'var(--fs-12)',
                  display: 'flex',
                  alignItems: 'center',
                  gap: 6,
                }}
                onMouseEnter={(e) => { if (!disabled) (e.currentTarget as HTMLElement).style.background = 'var(--bg-secondary)'; (e.currentTarget as HTMLElement).style.color = 'var(--text-primary)'; }}
                onMouseLeave={(e) => { if (!disabled) (e.currentTarget as HTMLElement).style.background = d.id === value ? 'var(--accent-light)' : 'transparent'; (e.currentTarget as HTMLElement).style.color = 'var(--text-primary)'; }}
              >
                <span style={{ color: 'var(--text-primary)' }}>{d.name}</span>
                {isSelf && <span style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>(自身)</span>}
                {hasCycle && !isSelf && <span style={{ fontSize: 'var(--fs-10)', color: 'var(--danger)' }}>(产生闭环)</span>}
              </div>
            );
          })}
          {invalidIds.size > 0 && (
            <div style={{ padding: '4px 12px', fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', borderTop: '1px solid var(--border)' }}>
              已过滤 {invalidIds.size} 个不合法工作流
            </div>
          )}
        </div>
      )}

    </div>
  );
};

export const WorkflowNodeConfig: React.FC<Props> = ({ node, onUpdate, onClose, onOpenSubflow, stages, stageEdges, definitionId }) => {
  const meta = getNodeTypeMeta(node.type);
  const builtinFields = NODE_TYPE_CONFIG_MAP[node.type as WorkflowNodeType]?.fields || [];
  const configFields: SchemaConfigField[] = builtinFields.length > 0
    ? builtinFields
    : [];
  const [params, setParams] = useState<Record<string, unknown>>(node.params || {});
  /**
   * `params` 的同步镜像。写入必须以**最新值**为底，而 `setState` 在同一个 tick 内不更新：
   * 级联同时改上级与下级、组件挂载时连写多个默认值，只读 state 会把前一次写入丢掉。
   */
  const paramsRef = useRef<Record<string, unknown>>(node.params || {});
  /** 已同步过参数的节点 id：用来跳过"首次挂载那次多余的同步"（见下面的 effect） */
  const syncedNodeIdRef = useRef<string>(node.id);
  const { getEnabledAgentTypes, getDisplayName, loading: agentsLoading } = useAgentRegistry();
  const enabledAgentTypes = getEnabledAgentTypes();
  /** CLI 类 Agent（除 api 以外的可用类型）：Agent 节点二级菜单的 CLI 分支 */
  const cliAgentTypes = enabledAgentTypes.filter((t) => t !== 'api');
  /** Agent 节点一级菜单的取值：agent_type 是唯一事实源，api 单独一类 */
  const agentKind: 'cli' | 'api' = params.agent_type === 'api' ? 'api' : 'cli';
  const { definitions, loadDefinitions } = useWorkflowStore();
  const { sessions: appSessions, refreshSessions } = useSessionStore();
  const pluginStore = usePluginStore();
  // 细粒度订阅 discover 用到的两项：避免把整个 store 对象写进 effect 依赖（store 每次更新对象都变，
  // 会让下面的「重新扫描」effect 反复触发）。discover 是稳定 action，plugins 只在其内容变化时变。
  const pluginStorePlugins = usePluginStore((s) => s.plugins);
  const pluginStoreDiscover = usePluginStore((s) => s.discover);
  const { providers: apiProviders, fetchProviders } = useApiProviderStore();
  /** API 类型 Agent 的「模型」是否用自定义输入（提供商没预置模型时也走输入框） */
  const [apiCustomModel, setApiCustomModel] = useState(false);
  // 插件下拉的选中值直接取自节点参数，避免切节点时下拉与「插件配置」面板指向不同插件
  const selectedPluginId = (params.plugin_id as string) || node.pluginId || '';
  const inputBaseKeyRef = useRef<string>('');
  const outputBaseKeyRef = useRef<string>('');
  const [resumeDropdownOpen, setResumeDropdownOpen] = useState(false);
  const [expandedResumeStages, setExpandedResumeStages] = useState<Set<string>>(new Set());
  const [expandedResumeNodes, setExpandedResumeNodes] = useState<Set<string>>(new Set());
  const [resumeSearchQuery, setResumeSearchQuery] = useState('');
  const panelRef = useRef<HTMLDivElement | null>(null);

  
  
  // plugin 节点可选插件列表：纯派生自 pluginStore.plugins，渲染期直接算（原先 state + effect
  // 会在 effect 里同步 setState 造成级联渲染）。
  // 必须一并过滤 `enabled`：停用的插件不该出现在这里（否则停用后仍能拖出它的节点）。
  const availablePlugins = useMemo(
    () => (pluginStore.plugins || [])
      .filter((p) => p.enabled && pluginSupportsWorkflow(p))
      .map((p) => ({ id: p.manifest.id, name: p.manifest.name })),
    [pluginStore.plugins],
  );

  // plugin 节点：列表为空或无可用于工作流的插件时，触发一次补扫。
  //
  // 必须熔断（只补扫一次）：`discover()` 之后 `plugins` 是**新的数组引用**，会让本 effect 再跑一轮；
  // 若一个可用于工作流的插件都没有（例如用户把它们全停用了），就会变成
  // "扫描 → 列表仍为空 → 再扫描" 的无限打转，每轮还带一次 IPC。
  const pluginRescannedRef = useRef(false);
  useEffect(() => {
    if (node.type !== 'plugin') return;
    if (pluginRescannedRef.current) return;
    const plugins = pluginStorePlugins || [];
    if (plugins.some(pluginSupportsWorkflow)) return;
    if (!pluginStoreDiscover) return;
    pluginRescannedRef.current = true;
    pluginStoreDiscover();
  }, [node.type, pluginStorePlugins, pluginStoreDiscover]);

useEffect(() => {
    if (node.type === 'subflow' && definitions.length === 0) {
      loadDefinitions();
    }
  }, [node.type, definitions.length, loadDefinitions]);

  /**
   * 切换节点时才重新同步 params；**首次挂载不重设**。
   *
   * useState 已经用 node.params 初始化过，而 effect 的执行顺序是"子先父后"：插件表单挂载时
   * 写回的默认值（如 `operation = add`）先落进这里，若本 effect 再跑一次 `setParams(node.params)`，
   * 就会用**挂载前的旧快照**把它盖掉；此后每次 `onParamsChange` 又以这个旧快照为底，
   * 参数被静默丢掉 —— 表现为"选默认项执行时报 undefined，手动换一个选项就正常"。
   */
  useEffect(() => {
    if (syncedNodeIdRef.current === node.id) return;
    syncedNodeIdRef.current = node.id;
    const next = node.params || {};
    paramsRef.current = next;
    setParams(next);
  }, [node.id, node.params]);

  /**
   * 切换节点时重置「延续会话」下拉的瞬态状态（开合 / 展开 / 搜索）。
   *
   * 面板组件在节点之间是复用的（切换只换 node prop），不重置的话上一个节点手动展开的组、
   * 输入的搜索词会显示在下一个节点的下拉里；输入/输出映射下拉通过 `key={node.id}` 重建
   * 规避了同一问题，这里用同一口径把会话下拉也归零。
   *
   * 用「渲染期调整状态」（React 官方推荐的 adjust-during-render 写法）代替 effect：
   * 在发现 node.id 变化的同一次渲染里就归零，最终渲染结果与原先（提交后再归零）一致，
   * 且不会因 effect 里同步 setState 触发级联渲染。
   */
  const [resumeStateNodeId, setResumeStateNodeId] = useState(node.id);
  if (resumeStateNodeId !== node.id) {
    setResumeStateNodeId(node.id);
    setResumeDropdownOpen(false);
    setExpandedResumeStages(new Set());
    setExpandedResumeNodes(new Set());
    setResumeSearchQuery('');
  }

  const handleParamChange = (key: string, value: unknown) => {
    // 以镜像为底（而不是 state）：同一个 tick 内连续写入（级联重置上级+下级、插件挂载时
    // 连写多个默认值）必须能叠加，只读 state 会让后一次写入丢掉前一次。ref 只在事件
    // 回调里读写。
    const newParams = { ...paramsRef.current, [key]: value };
    paramsRef.current = newParams;
    setParams(newParams);
    onUpdate({ params: newParams });
  };

  /**
   * 切换命令后的节点参数：
   * 1. 删除只属于旧命令、新命令未声明的键；
   * 2. 新命令声明了、但**取值不在其 `enum` 内**（或为空串）的键也删掉，视为"未设置" ——
   *    级联选择器的下级最典型：`func` 在上一个命令下是 `sin`，切到对数命令后 `sin` 不合法，
   *    留着会让 handler 报"无效的函数: sin"；删掉后默认值补齐 / 表单重填。
   *    含 `{{}}` 的值不删：它可能在运行时解析成合法取值。
   * 插件自写组件写入的模式外键无法归属，一律保留。
   */
  const paramsAfterCommandChange = (pluginId: string, nextCommandId: string): Record<string, unknown> => {
    const nextCommand = findPluginCommand(pluginId, nextCommandId);
    const nextKeys = commandParamKeys(nextCommand);
    // 以镜像为底：插件表单可能在同一个 tick 里刚写过参数，状态还没跟上
    const newParams: Record<string, unknown> = { ...paramsRef.current };
    for (const key of commandParamKeys(findPluginCommand(pluginId, node.commandId))) {
      if (!nextKeys.includes(key) && key in newParams) {
        delete newParams[key];
      }
    }
    for (const [key, property] of Object.entries(nextCommand?.input?.properties ?? {})) {
      if (!(key in newParams)) continue;
      const value = newParams[key];
      if (typeof value === 'string' && value.includes('{{')) continue;
      const isEmpty = value === '' || value === undefined || value === null;
      const enumList = Array.isArray(property.enum) ? property.enum : [];
      const outOfEnum = enumList.length > 0 && !enumList.some((item) => String(item) === String(value));
      if (isEmpty || outOfEnum) {
        delete newParams[key];
      }
    }
    return newParams;
  };

  /** 已按 (节点,插件,命令) 补齐过默认值的标记：每个命令只补一次，
      用户在表单里主动清空的值不会被反复填回 */
  const seededDefaultsRef = useRef<string>('');

  /**
   * 补齐命令参数的默认值：`input.properties[key].default` 已声明、而 params 里缺失/空串/不在 enum 内时写回默认值。
   *
   * 不补会怎样：表单把默认值**显示**出来（看着已经选好了），params 里却没有这个键 ——
   * handler 收到空值报错，而用户"再选一次默认项"也不会触发 onChange（值没变），
   * 于是表现为"选默认项必错、选非默认项就正常"。
   */
  useEffect(() => {
    if (node.type !== 'plugin' || !params.plugin_id || !node.commandId) return;
    const seedKey = `${node.id}|${params.plugin_id}|${node.commandId}`;
    if (seededDefaultsRef.current === seedKey) return;
    seededDefaultsRef.current = seedKey;

    const newParams = applyCommandParamDefaults((params.plugin_id as string), node.commandId, paramsRef.current);
    if (newParams === paramsRef.current) return;
    paramsRef.current = newParams;
    setParams(newParams);
    onUpdate({ params: newParams });
  }, [node.id, node.type, node.commandId, params.plugin_id, onUpdate]);

  /** 已请求过 API 厂商列表（列表为空时只请求一次，避免空列表触发反复拉取） */
  const apiProvidersRequestedRef = useRef(false);
  /** 上次为 API 节点补齐过的 (节点, 厂商)：同一厂商只补一次，用户换厂商时才重填模型 */
  const apiSeededRef = useRef<string>('');

  /**
   * agent 节点选 API 类型时的厂商/模型联级：拉取厂商列表，并把「当前该用的厂商 + 模型」落进 params。
   *
   * 与插件命令参数同理：下拉里显示的值必须在 params 里，否则 handler/后端拿不到（此前该节点只有
   * agent_type，api 运行时会去拼 CLI 命令，报 program not found）。换厂商时模型跟着改成新厂商的
   * 第一个可用值（级联下级重置）。
   */
  useEffect(() => {
    if (node.type !== 'agent' || params.agent_type !== 'api') return;
    if (apiProviders.length === 0) {
      if (!apiProvidersRequestedRef.current) {
        apiProvidersRequestedRef.current = true;
        fetchProviders();
      }
      return;
    }
    const provider = apiProviders.find((p) => p.id === params.api_provider) || apiProviders[0];
    const seedKey = `${node.id}|${provider.id}`;
    if (apiSeededRef.current === seedKey) return;
    apiSeededRef.current = seedKey;

    const model = provider.models.includes(params.api_model as string)
      ? (params.api_model as string)
      : (provider.models[0] || '');
    if (params.api_provider !== provider.id) handleParamChange('api_provider', provider.id);
    if (model && params.api_model !== model) handleParamChange('api_model', model);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- handleParamChange 每轮渲染都会重建，列进来只会让本 effect 空跑
  }, [node.id, node.type, params.agent_type, params.api_provider, params.api_model, apiProviders, fetchProviders]);

  /** 已请求过会话列表（「延续会话」里"已有会话"分组的候选来源） */
  const sessionsRequestedRef = useRef(false);

  /**
   * agent 节点的 `agent_type` 兜底：老节点可能没有这个键，而一级/二级菜单都必须"显示什么就写什么"
   * （下拉显示 CLI 类型、params 却没有 → 引擎校验直接报缺少 agent_type）。等 Agent 列表加载完再补，
   * 避免列表空时把 CLI 节点误判成 api。
   */
  useEffect(() => {
    if (node.type !== 'agent' || agentsLoading || params.agent_type) return;
    handleParamChange('agent_type', cliAgentTypes[0] || 'api');
    // eslint-disable-next-line react-hooks/exhaustive-deps -- handleParamChange 每轮渲染都会重建，列进来只会让本 effect 空跑
  }, [node.id, node.type, params.agent_type, agentsLoading, cliAgentTypes]);

  /** api 类型 + 延续会话：拉一次会话列表，供"恢复已有会话"选择 */
  useEffect(() => {
    if (node.type !== 'agent' || params.agent_type !== 'api' || params.session_mode !== 'resume') return;
    if (sessionsRequestedRef.current) return;
    sessionsRequestedRef.current = true;
    refreshSessions();
  }, [node.type, params.agent_type, params.session_mode, refreshSessions]);

  /**
   * 延续目标是"前序节点暴露的 session_id"时，模型来源固定为「跟随节点」。
   *
   * 面板上不提供「跟随会话」（见 isNodeSessionRef 说明），这里把此前存过的旧值一并归一化，
   * 避免"面板显示跟随节点、运行时却按会话模型"的不一致。
   */
  useEffect(() => {
    if (node.type !== 'agent' || params.agent_type !== 'api') return;
    if (!isNodeSessionRef(params.session_mode, params.resume_session_ref)) return;
    if (params.resume_model_source === 'node') return;
    handleParamChange('resume_model_source', 'node');
    // eslint-disable-next-line react-hooks/exhaustive-deps -- handleParamChange 每轮渲染都会重建，列进来只会让本 effect 空跑
  }, [node.id, node.type, params.agent_type, params.session_mode, params.resume_session_ref, params.resume_model_source]);

  /** 切换插件：旧插件的参数对新插件没有意义，只保留平台管理的 plugin_id；
      仅清空选择时保留已填参数，便于改回同一插件。命令信息合并进同一次更新，
      避免两次 onUpdate 相互覆盖（handleUpdateNode 基于同一份 stages 计算）。 */
  const applyPluginChange = (pid: string) => {
    const newParams = pid ? { plugin_id: pid } : { ...paramsRef.current, plugin_id: '' };
    paramsRef.current = newParams;
    setParams(newParams);
    onUpdate({ params: newParams, pluginId: pid, commandId: undefined });
  };

  /** 切换命令：先按新命令的模式清理参数（见 paramsAfterCommandChange），再用新命令声明的默认值补齐 */
  const applyCommandChange = (nextCommandId: string) => {
    const newParams = applyCommandParamDefaults(
      selectedPluginId,
      nextCommandId,
      paramsAfterCommandChange(selectedPluginId, nextCommandId),
    );
    paramsRef.current = newParams;
    setParams(newParams);
    onUpdate({ commandId: nextCommandId, params: newParams });
  };

  /** 选择延续会话的引用（下拉项点击，非原生 select） */
  const selectResumeRef = (value: string) => {
    handleParamChange('resume_session_ref', value);
    setResumeDropdownOpen(false);
  };

  /**
   * 切换一级菜单（CLI Agent / API Agent）：换类就换 `agent_type`（二级的具体选择落在它上面），
   * 并清掉延续会话引用 —— CLI 的 resume 值是 CLI 的 agent session id、API 的是应用会话 id，
   * 混用会静默传错（换类后原来的引用一定不再成立）。
   */
  const applyAgentKindChange = (kind: string) => {
    const nextAgentType = kind === 'api' ? 'api' : (cliAgentTypes[0] || 'claude');
    if (nextAgentType !== params.agent_type) handleParamChange('agent_type', nextAgentType);
    if (params.resume_session_ref) handleParamChange('resume_session_ref', '');
  };

  /** 输入映射参数选项（`{{` 触发补全的候选；插件参数同样只能引用这些键） */
  const templateVariables: TemplateVariableGroup[] | undefined = (() => {
    const keys = Object.keys(node.inputMapping || {});
    if (keys.length === 0) return undefined;
    return [{ group: '', children: [{ group: '输入参数', options: keys.map(k => ({ value: k, label: k })) }], options: [] }];
  })();

  return (
    <div ref={panelRef}>
      {/*
        ⚠️ 这一层 Fragment 不是随手包裹，**请勿删**：本组件根节点下的子节点极多，
        直接铺在 <div> 下会让 TS 在某个子节点位置推断退化、把该子节点判成 unknown，
        报出 TS2322「Type 'unknown' is not assignable to type 'ReactNode'」。
        该报错与那个子节点的**内容完全无关**：换成 `&&`、三元、裸变量、`<></>`、
        甚至 `x as React.ReactNode`（断言成自身类型）都照样报，且 `tsc --noEmit` 同样复现。
        多包这一层即消失。渲染上 Fragment 不产生任何节点，纯属绕开编译器的推断退化。
      */}
      <>
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
        {/* textarea 类型的第一个字段（如 transform 的 script）延后到输入映射之后渲染 */}
        {configFields.length > 0 && configFields[0].type !== 'textarea' && (
          <div style={{ marginTop: 10 }}>
            <label style={S.label()}>{configFields[0].label}</label>
            {configFields[0].type === 'select' ? (
              <>
              <div style={{ display: 'flex', gap: 8, alignItems: 'flex-end' }}>
                {/* 一级：Agent 类型（CLI / API）。具体 agent_type 由二级决定 */}
                {configFields[0].key === 'agent_type' ? (
                  <Select
                    value={agentKind}
                    onChange={applyAgentKindChange}
                    options={[
                      ...(cliAgentTypes.length > 0 ? [{ value: 'cli', label: 'CLI Agent' }] : []),
                      { value: 'api', label: 'API Agent' },
                    ]}
                    className="w-full min-w-0"
                  />
                ) : (
                <Select
                  value={(params[configFields[0].key] as string) || configFields[0].placeholder || ''}
                  onChange={(v) => handleParamChange(configFields[0].key, v)}
                  options={[
                    { value: '', label: '选择...' },
                    /* 插件节点：从 field.options 渲染 */
                    ...((configFields[0] as SchemaConfigField).options ?? []),
                    /* 内置节点：硬编码选项 */
                    ...(configFields[0].key === 'method' ? ['GET', 'POST', 'PUT', 'DELETE'].map((v) => ({ value: v, label: v })) : []),
                    ...(configFields[0].key === 'inputType' ? INPUT_TYPE_OPTIONS : []),
                  ]}
                  placeholder="选择..."
                  className="w-full min-w-0"
                />
                )}
                {/* 会话方式 — 与 Agent 类型同行 */}
                {configFields[0].key === 'agent_type' && (() => {
                  const sessionMode = (params['session_mode'] as string) || 'new';
                  return (
                    <Select
                      value={sessionMode}
                      onChange={(v) => handleParamChange('session_mode', v)}
                      options={[
                        { value: 'new', label: '新会话' },
                        { value: 'resume', label: '延续会话' },
                      ]}
                      className="w-full min-w-0"
                      style={{ minWidth: 90 }}
                    />
                  );
                })()}
              </div>
              {configFields[0].key === 'agent_type' && params['session_mode'] === 'resume' && (() => {
                const resumeRef = (params['resume_session_ref'] as string) || '';
                const groups: OptionGroup[] = (() => {
                  // 延续会话的候选必须是「真正携带会话 ID 的字段」，且来自同 agent 类型的 agent 节点
                  // （类型不一致 = 另一套会话体系，无从 resume）。输入映射不受此限——那里只是取值。
                  const nodeById = new Map<string, WorkflowNode>();
                  const stageNameById = new Map<string, string>();
                  for (const st of stages || []) {
                    stageNameById.set(st.id, st.name);
                    for (const n of st.nodes) nodeById.set(n.id, n);
                  }
                  /**
                   * 反查"这个引用是不是会话 ID 字段"：是则返回它的归属（字段名 / 节点名 / 阶段名）。
                   *
                   * 唯一判据：被引用节点的**输出映射路径等于 `{{session_id}}`**——
                   * 引擎只对 `{{session_id}}` 这条路径注入 agent 会话 id
                   * （见 engine.rs expose_node_output 的 session_id 分支），
                   * 字段名（映射键）由用户自定义，所以不能按名字猜，只能按路径反查。
                   * 因此 `output`、`result` 之类的同级字段一律不列入候选；
                   * 节点没暴露会话 ID 时，它的 [node] 分组会整体消失（先暴露后引用）。
                   */
                  const resolveSessionField = (refValue: string) => {
                    const inner = /^\{\{(.+?)\}\}$/.exec(refValue.trim())?.[1];
                    if (!inner) return null;
                    const parts = inner.split('.');
                    if (parts.length < 3) return null;
                    const [fieldKey, refNodeId, refStageId] = parts;
                    const refNode = nodeById.get(refNodeId);
                    if (!refNode || refNode.type !== 'agent') return null;
                    if (refNode.params?.agent_type !== params['agent_type']) return null;
                    const mapping = refNode.outputMapping as Record<string, unknown> | undefined;
                    if (!mapping || String(mapping[fieldKey] ?? '').trim() !== '{{session_id}}') return null;
                    return { nodeLabel: refNode.label, stageName: stageNameById.get(refStageId) || '' };
                  };
                  // 标签补上「阶段名·节点名」以便区分：多个 Agent 节点都暴露会话 ID 时，
                  // 光写字段名看不出是哪一段会话（如 session_id（起始阶段·Agent任务））。
                  const mapResumable = (
                    options: { value: string; label: string; title?: string }[],
                  ): { value: string; label: string; title?: string }[] =>
                    options.flatMap((o) => {
                      const hit = resolveSessionField(o.value);
                      if (!hit) return [];
                      const where = [hit.stageName, hit.nodeLabel].filter(Boolean).join('·');
                      return [{ ...o, label: where ? `${o.label}（${where}）` : o.label }];
                    });
                  const keepResumable = (list: OptionGroup[]): OptionGroup[] =>
                    list
                      .map((g) => ({
                        ...g,
                        options: mapResumable(g.options),
                        children: (g.children || [])
                          .map((c) => ({ ...c, options: mapResumable(c.options) }))
                          .filter((c) => c.options.length > 0),
                      }))
                      .filter((g) => g.options.length > 0 || (g.children?.length ?? 0) > 0);
                  const base = keepResumable(stages ? getPredecessorOutputOptions(node.id, stages, stageEdges) : []);
                  // "恢复会话"：api 类型可直接续跑应用里已有的 API 会话（值就是会话 id，后端据此续跑并沿用其上下文）。
                  // CLI 类型不列：那里的 resume_session_ref 会被当成 CLI 的 agent session id 传给 --resume，塞应用会话 id 是错的。
                  if (params['agent_type'] !== 'api') return base;
                  const options = appSessions
                    // 只列用户自己的 API 会话。`origin` 前缀 `workflow` 是工作流 Agent 节点
                    // 每次执行自动新建的内部会话（标题「工作流 · <节点名>」）：同名不可辨识，
                    // 且按"先暴露后引用"它们应由该节点的 session_id 输出映射引用，不从这里捞。
                    // 判据取会话行上的来源列，不靠标题文本——标题是展示用的可变文案。
                    .filter((s) => s.agentType === 'api' && !isWorkflowSession(s.origin))
                    .map((s) => {
                      const title = s.title || s.id;
                      // 附上时间：会话标题可能重复（如都取自首条消息），没有时间就分不清是哪一个
                      const ts = new Date((s.updatedAt || s.createdAt || 0) * 1000);
                      const stamp = isNaN(ts.getTime())
                        ? ''
                        : `${String(ts.getMonth() + 1).padStart(2, '0')}-${String(ts.getDate()).padStart(2, '0')} ${String(ts.getHours()).padStart(2, '0')}:${String(ts.getMinutes()).padStart(2, '0')}`;
                      // 标题按显示宽度截断（中文按 1 字、ASCII 按 0.55 字），保证「标题 · 时间」单行显示完；
                      // 完整标题放到 title 里，悬浮可看全文
                      const short = truncateSessionTitle(title);
                      const full = stamp ? `${title} · ${stamp}` : title;
                      return { value: s.id, label: stamp ? `${short} · ${stamp}` : short, title: full };
                    });
                  return options.length > 0 ? [...base, { group: '已有会话', options, kind: 'sessions' as const }] : base;
                })();
                const displayLabel = (() => {
                  if (!resumeRef) return '';
                  for (const g of groups) {
                    if (g.options) for (const opt of g.options) if (opt.value === resumeRef) return opt.label;
                    if (g.children) for (const c of g.children) for (const opt of c.options) if (opt.value === resumeRef) return opt.label;
                  }
                  return resumeRef;
                })();
                return (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 4, width: '100%' }}>
                    <div style={{ position: 'relative' }}>
                      <div
                        onClick={() => setResumeDropdownOpen(v => !v)}
                        style={{
                          ...S.select(),
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
                          <input
                            autoFocus
                            value={resumeSearchQuery}
                            onChange={(e) => { e.stopPropagation(); setResumeSearchQuery(e.target.value); }}
                            placeholder="搜索字段…"
                            onClick={(e) => e.stopPropagation()}
                            onKeyDown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); setResumeDropdownOpen(false); } }}
                            style={{
                              width: '100%',
                              padding: '6px 8px',
                              fontSize: 'var(--fs-11)',
                              border: 'none',
                              borderBottom: '1px solid var(--border)',
                              outline: 'none',
                              background: 'var(--bg-primary)',
                              color: 'var(--text-primary)',
                              boxSizing: 'border-box',
                            }}
                          />
                          {(() => {
                            const showAll = resumeSearchQuery.length > 0;
                            const q = resumeSearchQuery.toLowerCase();
                            const filteredGroups = showAll ? groups.reduce((acc, stage) => {
                              const stageMatch = stage.group?.toLowerCase().includes(q);
                              const filteredOptions = stage.options.filter(opt => opt.label.toLowerCase().includes(q));
                              const filteredChildren = (stage.children || []).reduce((ca, node) => {
                                const nodeMatch = node.group.toLowerCase().includes(q);
                                const filteredNodeOpts = node.options.filter(opt => opt.label.toLowerCase().includes(q));
                                if (nodeMatch || filteredNodeOpts.length > 0) {
                                  ca.push({ ...node, options: nodeMatch ? node.options : filteredNodeOpts });
                                }
                                return ca;
                              }, [] as OptionGroup[]);
                              if (stageMatch || filteredOptions.length > 0 || filteredChildren.length > 0) {
                                acc.push({ ...stage, options: stageMatch ? stage.options : filteredOptions, children: filteredChildren });
                              }
                              return acc;
                            }, [] as OptionGroup[]) : groups;
                            if (filteredGroups.length === 0) {
                              return <div style={{ padding: '12px 8px', fontSize: 'var(--fs-11)', color: 'var(--text-tertiary)', textAlign: 'center' }}>{resumeSearchQuery ? '无匹配字段' : '暂无可用字段'}</div>;
                            }
                            return filteredGroups.map((stage, si) => (
                            <div key={si}>
                              {stage.group && (
                                <div
                                  onClick={() => {
                                    const k = 'sr:' + stage.group;
                                    setExpandedResumeStages(prev => {
                                      const next = new Set(prev);
                                      if (next.has(k)) next.delete(k); else next.add(k);
                                      return next;
                                    });
                                  }}
                                  style={{ padding: '4px 8px', fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', fontWeight: 600, borderBottom: '1px solid var(--border)', background: 'var(--bg-tertiary)', cursor: 'pointer', userSelect: 'none' }}
                                >
                                  {expandedResumeStages.has('sr:' + stage.group) ? '\u25bc' : '\u25b6'} {stage.kind === 'sessions' ? stage.group : `[step${si + 1}] ${stage.group}`}
                                </div>
                              )}
                              {(showAll || expandedResumeStages.has('sr:' + stage.group)) && (
                              <>
                              {stage.options.length > 0 && stage.options.map((opt) => (
                                <div key={opt.value} title={opt.title ?? opt.label} onClick={() => {
                                  // eslint-disable-next-line react-hooks/refs -- 事件回调，不在渲染期调用
                                  selectResumeRef(opt.value);
                                }} className={opt.value === resumeRef ? 'pd-option pd-option-selected' : 'pd-option'} style={{ padding: '6px 8px 6px 16px', borderBottom: '1px solid var(--border)' }}>
                                  {opt.label}
                                </div>
                              ))}
                              {stage.children && stage.children.map((nodeGroup, ni) => (
                                <div key={ni}>
                                  <div
                                    onClick={() => {
                                      const k = 'nr:' + stage.group + ':' + ni;
                                      setExpandedResumeNodes(prev => {
                                        const next = new Set(prev);
                                        if (next.has(k)) next.delete(k); else next.add(k);
                                        return next;
                                      });
                                    }}
                                    style={{ padding: '6px 8px 6px 16px', fontSize: 'var(--fs-10)', color: 'var(--text-secondary)', fontWeight: 500, borderBottom: '1px solid var(--border)', cursor: 'pointer', userSelect: 'none' }}
                                  >
                                    {expandedResumeNodes.has('nr:' + stage.group + ':' + ni) ? '\u25bc' : '\u25b6'} {nodeGroup.group.startsWith('[') ? nodeGroup.group : `[node] ${nodeGroup.group}`}
                                  </div>
                                  {(showAll || expandedResumeNodes.has('nr:' + stage.group + ':' + ni)) && nodeGroup.options.map((opt) => (
                                    <div key={opt.value} title={opt.title ?? opt.label} onClick={() => selectResumeRef(opt.value)} className={opt.value === resumeRef ? 'pd-option pd-option-selected' : 'pd-option'} style={{ padding: '6px 8px 6px 32px', borderBottom: '1px solid var(--border)' }}>
                                      {opt.label}
                                    </div>
                                  ))}
                                </div>
                              ))}
                              </>
                              )}
                            </div>
                          ));
                        })()}
                        </div>
                      )}
                    </div>
                    {agentKind === 'api' && (() => {
                      // 模型来源：跟随会话（沿用会话行里的模型）/ 跟随节点（用本节点配置的模型）。
                      // 延续目标是前序节点暴露的 session_id 时**不提供「跟随会话」**——那段会话的模型
                      // 就是那个节点当次的模型，跟随它等于静默忽略本节点配置（见 isNodeSessionRef）。
                      const nodeSessionRef = isNodeSessionRef(params.session_mode, params.resume_session_ref);
                      const followsSession = followsSessionModel(
                        params.session_mode,
                        params.resume_session_ref,
                        params.resume_model_source,
                      );
                      // 「跟随会话」用的是哪个模型，要具体到哪条会话上（引用「已有会话」的 id 时取该会话记录的模型）
                      const refSession = followsSession ? appSessions.find((s) => s.id === resumeRef) : undefined;
                      const providerName = (pid?: string) => apiProviders.find((p) => p.id === pid)?.name;
                      const followModel = refSession
                        ? [providerName(refSession.apiProvider), refSession.apiModel].filter(Boolean).join(' · ')
                        : '';
                      return (
                        <div style={{ marginTop: 10 }}>
                          <label style={S.label()}>模型来源</label>
                          <Select
                            value={followsSession ? 'session' : 'node'}
                            onChange={(v) => handleParamChange('resume_model_source', v)}
                            options={[
                              ...(nodeSessionRef ? [] : [{ value: 'session', label: '跟随会话（用会话自身的模型）' }]),
                              { value: 'node', label: '跟随节点（用本节点配置的模型）' },
                            ]}
                            className="w-full min-w-0"
                          />
                          <div style={{ marginTop: 4, fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>
                            {nodeSessionRef
                              ? '延续的是前序节点运行时创建的会话（它的模型就是那个节点的模型），本轮模型固定用下方本节点配置的 厂商/模型。'
                              : followsSession
                                ? `本轮实际使用${refSession ? '该会话' : '会话'}的模型${followModel ? `：${followModel}` : ''}；下方「API 厂商 / 模型」置灰、运行期不生效（仅新建会话时使用）。`
                                : '本轮按下方节点配置的 厂商/模型 发请求；会话上下文照旧延续，且不会改写该会话自身的模型配置。'}
                          </div>
                        </div>
                      );
                    })()}
                    <div style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>
                      {agentKind === 'api'
                        ? '可延续前序节点输出的会话，或从「已有会话」里挑一条已有 API 会话继续——延续的是该会话的上下文，用哪个模型由上面的「模型来源」决定'
                        : '选择前序节点通过输出映射声明的会话变量，用于延续该节点的 Agent 会话上下文'}
                    </div>
                  </div>
                );
              })()}
              {/* 二级（CLI 分支）：具体使用哪个 CLI Agent */}
              {configFields[0].key === 'agent_type' && agentKind === 'cli' && (
                <div style={{ marginTop: 10 }}>
                  <label style={S.label()}>CLI 类型</label>
                  <Select
                    value={(params['agent_type'] as string) || ''}
                    onChange={(v) => handleParamChange('agent_type', v)}
                    options={[
                      /* 存的 CLI 类型当前不可用（未安装/被禁用）时仍显示原值，否则下拉会落到第一项，
                          看起来像"节点配置与 JSON 不一致" */
                      ...(params['agent_type'] && !cliAgentTypes.includes(params['agent_type'] as string)
                        ? [{ value: params['agent_type'] as string, label: `${getDisplayName(params['agent_type'] as string)}（当前不可用）` }]
                        : []),
                      ...cliAgentTypes.map((t) => ({ value: t, label: getDisplayName(t) })),
                    ]}
                    className="w-full min-w-0"
                  />
                </div>
              )}
              {/* 二级（API 分支）：厂商 → 三级：模型（与会话模式「新建会话」同源，见 SessionList） */}
              {configFields[0].key === 'agent_type' && (() => {
                if (params['agent_type'] !== 'api') return null;
                const securityMode = (params['security_mode'] as SecurityModeValue) || 'standard';
                const securityHint = SECURITY_MODE_OPTIONS.find((m) => m.value === securityMode)?.workflowEffect
                  ?? SECURITY_MODE_OPTIONS[1].workflowEffect;
                const provider = apiProviders.find((p) => p.id === params['api_provider']);
                // 「延续会话 + 模型来源=跟随会话」时，节点上的 厂商/模型 运行期不生效 → 置灰（要改就去改上面的「模型来源」）。
                // 新会话模式、以及延续目标是节点创建的会话（固定跟随节点）时都不置灰——那时这俩就是本节点自己的配置。
                const modelGreyedOut = followsSessionModel(
                  params['session_mode'],
                  params['resume_session_ref'],
                  params['resume_model_source'],
                );
                const greyStyle = { opacity: modelGreyedOut ? 0.5 : 1, cursor: modelGreyedOut ? 'not-allowed' : undefined };
                if (apiProviders.length === 0) {
                  return (
                    <div style={{ marginTop: 8, fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>
                      暂无配置的 API 提供商：请先在「设置 → API集成配置」里添加，再回到这里选择。
                    </div>
                  );
                }
                return (
                  <>
                    <div style={{ marginTop: 10 }}>
                      <label style={S.label()}>API 厂商</label>
                      {modelGreyedOut && (
                        <div style={{ marginBottom: 4, fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>
                          延续会话使用会话自身的模型，此处两项运行期不生效（仅「新建会话」时使用）
                        </div>
                      )}
                      <Select
                        value={(params['api_provider'] as string) || ''}
                        onChange={(v) => handleParamChange('api_provider', v)}
                        disabled={modelGreyedOut}
                        options={[
                          { value: '', label: '选择提供商' },
                          /* 存的是已不在配置列表里的厂商 id（厂商被删/改名）：仍把原值显示出来，
                              否则下拉会落到第一项，看起来像"节点配置与 JSON 不一致" */
                          ...(params['api_provider'] && !apiProviders.some((p) => p.id === params['api_provider'])
                            ? [{ value: params['api_provider'] as string, label: `${params['api_provider']}（该厂商已不在配置中）` }]
                            : []),
                          ...apiProviders.map((p) => ({
                            value: p.id,
                            label: `${p.name}${p.apiKeySet ? ' (已配置)' : ' (未配置Key)'}`,
                          })),
                        ]}
                        placeholder="选择提供商"
                        className="w-full min-w-0"
                      />
                    </div>
                    {provider ? (
                      <div style={{ marginTop: 10 }}>
                        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                          <label style={{ ...S.label(), marginBottom: 0 }}>模型</label>
                          <button
                            onClick={() => setApiCustomModel((v) => !v)}
                            disabled={modelGreyedOut}
                            style={{
                              padding: '1px 6px', fontSize: 'var(--fs-10)', borderRadius: 'var(--radius-md)',
                              cursor: modelGreyedOut ? 'not-allowed' : 'pointer',
                              opacity: modelGreyedOut ? 0.5 : 1,
                              border: '1px solid var(--border)',
                              background: apiCustomModel ? 'var(--bg-tertiary)' : 'transparent',
                              color: apiCustomModel ? 'var(--accent)' : 'var(--text-tertiary)',
                            }}
                          >
                            {apiCustomModel ? '取消自定义' : '自定义'}
                          </button>
                        </div>
                        {apiCustomModel || provider.models.length === 0 ? (
                          <input
                            value={(params['api_model'] as string) || ''}
                            onChange={(e) => handleParamChange('api_model', e.target.value)}
                            placeholder="输入模型名称，如 gpt-4o-2024-08-06"
                            disabled={modelGreyedOut}
                            style={{ ...S.input(), marginTop: 4, ...greyStyle }}
                          />
                        ) : (
                          <Select
                            value={(params['api_model'] as string) || ''}
                            onChange={(v) => handleParamChange('api_model', v)}
                            disabled={modelGreyedOut}
                            style={{ marginTop: 4 }}
                            options={[
                              { value: '', label: '选择模型' },
                              /* 模型不在该厂商的模型清单里（手工填过 / 清单变化）也要显示原值 */
                              ...(params['api_model'] && !provider.models.includes(params['api_model'] as string)
                                ? [{ value: params['api_model'] as string, label: `${params['api_model']}（不在厂商模型清单中）` }]
                                : []),
                              ...provider.models.map((m) => ({ value: m, label: m })),
                            ]}
                            placeholder="选择模型"
                            className="w-full min-w-0"
                          />
                        )}
                      </div>
                    ) : (
                      // 厂商不在配置列表里（被删/改名）：仍把 JSON 里的模型名显示出来，避免"面板与 JSON 不一致"
                      <div style={{ marginTop: 10 }}>
                        <label style={S.label()}>模型</label>
                        <input
                          value={(params['api_model'] as string) || ''}
                          onChange={(e) => handleParamChange('api_model', e.target.value)}
                          placeholder="模型名（按已保存的值显示）"
                          disabled={modelGreyedOut}
                          style={{ ...S.input(), marginTop: 4, ...greyStyle }}
                        />
                        <div style={{ marginTop: 4, fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>
                          当前厂商（{(params['api_provider'] as string) || '未选择'}）不在已配置的 API 提供商里，模型名按原值显示；请在上方选择有效厂商，运行期才能真正生效。
                        </div>
                      </div>
                    )}
                    {/* 三级之后的「工具授权」：节点级，运行前定好；命中审批的调用会暂停等用户确认 */}
                    <div style={{ marginTop: 10 }}>
                      <label style={S.label()}>工具授权</label>
                      <Select
                        value={securityMode}
                        onChange={(v) => handleParamChange('security_mode', v)}
                        options={SECURITY_MODE_OPTIONS.map((m) => ({ value: m.value, label: m.label }))}
                        className="w-full min-w-0"
                      />
                      <div style={{ marginTop: 4, fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>
                        {securityHint}（仅 API Agent 生效）
                      </div>
                    </div>
                  </>
                );
              })()}
              </>
            ) : configFields[0].type === 'subflow_select' ? (
              <SubflowSelector
                definitions={definitions}
                currentDefinitionId={definitionId}
                value={(params[configFields[0].key] as string) || ''}
                onChange={(v) => handleParamChange(configFields[0].key, v)}
                onOpenSubflow={onOpenSubflow}
                onCreateNew={() => {
                  const newId = 'wf_' + Date.now().toString(36);
                  handleParamChange(configFields[0].key, newId);
                }}
                loadDefinitions={loadDefinitions}
              />
            ) : configFields[0].type === 'plugin_select' ? (
              <>
              <Select
                value={selectedPluginId || ''}
                onChange={applyPluginChange}
                options={[
                  { value: '', label: '选择插件' },
                  ...availablePlugins.map((p) => ({ value: p.id, label: p.name })),
                ]}
                placeholder="选择插件"
                className="w-full min-w-0"
              />
              {/* 命令选择器：根据已选插件动态显示 */}
              {selectedPluginId && (() => {
                const cmds = pluginCommands(selectedPluginId);
                if (cmds.length === 0) return null;
                return (
                  <div style={{ marginTop: 6 }}>
                    <label style={{ ...S.label(), marginBottom: 4, display: 'block' }}>选择命令</label>
                    <Select
                      value={node.commandId || ''}
                      onChange={applyCommandChange}
                      options={[
                        { value: '', label: '选择命令' },
                        ...cmds.map((cmd) => ({ value: cmd.id, label: cmd.title })),
                      ]}
                      placeholder="选择命令"
                      className="w-full min-w-0"
                    />
                  </div>
                );
              })()}
              </>
            ) : (
              <input
                type={configFields[0].type || 'text'}
                value={(params[configFields[0].key] as string) || ''}
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
      {node.type !== 'start' ? (
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
            key={'in:' + node.id}
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
      ) : null}

      {/* ===== textarea 类型的第一个字段（如 transform 的转换脚本） — 在输入映射之后渲染 ===== */}
      {configFields.length > 0 && configFields[0].type === 'textarea' && (
        <div style={S.sectionGap}>
          <div style={S.sectionTitle}>{configFields[0].label}</div>
          <TemplateField
            value={(params[configFields[0].key] as string) || ''}
            onChange={(v) => handleParamChange(configFields[0].key, v)}
            variables={templateVariables}
            placeholder={configFields[0].placeholder}
            multiline
            rows={(configFields[0] as SchemaConfigField & { rows?: number }).rows ?? 4}
            style={S.textarea}
            // transform 节点的 script 是 JS 代码，`{{` 不是模板变量，关掉补全
            enableCompletion={!(node.type === 'transform' && configFields[0].key === 'script')}
          />
          {/* transform 脚本变量可见化：列出本节点输入映射的字段名（脚本里直接当变量用） */}
          {node.type === 'transform' && configFields[0].key === 'script' && (() => {
            const inputKeys = Object.keys(node.inputMapping || {});
            return (
              <div style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', marginTop: 4 }}>
                {inputKeys.length > 0
                  ? `本节点可用变量：${inputKeys.join('、')}（与 JS 保留字/内置对象同名的字段名不可直接使用，请改名）`
                  : '本节点尚未配置输入映射，脚本里没有可用变量'}
              </div>
            );
          })()}
        </div>
      )}

      {/* ===== 节点配置 ===== */}
      {configFields.slice(1).length > 0 && (
        <div style={S.sectionGap}>
          <div style={S.sectionTitle}>节点配置</div>

          {/* 其余配置字段（跳过第一个已在基本信息中） */}
          {configFields.slice(1).map((field) => (
            <div key={field.key} style={S.fieldGap}>
              <label style={S.label()}>{field.label}</label>
              {field.type === 'textarea' ? (
                <TemplateField
                  value={(params[field.key] as string) || ''}
                  onChange={(v) => handleParamChange(field.key, v)}
                  variables={templateVariables}
                  placeholder={field.placeholder}
                  multiline
                  rows={(field as SchemaConfigField & { rows?: number }).rows ?? 4}
                  style={S.textarea}
                />
              ) : field.type === 'subflow_select' ? (
                <div>
                  <SubflowSelector
                    definitions={definitions}
                    currentDefinitionId={definitionId}
                    value={(params[field.key] as string) || ''}
                    onChange={(v) => handleParamChange(field.key, v)}
                    onOpenSubflow={onOpenSubflow}
                    onCreateNew={() => {
                      const newId = 'wf_' + Date.now().toString(36);
                      handleParamChange(field.key, newId);
                    }}
                    loadDefinitions={loadDefinitions}
                  />
                </div>
              ) : field.type === 'select' ? (
                <Select
                  value={(params[field.key] as string) || field.placeholder || ''}
                  onChange={(v) => handleParamChange(field.key, v)}
                  options={[
                    { value: '', label: '选择...' },
                    /* 插件节点：从 field.options 渲染 */
                    ...((field as SchemaConfigField).options ?? []),
                    /* 内置节点：硬编码选项 */
                    ...(field.key === 'agent_type' ? enabledAgentTypes.map((v) => ({ value: v, label: v })) : []),
                    ...(field.key === 'method' ? ['GET', 'POST', 'PUT', 'DELETE'].map((v) => ({ value: v, label: v })) : []),
                    ...(field.key === 'inputType' ? INPUT_TYPE_OPTIONS : []),
                  ]}
                  placeholder="选择..."
                  className="w-full min-w-0"
                />
              ) : field.type === 'checkbox' ? (
                <label className="flex items-center gap-2" style={{ cursor: 'pointer', fontSize: 'var(--fs-12)', color: 'var(--text-secondary)' }}>
                  <input
                    type="checkbox"
                    checked={!!params[field.key]}
                    onChange={(e) => handleParamChange(field.key, e.target.checked)}
                  />
                  {(field as SchemaConfigField).description || field.placeholder}
                </label>
              ) : (
                <input
                  type={field.type}
                  value={(params[field.key] as string) || ''}
                  onChange={(e) => handleParamChange(field.key, field.type === 'number' ? Number(e.target.value) : e.target.value)}
                  placeholder={field.placeholder}
                  style={S.input()}
                />
              )}
            </div>
          ))}
        </div>
      )}

{/* ===== Interact 节点专属配置（按 inputType 动态显示） ===== */}
{node.type === 'interact' && (() => {
  const inputType = (params['inputType'] as string) || 'text';

  // ---- options 文本 <-> JSON 转换 ----
  // 存储格式：params.optionsJson = JSON 字符串 '[{"label":"通过","value":"approve"},...]'
  // 展示格式：每行一条 "label:value"
  const optionsText = (() => {
    const raw = params['optionsJson'] as string | undefined;
    if (!raw) return '';
    try {
      const arr = JSON.parse(raw) as Array<{ label: string; value: string }>;
      if (!Array.isArray(arr)) return '';
      return arr.map(o => `${o.label || ''}:${o.value || ''}`).join('\n');
    } catch { return ''; }
  })();

  const setOptionsFromText = (text: string) => {
    const lines = text.split('\n').map(l => l.trim()).filter(Boolean);
    const arr = lines.map(line => {
      // 支持两种分隔符：英文冒号 ":" 或中文全角 "："
      const sep = line.includes('：') ? '：' : ':';
      const idx = line.indexOf(sep);
      if (idx === -1) {
        // 整行既是 label 又是 value
        return { label: line, value: line };
      }
      const label = line.substring(0, idx).trim();
      const value = line.substring(idx + sep.length).trim();
      return { label, value };
    });
    handleParamChange('optionsJson', arr.length > 0 ? JSON.stringify(arr) : '');
  };

  return (
    <div style={S.sectionGap}>
      <div style={S.sectionTitle}>交互专属配置 ({inputType})</div>

      {/* text / file 类型：显示占位符 + 默认值 */}
      {(inputType === 'text' || inputType === 'file') && (
        <>
          <div style={S.fieldGap}>
            <label style={S.label()}>输入框占位符</label>
            <input
              type="text"
              value={(params['placeholder'] as string) || ''}
              onChange={(e) => handleParamChange('placeholder', e.target.value)}
              placeholder={inputType === 'file' ? '运行时读取文件内容的占位提示...' : '请输入...'}
              style={S.input()}
            />
          </div>
          <div style={S.fieldGap}>
            <label style={S.label()}>默认值（超时或跳过时使用）</label>
            <input
              type="text"
              value={(params['defaultValue'] as string) || ''}
              onChange={(e) => handleParamChange('defaultValue', e.target.value)}
              placeholder="默认响应内容"
              style={S.input()}
            />
          </div>
        </>
      )}

      {/* select 类型：选项列表 + 允许自定义 */}
      {inputType === 'select' && (
        <>
          <div style={S.fieldGap}>
            <label style={S.label()}>选项列表（每行一条，格式：显示文本:值）</label>
            <textarea
              value={optionsText}
              onChange={(e) => setOptionsFromText(e.target.value)}
              placeholder={'通过:approve\n拒绝:reject\n（不填冒号时，整行同时作为显示和值）'}
              rows={5}
              style={S.textarea}
            />
            <div style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', marginTop: 4 }}>
              支持分隔符：英文 ":" 或中文 "："
            </div>
          </div>
          <div style={S.fieldGap}>
            <label className="flex items-center gap-2" style={{ cursor: 'pointer', fontSize: 'var(--fs-12)', color: 'var(--text-secondary)' }}>
              <input
                type="checkbox"
                checked={!!params['allowCustom']}
                onChange={(e) => handleParamChange('allowCustom', e.target.checked)}
              />
              允许用户自定义值（除预设选项外的自由输入）
            </label>
          </div>
        </>
      )}

      {/* confirm 类型：无额外字段，仅展示说明 */}
      {inputType === 'confirm' && (
        <div style={{ fontSize: 'var(--fs-11)', color: 'var(--text-tertiary)', padding: '6px 0' }}>
          确认类型无需额外配置：运行时弹出"确认 / 取消"两个按钮，响应值分别为 <code>confirm</code> / <code>cancel</code>。
        </div>
      )}
    </div>
  );
})()}


{/* ===== 插件配置（命令级组件优先，其次插件默认组件，最后按所选命令的 input 模式生成） ===== */}
      {node.type === 'plugin' && params.plugin_id && (() => {
        const pluginId = params.plugin_id as string;
        const PluginComp = pluginRegistry.getWorkflowConfig(pluginId, node.commandId);
        const command = findPluginCommand(pluginId, node.commandId);
        return (
          <div style={S.sectionGap}>
            <div style={S.sectionTitle}>插件配置</div>
            {/* 变量上下文：插件表单里的 TemplateField 未显式传 variables 时按此兜底，
                避免"作者漏传 → {{ 补全空列表"（变量本就属于宿主） */}
            <TemplateVariablesContext.Provider value={templateVariables}>
            {PluginComp ? (
              <PluginComp
                // 按「插件 + 命令」重建表单实例：同一个命令内打字不会重建，
                // 但切换命令必须重建 —— 否则组件里"挂载时初始化一次"的逻辑（写默认值、
                // 级联下级选项）会带着上一个命令的本地状态继续用，表现为"切到默认命令后
                // 执行报错、换一个选项就正常"。
                key={`${params.plugin_id}|${node.commandId || ''}`}
                params={params}
                onParamsChange={handleParamChange}
                api={pluginRegistry.getPluginAPI(params.plugin_id as string) || undefined}
                commandId={node.commandId || ''}
                command={command}
                variables={templateVariables}
                TemplateField={TemplateField}
              />
            ) : command && commandParamKeys(command).length > 0 ? (
              <SchemaParamForm
                // 同上：平台表单按命令重建，避免残留上一个命令填过的参数
                key={`${params.plugin_id}|${node.commandId || ''}`}
                command={command}
                params={params}
                onParamsChange={handleParamChange}
                variables={templateVariables}
              />
            ) : (
              <div style={{ fontSize: 'var(--fs-11)', color: 'var(--text-tertiary)' }}>
                {node.commandId
                  ? '该命令没有参数表单：参数请在节点的「输入映射」里配置，或由插件补充 input 模式 / workflow_config 组件。'
                  : '先在上面「选择命令」，这里会显示该命令的参数表单。'}
              </div>
            )}
            </TemplateVariablesContext.Provider>
          </div>
        );
      })()}

            {node.type !== 'end' && (
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
            key={'out:' + node.id}
            value={node.outputMapping}
            onChange={(v) => {
              onUpdate({ outputMapping: v });
              const keys = Object.keys(v || {});
              if (keys.length === 1) outputBaseKeyRef.current = keys[0];
            }}
            keyPlaceholder="输出字段名"
            valuePlaceholder={'输入文本或选择输出字段（支持 content.score 这类路径）'}
            baseKeyRef={outputBaseKeyRef}
            valueOptions={getOutputFieldOptions(node.type as WorkflowNodeType)} />
      </div>
      )}

      {/* ===== 控制属性 ===== */}
      <div style={S.sectionGap}>
        <div style={S.sectionTitle}>控制属性</div>
        <div className="grid grid-cols-2" style={{ gap: 8 }}>
          {[
            { key: 'delayMs', label: '延迟 (ms)', ph: '0' },
            { key: 'timeoutMs', label: '超时 (ms)', ph: '留空 = 不限制' },
          ].map((item) => (
            <div key={item.key}>
              <label style={S.labelSm()}>{item.label}</label>
              <input
                type="number"
                value={(item.key === 'delayMs' ? node.delayMs : node.timeoutMs) || ''}
                onChange={(e) => onUpdate({ [item.key]: e.target.value ? Number(e.target.value) : undefined })}
                placeholder={item.ph}
                style={S.input('var(--fs-11)', '4px 8px')}
              />
            </div>
          ))}
        </div>
      </div>
      </>
    </div>
  );
};
