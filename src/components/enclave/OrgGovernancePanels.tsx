/**
 * 飞地「组织与权限」治理面板：数据出境总闸 / 主机应急开关 / 变更审计。
 *
 * 权限口径：**所有治理按钮的可用性都来自后端 `enclave_org_caps.actions`**（授权器唯一来源），
 * 前端不做角色推断；`policy.write` / `emergency.set` 不在列表里就一律禁用。
 */

import { useEffect, useState } from 'react';
import { ShieldAlert, ShieldCheck, Power, History, Loader2, Server, RefreshCw, ArrowLeftRight } from 'lucide-react';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
import { useEnclaveOrgStore } from '../../stores/enclaveOrgStore';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import type { TransferExportOutcome, TransferPhase, TransferView } from '../../types/enclave';

const ACTION_LABEL: Record<string, string> = {
  'policy.allow_cloud_model': '数据出境策略',
  'host.emergency': '主机应急开关',
  'host.transfer': '主机转移',
  'files.root': '本机团队文件根目录',
  'transfer.export': '主机转移 · 导出迁移包',
  'transfer.import': '主机转移 · 导入迁移包',
  'transfer.offer': '主机转移 · 发起局域网转移',
  'transfer.accept': '主机转移 · 接收方接受',
  'transfer.verify': '主机转移 · 校验通过',
  'transfer.complete': '主机转移 · 云端生效',
  'transfer.fail': '主机转移 · 失败',
  'transfer.data_destroyed': '主机转移 · 旧机数据销毁',
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
 * 后端口径（见 `commands/enclave*`）：
 * - `policy.allow_cloud_model` / `host.emergency`：`{ from: bool, to: bool }`
 * - `files.root`：`{ from: string, to: string }`（目录路径）
 * 解析失败时原样展示，避免把留痕藏起来。
 */
function formatDetail(action: string, detail: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(detail);
  } catch {
    return detail || '-';
  }
  const obj = parsed as { from?: unknown; to?: unknown; redacted?: unknown } | null;
  if (obj && obj.redacted === true) {
    return '本机路径仅所有者可见';
  }
  // 转移状态流转：from / to 是 phase 字符串，不是路径
  if (action.startsWith('transfer.') && obj && typeof obj.from === 'string' && typeof obj.to === 'string') {
    return `状态：${obj.from} → ${obj.to}`;
  }
  if (obj && typeof obj.from === 'string' && typeof obj.to === 'string') {
    // 历史留痕可能带 Windows 扩展长度前缀（`\\?\` / `\\?\UNC\`），展示时统一剥掉
    const clean = (p: string) => p.replace(/^\\\\\?\\UNC\\/, '\\\\').replace(/^\\\\\?\\/, '');
    return `本机基础目录：${clean(obj.from)} → ${clean(obj.to)}`;
  }
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
 * 主机与连接（M7）：主机侧展示对外服务的监听状态；成员侧展示发现结果与手动地址。
 *
 * 手动地址是成员**本机**设置，优先于 UDP 自动发现（局域网广播常被 Windows 防火墙拦截）。
 */
function HostConnectionCard({ orgId }: { orgId: string }) {
  const host = useEnclaveOrgStore((s) => s.host);
  const [addr, setAddr] = useState('');
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  // 组织切换 / 状态刷新后回填已保存的手动地址
  useEffect(() => {
    setAddr(host?.manualAddr ?? '');
  }, [host?.manualAddr, orgId]);

  const saveAddr = async () => {
    setBusy(true);
    const ok = await useEnclaveOrgStore.getState().setHostAddr(orgId, addr.trim());
    setBusy(false);
    if (ok) showToast(addr.trim() ? '已保存主机地址' : '已清除手动主机地址', 'success');
  };

  const refresh = async () => {
    setRefreshing(true);
    await useEnclaveOrgStore.getState().loadHost(orgId);
    setRefreshing(false);
  };

  const roleText = host?.role === 'host' ? '本机是主机' : host?.role === 'member' ? '本机是成员' : '角色未确定';
  const roleColor = host?.role === 'host' ? 'var(--accent)' : 'var(--text-tertiary)';

  return (
    <div
      className="rounded-lg px-3 py-2"
      style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
    >
      <div className="flex items-center gap-2">
        <Server size={13} style={{ color: roleColor }} />
        <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
          主机与连接
        </span>
        <span className="text-[11px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: roleColor }}>
          {roleText}
        </span>
        <div className="flex-1" />
        <button
          onClick={() => void refresh()}
          disabled={refreshing}
          className="text-[11px] px-1.5 py-0.5 rounded flex items-center gap-1"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
        >
          {refreshing ? <Loader2 size={10} className="animate-spin" /> : <RefreshCw size={10} />}
          刷新状态
        </button>
      </div>

      {host?.role === 'host' && (
        <div className="mt-2 flex flex-col gap-1 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          <div className="flex items-center gap-2">
            <span
              className="text-[10px] px-1.5 py-0.5 rounded"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: host.running ? 'var(--status-success, var(--accent))' : 'var(--status-danger)',
              }}
            >
              {host.running ? `服务运行中 · 端口 ${host.port}` : '服务未运行'}
            </span>
            <span>主机名 {host.hostName}</span>
          </div>
          {!host.running && (
            <div style={{ color: host.lastError ? 'var(--status-danger)' : 'var(--text-tertiary)' }}>
              {host.lastError ?? '仅当组织所有者（owner）登录本机时才启动对外服务'}
            </div>
          )}
          <div>本机对外服务的组织：{host.servedOrgIds.length}</div>
        </div>
      )}

      {host?.role === 'member' && (
        <div className="mt-2 flex flex-col gap-1.5 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          <div>
            自动发现：
            {host.discoveredAddr
              ? `${host.discoveredHostName || '主机'}（${host.discoveredAddr}）`
              : '未发现（主机离线，或局域网 UDP 广播被防火墙拦截）'}
          </div>
          <div>
            当前连接目标：
            <span style={{ color: host.manualAddr || host.discoveredAddr ? 'var(--text-secondary)' : 'var(--status-danger)' }}>
              {host.manualAddr ? `手动 · ${host.manualAddr}` : host.discoveredAddr ? `自动 · ${host.discoveredAddr}` : '无'}
            </span>
          </div>
          <div className="flex items-center gap-1.5">
            <input
              value={addr}
              onChange={(e) => setAddr(e.target.value)}
              placeholder="手动填写主机地址，如 192.168.1.9:47820"
              className="flex-1 h-7 px-2 rounded text-[11px] outline-none"
              style={{ backgroundColor: 'var(--bg-side)', border: '1px solid var(--border)', color: 'var(--text-primary)' }}
            />
            <button
              onClick={() => void saveAddr()}
              disabled={busy}
              className="h-7 px-2 rounded text-[11px] flex items-center gap-1"
              style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: busy ? 0.6 : 1 }}
            >
              {busy && <Loader2 size={10} className="animate-spin" />}
              保存
            </button>
            {!!host.manualAddr && (
              <button
                onClick={() => {
                  setAddr('');
                  void useEnclaveOrgStore.getState().setHostAddr(orgId, '');
                }}
                className="h-7 px-2 rounded text-[11px]"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                清除
              </button>
            )}
          </div>
          <div>留空即回到自动发现；地址格式为「主机 IP:端口」。</div>
        </div>
      )}

      {(!host || host.role === 'unknown') && (
        <div className="mt-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          本机尚无该组织的治理快照；请先点上方「刷新」同步组织与成员。
        </div>
      )}
    </div>
  );
}

const PHASE_COLOR: Record<TransferPhase, string> = {
  pending: 'var(--text-secondary)',
  accepted: 'var(--text-secondary)',
  migrating: 'var(--accent)',
  verified: 'var(--accent)',
  completed: 'var(--status-success, var(--accent))',
  rejected: 'var(--status-danger)',
  expired: 'var(--text-tertiary)',
  failed: 'var(--status-danger)',
};

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** 生成迁移包口令：去掉易混淆字符（0/O、1/l/I），24 位足够线下转述。 */
function randomPassphrase(len = 24): string {
  const alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789';
  const bytes = new Uint8Array(len);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join('');
}

async function copyText(text: string, label: string): Promise<void> {
  if (!text) {
    showToast(`请先填写${label}`, 'error');
    return;
  }
  try {
    await navigator.clipboard.writeText(text);
    showToast(`${label}已复制`, 'success');
  } catch (e) {
    showToast(`复制失败：${errorMessage(e)}`, 'error');
  }
}

/**
 * 主机转移（M8）：线下加密迁移包导出/导入 + 状态推进 + 旧机销毁。
 *
 * 口径（设计 §8.7）：数据面只走内网/线下介质，不经云；`verified` 必须早于
 * `completed`（云端 ownership 生效），之后旧主机才允许销毁数据。
 */
function TransferCard({ orgId }: { orgId: string }) {
  const transfers = useEnclaveOrgStore((s) => s.transfers);
  const caps = useEnclaveOrgStore((s) => s.caps);
  const detail = useEnclaveOrgStore((s) => s.detail);
  const lanPending = useEnclaveOrgStore((s) => s.lanPending);
  const lanPendingError = useEnclaveOrgStore((s) => s.lanPendingError);
  const [pass, setPass] = useState('');
  const [outPath, setOutPath] = useState('');
  const [exported, setExported] = useState<TransferExportOutcome | null>(null);
  const [importPath, setImportPath] = useState('');
  const [importPass, setImportPass] = useState('');
  const [busy, setBusy] = useState<'export' | 'import' | null>(null);
  const [targetId, setTargetId] = useState('');
  const [offering, setOffering] = useState(false);
  const [accepting, setAccepting] = useState<string | null>(null);

  const canTransfer = !!caps?.actions.includes('host.transfer');
  const isMember = !!caps && !canTransfer;

  // 成员端：进入卡片即拉一次待确认请求（失败只在本卡片提示，不影响其它面板）
  useEffect(() => {
    if (isMember) void useEnclaveOrgStore.getState().loadLanPending(orgId);
  }, [orgId, isMember]);

  const meId = caps?.identity?.userId ?? '';
  const targets = (detail?.members ?? []).filter(
    (m) => m.seatStatus !== 'removed' && m.userId !== meId && m.role !== 'owner',
  );

  /** 选择导出位置（.pdesk） */
  const pickExportPath = async () => {
    try {
      const stamp = new Date().toISOString().slice(0, 10).replace(/-/g, '');
      const picked = await saveDialog({
        title: '选择迁移包导出位置',
        defaultPath: `enclave-${orgId}-${stamp}.pdesk`,
        filters: [{ name: '飞地迁移包', extensions: ['pdesk'] }],
      });
      if (typeof picked === 'string' && picked) setOutPath(picked);
    } catch (e) {
      showToast(`选择导出位置失败：${errorMessage(e)}`, 'error');
    }
  };

  /** 选择待导入的迁移包 */
  const pickImportPath = async () => {
    try {
      const picked = await openDialog({
        title: '选择飞地迁移包',
        multiple: false,
        filters: [{ name: '飞地迁移包', extensions: ['pdesk'] }],
      });
      const path = Array.isArray(picked) ? picked[0] : picked;
      if (typeof path === 'string' && path) setImportPath(path);
    } catch (e) {
      showToast(`选择迁移包失败：${errorMessage(e)}`, 'error');
    }
  };

  const pastePassphrase = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text.trim()) setImportPass(text.trim());
    } catch (e) {
      showToast(`读取剪贴板失败：${errorMessage(e)}`, 'error');
    }
  };

  const doExport = async () => {
    if (pass.trim().length < 8) {
      showToast('迁移包口令至少 8 位', 'error');
      return;
    }
    setBusy('export');
    const outcome = await useEnclaveOrgStore.getState().exportTransfer(orgId, pass.trim(), outPath);
    setBusy(null);
    if (outcome) {
      setExported(outcome);
      showToast(`迁移包已导出（${formatBytes(outcome.size)}，${outcome.fileCount} 个文件）`, 'success');
    }
  };

  const doImport = async () => {
    if (!importPath.trim()) {
      showToast('请填写迁移包路径', 'error');
      return;
    }
    if (importPass.trim().length < 8) {
      showToast('迁移包口令至少 8 位', 'error');
      return;
    }
    setBusy('import');
    const outcome = await useEnclaveOrgStore.getState().importTransfer(importPath.trim(), importPass.trim());
    setBusy(null);
    if (outcome) {
      setImportPath('');
      setImportPass('');
      showToast(
        `迁移包已导入并校验（${outcome.fileCount} 个文件）。云端完成转让后即可接管主机。`,
        'success',
      );
    }
  };

  /** 发起局域网转移（owner 选受让人，生成迁移包等待对方确认） */
  const startOffer = async () => {
    if (!targetId) {
      showToast('请选择受让人', 'error');
      return;
    }
    const target = targets.find((t) => t.userId === targetId);
    const ok = await confirmDialog({
      title: '发起局域网转移',
      message:
        `将组织「${orgId}」转让给 ${target?.displayName || targetId}。\n`
        + '本机会生成加密迁移包；对方在同一局域网确认后自动传输并校验。\n'
        + '云端所有权变更仍需平台完成，旧机数据在云端生效前不会被销毁。\n\n是否继续？',
      confirmText: '发起转移',
    });
    if (!ok) return;
    setOffering(true);
    const outcome = await useEnclaveOrgStore.getState().offerLanTransfer(orgId, targetId);
    setOffering(false);
    if (outcome) {
      setTargetId('');
      showToast(
        `已生成转移请求（${formatBytes(outcome.size)}，${outcome.fileCount} 个文件），等待受让人确认`,
        'success',
      );
    }
  };

  /** 受让人接受并自动迁移 */
  const acceptPending = async (t: TransferView) => {
    const ok = await confirmDialog({
      title: '接受主机转移',
      message:
        '接受后将自动从主机下载并校验组织数据。\n'
        + '迁移完成前请保持主机在线、飞地对外服务开启。\n\n是否继续？',
      confirmText: '接受并迁移',
    });
    if (!ok) return;
    setAccepting(t.id);
    const outcome = await useEnclaveOrgStore.getState().acceptLanTransfer(orgId, t.id);
    setAccepting(null);
    if (outcome) {
      showToast(
        `迁移完成：已导入并校验 ${outcome.fileCount} 个文件。云端完成所有权转让后即可接管主机。`,
        'success',
      );
    }
  };

  const markVerified = async (t: TransferView) => {
    const ok = await confirmDialog({
      title: '标记迁移已校验',
      message: '请先在新主机确认迁移包已导入且校验通过。标记后进入「已校验」，等待云端完成 ownership 转让。',
      confirmText: '已在新机校验',
    });
    if (!ok) return;
    if (await useEnclaveOrgStore.getState().advanceTransfer(t.id, 'verified')) {
      showToast('已标记为已校验', 'success');
    }
  };

  const markCompleted = async (t: TransferView) => {
    const ok = await confirmDialog({
      title: '确认云端已生效',
      message:
        '只有在平台已完成组织转让（ownership 已变更）后才能标记。\n'
        + '标记后旧主机才允许销毁数据，且状态不可回退。',
      confirmText: '云端已生效',
    });
    if (!ok) return;
    if (await useEnclaveOrgStore.getState().advanceTransfer(t.id, 'completed')) {
      showToast('已标记云端生效', 'success');
    }
  };

  const destroyOld = async (t: TransferView) => {
    const ok = await confirmDialog({
      title: '销毁旧主机团队数据',
      message:
        `将删除本机组织「${orgId}」的全部团队数据（数据库记录与文件），不可恢复。\n`
        + '请确认新主机已接管且云端已生效。',
      confirmText: '确认销毁',
    });
    if (!ok) return;
    if (await useEnclaveOrgStore.getState().destroyTransfer(t.id, orgId)) {
      showToast('旧主机数据已销毁', 'success');
    }
  };

  const actionButton = (label: string, onClick: () => void, danger = false) => (
    <button
      onClick={onClick}
      className="text-[10px] px-1.5 py-0.5 rounded"
      style={{
        backgroundColor: 'var(--bg-tertiary)',
        color: danger ? 'var(--status-danger)' : 'var(--text-secondary)',
        border: '1px solid var(--border)',
      }}
    >
      {label}
    </button>
  );

  const inputStyle = {
    backgroundColor: 'var(--bg-side)',
    border: '1px solid var(--border)',
    color: 'var(--text-primary)',
  } as const;

  return (
    <div
      className="rounded-lg px-3 py-2"
      style={{ backgroundColor: 'var(--bg-content)', border: '1px solid var(--border)' }}
    >
      <div className="flex items-center gap-2">
        <ArrowLeftRight size={13} style={{ color: 'var(--text-tertiary)' }} />
        <span className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
          主机转移
        </span>
        <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          数据只走内网或线下介质，不经云
        </span>
      </div>
      <div className="mt-1 text-[10px] leading-4" style={{ color: 'var(--text-tertiary)' }}>
        顺序：导出/导入并校验（verified）→ 云端完成所有权转让（completed）→ 旧主机销毁数据。
        未校验不得生效；云端未生效不得销毁。
      </div>

      {/* 转出 / 接管互斥：owner = 主机 → 转出；成员 → 接管 */}
      {!caps && (
        <div className="mt-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          治理信息尚未加载，暂不能发起或接收转移。
        </div>
      )}

      {caps && canTransfer && (
        <div className="mt-2 flex flex-col gap-1.5">
          <div className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            局域网自动迁移
          </div>
          <div className="flex items-center gap-1.5">
            <select
              value={targetId}
              onChange={(e) => setTargetId(e.target.value)}
              className="flex-1 min-w-0 h-7 px-2 rounded text-[11px] outline-none"
              style={inputStyle}
            >
              <option value="">选择受让人（组织成员）</option>
              {targets.map((m) => (
                <option key={m.userId} value={m.userId}>
                  {m.displayName || m.email || m.userId}
                  {m.email && m.displayName ? `（${m.email}）` : ''}
                </option>
              ))}
            </select>
            <button
              onClick={() => void startOffer()}
              disabled={!targetId || offering}
              className="h-7 px-2 rounded text-[11px] flex items-center gap-1 shrink-0"
              style={{
                backgroundColor: 'var(--accent)',
                color: '#fff',
                opacity: !targetId || offering ? 0.5 : 1,
              }}
            >
              {offering && <Loader2 size={10} className="animate-spin" />}
              发起转移
            </button>
          </div>
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            受让人需与本机在同一局域网并已登录；对方确认后自动下载、校验并合并数据。
          </div>
        </div>
      )}

      {caps && canTransfer && (
        <div className="mt-2 flex flex-col gap-1.5">
          <div className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            线下迁移包（异地兜底）
          </div>
          <div className="flex items-center gap-1.5">
            <input
              readOnly
              value={outPath}
              placeholder="迁移包导出位置（留空用默认目录，可点右侧选择）"
              className="flex-1 min-w-0 h-7 px-2 rounded text-[11px] outline-none"
              style={inputStyle}
            />
            <button onClick={() => void pickExportPath()} className="h-7 px-2 rounded text-[11px] shrink-0"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}>
              选择
            </button>
            <input
              type="password"
              value={pass}
              onChange={(e) => setPass(e.target.value)}
              placeholder="迁移包口令（至少 8 位）"
              className="w-[180px] h-7 px-2 rounded text-[11px] outline-none"
              style={inputStyle}
            />
            <button onClick={() => setPass(randomPassphrase())} className="h-7 px-2 rounded text-[11px] shrink-0"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}>
              生成
            </button>
            <button onClick={() => void copyText(pass, '口令')} className="h-7 px-2 rounded text-[11px] shrink-0"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}>
              复制
            </button>
            <button
              onClick={() => void doExport()}
              disabled={busy === 'export'}
              className="h-7 px-2 rounded text-[11px] flex items-center gap-1 shrink-0"
              style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: busy === 'export' ? 0.6 : 1 }}
            >
              {busy === 'export' && <Loader2 size={10} className="animate-spin" />}
              导出迁移包
            </button>
          </div>
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            口令不随包传输，请通过线下或另一条可信渠道告知接收方。
          </div>
          {exported && (
            <div className="text-[10px] break-all" style={{ color: 'var(--text-tertiary)' }}>
              已导出：{exported.path}（sha256 {exported.packageHash.slice(0, 16)}…，{formatBytes(exported.size)}）
            </div>
          )}
        </div>
      )}

      {caps && !canTransfer && (
        <div className="mt-2 flex flex-col gap-1.5">
          <div className="flex items-center gap-2">
            <span className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
              待接收的转移请求
            </span>
            <button
              onClick={() => void useEnclaveOrgStore.getState().loadLanPending(orgId)}
              className="text-[10px] px-1.5 py-0.5 rounded flex items-center gap-1"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
            >
              <RefreshCw size={10} />
              刷新
            </button>
          </div>
          {lanPendingError && (
            <div className="text-[10px]" style={{ color: 'var(--status-danger)' }}>
              {lanPendingError}
            </div>
          )}
          {!lanPendingError && lanPending.length === 0 && (
            <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              暂无待确认请求（主机发起后会出现在这里）
            </div>
          )}
          {lanPending.map((t) => (
            <div key={t.id} className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              <span className="px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--accent)' }}>
                待确认
              </span>
              <span>{formatTime(t.createdAt)}</span>
              <span className="break-all">sha256 {t.packageHash.slice(0, 12)}…</span>
              <div className="flex-1" />
              <button
                onClick={() => void acceptPending(t)}
                disabled={accepting === t.id}
                className="h-6 px-2 rounded text-[10px] flex items-center gap-1"
                style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: accepting === t.id ? 0.6 : 1 }}
              >
                {accepting === t.id && <Loader2 size={10} className="animate-spin" />}
                接受并迁移
              </button>
            </div>
          ))}
        </div>
      )}

      {caps && !canTransfer && (
        <div className="mt-2 flex flex-col gap-1.5">
          <div className="text-[11px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            线下迁移包（异地兜底）
          </div>
          <div className="flex items-center gap-1.5">
            <input
              readOnly
              value={importPath}
              placeholder="迁移包路径（点击右侧选择）"
              className="flex-1 min-w-0 h-7 px-2 rounded text-[11px] outline-none"
              style={inputStyle}
            />
            <button onClick={() => void pickImportPath()} className="h-7 px-2 rounded text-[11px] shrink-0"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}>
              选择
            </button>
            <input
              type="password"
              value={importPass}
              onChange={(e) => setImportPass(e.target.value)}
              placeholder="迁移包口令"
              className="w-[180px] h-7 px-2 rounded text-[11px] outline-none"
              style={inputStyle}
            />
            <button onClick={() => void pastePassphrase()} className="h-7 px-2 rounded text-[11px] shrink-0"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}>
              粘贴
            </button>
            <button onClick={() => void copyText(importPass, '口令')} className="h-7 px-2 rounded text-[11px] shrink-0"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}>
              复制
            </button>
            <button
              onClick={() => void doImport()}
              disabled={busy === 'import'}
              className="h-7 px-2 rounded text-[11px] flex items-center gap-1 shrink-0"
              style={{ backgroundColor: 'var(--accent)', color: '#fff', opacity: busy === 'import' ? 0.6 : 1 }}
            >
              {busy === 'import' && <Loader2 size={10} className="animate-spin" />}
              导入并校验
            </button>
          </div>
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            导入成功后记录为「已校验」；待云端完成所有权转让后才会成为主机。
          </div>
        </div>
      )}

      {/* 记录 */}
      {transfers.length > 0 && (
        <div className="mt-2 flex flex-col gap-1">
          {transfers.slice(0, 5).map((t) => (
            <div key={t.id} className="flex items-center gap-2 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              <span
                className="px-1.5 py-0.5 rounded"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: PHASE_COLOR[t.phase] }}
              >
                {t.direction === 'out' ? '转出' : '接管'} · {t.phaseLabel}
              </span>
              <span>{formatTime(t.createdAt)}</span>
              {t.packageHash && <span className="break-all">sha256 {t.packageHash.slice(0, 12)}…</span>}
              <div className="flex-1" />
              {canTransfer && t.direction === 'out' && t.phase === 'migrating'
                && actionButton('标记已校验', () => void markVerified(t))}
              {canTransfer && t.direction === 'out' && t.phase === 'verified'
                && actionButton('标记云端已生效', () => void markCompleted(t))}
              {canTransfer && t.direction === 'out' && t.phase === 'completed'
                && actionButton('销毁旧机数据', () => void destroyOld(t), true)}
              {t.direction === 'in' && t.phase === 'verified' && (
                <span>等待云端完成所有权转让</span>
              )}
            </div>
          ))}
        </div>
      )}
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
  // 审计含主机本机路径等敏感信息：仅 owner / admin 可见（后端同样拒绝）
  const canAudit = !!caps?.actions.includes('org.admin');
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

      {/* 主机与连接（M7）：主机监听状态 / 成员发现与手动地址 */}
      <HostConnectionCard orgId={activeOrgId} />

      {/* 主机转移（M8）：迁移包导出导入 + 状态推进 + 旧机销毁 */}
      <TransferCard orgId={activeOrgId} />

      {/* 审计流水（仅 owner / admin；member 不渲染） */}
      {canAudit && (
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
      )}
    </div>
  );
}
