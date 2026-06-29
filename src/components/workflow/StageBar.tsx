/**
 * StageBar — 阶段栏组件
 *
 * 显示工作流的阶段概览，支持折叠/展开/拖拽排序。
 * 每个阶段显示名称、节点数量、Gate 状态。
 */

import React, { useState } from 'react';
import type { Stage, GateConfig } from '../../types/workflow';

interface Props {
  stages: Stage[];
  activeStageId?: string;
  onStageClick: (stageId: string) => void;
  onStageReorder: (stages: Stage[]) => void;
  onAddStage: () => void;
  onDeleteStage: (stageId: string) => void;
  onRenameStage: (stageId: string, name: string) => void;
  onUpdateGate: (stageId: string, gate: Partial<GateConfig>) => void;
}

const GATE_STRATEGY_LABELS: Record<string, string> = {
  all: '全部完成',
  any: '任一完成',
  count: '计数',
  threshold: '阈值',
};

const MERGE_STRATEGY_LABELS: Record<string, string> = {
  merge: 'merge',
  concat: 'concat',
  pick_first: 'pick_first',
  custom: '自定义',
};

export const StageBar: React.FC<Props> = ({
  stages,
  activeStageId,
  onStageClick,
  onStageReorder,
  onAddStage,
  onDeleteStage,
  onRenameStage,
  onUpdateGate,
}) => {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [dragIndex, setDragIndex] = useState<number | null>(null);

  const toggleCollapse = (stageId: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(stageId)) next.delete(stageId);
      else next.add(stageId);
      return next;
    });
  };

  const handleDragStart = (index: number) => {
    setDragIndex(index);
  };

  const handleDragOver = (e: React.DragEvent, index: number) => {
    e.preventDefault();
    if (dragIndex === null || dragIndex === index) return;
    const newStages = [...stages];
    const [moved] = newStages.splice(dragIndex, 1);
    newStages.splice(index, 0, moved);
    onStageReorder(newStages.map((s, i) => ({ ...s, order: i })));
    setDragIndex(index);
  };

  const handleDragEnd = () => {
    setDragIndex(null);
  };

  return (
    <div style={{ display: 'flex', gap: 12, padding: '12px 16px', overflow: 'auto' }}>
      {stages.map((stage, index) => {
        const isActive = stage.id === activeStageId;
        const isCollapsed = collapsed.has(stage.id);
        const nodeCount = stage.nodes.length;
        const edgeCount = stage.edges.length;

        return (
          <div
            key={stage.id}
            draggable
            onDragStart={() => handleDragStart(index)}
            onDragOver={(e) => handleDragOver(e, index)}
            onDragEnd={handleDragEnd}
            onClick={() => onStageClick(stage.id)}
            style={{
              minWidth: 200,
              border: isActive ? '1px solid #58a6ff' : '1px solid #30363d',
              borderRadius: 8,
              background: isActive ? '#1c2128' : '#161b22',
              cursor: 'pointer',
              opacity: dragIndex === index ? 0.5 : 1,
              transition: 'all .15s',
            }}
          >
            {/* 阶段标题 */}
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                padding: '8px 12px',
                borderBottom: '1px solid #21262d',
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <span style={{
                  display: 'inline-flex', width: 20, height: 20, alignItems: 'center',
                  justifyContent: 'center', borderRadius: 4, background: '#1f6feb33',
                  color: '#58a6ff', fontSize: 11,
                }}>
                  {stage.order + 1}
                </span>
                <input
                  value={stage.name}
                  onClick={(e) => e.stopPropagation()}
                  onChange={(e) => onRenameStage(stage.id, e.target.value)}
                  style={{
                    background: 'transparent', border: 'none', color: '#f0f6fc',
                    fontSize: 13, fontWeight: 400, outline: 'none', width: 80,
                  }}
                />
              </div>
              <div style={{ display: 'flex', gap: 4 }}>
                <span style={{ fontSize: 10, color: '#8b949e' }}>{nodeCount} 节点</span>
                <button
                  onClick={(e) => { e.stopPropagation(); toggleCollapse(stage.id); }}
                  style={{ padding: '2px 6px', borderRadius: 4, border: 'none', background: 'transparent', color: '#8b949e', fontSize: 12, cursor: 'pointer' }}
                >
                  {isCollapsed ? '□' : '─'}
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); onDeleteStage(stage.id); }}
                  style={{ padding: '2px 6px', borderRadius: 4, border: 'none', background: 'transparent', color: '#f85149', fontSize: 12, cursor: 'pointer' }}
                >
                  ×
                </button>
              </div>
            </div>

            {/* Gate 信息 */}
            {!isCollapsed && (
              <div style={{ padding: '8px 12px' }}>
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 4 }}>
                  <span style={{ fontSize: 10, color: '#8b949e', display: 'flex', alignItems: 'center', gap: 4 }}>
                    ⊞ 门控
                  </span>
                  <span style={{ fontSize: 10, color: '#3fb950', display: 'flex', alignItems: 'center', gap: 3 }}>
                    <span style={{ width: 5, height: 5, borderRadius: '50%', background: '#3fb950' }} />
                    {nodeCount}/{nodeCount}
                  </span>
                </div>
                <div style={{ display: 'flex', gap: 8, fontSize: 10, color: '#8b949e' }}>
                  <span>
                    策略: <span style={{ color: '#c9d1d9', background: '#21262d', padding: '1px 4px', borderRadius: 2 }}>
                      {GATE_STRATEGY_LABELS[stage.gate.strategy] || stage.gate.strategy}
                    </span>
                  </span>
                  <span>
                    合并: <span style={{ color: '#c9d1d9', background: '#21262d', padding: '1px 4px', borderRadius: 2 }}>
                      {MERGE_STRATEGY_LABELS[stage.gate.mergeStrategy] || stage.gate.mergeStrategy}
                    </span>
                  </span>
                </div>
                {edgeCount > 0 && (
                  <div style={{ fontSize: 10, color: '#8b949e', marginTop: 4 }}>
                    {edgeCount} 条连线
                  </div>
                )}
              </div>
            )}
          </div>
        );
      })}

      {/* 添加阶段按钮 */}
      <button
        onClick={onAddStage}
        style={{
          minWidth: 200,
          border: '1px dashed #30363d',
          borderRadius: 8,
          background: 'transparent',
          color: '#8b949e',
          fontSize: 13,
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          gap: 6,
          transition: 'all .15s',
        }}
        onMouseEnter={(e) => { e.currentTarget.style.borderColor = '#58a6ff'; e.currentTarget.style.color = '#58a6ff'; }}
        onMouseLeave={(e) => { e.currentTarget.style.borderColor = '#30363d'; e.currentTarget.style.color = '#8b949e'; }}
      >
        + 添加阶段
      </button>
    </div>
  );
};
