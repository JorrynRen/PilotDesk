/**
 * 飞地「组织与权限」治理面板：数据出境总闸 / 主机应急开关 / 变更审计。
 *
 * 权限口径：**所有治理按钮的可用性都来自后端 `enclave_org_caps.actions`**（授权器唯一来源），
 * 前端不做角色推断；`policy.write` / `emergency.set` 不在列表里就一律禁用。
 */

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { ShieldAlert, ShieldCheck, Power, History, Loader2, FolderOpen } from 'lucide-react';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import type { TeamFilesRootView } from '../../types/enclave';

const ACTION_LABEL: Record<string, string> = {
  'policy.allow_cloud_model': '数据出境策略',
  'host.emergency': '主机应急开关',
  'host.transfer': '主机转移',
  'files.root': '团队文件根',
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
    // `host.emergency` 落库的是 `disabled`（true = 关闭），展示口径要翻转成「对外服务 开/关」
    if (action === 'host.emergency') {
      const service = (disabled: boolean) => (disabled ? '关闭' : '开启');
      return `对外服务：${service(obj.from)} → ${service(obj.to)}`;
    }
    const allow = (v: boolean) => (v ? '开启' : '关闭');
    return `团队数据出境：${allow(obj.from)} → ${allow(obj.to)}`;
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
  /** 已格式化的操作人（姓名 + 邮箱），由调用方解析，避免这里再碰用户 id */
  updatedByLabel?: string;
  updatedAt?: number;
}) {
  const { title, desc, on, canEdit, busy, danger, onToggle, updatedByLabel, updatedAt } = props;
  // `danger` = 当前状态本身是风险状态（如允许出境 / 对外服务已关闭），只影响状态色，不影响开关方向
  const stateColor = danger ? 'var(--status-danger)' : on ? 'var(--accent)' : 'var(--text-tertiary)';
  const buttonColor = on ? (danger ? 'var(--status-danger)' : 'var(--accent)') : 'var(--bg-tertiary)';
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
              color: stateColor,
            }}
          >
            {on ? '已开启' : '已关闭'}
          </span>
        </div>
        <div className="text-[11px] mt-1 leading-4" style={{ color: 'var(--text-tertiary)' }}>
          {desc}
        </div>
        {/* 注意用布尔判断：`updatedAt` 为 0 时直接 `(updatedBy || updatedAt) && …` 会把 0 渲染出来 */}
        {!!(updatedByLabel || updatedAt) && (
          <div className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
            最近变更：{updatedByLabel || '-'} · {formatTime(updatedAt ?? 0)}
          </div>
        )}
      </div>
      <button
        onClick={onToggle}
        disabled={!canEdit || busy}
        className="shrink-0 text-xs px-2.5 py-1 rounded flex items-center gap-1"
        style={{
          backgroundColor: on ? buttonColor : 'var(--bg-tertiary)',
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
  const detail = useEnclaveOrgStore((s) => s.detail);
  const audit = useEnclaveOrgStore((s) => s.audit);
  const [busy, setBusy] = useState<'policy' | 'emergency' | null>(null);
  const [rootView, setRootView] = useState<TeamFilesRootView | null>(null);
  const [rootBusy, setRootBusy] = useState(false);

  useEffect(() => {
    void invoke<TeamFilesRootView>('enclave_files_root_get').then(setRootView).catch(() => {});
  }, []);

  if (!activeOrgId) return null;

  // 操作人展示：姓名 + 邮箱（优先本机登录身份，其次成员快照），都没有才回落到平台用户 id
  const actorLabel = (actorId: string): string => {
    if (!actorId) return '-';
    const withEmail = (name: string, email: string): string =>
      name && email ? `${name}（${email}）` : name || email || actorId;
    const me = caps?.identity;
    if (me && me.userId === actorId) return withEmail(me.nickname, me.email);
    const m = detail?.members.find((x) => x.userId === actorId);
    if (m) return withEmail(m.displayName, m.email);
    return actorId;
  };

  const canWritePolicy = !!caps?.actions.includes('policy.write');
  const canEmergency = !!caps?.actions.includes('emergency.set');
  const canEditRoot = !!caps?.actions.includes('files.root.write');
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
          danger={allowCloud}
          onToggle={() => void togglePolicy()}
          updatedByLabel={policy?.updatedBy ? actorLabel(policy.updatedBy) : ''}
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
          title="对外服务"
          desc="开启时成员可连接本机；关闭后立即断开全部外部连接（成员端无法访问）。本机使用不受影响；纯本地、断网可用、不改云端数据，操作留痕。"
          on={!emergencyOn}
          canEdit={canEmergency}
          busy={busy === 'emergency'}
          danger={emergencyOn}
          onToggle={() => void toggleEmergency()}
          updatedByLabel={emergency?.updatedBy ? actorLabel(emergency.updatedBy) : ''}
          updatedAt={emergency?.updatedAt}
        />
        {!canEmergency && (
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            仅组织所有者（owner）可操作
          </div>
        )}
      </div>

      {/* 团队文件根（§13） */}
      <div
        className="rounded-lg px-3 py-2"
        style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2">
          <FolderOpen size={13} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
            团队文件根
          </span>
          {rootView && (
            <span className="text-[11px]" style={{ color: rootView.available ? 'var(--text-tertiary)' : 'var(--status-danger)' }}>
              {rootView.available ? (rootView.custom ? '自定义' : '默认') : '不可用'}
            </span>
          )}
          <div className="flex-1" />
          <button
            onClick={async () => {
              const picked = await openDialog({ directory: true, multiple: false });
              if (typeof picked !== 'string') return;
              const ok = await confirmDialog({
                title: '更改团队文件根目录',
                message: `团队文件将从当前位置迁移到：\n${picked}\n\n迁移会逐个文件做 sha256 校验，失败会自动回滚。是否继续？`,
                confirmText: '开始迁移',
              });
              if (!ok) return;
              setRootBusy(true);
              try {
                const next = await invoke<TeamFilesRootView>('enclave_files_root_set', {
                  orgId: activeOrgId,
                  path: picked,
                });
                setRootView(next);
                showToast('团队文件根已更新', 'success');
              } catch (e) {
                showToast(`更改失败：${e instanceof Error ? e.message : String(e)}`, 'error');
              } finally {
                setRootBusy(false);
              }
            }}
            disabled={!canEditRoot || rootBusy}
            className="text-xs px-2 py-1 rounded flex items-center gap-1"
            style={{
              backgroundColor: 'var(--bg-tertiary)',
              color: 'var(--text-secondary)',
              border: '1px solid var(--border)',
              opacity: !canEditRoot || rootBusy ? 0.5 : 1,
            }}
            title={canEditRoot ? '' : '仅组织所有者可修改'}
          >
            {rootBusy ? <Loader2 size={11} className="animate-spin" /> : <FolderOpen size={11} />}
            选择目录
          </button>
        </div>
        <div className="mt-1 text-[11px] font-mono break-all" style={{ color: 'var(--text-tertiary)' }}>
          {rootView?.root ?? '读取中…'}
        </div>
        <div className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
          与个人知识库完全独立；每个组织一个子目录。仅支持本地固定盘，盘不可用时会拒绝写入而不是回退。
        </div>
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
                  <td className="px-3 py-1 whitespace-nowrap">{actorLabel(a.actorId)}</td>
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
