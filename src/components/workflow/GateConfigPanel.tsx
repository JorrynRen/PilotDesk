/**
 * GateConfigPanel — 阶段门控配置面板
 *
 * 可视化编辑 Gate 策略和合并策略。
 * 支持 all/any/count/threshold 四种门控策略，
 * 支持 merge/concat/pick_first/custom 四种合并策略。
 */

import React from 'react';
import type { GateConfig, GateStrategy, MergeStrategy } from '../../types/workflow';

interface Props {
  gate: GateConfig;
  stageName: string;
  nodeCount: number;
  onUpdate: (gate: Partial<GateConfig>) => void;
  onClose: () => void;
}

const GATE_STRATEGIES: { value: GateStrategy; label: string; desc: string }[] = [
  { value: 'all', label: '全部执行成功', desc: '所有上游节点执行成功后才进入下一阶段' },
  { value: 'count', label: '指定数量执行成功', desc: '指定数量的上游节点执行成功后进入下一阶段' },
  { value: 'threshold', label: '按合并运算值判断', desc: '根据合并运算值是否满足条件判断' },
];

const MERGE_STRATEGIES: { value: MergeStrategy; label: string; desc: string }[] = [
  { value: 'merge', label: '合并对象', desc: '将所有输出合并为单个对象，键名加节点 ID 前缀' },
  { value: 'concat', label: '拼接数组', desc: '将所有输出拼接为数组' },
  { value: 'pick_first', label: '取首个', desc: '取第一个完成的节点输出' },
  { value: 'custom', label: '自定义脚本', desc: '使用自定义脚本合并' },
];

export const GateConfigPanel: React.FC<Props> = ({ gate, stageName, nodeCount, onUpdate, onClose }) => {
  return (
    <div style={{
      position: 'fixed', right: 0, top: 0, bottom: 0, width: 380,
      background: '#161b22', borderLeft: '1px solid #30363d',
      zIndex: 100, overflow: 'auto', padding: 24,
    }}>
      {/* 标题 */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 24 }}>
        <div>
          <div style={{ fontSize: 11, color: '#8b949e', marginBottom: 2 }}>阶段门控配置</div>
          <div style={{ fontSize: 16, fontWeight: 600, color: '#f0f6fc' }}>{stageName}</div>
        </div>
        <button onClick={onClose}
          style={{ padding: '4px 12px', borderRadius: 6, border: '1px solid #30363d', background: '#21262d', color: '#c9d1d9', fontSize: 12, cursor: 'pointer' }}>
          关闭
        </button>
      </div>

      {/* 状态概览 */}
      <div style={{
        padding: '12px 16px', borderRadius: 8, border: '1px solid #21262d',
        background: '#0d1117', marginBottom: 24, display: 'flex', justifyContent: 'space-between',
      }}>
        <div style={{ textAlign: 'center' }}>
          <div style={{ fontSize: 24, fontWeight: 700, color: '#3fb950' }}>{nodeCount}</div>
          <div style={{ fontSize: 11, color: '#8b949e' }}>节点总数</div>
        </div>
        <div style={{ width: 1, background: '#21262d' }} />
        <div style={{ textAlign: 'center' }}>
          <div style={{ fontSize: 24, fontWeight: 700, color: '#58a6ff' }}>{nodeCount}</div>
          <div style={{ fontSize: 11, color: '#8b949e' }}>已就绪</div>
        </div>
        <div style={{ width: 1, background: '#21262d' }} />
        <div style={{ textAlign: 'center' }}>
          <div style={{ fontSize: 24, fontWeight: 700, color: '#d29922' }}>0</div>
          <div style={{ fontSize: 11, color: '#8b949e' }}>进行中</div>
        </div>
      </div>

      {/* 门控策略 */}
      <div style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 12, fontWeight: 600, color: '#f0f6fc', marginBottom: 12, display: 'flex', alignItems: 'center', gap: 6 }}>
          <span>⊞</span> 门控策略
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {GATE_STRATEGIES.map((s) => (
            <label
              key={s.value}
              style={{
                display: 'flex', alignItems: 'flex-start', gap: 10, padding: '10px 12px',
                borderRadius: 8, border: gate.strategy === s.value ? '1px solid #58a6ff' : '1px solid #30363d',
                background: gate.strategy === s.value ? '#1f6feb11' : '#0d1117',
                cursor: 'pointer', transition: 'all .15s',
              }}
            >
              <input
                type="radio"
                name="gate-strategy"
                checked={gate.strategy === s.value}
                onChange={() => onUpdate({ strategy: s.value })}
                style={{ marginTop: 2, accentColor: '#58a6ff' }}
              />
              <div>
                <div style={{ fontSize: 12, fontWeight: 600, color: '#f0f6fc', marginBottom: 2 }}>{s.label}</div>
                <div style={{ fontSize: 11, color: '#8b949e' }}>{s.desc}</div>
              </div>
            </label>
          ))}
        </div>
      </div>

      {/* 计数配置 */}
      {gate.strategy === 'count' && (
        <div style={{ marginBottom: 24, padding: '12px 16px', borderRadius: 8, border: '1px solid #21262d', background: '#0d1117' }}>
          <label style={{ fontSize: 11, color: '#8b949e', display: 'block', marginBottom: 6 }}>完成节点数</label>
          <input
            type="number"
            value={gate.threshold ?? Math.ceil(nodeCount / 2)}
            onChange={(e) => onUpdate({ threshold: Number(e.target.value) })}
            min={1}
            max={nodeCount}
            style={{ width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none' }}
          />
          <div style={{ fontSize: 10, color: '#8b949e', marginTop: 4 }}>至少 {gate.threshold ?? Math.ceil(nodeCount / 2)}/{nodeCount} 个节点完成</div>
        </div>
      )}

      {/* 阈值配置 */}
      {gate.strategy === 'threshold' && (() => {
        const THRESHOLD_OPS = [
          { value: '>=', label: '大于等于' },
          { value: '>',  label: '大于' },
          { value: '==',  label: '等于' },
          { value: '<=', label: '小于等于' },
          { value: '<',  label: '小于' },
        ];
        // 从现有 threshold 字符串中解析运算符和数值，格式: "op value"（如 ">= 60"）
        const parts = (gate.threshold ?? '>= ').trim().split(/\s+/);
        const currentOp = THRESHOLD_OPS.find(o => o.value === parts[0]) ? parts[0] : '>=';
        const currentValue = parts.length > 1 ? parts.slice(1).join(' ') : (parts[0] in THRESHOLD_OPS ? '' : parts[0]);
        const handleOpChange = (op: string) => {
          onUpdate({ threshold: `${op} ${currentValue || ''}`.trim() });
        };
        const handleValueChange = (val: string) => {
          onUpdate({ threshold: `${currentOp} ${val}`.trim() });
        };
        return (
          <div style={{ marginBottom: 24, padding: '12px 16px', borderRadius: 8, border: '1px solid #21262d', background: '#0d1117' }}>
            <label style={{ fontSize: 11, color: '#8b949e', display: 'block', marginBottom: 6 }}>阈值表达式</label>
            <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <select
                value={currentOp}
                onChange={(e) => handleOpChange(e.target.value)}
                style={{
                  padding: '8px 10px', borderRadius: 6, border: '1px solid #30363d',
                  background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none',
                  minWidth: 88, cursor: 'pointer', flexShrink: 0,
                }}
              >
                {THRESHOLD_OPS.map(op => (
                  <option key={op.value} value={op.value}>{op.label}</option>
                ))}
              </select>
              <input
                value={currentValue}
                onChange={(e) => handleValueChange(e.target.value)}
                placeholder="输入阈值，如 60"
                style={{
                  flex: 1, padding: '8px 12px', borderRadius: 6, border: '1px solid #30363d',
                  background: '#0d1117', color: '#c9d1d9', fontSize: 12, outline: 'none',
                }}
              />
            </div>
            <div style={{ fontSize: 10, color: '#8b949e', marginTop: 4 }}>
              合并值 {currentOp} {currentValue || '…'} 时通过
            </div>
          </div>
        );
      })()}

      {/* 合并策略 */}
      <div style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 12, fontWeight: 600, color: '#f0f6fc', marginBottom: 12, display: 'flex', alignItems: 'center', gap: 6 }}>
          <span>⊕</span> 合并策略
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {MERGE_STRATEGIES.map((s) => (
            <label
              key={s.value}
              style={{
                display: 'flex', alignItems: 'flex-start', gap: 10, padding: '10px 12px',
                borderRadius: 8, border: gate.mergeStrategy === s.value ? '1px solid #a371f7' : '1px solid #30363d',
                background: gate.mergeStrategy === s.value ? '#8957e511' : '#0d1117',
                cursor: 'pointer', transition: 'all .15s',
              }}
            >
              <input
                type="radio"
                name="merge-strategy"
                checked={gate.mergeStrategy === s.value}
                onChange={() => onUpdate({ mergeStrategy: s.value })}
                style={{ marginTop: 2, accentColor: '#a371f7' }}
              />
              <div>
                <div style={{ fontSize: 12, fontWeight: 600, color: '#f0f6fc', marginBottom: 2 }}>{s.label}</div>
                <div style={{ fontSize: 11, color: '#8b949e' }}>{s.desc}</div>
              </div>
            </label>
          ))}
        </div>
      </div>

      {/* 自定义合并脚本 */}
      {gate.mergeStrategy === 'custom' && (
        <div style={{ marginBottom: 24, padding: '12px 16px', borderRadius: 8, border: '1px solid #21262d', background: '#0d1117' }}>
          <label style={{ fontSize: 11, color: '#8b949e', display: 'block', marginBottom: 6 }}>自定义合并脚本</label>
          <textarea
            value={gate.customScript || ''}
            onChange={(e) => onUpdate({ customScript: e.target.value })}
            placeholder={'// 参数: outputs = { nodeId: output }\n// 返回合并后的值\nreturn Object.values(outputs);'}
            rows={6}
            style={{ width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid #30363d', background: '#0d1117', color: '#c9d1d9', fontSize: 11, outline: 'none', resize: 'vertical', fontFamily: 'monospace' }}
          />
        </div>
      )}

      {/* 说明 */}
      <div style={{ padding: '12px 16px', borderRadius: 8, border: '1px solid #21262d', background: '#0d1117' }}>
        <div style={{ fontSize: 11, fontWeight: 600, color: '#8b949e', marginBottom: 6 }}>💡 工作流执行流程</div>
        <div style={{ fontSize: 11, color: '#8b949e', lineHeight: 1.6 }}>
          1. 阶段内节点按 DAG 拓扑排序执行<br />
          2. 边上的条件决定节点是否执行<br />
          3. 所有节点执行完成后，检查门控策略<br />
          4. 满足条件后执行合并策略<br />
          5. 合并结果传递到下一阶段
        </div>
      </div>
    </div>
  );
};
