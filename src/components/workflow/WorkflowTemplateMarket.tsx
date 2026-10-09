/**
 * WorkflowTemplateMarket — 工作流模板市场（在线市场）
 *
 * 数据全部来自远端索引 `server/market/workflow/workflow-index.json`
 * （由 server/scripts/generate-workflow-index.mjs 生成；分类/标签/作者等人工字段在 catalog.json）。
 * 页面只拉一次索引，**详情弹窗零请求** —— 浏览/筛选/详情要用的字段索引里已带全。
 *
 * 设计要点：
 * - 视觉风格沿用 PilotDesk 设计 Token（globals.css 的 --bg-* / --text-* / --border / --accent）
 * - 三个层级：
 *     顶部 Tab：精选 / 浏览全部 / 我的收藏（默认浏览全部）
 *     左栏：分类列表（单层） + 计数
 *     主区：卡片网格（可切换 列表/网格 模式）
 * - 筛选只有三个维度：分类 / 触发方式 / 安装前置；排序：推荐（精选优先）/ 名称 / 节点数
 * - 「已安装 / 可更新」来自本地安装记录（Rust 侧 app_settings 的 workflow_market_installs），
 *   与索引里的版本号比对得出；安装动作本身也走 Rust 命令（下载 + 导入 + 写记录）
 * - 收藏是纯本地偏好（localStorage），只用来支撑「我的收藏」Tab
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import {
  Search, RefreshCw, Download, Grid3X3, List, SlidersHorizontal,
  Filter, X, Heart, FolderOpen, Sparkles, User, Eye,
  Zap, CheckCircle, LayoutGrid,
} from 'lucide-react';
import { getNodeTypeMeta } from '../../workflow/WorkflowDefinition';
import { Select } from '../common/Select';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { buildImportMemberMessage } from '../../utils/workflowImport';
import type { ImportMemberOutcome } from '../../utils/workflowImport';

// ── 数据类型 ──

/** 远端索引条目（字段说明见 generate-workflow-index.mjs 文件头） */
interface WorkflowTemplate {
  /** 模板 id = 模板目录名（详见索引生成脚本，全局唯一且稳定） */
  id: string;
  name: string;
  version: string;
  description: string;
  /** 已 URL 编码的主文件路径（相对 server/market），安装命令用 */
  path: string;
  dir: string;
  mainFile: string;
  subFiles: string[];
  stageCount: number;
  nodeCount: number;
  /** 触发方式原始值：manual / cron / event（'' = 未设置，展示与筛选都按手动处理） */
  triggerType: string;
  /** 节点类型列表（去重，保持首现顺序），详情里渲染成中文胶囊 */
  nodeTypes: string[];
  /** 精选：只有市场侧 feature 清单里的模板才带 */
  featured?: boolean;
  category: string;
  tags: string[];
  author: string;
  minAppVersion: string;
  requirements: string[];
}

/** 本地安装记录（app_settings › workflow_market_installs 的 map 值） */
interface InstalledRecord {
  version: string;
  mainId: string;
  subIds: string[];
  /** 安装时间（epoch 毫秒，Rust 侧 utils::now() 写入） */
  installedAt: number;
}

type InstallState = 'not-installed' | 'installed' | 'update-available';

// ── 分类 / 触发方式 ──

/**
 * 分类元数据。生成器侧白名单只有 automation/agent/data/devops/creative，
 * 非法值已归入 other，所以这里只需补 other 兜底，不必再容错别的值。
 */
const CATEGORY_META: Record<string, { name: string; icon: string; color: string }> = {
  automation: { name: '自动化办公', icon: '⚙️', color: '#3B82F6' },
  agent:      { name: 'Agent 协作', icon: '🤖', color: '#8B5CF6' },
  data:       { name: '数据处理',   icon: '📊', color: '#10B981' },
  devops:     { name: '研发效能',   icon: '🛠️', color: '#F59E0B' },
  creative:   { name: '内容创作',   icon: '🎨', color: '#EC4899' },
  other:      { name: '其他',       icon: '📦', color: '#6B7280' },
};

const categoryMeta = (id: string) => CATEGORY_META[id] || CATEGORY_META.other;

/** 分类展示顺序（左栏列表按它排） */
const CATEGORY_ORDER = ['automation', 'agent', 'data', 'devops', 'creative', 'other'];

/** 触发方式中文映射：'' = 编辑器里未设置，按手动处理 */
const TRIGGER_LABEL: Record<string, string> = { manual: '手动', cron: '定时', event: '事件' };
const triggerLabel = (v: string) => TRIGGER_LABEL[v] || TRIGGER_LABEL.manual;
/** 归一化后的触发值（筛选、比较都用它，保证 '' 与 manual 等价） */
const normalizedTrigger = (v: string) => (TRIGGER_LABEL[v] ? v : 'manual');

const TRIGGER_OPTIONS: Array<{ key: string; label: string }> = [
  { key: 'all', label: '全部' },
  { key: 'manual', label: '手动触发' },
  { key: 'cron', label: '定时运行' },
  { key: 'event', label: '事件驱动' },
];

const REQUIREMENT_OPTIONS: Array<{ key: string; label: string }> = [
  { key: 'all', label: '全部' },
  { key: 'required', label: '有安装前置' },
  { key: 'none', label: '无需前置' },
];

const SORT_OPTIONS = [
  { key: 'recommended', label: '推荐排序' },
  { key: 'name',        label: '名称 A-Z' },
  { key: 'nodes',       label: '节点数最多' },
] as const;

// ── 收藏（localStorage） ──

const FAVORITES_KEY = 'pilotdesk-workflow-market-favorites';

/** 收藏是纯本地偏好：读不到不影响浏览，返回空集合即可 */
function loadFavorites(): Set<string> {
  try {
    const raw = localStorage.getItem(FAVORITES_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(arr) ? arr.filter((x): x is string => typeof x === 'string') : []);
  } catch {
    return new Set();
  }
}

// ── 索引条目归一化 ──

/**
 * 远端 JSON → 组件模型。
 *
 * 索引是外部数据，字段缺失时给稳定默认值而不是让页面崩掉；连 id/mainFile 都没有的条目
 * （装也装不了）直接丢弃。生成器已保证形状，这里的容错只是防御远端被改坏。
 */
function normalizeTemplate(raw: Record<string, unknown> | null | undefined): WorkflowTemplate | null {
  if (!raw) return null;
  const id = typeof raw.id === 'string' ? raw.id : '';
  const mainFile = typeof raw.mainFile === 'string' ? raw.mainFile : '';
  if (!id || !mainFile) return null;
  const strArr = (v: unknown): string[] =>
    Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
  return {
    id,
    name: typeof raw.name === 'string' && raw.name ? raw.name : id,
    version: typeof raw.version === 'string' ? raw.version : '',
    description: typeof raw.description === 'string' ? raw.description : '',
    path: typeof raw.path === 'string' ? raw.path : '',
    dir: typeof raw.dir === 'string' ? raw.dir : id,
    mainFile,
    subFiles: strArr(raw.subFiles),
    stageCount: typeof raw.stageCount === 'number' && Number.isFinite(raw.stageCount) ? raw.stageCount : 0,
    nodeCount: typeof raw.nodeCount === 'number' && Number.isFinite(raw.nodeCount) ? raw.nodeCount : 0,
    triggerType: typeof raw.triggerType === 'string' ? raw.triggerType : '',
    nodeTypes: strArr(raw.nodeTypes),
    ...(raw.featured === true ? { featured: true as const } : {}),
    category: typeof raw.category === 'string' && raw.category ? raw.category : 'other',
    tags: strArr(raw.tags),
    author: typeof raw.author === 'string' ? raw.author : '',
    minAppVersion: typeof raw.minAppVersion === 'string' ? raw.minAppVersion : '',
    requirements: strArr(raw.requirements),
  };
}

// ── 安装按钮（卡片 / 列表 / 详情三处共用） ──

function InstallButton({
  state, installing, size = 'sm', onClick,
}: {
  state: InstallState;
  installing: boolean;
  size?: 'sm' | 'md';
  onClick: (e: React.MouseEvent) => void;
}) {
  const pad = size === 'md' ? 'px-4 py-1.5 text-xs' : 'px-2 py-1 text-[10px]';
  const gap = size === 'md' ? 6 : 3;
  if (installing) {
    return (
      <button disabled className={`pd-btn ${pad} rounded`}
        style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
        安装中…
      </button>
    );
  }
  if (state === 'installed') {
    return (
      <span className="flex items-center gap-1" style={{ color: '#10B981', fontSize: size === 'md' ? 12 : 10 }}>
        <CheckCircle size={size === 'md' ? 12 : 10} /> 已安装
      </span>
    );
  }
  const isUpdate = state === 'update-available';
  return (
    <button
      onClick={onClick}
      className={`pd-btn ${pad} rounded`}
      style={{
        backgroundColor: isUpdate ? '#F59E0B' : 'var(--accent)',
        color: '#fff',
        display: 'inline-flex', alignItems: 'center', gap,
      }}
    >
      <Download size={size === 'md' ? 12 : 10} /> {isUpdate ? '更新' : '安装'}
    </button>
  );
}

// ── 卡片：网格模式 ──

function TemplateCardGrid({
  t, installState, installing, liked, onToggleFav, onInstall, onOpenDetail,
}: {
  t: WorkflowTemplate;
  installState: InstallState;
  installing: boolean;
  liked: boolean;
  onToggleFav: (id: string) => void;
  onInstall: (t: WorkflowTemplate) => void;
  onOpenDetail: (t: WorkflowTemplate) => void;
}) {
  const cat = categoryMeta(t.category);

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
      {/* 顶部：分类色带 + 分类图标 + 名称版本 + 收藏 */}
      <div className="relative px-3 pt-3 pb-2"
        style={{ backgroundImage: `linear-gradient(135deg, ${cat.color}22, transparent 70%)` }}
      >
        <div className="flex items-start justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <div className="flex items-center justify-center shrink-0"
              style={{ width: 28, height: 28, borderRadius: 6, backgroundColor: `${cat.color}18`, fontSize: 16 }}>
              {cat.icon}
            </div>
            <div className="min-w-0">
              <div className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }} title={t.name}>
                {t.name}
              </div>
              <div className="flex items-center gap-1.5 mt-0.5">
                <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>v{t.version}</span>
                <span className="text-[9px] px-1.5 rounded-full"
                  style={{ color: cat.color, backgroundColor: `${cat.color}18` }}>
                  {cat.name}
                </span>
              </div>
            </div>
          </div>
          <button
            onClick={(e) => { e.stopPropagation(); onToggleFav(t.id); }}
            className="pd-btn p-1 rounded shrink-0"
            style={{ color: liked ? '#EF4444' : 'var(--text-tertiary)' }}
            title={liked ? '已收藏' : '收藏'}
          >
            <Heart size={12} fill={liked ? '#EF4444' : 'none'} />
          </button>
        </div>
      </div>

      {/* 描述（两行） */}
      <div className="px-3 pb-2">
        <p className="text-[10px] line-clamp-2" style={{
          color: 'var(--text-secondary)',
          lineHeight: 1.5,
          minHeight: 30,
          margin: 0,
        }}>{t.description || '暂无简介'}</p>
      </div>

      {/* 标签（最多 3 个） */}
      <div className="px-3 pb-2 flex flex-wrap gap-1">
        {t.tags.slice(0, 3).map(tag => (
          <span key={tag} className="text-[9px] px-1.5 py-0.5 rounded"
            style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
            {tag}
          </span>
        ))}
      </div>

      {/* 底部：触发方式 + 阶段/节点数 | 安装 */}
      <div className="flex items-center justify-between gap-2 px-3 py-2"
        style={{ borderTop: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2.5 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
          <span className="flex items-center gap-0.5"><Zap size={9} /> {triggerLabel(t.triggerType)}</span>
          <span className="flex items-center gap-0.5">
            <LayoutGrid size={9} /> {t.stageCount} 阶段 · {t.nodeCount} 节点
          </span>
        </div>
        <InstallButton
          state={installState}
          installing={installing}
          onClick={(e) => { e.stopPropagation(); onInstall(t); }}
        />
      </div>
    </div>
  );
}

// ── 卡片：列表模式 ──

function TemplateCardList({
  t, installState, installing, liked, onToggleFav, onInstall, onOpenDetail,
}: {
  t: WorkflowTemplate;
  installState: InstallState;
  installing: boolean;
  liked: boolean;
  onToggleFav: (id: string) => void;
  onInstall: (t: WorkflowTemplate) => void;
  onOpenDetail: (t: WorkflowTemplate) => void;
}) {
  const cat = categoryMeta(t.category);

  return (
    <div className="rounded-lg overflow-hidden"
      style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)', cursor: 'pointer' }}
      onClick={() => onOpenDetail(t)}
      onMouseEnter={(e) => { e.currentTarget.style.borderColor = 'var(--accent)'; }}
      onMouseLeave={(e) => { e.currentTarget.style.borderColor = 'var(--border)'; }}
    >
      <div className="flex items-center gap-3 px-3 py-2">
        {/* 分类图标 */}
        <div className="shrink-0 flex items-center justify-center rounded-lg"
          style={{ width: 48, height: 48, backgroundColor: `${cat.color}18`, fontSize: 24 }}>
          {cat.icon}
        </div>

        {/* 信息 */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-medium truncate" style={{ color: 'var(--text-primary)' }}>{t.name}</span>
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>v{t.version}</span>
            <span className="text-[9px] px-1.5 rounded-full shrink-0"
              style={{ color: cat.color, backgroundColor: `${cat.color}18` }}>
              {cat.name}
            </span>
          </div>
          <p className="text-[11px] truncate mt-0.5" style={{ color: 'var(--text-secondary)', margin: 0 }}>
            {t.description || '暂无简介'}
          </p>
          <div className="flex items-center gap-3 mt-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            <span className="flex items-center gap-0.5"><User size={9} /> {t.author || '未署名'}</span>
            <span className="flex items-center gap-0.5"><Zap size={9} /> {triggerLabel(t.triggerType)}</span>
            <span className="flex items-center gap-0.5">
              <LayoutGrid size={9} /> {t.stageCount} 阶段 · {t.nodeCount} 节点
            </span>
            {t.tags.slice(0, 3).map(tag => (
              <span key={tag} className="px-1.5 py-0.5 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
                {tag}
              </span>
            ))}
          </div>
        </div>

        {/* 操作 */}
        <div className="shrink-0 flex items-center gap-2">
          <button onClick={(e) => { e.stopPropagation(); onToggleFav(t.id); }}
            className="pd-btn p-1 rounded" style={{ color: liked ? '#EF4444' : 'var(--text-tertiary)' }}
            title={liked ? '已收藏' : '收藏'}>
            <Heart size={12} fill={liked ? '#EF4444' : 'none'} />
          </button>
          <button onClick={(e) => { e.stopPropagation(); onOpenDetail(t); }}
            className="pd-btn p-1 rounded" style={{ color: 'var(--text-secondary)' }}
            title="查看详情">
            <Eye size={12} />
          </button>
          <InstallButton state={installState} installing={installing} onClick={(e) => { e.stopPropagation(); onInstall(t); }} />
        </div>
      </div>
    </div>
  );
}

// ── 详情弹窗（Portal 居中模态，与插件 README 弹窗同一套观感） ──

function TemplateDetailDialog({
  template, installState, installing, onClose, onInstall,
}: {
  template: WorkflowTemplate;
  installState: InstallState;
  installing: boolean;
  onClose: () => void;
  onInstall: (t: WorkflowTemplate) => void;
}) {
  const cat = categoryMeta(template.category);

  return createPortal((
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[680px] max-h-[80vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)', overflow: 'hidden' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="shrink-0 px-5 py-3 flex items-start justify-between gap-3"
          style={{
            borderBottom: '1px solid var(--border)',
            backgroundImage: `linear-gradient(135deg, ${cat.color}22, transparent 70%)`,
          }}
        >
          <div className="flex items-start gap-3 min-w-0 flex-1">
            <div className="shrink-0 rounded-lg flex items-center justify-center"
              style={{ width: 52, height: 52, backgroundColor: `${cat.color}22`, fontSize: 28 }}>
              {cat.icon}
            </div>
            <div className="min-w-0">
              <h3 className="text-base font-medium" style={{ color: 'var(--text-primary)' }}>{template.name}</h3>
              <div className="flex items-center gap-2 mt-1 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                <span>v{template.version}</span>
                <span>·</span>
                <span className="flex items-center gap-1"><User size={10} /> {template.author || '未署名'}</span>
                <span>·</span>
                <span>{template.stageCount} 阶段 / {template.nodeCount} 节点</span>
              </div>
              <div className="flex items-center gap-2 mt-2 flex-wrap">
                <span className="text-[10px] px-2 py-0.5 rounded-full"
                  style={{ color: cat.color, backgroundColor: `${cat.color}18` }}>
                  {cat.icon} {cat.name}
                </span>
                <span className="text-[10px] px-2 py-0.5 rounded-full flex items-center gap-1"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
                  <Zap size={9} /> {triggerLabel(template.triggerType)}
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

        {/* Body：全部内容来自索引，打开弹窗不发任何请求 */}
        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-5 pd-scroll-stable">
          {/* 简介 */}
          <div>
            <h4 className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>模板简介</h4>
            <p className="text-xs leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
              {template.description || '作者未填写简介'}
            </p>
          </div>

          {/* 节点组成：nodeTypes 中文胶囊（原缩略流程图已删——索引里的类型清单足够判断模板用不用得上） */}
          <div>
            <h4 className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>节点组成</h4>
            {template.nodeTypes.length === 0 ? (
              <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>暂无</span>
            ) : (
              <div className="flex flex-wrap gap-1.5">
                {template.nodeTypes.map(type => {
                  const meta = getNodeTypeMeta(type);
                  return (
                    <span key={type} className="text-[10px] px-2 py-0.5 rounded-full"
                      style={{
                        color: meta.color,
                        backgroundColor: `${meta.color}14`,
                        border: `1px solid ${meta.color}55`,
                      }}>
                      {meta.icon} {meta.label}
                    </span>
                  );
                })}
              </div>
            )}
            {template.subFiles.length > 0 && (
              <div className="text-[10px] mt-2" style={{ color: 'var(--text-tertiary)' }}>
                包含 {template.subFiles.length} 个子工作流文件，安装时一并导入
              </div>
            )}
          </div>

          {/* 安装要求 */}
          <div>
            <h4 className="text-xs mb-2" style={{ color: 'var(--text-primary)' }}>安装要求</h4>
            <div className="text-xs space-y-1" style={{ color: 'var(--text-secondary)' }}>
              <div>
                最低 PilotDesk 版本：
                {template.minAppVersion
                  ? <span className="font-mono">{template.minAppVersion}+</span>
                  : <span style={{ color: 'var(--text-tertiary)' }}>无</span>}
              </div>
              {template.requirements.length === 0 ? (
                <div>前置要求：<span style={{ color: 'var(--text-tertiary)' }}>无</span></div>
              ) : (
                <div>
                  前置要求：
                  <ul className="mt-1 space-y-1">
                    {template.requirements.map((r, i) => (
                      <li key={i} className="flex items-start gap-2">
                        <CheckCircle size={12} style={{ color: '#10B981', marginTop: 2, flexShrink: 0 }} />
                        {r}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          </div>
        </div>

        {/* Footer：只留安装动作（原「导入 JSON」占位按钮已删——安装走索引下载，不需要用户选文件） */}
        <div className="shrink-0 px-5 py-3 flex items-center justify-end gap-2"
          style={{ borderTop: '1px solid var(--border)' }}>
          <InstallButton
            state={installState}
            installing={installing}
            size="md"
            onClick={() => onInstall(template)}
          />
        </div>
      </div>
    </div>
  ), document.body);
}

// ── 主组件 ──

export const WorkflowTemplateMarket: React.FC<{
  /** 安装成功后的出口：父级只负责导航（反馈由本组件自己 toast，避免重复提示） */
  onUseTemplate?: (tplId: string) => void;
}> = ({ onUseTemplate }) => {
  /**
   * 顶部页签：精选 / 浏览全部 / 我的收藏。
   *
   * 落地态取「浏览全部」而不是「精选」：topTab 也是**筛选条件**（精选会把列表过滤成
   * featured 子集）。若默认就落在精选，"精选"这个条件一进页面就恒成立，筛选角标会显示 1
   * 而用户什么都没点，也没法取消。默认落在"不施加任何条件"的浏览全部，条件才可解释。
   */
  const [topTab, setTopTab] = useState<'featured' | 'browse' | 'favorites'>('browse');
  const [viewMode, setViewMode] = useState<'grid' | 'list'>('grid');
  const [searchQuery, setSearchQuery] = useState('');
  const [sort, setSort] = useState<typeof SORT_OPTIONS[number]['key']>('recommended');
  const [activeCategory, setActiveCategory] = useState('all');
  const [triggerFilter, setTriggerFilter] = useState('all');
  const [requirementFilter, setRequirementFilter] = useState('all');
  const [favorites, setFavorites] = useState<Set<string>>(loadFavorites);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [installs, setInstalls] = useState<Record<string, InstalledRecord>>({});
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [detailTpl, setDetailTpl] = useState<WorkflowTemplate | null>(null);
  const [showFilters, setShowFilters] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  /** 拉取远端索引 + 本地安装记录（两条命令互不依赖，并行发） */
  const loadAll = useCallback(async () => {
    try {
      const [index, records] = await Promise.all([
        invoke<{ templates?: Array<Record<string, unknown>> }>('workflow_market_index'),
        invoke<Record<string, InstalledRecord>>('workflow_market_installs'),
      ]);
      const list = Array.isArray(index?.templates)
        ? index.templates.map(normalizeTemplate).filter((t): t is WorkflowTemplate => t !== null)
        : [];
      setTemplates(list);
      setInstalls(records || {});
      setLoadError(null);
    } catch (e) {
      setLoadError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // 不在 effect 体内同步调用：loadAll 会先 setLoading 之类的状态，属于"effect 体内同步 setState"
    // （`react-hooks/set-state-in-effect` 判为级联渲染）。推到微任务 —— 同一个任务、早于绘制，行为一致。
    void Promise.resolve().then(() => loadAll());
  }, [loadAll]);

  const handleRefresh = async () => {
    setRefreshing(true);
    await loadAll();
    setRefreshing(false);
  };

  const toggleFav = (id: string) => {
    setFavorites(prev => {
      const s = new Set(prev);
      if (s.has(id)) s.delete(id); else s.add(id);
      try { localStorage.setItem(FAVORITES_KEY, JSON.stringify([...s])); } catch { /* 本地偏好写不进去不影响浏览 */ }
      return s;
    });
  };

  /** 安装状态由「本地记录 + 索引版本」派生，不另存状态位 */
  const installStateOf = (t: WorkflowTemplate): InstallState => {
    const rec = installs[t.id];
    if (!rec) return 'not-installed';
    return rec.version !== t.version ? 'update-available' : 'installed';
  };

  /**
   * 安装模板：Rust 侧完成 下载→导入→写安装记录，这里只负责反馈与刷新状态。
   *
   * 安装成功会跳到「工作流定义」（父级的 onUseTemplate 只做导航），
   * 提示文案由本组件负责，父级不再重复 toast。
   */
  const installTpl = async (t: WorkflowTemplate) => {
    if (installingId) return; // 一次只装一个：下载+导入是重动作，避免并发写同一份定义
    setInstallingId(t.id);
    try {
      const installRes = await invoke<{ importMembers?: ImportMemberOutcome[] }>(
        'workflow_market_install',
        { id: t.id },
      );
      const records = await invoke<Record<string, InstalledRecord>>('workflow_market_installs');
      setInstalls(records || {});
      // 安装即导入：逐成员给出「新建 / 覆盖 / 跳过」提示（用户对「未新建」有知情权）
      const detail = buildImportMemberMessage(installRes?.importMembers);
      const installedMsg = `已安装模板「${t.name}」，可在"工作流定义"中查看`;
      showToast(
        detail ? `${installedMsg}；${detail.message}` : installedMsg,
        detail ? detail.type : 'success',
      );
      onUseTemplate?.(t.id);
    } catch (e) {
      showToast(`安装失败：${errorMessage(e)}`, 'error');
    } finally {
      setInstallingId(null);
    }
  };

  // ── 筛选 / 排序 ──

  const filtered = useMemo(() => {
    let list = templates.slice();

    // 顶部 Tab
    if (topTab === 'favorites') {
      list = list.filter(t => favorites.has(t.id));
    } else if (topTab === 'featured') {
      list = list.filter(t => t.featured === true);
    }

    // 分类（单层）
    if (activeCategory !== 'all') {
      list = list.filter(t => t.category === activeCategory);
    }
    // 触发方式
    if (triggerFilter !== 'all') {
      list = list.filter(t => normalizedTrigger(t.triggerType) === triggerFilter);
    }
    // 安装前置（有无 requirements）
    if (requirementFilter === 'required') {
      list = list.filter(t => t.requirements.length > 0);
    } else if (requirementFilter === 'none') {
      list = list.filter(t => t.requirements.length === 0);
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
    const byName = (a: WorkflowTemplate, b: WorkflowTemplate) => a.name.localeCompare(b.name, 'zh-CN');
    switch (sort) {
      case 'name': list.sort(byName); break;
      case 'nodes': list.sort((a, b) => b.nodeCount - a.nodeCount || byName(a, b)); break;
      case 'recommended':
      default:
        list.sort((a, b) => (b.featured ? 1 : 0) - (a.featured ? 1 : 0) || byName(a, b));
    }
    return list;
  }, [templates, topTab, favorites, activeCategory, triggerFilter, requirementFilter, searchQuery, sort]);

  /** 左栏分类计数（只列有模板的分类，避免出现一排 0） */
  const sidebarCategories = useMemo(() => {
    const counts = new Map<string, number>();
    for (const t of templates) counts.set(t.category, (counts.get(t.category) || 0) + 1);
    return CATEGORY_ORDER
      .filter(id => (counts.get(id) || 0) > 0)
      .map(id => ({ id, ...categoryMeta(id), count: counts.get(id) || 0 }));
  }, [templates]);

  /**
   * 筛选角标：只统计**会限制结果集合**的条件 —— 分类、触发方式、安装前置、顶部页签。
   * 排序与显示方式不计入：它们改变顺序/排布，不改变"有几个模板"，计进去角标会撒谎。
   */
  const activeFilterCount = (activeCategory !== 'all' ? 1 : 0)
    + (triggerFilter !== 'all' ? 1 : 0)
    + (requirementFilter !== 'all' ? 1 : 0)
    + (topTab !== 'browse' ? 1 : 0);

  const resetFilters = () => {
    setActiveCategory('all');
    setTriggerFilter('all');
    setRequirementFilter('all');
    setTopTab('browse');
    setSearchQuery('');
  };

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-primary)' }}>
      {/* ── 顶栏（一行紧凑布局）
           左：精选/浏览全部/我的收藏 Tab + 搜索框
           右：排序 → 显示方式 → 筛选 → 刷新
         ─────────────────────────────────────────────────────── */}
      <div className="shrink-0 pl-0 pr-0 py-1.5 flex items-center gap-2"
        style={{ borderBottom: '1px solid var(--border)' }}>
        {/* Tab 组 */}
        <div className="flex items-center rounded-lg shrink-0" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
          {[
            { key: 'featured', label: '精选', icon: <Sparkles size={11} /> },
            { key: 'browse',   label: '浏览全部', icon: <FolderOpen size={11} /> },
            { key: 'favorites', label: '我的收藏', icon: <Heart size={11} /> },
          ].map((tab) => (
            <button
              key={tab.key}
              onClick={() => setTopTab(tab.key as typeof topTab)}
              className="pd-btn px-2 py-1 h-7 rounded-md text-[11px]"
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

        {/* 搜索框 */}
        <div className="relative flex-1 min-w-0 max-w-md">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="搜索模板名称 / 标签 / 作者 / 描述..."
            className="block w-full pl-8 pr-3 rounded-md text-[11px] outline-none"
            style={{
              height: 28, /* 对齐同行 Select(size=sm) 与 Tab 组 */
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
        </div>

        {/* 右侧工具组 */}
        <div className="ml-auto flex items-center gap-1.5 shrink-0">
          <div className="flex items-center gap-1.5">
            <span className="text-[11px] shrink-0" style={{ color: 'var(--text-tertiary)' }}>排序</span>
            <Select
              value={sort}
              onChange={(v) => setSort(v as typeof sort)}
              size="sm"
              options={SORT_OPTIONS.map(o => ({ value: o.key, label: o.label }))}
            />
          </div>

          {/* 视图模式切换 */}
          <div className="flex rounded p-0.5" style={{ backgroundColor: 'var(--bg-tertiary)' }}>
            <button
              onClick={() => setViewMode('grid')}
              className="pd-btn p-0.5 rounded"
              style={{
                backgroundColor: viewMode === 'grid' ? 'var(--bg-primary)' : 'transparent',
                color: viewMode === 'grid' ? 'var(--text-primary)' : 'var(--text-tertiary)',
                width: 24, height: 24, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
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
                width: 24, height: 24, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              }}
              title="列表视图">
              <List size={13} />
            </button>
          </div>

          <button onClick={() => setShowFilters(f => !f)}
            className="pd-btn px-2 py-0.5 rounded text-[11px] relative"
            style={{
              backgroundColor: showFilters || activeFilterCount > 0 ? 'var(--accent-light)' : 'var(--bg-tertiary)',
              color: showFilters || activeFilterCount > 0 ? 'var(--accent)' : 'var(--text-secondary)',
              display: 'inline-flex', alignItems: 'center', gap: 4,
              height: 28,
            }}>
            <Filter size={12} />
            筛选
            {activeFilterCount > 0 && (
              <span className="absolute -top-1 -right-1 text-[9px] w-4 h-4 rounded-full flex items-center justify-center"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}>
                {activeFilterCount}
              </span>
            )}
          </button>
          <button onClick={handleRefresh}
            className="pd-btn p-1 rounded"
            style={{ color: 'var(--text-secondary)', width: 28, height: 28, display: 'inline-flex', alignItems: 'center', justifyContent: 'center' }}
            title="刷新模板索引">
            <RefreshCw size={13} className={refreshing ? 'pd-animate-spin' : ''} />
          </button>
        </div>
      </div>

      {/* ── 筛选抽屉（可选展开）：触发方式 / 安装前置 ── */}
      {showFilters && (
        <div className="shrink-0 pl-0 pr-0 py-1.5" style={{ borderBottom: '1px solid var(--border)' }}>
          <div className="rounded-lg px-2.5 py-1.5 flex flex-wrap items-center gap-x-3 gap-y-1.5"
            style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}>
            {/* 触发方式 */}
            <div className="flex items-center gap-2 flex-wrap">
              <label className="text-[11px] shrink-0" style={{ color: 'var(--text-secondary)' }}>触发方式</label>
              <div className="flex flex-wrap gap-1.5">
                {TRIGGER_OPTIONS.map(opt => (
                  <button key={opt.key}
                    onClick={() => {
                      // 再点一次已选中的项 = 取消该项（回到"全部"），省掉一次"清除全部筛选"
                      setTriggerFilter(triggerFilter === opt.key ? 'all' : opt.key);
                    }}
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

            {/* 安装前置 */}
            <div className="flex items-center gap-2 flex-wrap">
              <label className="text-[11px] shrink-0" style={{ color: 'var(--text-secondary)' }}>安装前置</label>
              <div className="flex flex-wrap gap-1.5">
                {REQUIREMENT_OPTIONS.map(opt => (
                  <button key={opt.key}
                    onClick={() => {
                      setRequirementFilter(requirementFilter === opt.key ? 'all' : opt.key);
                    }}
                    className="pd-btn px-2.5 py-0.5 rounded text-[11px]"
                    style={{
                      backgroundColor: requirementFilter === opt.key ? 'var(--accent-light)' : 'var(--bg-tertiary)',
                      color: requirementFilter === opt.key ? 'var(--accent)' : 'var(--text-secondary)',
                      border: `1px solid ${requirementFilter === opt.key ? 'var(--accent)' : 'var(--border)'}`,
                    }}>
                    {opt.label}
                  </button>
                ))}
              </div>
            </div>

            {activeFilterCount > 0 && (
              <div className="ml-auto shrink-0">
                <button
                  onClick={resetFilters}
                  className="pd-btn text-[11px] px-2 py-0.5 rounded"
                  style={{ color: 'var(--accent)', height: 22 }}>
                  清除全部筛选
                </button>
              </div>
            )}
          </div>
        </div>
      )}

      {/* ── 主体：左侧分类 + 右侧网格/列表（gap-2：靠缝 + 底色差分区，不画竖线） ── */}
      <div className="flex-1 flex min-h-0 gap-2">
        {/* 左：分类列表（单层，带计数；只列有模板的分类） */}
        <div className="w-56 shrink-0 h-full overflow-y-auto pd-scroll-stable rounded-lg"
          style={{ backgroundColor: 'var(--bg-side)' }}>
          <div className="px-3 py-3 space-y-0.5">
            <div className="text-[10px] mb-1.5 mt-2" style={{ color: 'var(--text-tertiary)' }}>
              <span className="flex items-center gap-1"><SlidersHorizontal size={10} /> 分类导航</span>
            </div>

            <button
              onClick={() => setActiveCategory('all')}
              className="pd-btn w-full px-2 py-1.5 rounded flex items-center justify-between gap-2 text-[11px]"
              style={{
                backgroundColor: activeCategory === 'all' ? 'var(--accent-light)' : 'transparent',
                color: activeCategory === 'all' ? 'var(--accent)' : 'var(--text-secondary)',
                fontWeight: activeCategory === 'all' ? 500 : 400,
              }}>
              <span className="flex items-center gap-1.5 min-w-0">
                <span className="shrink-0">📦</span>
                <span className="truncate">全部模板</span>
              </span>
              <span className="text-[9px] shrink-0"
                style={{ color: activeCategory === 'all' ? 'var(--accent)' : 'var(--text-tertiary)' }}>
                {templates.length}
              </span>
            </button>

            {sidebarCategories.map(cat => {
              const active = activeCategory === cat.id;
              return (
                <button
                  key={cat.id}
                  onClick={() => setActiveCategory(active ? 'all' : cat.id)}
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
              );
            })}
          </div>
        </div>

        {/* 右：结果区 */}
        <div className="flex-1 min-w-0 h-full flex flex-col">
          <div className="shrink-0 px-4 py-2 flex items-center justify-between"
            style={{ borderBottom: '1px solid var(--border-light)' }}>
            <div className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
              共找到 <span style={{ color: 'var(--text-primary)', fontWeight: 500 }}>{filtered.length}</span> 个模板
              {activeCategory !== 'all' && (
                <> · 在分类 <span style={{ color: 'var(--accent)' }}>
                  {categoryMeta(activeCategory).name}
                </span> 下</>
              )}
            </div>
          </div>

          <div className="flex-1 overflow-y-auto px-4 py-3 pd-scroll-stable">
            {loading ? (
              <div className="h-full flex flex-col items-center justify-center gap-2 py-12">
                <RefreshCw size={18} className="pd-animate-spin" style={{ color: 'var(--text-tertiary)' }} />
                <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>正在获取模板索引…</div>
              </div>
            ) : loadError ? (
              <div className="h-full flex flex-col items-center justify-center gap-2 py-12">
                <div style={{ fontSize: 40 }}>📡</div>
                <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                  模板索引获取失败：{loadError}
                </div>
                <button
                  onClick={handleRefresh}
                  className="pd-btn mt-2 px-3 py-1.5 rounded text-[11px]"
                  style={{ color: 'var(--accent)' }}>
                  重试
                </button>
              </div>
            ) : filtered.length === 0 ? (
              <div className="h-full flex flex-col items-center justify-center gap-2 py-12">
                <div style={{ fontSize: 40 }}>🗂️</div>
                <div className="text-xs" style={{ color: 'var(--text-secondary)' }}>
                  {searchQuery
                    ? `没有找到“${searchQuery}”相关模板`
                    : topTab === 'favorites'
                      ? '还没有收藏任何模板，点卡片右上角的心形可收藏'
                      : topTab === 'featured'
                        ? '暂无精选模板'
                        : '该分类下暂无模板'}
                </div>
                <button
                  onClick={resetFilters}
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
                    installState={installStateOf(t)}
                    installing={installingId === t.id}
                    liked={favorites.has(t.id)}
                    onToggleFav={toggleFav}
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
                    installState={installStateOf(t)}
                    installing={installingId === t.id}
                    liked={favorites.has(t.id)}
                    onToggleFav={toggleFav}
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
        <TemplateDetailDialog
          template={detailTpl}
          installState={installStateOf(detailTpl)}
          installing={installingId === detailTpl.id}
          onClose={() => setDetailTpl(null)}
          onInstall={installTpl}
        />
      )}
    </div>
  );
};

export default WorkflowTemplateMarket;