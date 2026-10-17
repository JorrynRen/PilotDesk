/**
 * 飞地「组织与权限」治理面板：数据出境总闸 / 主机应急开关 / 变更审计。
 *
 * 权限口径：**所有治理按钮的可用性都来自后端 `enclave_org_caps.actions`**（授权器唯一来源），
 * 前端不做角色推断；`policy.write` / `emergency.set` 不在列表里就一律禁用。
 */

import { useState } from 'react';
import { ShieldAlert, ShieldCheck, Power, History, Loader2 } from 'lucide-react';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';

const ACTION_LABEL: Record<string, string> = {
  'policy.allow_cloud_model': '数据出境策略',
  'host.emergency': '主机应急开关',
  'host.transfer': '主机转移',
};

function formatTime(unixSec: number): string {
  if (!unixSec) return '-';
  const d = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/**
 * 审计明细渲染：把后端留痕的结构化 JSON 翻成人话。
 *
 * 后端口径（见 `commands/enclave.rs`）：
 * - `policy.allow_cloud_model`：`{ from: bool, to: bool }`
 * - `host.emergency`：`{ from: bool, to: bool }`
 * 解析失败时原样展示，避免把留痕藏起来。
 */
function formatDetail(action: string, detail: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(detail);
  } catch {
    return detail || '-';
  }
  const obj = parsed as { from?: unknown; to?: unknown } | null;
  if (obj && typeof obj.from === 'boolean' && typeof obj.to === 'boolean') {
    const label = action === 'host.emergency' ? '对外服务' : '团队数据出境';
    const text = (v: boolean) => (v ? '开启' : '关闭');
    return `${label}：${text(obj.from)} → ${text(obj.to)}`;
  }
  return detail || '-';
}

/** 一行开关：左侧标题 + 说明，右侧按钮（受 canEdit 控制）。 */
function SwitchRow(props: {
  title: string;
  desc: string;
  on: boolean;
  canEdit: boolean;
  busy: boolean;
  danger?: boolean;
  onToggle: () => void;
  updatedBy?: string;
  updatedAt?: number;
}) {
  const { title, desc, on, canEdit, busy, danger, onToggle, updatedBy, updatedAt } = props;
  const onColor = danger ? 'var(--status-danger)' : 'var(--accent)';
  return (
    <div className="flex items-start gap-3 py-2">
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
            {title}
          </span>
          <span
            className="text-[10px] px-1.5 py-0.5 rounded"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: on ? onColor : 'var(--text-tertiary)',
            }}
          >
            {on ? '已开启' : '已关闭'}
          </span>
        </div>
        <div className="text-[11px] mt-1 leading-4" style={{ color: 'var(--text-tertiary)' }}>
          {desc}
        </div>
        {/* 注意用布尔判断：`updatedAt` 为 0 时直接 `(updatedBy || updatedAt) && …` 会把 0 渲染出来 */}
        {!!(updatedBy || updatedAt) && (
          <div className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
            最近变更：{updatedBy || '-'} · {formatTime(updatedAt ?? 0)}
          </div>
        )}
      </div>
      <button
        onClick={onToggle}
        disabled={!canEdit || busy}
        className="shrink-0 text-xs px-2.5 py-1 rounded flex items-center gap-1"
        style={{
          backgroundColor: on ? onColor : 'var(--bg-tertiary)',
          color: on ? '#fff' : 'var(--text-secondary)',
          border: '1px solid var(--border)',
          opacity: !canEdit || busy ? 0.5 : 1,
          cursor: !canEdit || busy ? 'not-allowed' : 'pointer',
        }}
        title={canEdit ? '' : '仅组织所有者可修改'}
      >
        {busy ? <Loader2 size={11} className="animate-spin" /> : <Power size={11} />}
        {on ? '关闭' : '开启'}
      </button>
    </div>
  );
}

/**
 * 治理区块：出境闸门 + 应急开关 + 审计流水。需先选中组织。
 */
export function OrgGovernanceSection() {
  const activeOrgId = useEnclaveOrgStore((s) => s.activeOrgId);
  const caps = useEnclaveOrgStore((s) => s.caps);
  const policy = useEnclaveOrgStore((s) => s.policy);
  const emergency = useEnclaveOrgStore((s) => s.emergency);
  const audit = useEnclaveOrgStore((s) => s.audit);
  const [busy, setBusy] = useState<'policy' | 'emergency' | null>(null);

  if (!activeOrgId) return null;

  const canWritePolicy = !!caps?.actions.includes('policy.write');
  const canEmergency = !!caps?.actions.includes('emergency.set');
  const allowCloud = !!policy?.allowCloudModel;
  const emergencyOn = !!emergency?.disabled;

  const togglePolicy = async () => {
    // 打开闸门必须二次确认，且明示「历史沉淀内容一并可被发送」
    if (!allowCloud) {
      const ok = await confirmDialog({
        title: '允许团队数据发送至云端模型',
        message:
          '开启后，组织知识库、任务与产出（含历史已沉淀内容）将可被发送至云端模型。\n'
          + '数据只会经当前配置为「云端」的 API Provider 出境；未配置云端 Provider 时不会发送。\n\n是否继续？',
        confirmText: '确认开启',
      });
      if (!ok) return;
    }
    setBusy('policy');
    const ok = await useEnclaveOrgStore.getState().setPolicy(activeOrgId, !allowCloud);
    setBusy(null);
    if (ok) showToast(allowCloud ? '已禁止团队数据出境' : '已允许团队数据发送至云端模型', 'success');
  };

  const toggleEmergency = async () => {
    if (!emergencyOn) {
      const ok = await confirmDialog({
        title: '临时关闭飞地对外服务',
        message:
          '关闭后，成员端将立即无法连接本机（已连接的会断开）。\n'
          + '本机使用不受影响；该操作纯本地、断网可用、不改云端数据，并会记入审计。\n\n是否继续？',
        confirmText: '确认关闭对外服务',
      });
      if (!ok) return;
    }
    setBusy('emergency');
    const ok = await useEnclaveOrgStore.getState().setEmergency(activeOrgId, !emergencyOn);
    setBusy(null);
    if (ok) showToast(emergencyOn ? '已恢复飞地对外服务' : '已临时关闭飞地对外服务', 'success');
  };

  return (
    <div className="shrink-0 flex flex-col gap-2">
      {/* 数据出境总闸 */}
      <div
        className="rounded-lg px-3 py-2"
        style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2">
          {allowCloud ? (
            <ShieldAlert size={13} style={{ color: 'var(--status-danger)' }} />
          ) : (
            <ShieldCheck size={13} style={{ color: 'var(--status-success, var(--accent))' }} />
          )}
          <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
            数据出境闸门
          </span>
          <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {allowCloud ? '团队数据可发送至云端模型' : '团队数据不得发送至云端模型（默认）'}
          </span>
        </div>
        <SwitchRow
          title="团队数据可发送至云端模型"
          desc="组织级总闸，默认关闭；只管 enclave.db 内的团队数据，个人数据不受影响。策略随主机/数据库迁移。"
          on={allowCloud}
          canEdit={canWritePolicy}
          busy={busy === 'policy'}
          danger
          onToggle={() => void togglePolicy()}
          updatedBy={policy?.updatedBy}
          updatedAt={policy?.updatedAt}
        />
        {!canWritePolicy && (
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            仅组织所有者（owner）可修改
          </div>
        )}
      </div>

      {/* 主机应急开关 */}
      <div
        className="rounded-lg px-3 py-2"
        style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2">
          <Power size={13} style={{ color: emergencyOn ? 'var(--status-danger)' : 'var(--text-tertiary)' }} />
          <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
            主机应急
          </span>
          <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {emergencyOn ? '对外服务已临时关闭（本机照常）' : '对外服务正常'}
          </span>
        </div>
        <SwitchRow
          title="临时关闭飞地对外服务"
          desc="不按成员禁用；用于成员移除等治理滞后时立即阻断外部访问。纯本地、断网可用、不改云端数据，操作留痕。"
          on={emergencyOn}
          canEdit={canEmergency}
          busy={busy === 'emergency'}
          danger
          onToggle={() => void toggleEmergency()}
          updatedBy={emergency?.updatedBy}
          updatedAt={emergency?.updatedAt}
        />
        {!canEmergency && (
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            仅组织所有者（owner）可操作
          </div>
        )}
      </div>

      {/* 审计流水 */}
      <div
        className="rounded-lg flex flex-col overflow-hidden"
        style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
      >
        <div
          className="shrink-0 h-8 px-3 flex items-center gap-2"
          style={{ borderBottom: '1px solid var(--border)' }}
        >
          <History size={12} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
            变更审计
          </span>
          <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            {audit.length}
          </span>
        </div>
        <div className="max-h-[180px] overflow-auto">
          <table className="w-full text-xs" style={{ borderCollapse: 'collapse' }}>
            <thead>
              <tr style={{ color: 'var(--text-tertiary)' }}>
                <th className="text-left px-3 py-1.5 font-normal">时间</th>
                <th className="text-left px-3 py-1.5 font-normal">操作</th>
                <th className="text-left px-3 py-1.5 font-normal">操作人</th>
                <th className="text-left px-3 py-1.5 font-normal">明细</th>
              </tr>
            </thead>
            <tbody>
              {audit.map((a) => (
                <tr key={a.id} style={{ borderTop: '1px solid var(--border)', color: 'var(--text-secondary)' }}>
                  <td className="px-3 py-1 whitespace-nowrap">{formatTime(a.createdAt)}</td>
                  <td className="px-3 py-1 whitespace-nowrap">{ACTION_LABEL[a.action] ?? a.action}</td>
                  <td className="px-3 py-1 whitespace-nowrap">{a.actorId || '-'}</td>
                  <td className="px-3 py-1 break-all">{formatDetail(a.action, a.detail)}</td>
                </tr>
              ))}
              {audit.length === 0 && (
                <tr>
                  <td colSpan={4} className="px-3 py-3 text-center" style={{ color: 'var(--text-tertiary)' }}>
                    暂无审计记录（策略 / 应急变更会留痕）
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}
