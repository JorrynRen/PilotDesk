/**
 * WorkflowTemplateMarket — 工作流模板市场（在线市场）UI 原型
 *
 * 设计要点：
 * - 视觉风格完全沿用 PilotDesk 设计 Token（globals.css 的 --bg-* / --text-* / --border / --accent）
 * - 组件风格参考 OnlinePluginStore.tsx 和 Inspiration/MarketPage.tsx
 * - 数据源目前用 mockData（前端静态假数据），便于 UI 评审；后续接入后端只需要替换 fetch 逻辑
 * - 三个层级：
 *     顶部 Tab：精选 / 分类列表 / 我的收藏
 *     左栏：分类树 + 过滤条件
 *     主区：卡片网格（可切换 列表/网格 模式）
 * - 点击卡片：右侧展开详情抽屉（Preview），含安装/预览流程节点图
 */

import React, { useState, useMemo } from 'react';
import {
  Search, RefreshCw, Download, Star, Grid3X3, List, SlidersHorizontal,
  ChevronRight, Filter, X, Heart, FolderOpen, Sparkles, Clock, User,
  Zap, TrendingUp, CheckCircle, Eye, LayoutGrid, Code2, PackageOpen,
  ArrowLeftToLine,
} from 'lucide-react';
import { createPortal } from 'react-dom';

// ── 数据类型 ──
type InstallStatus = 'not-installed' | 'installed' | 'update-available' | 'installing';

interface WorkflowTemplateTag {
  id: string;
  name: string;
  /** 分类大类：用于左栏分类树的一级节点 */
  category: string;
}

interface WorkflowTemplateNodePreview {
  id: string;
  nodeType: string;   // 如 start / llm / http / condition / end
  label: string;
  column: number;     // 布局列，用于渲染流程图
  row: number;
}

interface WorkflowTemplate {
  id: string;
  name: string;
  description: string;
  category: string;
  tags: string[];
  author: string;
  avatar?: string;
  icon: string;                 // emoji 或图标
  accentColor: string;          // 模板主色（用作卡片 header 渐变）
  version: string;
  minAppVersion: string;
  updatedAt: string;            // '2026-07-14'
  downloads: number;
  likes: number;
  difficulty: '入门' | '中级' | '进阶';
  estimateMinutes: number;      // 预估配置耗时
  nodeCount: number;            // 节点数
  triggerType: '手动' | '定时' | '事件';
  installStatus: InstallStatus;
  verified?: boolean;           // 官方 / 认证作者
  /** 预览节点（绘制缩略流程图） */
  previewNodes: WorkflowTemplateNodePreview[];
  /** 步骤要点（详情展示） */
  highlights: string[];
}

// ── Mock 数据 ──
const CATEGORIES: Array<{ id: string; name: string; icon: string; count: number; sub?: Array<{ id: string; name: string; count: number }> }> = [
  { id: 'all', name: '全部模板', icon: '📦', count: 128 },
  { id: 'automation', name: '自动化办公', icon: '⚙️', count: 41, sub: [
    { id: 'auto-email', name: '邮件与通知', count: 13 },
    { id: 'auto-report', name: '报表生成', count: 11 },
    { id: 'auto-file', name: '文件处理', count: 9 },
    { id: 'auto-web', name: '网页采集', count: 8 },
  ]},
  { id: 'agent', name: 'Agent 协作', icon: '🤖', count: 32, sub: [
    { id: 'agent-multi', name: '多 Agent 协作', count: 14 },
    { id: 'agent-review', name: '审查与校对', count: 8 },
    { id: 'agent-research', name: '研究与写作', count: 10 },
  ]},
  { id: 'data', name: '数据处理', icon: '📊', count: 27, sub: [
    { id: 'data-etl', name: 'ETL 与清洗', count: 10 },
    { id: 'data-analytics', name: '数据分析', count: 9 },
    { id: 'data-visualize', name: '可视化报告', count: 8 },
  ]},
  { id: 'devops', name: '研发效能', icon: '🛠️', count: 16, sub: [
    { id: 'devops-ci', name: 'CI/CD', count: 7 },
    { id: 'devops-monitor', name: '监控告警', count: 9 },
  ]},
  { id: 'creative', name: '内容创作', icon: '🎨', count: 12 },
];

const TRIGGER_OPTIONS: Array<{ key: 'all' | '手动' | '定时' | '事件'; label: string }> = [
  { key: 'all', label: '全部' },
  { key: '手动', label: '手动触发' },
  { key: '定时', label: '定时运行' },
  { key: '事件', label: '事件驱动' },
];

const DIFFICULTY_OPTIONS: Array<{ key: 'all' | '入门' | '中级' | '进阶'; label: string; color: string }> = [
  { key: 'all', label: '全部', color: 'var(--text-secondary)' },
  { key: '入门', label: '入门', color: '#10B981' },
  { key: '中级', label: '中级', color: '#F59E0B' },
  { key: '进阶', label: '进阶', color: '#EF4444' },
];

const SORT_OPTIONS = [
  { key: 'recommended', label: '推荐排序' },
  { key: 'downloads',   label: '下载量最高' },
  { key: 'likes',       label: '收藏最多' },
  { key: 'updated',     label: '最近更新' },
  { key: 'name',        label: '名称 A-Z' },
] as const;

/** 生成缩略流程图节点（mock） */
function makePreview(template: string): WorkflowTemplateNodePreview[] {
  switch (template) {
    case 'digest': return [
      { id: '1', nodeType: 'start', label: '每日 09:00', column: 0, row: 1 },
      { id: '2', nodeType: 'http',  label: '聚合新闻源', column: 1, row: 0 },
      { id: '3', nodeType: 'llm',   label: 'AI 摘要', column: 2, row: 0 },
      { id: '4', nodeType: 'http',  label: '抓取代码仓库', column: 1, row: 2 },
      { id: '5', nodeType: 'llm',   label: '生成周报', column: 2, row: 1 },
      { id: '6', nodeType: 'email', label: '邮件发送', column: 3, row: 1 },
      { id: '7', nodeType: 'end',   label: '完成', column: 4, row: 1 },
    ];
    case 'support': return [
      { id: '1', nodeType: 'start', label: '工单事件', column: 0, row: 1 },
      { id: '2', nodeType: 'llm',   label: '意图识别', column: 1, row: 1 },
      { id: '3', nodeType: 'condition', label: '是否常见问题', column: 2, row: 1 },
      { id: '4', nodeType: 'llm',   label: 'AI 自动回复', column: 3, row: 0 },
      { id: '5', nodeType: 'human', label: '人工介入', column: 3, row: 2 },
      { id: '6', nodeType: 'end',   label: '结束', column: 4, row: 1 },
    ];
    case 'data': return [
      { id: '1', nodeType: 'start', label: '触发', column: 0, row: 1 },
      { id: '2', nodeType: 'csv',   label: '读取 CSV', column: 1, row: 0 },
      { id: '3', nodeType: 'db',    label: '查询 SQL', column: 1, row: 2 },
      { id: '4', nodeType: 'code',  label: '清洗合并', column: 2, row: 1 },
      { id: '5', nodeType: 'llm',   label: '异常标注', column: 3, row: 1 },
      { id: '6', nodeType: 'excel', label: '输出报表', column: 4, row: 1 },
      { id: '7', nodeType: 'end',   label: '完成', column: 5, row: 1 },
    ];
    case 'simple': return [
      { id: '1', nodeType: 'start', label: '启动', column: 0, row: 0 },
      { id: '2', nodeType: 'llm',   label: '处理', column: 1, row: 0 },
      { id: '3', nodeType: 'end',   label: '结束', column: 2, row: 0 },
    ];
    case 'social': return [
      { id: '1', nodeType: 'start', label: '定时', column: 0, row: 0 },
      { id: '2', nodeType: 'llm',   label: '文案生成', column: 1, row: 0 },
      { id: '3', nodeType: 'condition', label: '人工审核', column: 2, row: 0 },
      { id: '4', nodeType: 'http',  label: '发布', column: 3, row: 0 },
      { id: '5', nodeType: 'end',   label: '结束', column: 4, row: 0 },
    ];
    default: return [];
  }
}

const MOCK_TEMPLATES: WorkflowTemplate[] = [
  {
    id: 'daily-digest',
    name: '每日资讯与周报自动推送',
    description: '每天早上 9 点自动聚合 RSS、代码仓库、团队协作文档，生成 AI 摘要并发送邮件给成员，每周一额外生成周度汇总报告。',
    category: 'automation',
    tags: ['邮件', '摘要', '周报', 'Cron'],
    author: 'PilotDesk 官方',
    icon: '📰',
    accentColor: '#3B82F6',
    version: '1.2.0',
    minAppVersion: '0.4.0',
    updatedAt: '2026-08-05',
    downloads: 1284,
    likes: 342,
    difficulty: '入门',
    estimateMinutes: 5,
    nodeCount: 7,
    triggerType: '定时',
    installStatus: 'not-installed',
    verified: true,
    previewNodes: makePreview('digest'),
    highlights: [
      '内置 10+ 新闻源模板，开箱即用',
      '支持自定义 LLM 提供商，可切换摘要模型',
      '失败自动重试 + 告警',
    ],
  },
  {
    id: 'ticket-auto-reply',
    name: '智能工单自动分类与回复',
    description: '接入工单系统（飞书/钉钉/Zendesk），对新工单做意图识别和分类，常见问题由 AI 自动回复，复杂问题路由给人工坐席并附带上下文摘要。',
    category: 'agent',
    tags: ['工单', '客服', '分类'],
    author: 'PilotDesk 官方',
    icon: '🎟️',
    accentColor: '#8B5CF6',
    version: '2.0.1',
    minAppVersion: '0.4.0',
    updatedAt: '2026-08-01',
    downloads: 938,
    likes: 287,
    difficulty: '中级',
    estimateMinutes: 15,
    nodeCount: 6,
    triggerType: '事件',
    installStatus: 'installed',
    verified: true,
    previewNodes: makePreview('support'),
    highlights: [
      '支持知识库 RAG 检索增强',
      '人工介入节点配置 SLA 超时',
      '自动同步回复结果到工单系统',
    ],
  },
  {
    id: 'csv-etl',
    name: 'CSV/SQL 数据清洗与报表生成',
    description: '从 CSV、Excel、MySQL/Postgres 读取数据，按自定义规则清洗合并，LLM 自动标注异常行，最后输出 Excel 报表并推送通知。',
    category: 'data',
    tags: ['ETL', '数据', 'Excel', 'SQL'],
    author: '阿明数据',
    icon: '📈',
    accentColor: '#10B981',
    version: '1.0.3',
    minAppVersion: '0.3.5',
    updatedAt: '2026-07-28',
    downloads: 672,
    likes: 198,
    difficulty: '中级',
    estimateMinutes: 10,
    nodeCount: 7,
    triggerType: '手动',
    installStatus: 'update-available',
    previewNodes: makePreview('data'),
    highlights: [
      '内置 20+ 清洗函数',
      '支持中文列名的 SQL 脚本',
      '异常行自动高亮与备注',
    ],
  },
  {
    id: 'social-post',
    name: '多平台社媒内容批量发布',
    description: '统一编辑一条内容，一键分发到微博、公众号、知乎、小红书；发布前走人工审核；发布后汇总互动数据。',
    category: 'creative',
    tags: ['社媒', '内容', '发布'],
    author: '创意工作室',
    icon: '📱',
    accentColor: '#EC4899',
    version: '0.9.2',
    minAppVersion: '0.4.0',
    updatedAt: '2026-07-20',
    downloads: 428,
    likes: 132,
    difficulty: '入门',
    estimateMinutes: 8,
    nodeCount: 5,
    triggerType: '手动',
    installStatus: 'not-installed',
    previewNodes: makePreview('social'),
    highlights: [
      '内置图片压缩与格式转换',
      '支持按平台差异化改写文案',
      '多账号统一管理',
    ],
  },
  {
    id: 'ci-quality',
    name: 'PR 代码质量门禁',
    description: '监听 GitHub / GitLab PR 事件，调用 SonarQube、lint、单元测试，并让 LLM 做代码审查评论；不达标自动打上标签并通知负责人。',
    category: 'devops',
    tags: ['CI', '代码审查', 'Git'],
    author: 'Ops 团队',
    icon: '🔍',
    accentColor: '#F59E0B',
    version: '1.5.0',
    minAppVersion: '0.4.0',
    updatedAt: '2026-07-30',
    downloads: 512,
    likes: 176,
    difficulty: '进阶',
    estimateMinutes: 20,
    nodeCount: 6,
    triggerType: '事件',
    installStatus: 'not-installed',
    previewNodes: makePreview('data'),
    highlights: [
      '支持 GitHub / GitLab / Gitea',
      '审查结果按文件级别回写 PR 评论',
      '门禁规则可自定义',
    ],
  },
  {
    id: 'invoice-ocr',
    name: '发票 OCR 识别与录入',
    description: '监控文件夹新增 PDF/图片，调用 OCR 识别发票关键信息，校验后写入 Excel 或财务系统，失败异常发送审核通知。',
    category: 'automation',
    tags: ['OCR', '财务', 'PDF'],
    author: '财务自动化小组',
    icon: '🧾',
    accentColor: '#0EA5E9',
    version: '1.1.0',
    minAppVersion: '0.3.5',
    updatedAt: '2026-07-15',
    downloads: 398,
    likes: 124,
    difficulty: '中级',
    estimateMinutes: 12,
    nodeCount: 5,
    triggerType: '事件',
    installStatus: 'not-installed',
    previewNodes: makePreview('simple'),
    highlights: [
      '支持增值税专票/普票',
      '金额自动校验与异常告警',
      '可导出多种格式',
    ],
  },
  {
    id: 'research-assistant',
    name: '论文/研报 AI 研究助手',
    description: '批量下载 ArXiv / 研报 PDF，使用多 Agent 分工：抽取要点、翻译、结构化笔记、生成 Markdown 表格并做对比分析。',
    category: 'agent',
    tags: ['研究', 'PDF', '多Agent'],
    author: '研究员小川',
    icon: '📚',
    accentColor: '#6366F1',
    version: '0.8.5',
    minAppVersion: '0.4.0',
    updatedAt: '2026-08-03',
    downloads: 712,
    likes: 256,
    difficulty: '进阶',
    estimateMinutes: 18,
    nodeCount: 6,
    triggerType: '手动',
    installStatus: 'not-installed',
    previewNodes: makePreview('support'),
    highlights: [
      '支持 ArXiv / SSRN 等多数据源',
      '长文本分块 + 并行摘要',
      '自动生成研究笔记和双向链接',
    ],
  },
  {
    id: 'monitor-heartbeat',
    name: '服务心跳与告警面板',
    description: '定时检查 HTTP/API/端口可用性，失败时自动升级告警（邮件 → IM → 电话），并生成历史可用性面板。',
    category: 'devops',
    tags: ['监控', '告警', '可用性'],
    author: 'PilotDesk 官方',
    icon: '📡',
    accentColor: '#EF4444',
    version: '1.0.0',
    minAppVersion: '0.4.0',
    updatedAt: '2026-08-08',
    downloads: 356,
    likes: 94,
    difficulty: '入门',
    estimateMinutes: 6,
    nodeCount: 4,
    triggerType: '定时',
    installStatus: 'not-installed',
    verified: true,
    previewNodes: makePreview('simple'),
    highlights: [
      '支持 HTTPS / TCP / DNS 多种检查',
      '告警升级时间线可配置',
      '自动生成可用性周报',
    ],
  },
];

// ── 小工具 ──
const formatK = (n: number): string => n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);

function relativeDay(dateStr: string): string {
  const d = new Date(dateStr);
  const diff = Math.floor((Date.now() - d.getTime()) / 86400000);
  if (diff <= 0) return '今天';
  if (diff === 1) return '昨天';
  if (diff < 30) return `${diff} 天前`;
  if (diff < 365) return `${Math.floor(diff / 30)} 个月前`;
  return `${Math.floor(diff / 365)} 年前`;
}

// ── 缩略流程图渲染 ──
const NODE_COLORS: Record<string, string> = {
  start: '#10B981', end: '#6B7280',
  llm: '#8B5CF6', http: '#3B82F6', condition: '#F59E0B',
  email: '#0EA5E9', human: '#EC4899', code: '#64748B',
  csv: '#10B981', db: '#0891B2', excel: '#16A34A',
};

function WorkflowMiniPreview({ nodes, accent }: { nodes: WorkflowTemplateNodePreview[]; accent: string }) {
  // 简单计算画布
  const rows = Math.max(...nodes.map(n => n.row), 0) + 1;
  const cols = Math.max(...nodes.map(n => n.column), 0) + 1;
  const cellW = 52, cellH = 36;
  const w = cols * cellW;
  const h = rows * cellH;

  // 画连接线：按 row 连到下一层（粗略 heuristic：同 row 的下一列直连，不同列折线）
  const byCol = new Map<number, WorkflowTemplateNodePreview[]>();
  nodes.forEach(n => {
    const list = byCol.get(n.column) || [];
    list.push(n);
    byCol.set(n.column, list);
  });

  return (
    <svg width="100%" viewBox={`0 -4 ${w} ${h + 8}`} style={{ maxHeight: 80 }}>
      {/* 连线 */}
      {Array.from(byCol.keys()).sort((a, b) => a - b).map(col => {
        const left = byCol.get(col) || [];
        const right = byCol.get(col + 1) || [];
        return left.flatMap((ln, li) => right.slice(0, Math.max(1, Math.ceil(right.length / Math.max(left.length, 1)))).map(rn => {
          const x1 = (ln.column + 1) * cellW - cellW / 2 - 14;
          const y1 = ln.row * cellH + cellH / 2;
          const x2 = rn.column * cellW + cellW / 2 - 14;
          const y2 = rn.row * cellH + cellH / 2;
          return (
            <line key={`${ln.id}-${rn.id}-${li}`}
              x1={x1} y1={y1} x2={x2} y2={y2}
              stroke="var(--border)" strokeWidth="1.2" strokeDasharray="2 2" />
          );
        }));
      })}
      {/* 节点 */}
      {nodes.map(n => (
        <g key={n.id} transform={`translate(${n.column * cellW + 6}, ${n.row * cellH + 6})`}>
          <rect
            width={cellW - 12} height={cellH - 12} rx={5} ry={5}
            fill={NODE_COLORS[n.nodeType] || accent}
            opacity={0.18}
            stroke={NODE_COLORS[n.nodeType] || accent}
            strokeWidth="0.8"
          />
          <text
            x={(cellW - 12) / 2} y={(cellH - 12) / 2 + 3}
            textAnchor="middle"
            fontSize="7"
            fill="var(--text-secondary)"
          >{n.label.slice(0, 5)}</text>
        </g>
      ))}
    </svg>
  );
}

// ── 卡片：网格模式 ──
function TemplateCardGrid({
  t, onToggleLike, onInstall, onOpenDetail,
}: {
  t: WorkflowTemplate;
  onToggleLike: (id: string) => void;
  onInstall: (id: string) => void;
  onOpenDetail: (t: WorkflowTemplate) => void;
}) {
  const [liked, setLiked] = useState(false);

  const actionBtn = () => {
    switch (t.installStatus) {
      case 'installed':
        return (
          <span className="flex items-center gap-1 text-[10px]" style={{ color: '#10B981' }}>
            <CheckCircle size={10} /> 已安装
          </span>
        );
      case 'update-available':
        return (
          <button
            onClick={(e) => { e.stopPropagation(); onInstall(t.id); }}
            className="pd-btn px-2 py-1 rounded text-[10px]"
            style={{ backgroundColor: '#F59E0B', color: '#fff', display: 'inline-flex', alignItems: 'center', gap: 3 }}
          >
            <Download size={10} /> 更新
          </button>
        );
      case 'installing':
        return (
          <button disabled className="pd-btn px-2 py-1 rounded text-[10px]"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
            安装中…
          </button>
        );
      default:
        return (
          <button
            onClick={(e) => { e.stopPropagation(); onInstall(t.id); }}
            className="pd-btn px-2 py-1 rounded text-[10px]"
            style={{ backgroundColor: 'var(--accent)', color: '#fff', display: 'inline-flex', alignItems: 'center', gap: 3 }}
          >
            <Download size={10} /> 使用模板
          </button>
        );
    }
  };

  return (
    <div
      className="rounded-lg overflow-hidden transition-all"
      style={{
        backgroundColor: 'var(--bg-secondary)',
        border: '1px solid var(--border)',
        cursor: 'pointer',
      }}
      onClick={() => onOpenDetail(t)}
      onMouseEnter={(e) => {
        e.currentTarget.style.borderColor = 'var(--accent)';
        e.currentTarget.style.transform = 'translateY(-1px)';
        e.currentTarget.style.boxShadow = 'var(--shadow-md)';
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.borderColor = 'var(--border)';
        e.currentTarget.style.transform = 'none';
        e.currentTarget.style.boxShadow = 'none';
      }}
    >
      {/* 顶部彩色带 + 图标 + 收藏 */}
      <div className="relative px-3 pt-3 pb-2"
        style={{
          backgroundImage: `linear-gradient(135deg, ${t.accentColor}22, transparent 70%)`,
        }}
      >
        <div className="flex items-start justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <div className="flex items-center justify-center shrink-0"
              style={{ width: 28, height: 28, borderRadius: 6, backgroundColor: `${t.accentColor}18`, fontSize: 16 }}>
              {t.icon}
            </div>
            <div className="min-w-0">
              <div className="flex items-center gap-1">
                <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }} title={t.name}>{t.name}</span>
                {t.verified && (
                  <span title="官方模板" className="text-[9px] font-medium" style={{
                    color: 'var(--accent)',
                    padding: '0 5px',
                    borderRadius: 999,
                    backgroundColor: 'var(--accent-bg)',
                    border: '0.5px solid var(--accent)',
                    lineHeight: '16px',
                  }}>官方</span>
                )}
              </div>
              <div className="flex items-center gap-2 mt-0.5">
                <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>v{t.version}</span>
                <span className="text-[9px]" style={{
                  color: DIFFICULTY_OPTIONS.find(d => d.key === t.difficulty)?.color || 'var(--text-tertiary)',
                  padding: '0 6px', borderRadius: 999,
                  backgroundColor: `${DIFFICULTY_OPTIONS.find(d => d.key === t.difficulty)?.color || '#999'}18`,
                }}>
                  {t.difficulty}
                </span>
              </div>
            </div>
          </div>
          <button
            onClick={(e) => { e.stopPropagation(); setLiked(l => !l); onToggleLike(t.id); }}
            className="pd-btn p-1 rounded shrink-0"
            style={{ color: liked ? '#EF4444' : 'var(--text-tertiary)' }}
            title={liked ? '已收藏' : '收藏'}
          >
            <Heart size={12} fill={liked ? '#EF4444' : 'none'} />
          </button>
        </div>
      </div>

      {/* 缩略流程图 */}
      <div className="px-3 pb-2" style={{ backgroundColor: 'transparent' }}>
        <div className="rounded p-1.5" style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border-light)' }}>
          <WorkflowMiniPreview nodes={t.previewNodes} accent={t.accentColor} />
        </div>
      </div>

      {/* 描述 */}
      <div className="px-3 pb-2">
        <p className="text-[10px] line-clamp-2" style={{
          color: 'var(--text-secondary)',
          lineHeight: 1.5,
          minHeight: 30,
          margin: 0,
        }}>{t.description}</p>
      </div>

      {/* 标签 */}
      <div className="px-3 pb-2 flex flex-wrap gap-1">
        {t.tags.slice(0, 3).map(tag => (
          <span key={tag} className="text-[9px] px-1.5 py-0.5 rounded"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
            {tag}
          </span>
        ))}
      </div>

      {/* 底部操作行 */}
      <div className="flex items-center justify-between gap-2 px-3 py-2"
        style={{ borderTop: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2.5 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          <span className="flex items-center gap-0.5"><Download size={9} /> {formatK(t.downloads)}</span>
          <span className="flex items-center gap-0.5"><TrendingUp size={9} /> {t.likes}</span>
          <span className="flex items-center gap-0.5"><LayoutGrid size={9} /> {t.nodeCount}</span>
        </div>
        {actionBtn()}
      </div>
    </div>
  );
}

// ── 卡片：列表模式 ──
function TemplateCardList({
  t, onToggleLike, onInstall, onOpenDetail,
}: {
  t: WorkflowTemplate;
  onToggleLike: (id: string) => void;
  onInstall: (id: string) => void;
  onOpenDetail: (t: WorkflowTemplate) => void;
}) {
  const [liked, setLiked] = useState(false);

  const installBtn = () => {
    switch (t.installStatus) {
      case 'installed':
        return (<span className="flex items-center gap-1 text-[10px]" style={{ color: '#10B981' }}><CheckCircle size={10} /> 已安装</span>);
      case 'update-available':
        return (
          <button onClick={(e) => { e.stopPropagation(); onInstall(t.id); }}
            className="pd-btn px-2.5 py-1 rounded text-[11px]"
            style={{ backgroundColor: '#F59E0B', color: '#fff', display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            <Download size={11} /> 更新
          </button>
        );
      default:
        return (
          <button onClick={(e) => { e.stopPropagation(); onInstall(t.id); }}
            className="pd-btn px-2.5 py-1 rounded text-[11px]"
            style={{ backgroundColor: 'var(--accent)', color: '#fff', display: 'inline-flex', alignItems: 'center', gap: 4 }}>
            <Download size={11} /> 使用模板
          </button>
        );
    }
  };

  return (
    <div className="rounded-lg overflow-hidden"
      style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      onClick={() => onOpenDetail(t)}
      onMouseEnter={(e) => { e.currentTarget.style.borderColor = 'var(--accent)'; }}
      onMouseLeave={(e) => { e.currentTarget.style.borderColor = 'var(--border)'; }}
    >
      <div className="flex items-center gap-3 px-3 py-2">
        {/* 左侧图标 */}
        <div className="shrink-0 flex items-center justify-center rounded-lg"
          style={{
            width: 48, height: 48,
            backgroundColor: `${t.accentColor}18`,
            fontSize: 24,
          }}>
          {t.icon}
        </div>
        {/* 中：信息 */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>{t.name}</span>
            {t.verified && (
              <span title="官方模板" className="text-[9px] font-medium" style={{
                color: 'var(--accent)',
                padding: '0 5px',
                borderRadius: 999,
                backgroundColor: 'var(--accent-bg)',
                border: '0.5px solid var(--accent)',
                lineHeight: '16px',
              }}>官方</span>
            )}
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>v{t.version}</span>
            <span className="text-[10px]" style={{
              color: DIFFICULTY_OPTIONS.find(d => d.key === t.difficulty)?.color,
              padding: '0 6px', borderRadius: 999,
              backgroundColor: `${DIFFICULTY_OPTIONS.find(d => d.key === t.difficulty)?.color}18`,
            }}>{t.difficulty}</span>
          </div>
          <p className="text-[11px] truncate mt-0.5" style={{ color: 'var(--text-secondary)', margin: 0 }}>
            {t.description}
          </p>
          <div className="flex items-center gap-3 mt-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            <span className="flex items-center gap-0.5"><User size={9} /> {t.author}</span>
            <span className="flex items-center gap-0.5"><Clock size={9} /> {relativeDay(t.updatedAt)}</span>
            <span className="flex items-center gap-0.5"><Zap size={9} /> {t.triggerType}</span>
            <span className="flex items-center gap-0.5"><LayoutGrid size={9} /> {t.nodeCount} 节点</span>
            <span>约 {t.estimateMinutes} 分钟配置</span>
          </div>
        </div>
        {/* 右：缩略流程图 */}
        <div className="shrink-0" style={{ width: 180 }}>
          <div className="rounded p-1.5" style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border-light)' }}>
            <WorkflowMiniPreview nodes={t.previewNodes} accent={t.accentColor} />
          </div>
        </div>
        {/* 右：操作 */}
        <div className="shrink-0 flex flex-col items-end gap-1.5">
          <div className="flex items-center gap-2">
            <button onClick={(e) => { e.stopPropagation(); setLiked(l => !l); onToggleLike(t.id); }}
              className="pd-btn p-1 rounded" style={{ color: liked ? '#EF4444' : 'var(--text-tertiary)' }}
              title={liked ? '已收藏' : '收藏'}>
              <Heart size={12} fill={liked ? '#EF4444' : 'none'} />
            </button>
            <button onClick={(e) => { e.stopPropagation(); onOpenDetail(t); }}
              className="pd-btn p-1 rounded" style={{ color: 'var(--text-secondary)' }}
              title="预览详情">
              <Eye size={12} />
            </button>
          </div>
          {installBtn()}
        </div>
      </div>
    </div>
  );
}

// ── 详情抽屉（Portal 全屏覆盖） ──
function TemplateDetailDrawer({
  template, onClose, onInstall,
}: {
  template: WorkflowTemplate;
  onClose: () => void;
  onInstall: (id: string) => void;
}) {
  const installBtn = () => {
    switch (template.installStatus) {
      case 'installed':
        return (
          <button disabled className="pd-btn px-4 py-1.5 rounded text-xs"
            style={{ backgroundColor: '#10B981', color: '#fff', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
            <CheckCircle size={12} /> 已安装
          </button>
        );
      case 'update-available':
        return (
          <button onClick={() => onInstall(template.id)} className="pd-btn px-4 py-1.5 rounded text-xs"
            style={{ backgroundColor: '#F59E0B', color: '#fff', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
            <Download size={12} /> 更新到 v{template.version}
          </button>
        );
      default:
        return (
          <button onClick={() => onInstall(template.id)} className="pd-btn px-4 py-1.5 rounded text-xs"
            style={{ backgroundColor: 'var(--accent)', color: '#fff', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
            <Download size={12} /> 使用此模板
          </button>
        );
    }
  };

  return createPortal((
    <div
      className="fixed inset-0 flex"
      style={{ backgroundColor: 'rgba(0,0,0,0.35)', zIndex: 99999 }}
      onClick={onClose}
    >
      <div className="flex-1" />
      <div
        className="h-full flex flex-col"
        style={{
          width: 640,
          backgroundColor: 'var(--bg-primary)',
          boxShadow: 'var(--shadow-lg)',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="shrink-0 px-5 py-3 flex items-start justify-between gap-3"
          style={{
            borderBottom: '1px solid var(--border)',
            backgroundImage: `linear-gradient(135deg, ${template.accentColor}22, transparent 70%)`,
          }}
        >
          <div className="flex items-start gap-3 min-w-0 flex-1">
            <div className="shrink-0 rounded-lg flex items-center justify-center"
              style={{ width: 52, height: 52, backgroundColor: `${template.accentColor}22`, fontSize: 28 }}>
              {template.icon}
            </div>
            <div className="min-w-0">
              <div className="flex items-center gap-1.5">
                <h3 className="text-base font-medium" style={{ color: 'var(--text-primary)' }}>{template.name}</h3>
                {template.verified && (
                  <span title="官方模板" className="text-[9px] font-medium" style={{
                    color: 'var(--accent)',
                    padding: '0 5px',
                    borderRadius: 999,
                    backgroundColor: 'var(--accent-bg)',
                    border: '0.5px solid var(--accent)',
                    lineHeight: '16px',
                  }}>官方</span>
                )}
              </div>
              <div className="flex items-center gap-2 mt-1 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                <span>v{template.version}</span>
                <span>·</span>
                <span className="flex items-center gap-1"><User size={10} /> {template.author}</span>
                <span>·</span>
                <span className="flex items-center gap-1"><Clock size={10} /> 更新于 {relativeDay(template.updatedAt)}</span>
              </div>
              <div className="flex items-center gap-2 mt-2 flex-wrap">
                <span className="text-[10px] px-2 py-0.5 rounded-full"
                  style={{
                    color: DIFFICULTY_OPTIONS.find(d => d.key === template.difficulty)?.color,
                    backgroundColor: `${DIFFICULTY_OPTIONS.find(d => d.key === template.difficulty)?.color}18`,
                  }}>
                  {template.difficulty}
                </span>
                <span className="text-[10px] px-2 py-0.5 rounded-full"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
                  {template.triggerType}
                </span>
                {template.tags.map(tag => (
                  <span key={tag} className="text-[10px] px-2 py-0.5 rounded-full"
                    style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
                    {tag}
                  </span>
                ))}
              </div>
            </div>
          </div>
          <button onClick={onClose} className="pd-btn p-1 rounded shrink-0"
            style={{ color: 'var(--text-secondary)' }}>
            <X size={16} />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-5">
          {/* 概览 Stats */}
          <div className="grid grid-cols-4 gap-2">
            {[
              { label: '下载量', value: formatK(template.downloads), icon: <Download size={11} /> },
              { label: '收藏数', value: template.likes, icon: <Heart size={11} /> },
              { label: '节点数', value: template.nodeCount, icon: <LayoutGrid size={11} /> },
              { label: '配置耗时', value: `≈ ${template.estimateMinutes} 分钟`, icon: <Clock size={11} /> },
            ].map(it => (
              <div key={it.label} className="rounded-lg px-3 py-2"
                style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
                <div className="flex items-center gap-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                  {it.icon} {it.label}
                </div>
                <div className="text-sm mt-0.5" style={{ color: 'var(--text-primary)' }}>{it.value}</div>
              </div>
            ))}
          </div>

          {/* 描述 */}
          <div>
            <h4 className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>模板简介</h4>
            <p className="text-xs leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
              {template.description}
            </p>
          </div>

          {/* 亮点 */}
          <div>
            <h4 className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>核心亮点</h4>
            <ul className="space-y-1.5">
              {template.highlights.map((h, i) => (
                <li key={i} className="flex items-start gap-2 text-xs" style={{ color: 'var(--text-secondary)' }}>
                  <CheckCircle size={12} style={{ color: '#10B981', marginTop: 1, flexShrink: 0 }} />
                  {h}
                </li>
              ))}
            </ul>
          </div>

          {/* 流程图预览 */}
          <div>
            <h4 className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>流程预览</h4>
            <div className="rounded-lg p-3 overflow-x-auto"
              style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
              <svg width="100%" viewBox={`0 -4 600 120`} style={{ minHeight: 110 }}>
                <WorkflowMiniPreview nodes={template.previewNodes.map(n => ({ ...n, column: n.column, row: n.row }))}
                  accent={template.accentColor} />
                {/* 这里实际渲染的是独立 SVG，WorkflowMiniPreview 内部自己生成根 svg，简化起见这里只放一个说明 */}
              </svg>
              {/* 注意：上方 svg 标签实际是为了占位不影响，MiniPreview 内部会再渲染一层 svg（嵌套无害） */}
              <div className="mt-2">
                <WorkflowMiniPreview nodes={template.previewNodes} accent={template.accentColor} />
              </div>
            </div>
          </div>

          {/* 版本要求 */}
          <div>
            <h4 className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>安装要求</h4>
            <div className="text-xs space-y-1" style={{ color: 'var(--text-secondary)' }}>
              <div>最低 PilotDesk 版本：<span className="font-mono">{template.minAppVersion}+</span></div>
              <div>兼容运行环境：Windows / macOS / Linux</div>
            </div>
          </div>
        </div>

        {/* Footer Actions */}
        <div className="shrink-0 px-5 py-3 flex items-center justify-between gap-2"
          style={{ borderTop: '1px solid var(--border)' }}>
          <div className="flex items-center gap-2">
            <button className="pd-btn px-3 py-1.5 rounded text-xs"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', display: 'inline-flex', alignItems: 'center', gap: 4 }}>
              <PackageOpen size={12} /> 导入 JSON
            </button>
          </div>
          <div className="flex items-center gap-2">
            {installBtn()}
          </div>
        </div>
      </div>
    </div>
  ), document.body);
}

// ── 主组件 ──
export const WorkflowTemplateMarket: React.FC<{
  onBack?: () => void;
  onUseTemplate?: (tplId: string) => void;
}> = ({ onBack, onUseTemplate }) => {
  const [topTab, setTopTab] = useState<'featured' | 'browse' | 'favorites'>('featured');
  const [viewMode, setViewMode] = useState<'grid' | 'list'>('grid');
  const [searchQuery, setSearchQuery] = useState('');
  const [sort, setSort] = useState<typeof SORT_OPTIONS[number]['key']>('recommended');
  const [activeCategoryId, setActiveCategoryId] = useState<string>('all');
  const [activeSubCategoryId, setActiveSubCategoryId] = useState<string | null>(null);
  const [triggerFilter, setTriggerFilter] = useState<typeof TRIGGER_OPTIONS[number]['key']>('all');
  const [difficultyFilter, setDifficultyFilter] = useState<typeof DIFFICULTY_OPTIONS[number]['key']>('all');
  const [favorites, setFavorites] = useState<Set<string>>(new Set());
  const [templates, setTemplates] = useState<WorkflowTemplate[]>(MOCK_TEMPLATES);
  const [detailTpl, setDetailTpl] = useState<WorkflowTemplate | null>(null);
  const [showFilters, setShowFilters] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  // 模拟刷新
  const handleRefresh = () => {
    setRefreshing(true);
    setTimeout(() => setRefreshing(false), 700);
  };

  const toggleFav = (id: string) => {
    setFavorites(prev => {
      const s = new Set(prev);
      if (s.has(id)) s.delete(id); else s.add(id);
      return s;
    });
  };

  const installTpl = (id: string) => {
    setTemplates(prev => prev.map(t => t.id === id ? { ...t, installStatus: 'installed' as InstallStatus } : t));
    // 如果当前打开的是这个模板，同步更新抽屉
    if (detailTpl?.id === id) setDetailTpl({ ...detailTpl, installStatus: 'installed' });
    onUseTemplate?.(id);
  };

  const filtered = useMemo(() => {
    let list = templates.slice();

    // 顶部 Tab
    if (topTab === 'favorites') {
      list = list.filter(t => favorites.has(t.id));
    } else if (topTab === 'featured') {
      // 精选 = 官方 + 下载量高（前 4）
      const verified = list.filter(t => t.verified || t.downloads > 500);
      verified.sort((a, b) => b.downloads - a.downloads);
      list = verified;
    }

    // 分类
    if (activeCategoryId !== 'all') {
      list = list.filter(t => t.category === activeCategoryId);
    }

    // 触发方式
    if (triggerFilter !== 'all') {
      list = list.filter(t => t.triggerType === triggerFilter);
    }
    // 难度
    if (difficultyFilter !== 'all') {
      list = list.filter(t => t.difficulty === difficultyFilter);
    }
    // 搜索
    const q = searchQuery.trim().toLowerCase();
    if (q) {
      list = list.filter(t => {
        const h = [t.name, t.description, t.author, ...t.tags].join(' ').toLowerCase();
        return h.includes(q);
      });
    }
    // 排序
    switch (sort) {
      case 'downloads': list.sort((a, b) => b.downloads - a.downloads); break;
      case 'likes': list.sort((a, b) => b.likes - a.likes); break;
      case 'updated': list.sort((a, b) => +new Date(b.updatedAt) - +new Date(a.updatedAt)); break;
      case 'name': list.sort((a, b) => a.name.localeCompare(b.name, 'zh-CN')); break;
      case 'recommended':
      default:
        list.sort((a, b) => (b.verified ? 1 : 0) - (a.verified ? 1 : 0) || b.downloads - a.downloads);
    }
    return list;
  }, [templates, topTab, favorites, activeCategoryId, triggerFilter, difficultyFilter, searchQuery, sort]);

  // ── 渲染 ──
  const categoryBadgeCount = (activeCategoryId !== 'all' ? 1 : 0)
    + (triggerFilter !== 'all' ? 1 : 0)
    + (difficultyFilter !== 'all' ? 1 : 0);

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* ── 顶栏（一行紧凑布局）
           左：精选Tab + 浏览全部Tab + 我的收藏Tab + 搜索框
           右：排序 → 显示方式 → 筛选 → 刷新
         ─────────────────────────────────────────────────────── */}
      <div className="shrink-0 pl-0 pr-0 py-1.5 flex items-center gap-2"
        style={{ borderBottom: '1px solid var(--border)' }}>
        {/* Tab 组：精选 / 浏览全部 / 我的收藏（原标题位置） */}
        <div className="flex items-center rounded-lg shrink-0" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          {[
            { key: 'featured', label: '精选', icon: <Sparkles size={11} /> },
            { key: 'browse',   label: '浏览全部', icon: <FolderOpen size={11} /> },
            { key: 'favorites', label: '我的收藏', icon: <Heart size={11} /> },
          ].map((tab, i) => (
            <button
              key={tab.key}
              onClick={() => setTopTab(tab.key as typeof topTab)}
              className="pd-btn px-2 py-1 rounded-md text-[11px]"
              style={{
                backgroundColor: topTab === tab.key ? 'var(--accent-light)' : 'transparent',
                color: topTab === tab.key ? 'var(--accent)' : 'var(--text-tertiary)',
                fontWeight: topTab === tab.key ? 500 : 400,
                display: 'inline-flex', alignItems: 'center', gap: 4,
                border: topTab === tab.key ? '1px solid var(--accent)' : '1px solid transparent',
              }}
            >
              {tab.icon}
              {tab.label}
              {tab.key === 'favorites' && favorites.size > 0 && (
                <span className="ml-1 text-[9px] px-1.5 rounded-full"
                  style={{ backgroundColor: 'var(--accent-light)', color: 'var(--accent)' }}>
                  {favorites.size}
                </span>
              )}
            </button>
          ))}
        </div>

        {/* 搜索框（Tab 之后） */}
        <div className="relative flex-1 min-w-0 max-w-md">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索模板名称 / 标签 / 作者 / 描述..."
            className="w-full pl-8 pr-3 py-1 rounded-md text-[11px] outline-none"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
              height: 24,
            }}
          />
        </div>

        {/* 右侧工具组 */}
        <div className="ml-auto flex items-center gap-1.5 shrink-0">
          {/* 排序（放在显示方式前面） */}
          <div className="flex items-center gap-1.5">
            <span className="text-[11px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>排序</span>
            <select
              value={sort}
              onChange={(e) => setSort(e.target.value as typeof sort)}
              className="text-[11px] rounded px-2 py-0.5"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                border: '1px solid var(--border)',
                color: 'var(--text-secondary)',
                height: 24,
              }}
            >
              {SORT_OPTIONS.map(o => <option key={o.key} value={o.key}>{o.label}</option>)}
            </select>
          </div>

          {/* 视图模式切换 */}
          <div className="flex rounded p-0.5" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
            <button
              onClick={() => setViewMode('grid')}
              className="pd-btn p-0.5 rounded"
              style={{
                backgroundColor: viewMode === 'grid' ? 'var(--bg-primary)' : 'transparent',
                color: viewMode === 'grid' ? 'var(--text-primary)' : 'var(--text-tertiary)',
                width: 22, height: 20, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              }}
              title="网格视图">
              <Grid3X3 size={13} />
            </button>
            <button
              onClick={() => setViewMode('list')}
              className="pd-btn p-0.5 rounded"
              style={{
                backgroundColor: viewMode === 'list' ? 'var(--bg-primary)' : 'transparent',
                color: viewMode === 'list' ? 'var(--text-primary)' : 'var(--text-tertiary)',
                width: 22, height: 20, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              }}
              title="列表视图">
              <List size={13} />
            </button>
          </div>

          <button onClick={() => setShowFilters(f => !f)}
            className="pd-btn px-2 py-0.5 rounded text-[11px] relative"
            style={{
              backgroundColor: showFilters || categoryBadgeCount > 0 ? 'var(--accent-light)' : 'var(--bg-tertiary)',
              color: showFilters || categoryBadgeCount > 0 ? 'var(--accent)' : 'var(--text-secondary)',
              display: 'inline-flex', alignItems: 'center', gap: 4,
              height: 24,
            }}>
            <Filter size={12} />
            筛选
            {categoryBadgeCount > 0 && (
              <span className="absolute -top-1 -right-1 text-[9px] w-4 h-4 rounded-full flex items-center justify-center"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}>
                {categoryBadgeCount}
              </span>
            )}
          </button>
          <button onClick={handleRefresh}
            className="pd-btn p-1 rounded"
            style={{ color: 'var(--text-secondary)', width: 24, height: 24, display: 'inline-flex', alignItems: 'center', justifyContent: 'center' }}
            title="刷新列表">
            <RefreshCw size={13} className={refreshing ? 'pd-animate-spin' : ''} />
          </button>
        </div>
      </div>

      {/* ── 筛选抽屉（可选展开）：标题+按钮+清除按钮全部同一行 ── */}
      {showFilters && (
        <div className="shrink-0 pl-0 pr-0 py-1.5" style={{ borderBottom: '1px solid var(--border)' }}>
          <div className="rounded-lg px-2.5 py-1.5 flex flex-wrap items-center gap-x-3 gap-y-1.5"
            style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
            {/* 触发方式：标题 + 按钮组 同一行 */}
            <div className="flex items-center gap-2 flex-wrap">
              <label className="text-[11px] shrink-0" style={{ color: 'var(--text-secondary)' }}>触发方式</label>
              <div className="flex flex-wrap gap-1.5">
                {TRIGGER_OPTIONS.map(opt => (
                  <button key={opt.key}
                    onClick={() => setTriggerFilter(opt.key)}
                    className="pd-btn px-2.5 py-0.5 rounded text-[11px]"
                    style={{
                      backgroundColor: triggerFilter === opt.key ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                      color: triggerFilter === opt.key ? 'var(--accent)' : 'var(--text-secondary)',
                      border: `1px solid ${triggerFilter === opt.key ? 'var(--accent)' : 'var(--border)'}`,
                    }}>
                    {opt.label}
                  </button>
                ))}
              </div>
            </div>

            {/* 难度等级：标题 + 按钮组 同一行 */}
            <div className="flex items-center gap-2 flex-wrap">
              <label className="text-[11px] shrink-0" style={{ color: 'var(--text-secondary)' }}>难度等级</label>
              <div className="flex flex-wrap gap-1.5">
                {DIFFICULTY_OPTIONS.map(opt => (
                  <button key={opt.key}
                    onClick={() => setDifficultyFilter(opt.key)}
                    className="pd-btn px-2.5 py-0.5 rounded text-[11px]"
                    style={{
                      backgroundColor: difficultyFilter === opt.key ? `${opt.color}18` : 'var(--bg-tertiary)',
                      color: difficultyFilter === opt.key ? opt.color : 'var(--text-secondary)',
                      border: `1px solid ${difficultyFilter === opt.key ? opt.color : 'var(--border)'}`,
                    }}>
                    {opt.label}
                  </button>
                ))}
              </div>
            </div>

            {/* 清除全部筛选：与筛选按钮同一行 */}
            {categoryBadgeCount > 0 && (
              <div className="ml-auto shrink-0">
                <button
                  onClick={() => { setTriggerFilter('all'); setDifficultyFilter('all'); setActiveCategoryId('all'); setActiveSubCategoryId(null); }}
                  className="pd-btn text-[11px] px-2 py-0.5 rounded"
                  style={{ color: 'var(--accent)', height: 22 }}>
                  清除全部筛选
                </button>
              </div>
            )}
          </div>
        </div>
      )}

      {/* ── 主体：左侧分类 + 右侧网格/列表 ── */}
      <div className="flex-1 flex min-h-0">
        {/* 左：分类树 */}
        <div className="w-56 shrink-0 h-full overflow-y-auto"
          style={{ borderRight: '1px solid var(--border)', backgroundColor: 'var(--bg-secondary)' }}>
          <div className="px-3 py-3 space-y-0.5">
            <div className="text-[10px] mb-1.5 mt-2" style={{ color: 'var(--text-tertiary)' }}>
              <span className="flex items-center gap-1"><SlidersHorizontal size={10} /> 分类导航</span>
            </div>
            {CATEGORIES.map(cat => {
              const active = activeCategoryId === cat.id;
              return (
                <div key={cat.id}>
                  <button
                    onClick={() => { setActiveCategoryId(cat.id); setActiveSubCategoryId(null); }}
                    className="pd-btn w-full px-2 py-1.5 rounded flex items-center justify-between gap-2 text-[11px]"
                    style={{
                      backgroundColor: active ? 'var(--accent-light)' : 'transparent',
                      color: active ? 'var(--accent)' : 'var(--text-secondary)',
                      fontWeight: active ? 500 : 400,
                    }}>
                    <span className="flex items-center gap-1.5 min-w-0">
                      <span className="shrink-0">{cat.icon}</span>
                      <span className="truncate">{cat.name}</span>
                    </span>
                    <span className="text-[9px] shrink-0"
                      style={{ color: active ? 'var(--accent)' : 'var(--text-tertiary)' }}>{cat.count}</span>
                  </button>
                  {cat.sub && active && (
                    <div className="ml-6 mt-0.5 space-y-0.5">
                      {cat.sub.map(sub => (
                        <button key={sub.id}
                          onClick={() => setActiveSubCategoryId(activeSubCategoryId === sub.id ? null : sub.id)}
                          className="pd-btn w-full px-2 py-1 rounded flex items-center justify-between gap-2 text-[10px]"
                          style={{
                            color: activeSubCategoryId === sub.id ? 'var(--accent)' : 'var(--text-tertiary)',
                            backgroundColor: activeSubCategoryId === sub.id ? 'var(--bg-tertiary)' : 'transparent',
                          }}>
                          <span className="flex items-center gap-1 truncate">
                            <ChevronRight size={8} />{sub.name}
                          </span>
                          <span className="shrink-0">{sub.count}</span>
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>

          <div className="px-3 py-2 mt-2" style={{ borderTop: '1px solid var(--border)' }}>
            <div className="text-[10px] mb-2" style={{ color: 'var(--text-tertiary)' }}>
              <span className="flex items-center gap-1"><Zap size={10} /> 快捷筛选</span>
            </div>
            <div className="space-y-1">
              {[
                { label: '⚡ 新手友好', onClick: () => { setDifficultyFilter('入门'); setShowFilters(true); } },
                { label: '🔥 本周热门', onClick: () => { setSort('downloads'); } },
                { label: '🛡️ 官方精选', onClick: () => { setTopTab('featured'); } },
                { label: '📝 手动执行', onClick: () => { setTriggerFilter('手动'); setShowFilters(true); } },
                { label: '⏰ 定时任务', onClick: () => { setTriggerFilter('定时'); setShowFilters(true); } },
              ].map(q => (
                <button key={q.label} onClick={q.onClick}
                  className="pd-btn w-full text-left px-2 py-1 rounded text-[11px]"
                  style={{ color: 'var(--text-secondary)' }}>
                  {q.label}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* 右：结果区 */}
        <div className="flex-1 min-w-0 h-full flex flex-col">
          {/* 结果条 */}
          <div className="shrink-0 px-4 py-2 flex items-center justify-between"
            style={{ borderBottom: '1px solid var(--border-light)' }}>
            <div className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
              共找到 <span style={{ color: 'var(--text-primary)', fontWeight: 500 }}>{filtered.length}</span> 个模板
              {activeCategoryId !== 'all' && (
                <> · 在分类 <span style={{ color: 'var(--accent)' }}>
                  {CATEGORIES.find(c => c.id === activeCategoryId)?.name || activeCategoryId}
                </span> 下</>
              )}
            </div>
          </div>

          {/* 列表区 */}
          <div className="flex-1 overflow-y-auto px-4 py-3">
            {filtered.length === 0 ? (
              <div className="h-full flex flex-col items-center justify-center gap-2 py-12">
                <div style={{ fontSize: 40 }}>🗂️</div>
                <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                  {searchQuery ? `没有找到“${searchQuery}”相关模板` : '该分类下暂无模板'}
                </div>
                <button
                  onClick={() => { setSearchQuery(''); setActiveCategoryId('all'); setTriggerFilter('all'); setDifficultyFilter('all'); }}
                  className="pd-btn mt-2 px-3 py-1.5 rounded text-[11px]"
                  style={{ color: 'var(--accent)' }}>
                  重置筛选条件
                </button>
              </div>
            ) : viewMode === 'grid' ? (
              <div className="grid gap-3"
                style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(280px, 1fr))' }}>
                {filtered.map(t => (
                  <TemplateCardGrid
                    key={t.id}
                    t={t}
                    onToggleLike={toggleFav}
                    onInstall={installTpl}
                    onOpenDetail={setDetailTpl}
                  />
                ))}
              </div>
            ) : (
              <div className="space-y-2">
                {filtered.map(t => (
                  <TemplateCardList
                    key={t.id}
                    t={t}
                    onToggleLike={toggleFav}
                    onInstall={installTpl}
                    onOpenDetail={setDetailTpl}
                  />
                ))}
              </div>
            )}
          </div>
        </div>
      </div>

      {detailTpl && (
        <TemplateDetailDrawer
          template={detailTpl}
          onClose={() => setDetailTpl(null)}
          onInstall={installTpl}
        />
      )}
    </div>
  );
};

export default WorkflowTemplateMarket;
