/**
 * 飞地 API 共享。
 *
 * 共享源定义可分享的 Provider 与模型范围；路由令牌和 Provider 配置都通过
 * 统一分发记录与传讯卡片交付。Provider Key 不进入团队数据库或消息元数据。
 */

import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import {
  Activity,
  Archive,
  Ban,
  Check,
  CircleDollarSign,
  Cloud,
  Copy,
  Fingerprint,
  Gauge,
  KeyRound,
  Layers3,
  Loader2,
  Network,
  Pencil,
  Plus,
  RefreshCw,
  Send,
  Server,
  ShieldAlert,
  ShieldCheck,
  Trash2,
  Users,
  Wifi,
  WifiOff,
} from 'lucide-react';
import { ApiShareConfigImportDialog } from '../components/enclave/ApiShareConfigImportDialog';
import { ConversionDialogShell } from '../components/common/ConversionDialogShell';
import { Select } from '../components/common/Select';
import { WorkbenchAssetPage } from '../components/layout';
import { EnclaveNavBar } from '../components/enclave/EnclaveNavBar';
import { useNowSeconds } from '../hooks/useNowSeconds';
import { useAccountStore } from '../stores/accountStore';
import { useEnclaveApiShareStore } from '../stores/enclaveApiShareStore';
import { confirmDialog } from '../stores/confirmStore';
import { useEnclaveMessengerStore } from '../stores/enclaveMessengerStore';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { showToast } from '../utils/toast';
import type {
  ApiShareDistributionKind,
  ApiShareDistributionView,
  ApiShareMemberAccessView,
  ApiShareSourceView,
  ApiShareTokenGrantInput,
  ApiShareTokenView,
  ApiShareUsagePoint,
  ApiShareUsageQuery,
  IssuedApiShareToken,
  MessengerMemberView,
} from '../types/enclave';

type ModalKind =
  | 'create-source'
  | 'edit-source'
  | 'create-token'
  | 'edit-token'
  | 'distribute'
  | 'member-access'
  | null;

type ExpiryPreset = 'never' | '7' | '30' | '90';
type TokenExpiryPreset = 'current' | ExpiryPreset;
type UsageRange = 'today' | '7d' | '30d' | 'all';
type UsageMetric = 'requests' | 'successRate' | 'promptTokens' | 'completionTokens';
type DistributionEntry = 'token' | 'standalone';

const API_SHARE_SELECT_STYLE = {
  height: 29,
  padding: '0 8px',
  fontSize: 10,
};

const USAGE_RANGE_OPTIONS: Array<{ value: UsageRange; label: string }> = [
  { value: 'today', label: '今日' },
  { value: '7d', label: '近 7 天' },
  { value: '30d', label: '近 30 天' },
  { value: 'all', label: '全部时间' },
];

const USAGE_METRIC_OPTIONS: Array<{ value: UsageMetric; label: string }> = [
  { value: 'requests', label: '请求数' },
  { value: 'successRate', label: '成功率' },
  { value: 'promptTokens', label: '输入' },
  { value: 'completionTokens', label: '输出' },
];

function fmtTime(unixSec: number | null): string {
  if (!unixSec) return '从未';
  const date = new Date(unixSec * 1000);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function fmtCount(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 10_000) return `${(value / 10_000).toFixed(1)} 万`;
  return value.toLocaleString('zh-CN');
}

function usageMetricValue(point: ApiShareUsagePoint, metric: UsageMetric): number {
  if (metric === 'successRate') {
    return point.requests > 0 ? (point.successRequests / point.requests) * 100 : 0;
  }
  return point[metric];
}

function fmtMetricValue(value: number, metric: UsageMetric): string {
  if (metric === 'successRate') {
    return `${value.toFixed(value > 0 && value < 100 ? 1 : 0)}%`;
  }
  return fmtCount(Math.round(value));
}

function UsageTrendChart({
  trend,
  metric,
  granularity,
}: {
  trend: ApiShareUsagePoint[];
  metric: UsageMetric;
  granularity: 'day' | 'month';
}) {
  const width = 640;
  const height = 178;
  const padding = { left: 42, right: 14, top: 14, bottom: 28 };
  const plotWidth = width - padding.left - padding.right;
  const plotHeight = height - padding.top - padding.bottom;
  const values = trend.map((point) => usageMetricValue(point, metric));
  const rawMax = Math.max(...values, 0);
  const yMax = metric === 'successRate' ? 100 : Math.max(rawMax * 1.12, 1);
  const hasCalls = trend.some((point) => point.requests > 0);
  const metricLabel = USAGE_METRIC_OPTIONS.find((option) => option.value === metric)?.label
    ?? '请求数';
  const xAt = (index: number) => (
    trend.length <= 1
      ? padding.left + plotWidth / 2
      : padding.left + (index / (trend.length - 1)) * plotWidth
  );
  const yAt = (value: number) => padding.top + plotHeight - (value / yMax) * plotHeight;
  const linePath = values
    .map((value, index) => `${index === 0 ? 'M' : 'L'} ${xAt(index)} ${yAt(value)}`)
    .join(' ');
  const areaPath = values.length > 1
    ? `M ${xAt(0)} ${padding.top + plotHeight} ${values
      .map((value, index) => `L ${xAt(index)} ${yAt(value)}`)
      .join(' ')} L ${xAt(values.length - 1)} ${padding.top + plotHeight} Z`
    : '';
  const labelStep = Math.max(1, Math.ceil(trend.length / 5));
  const labelIndices = new Set(
    trend
      .map((_, index) => index)
      .filter((index) => index === 0 || index === trend.length - 1 || index % labelStep === 0),
  );

  return (
    <div className="enclave-api-usage-chart-surface">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={`${metricLabel}${granularity === 'month' ? '月度' : '每日'}走势`}
      >
        {[0, 0.25, 0.5, 0.75, 1].map((ratio) => {
          const value = yMax * ratio;
          const y = yAt(value);
          return (
            <g key={ratio}>
              <line
                x1={padding.left}
                x2={width - padding.right}
                y1={y}
                y2={y}
                className="enclave-api-usage-chart-grid"
              />
              <text
                x={padding.left - 7}
                y={y + 3}
                textAnchor="end"
                className="enclave-api-usage-chart-axis"
              >
                {fmtMetricValue(value, metric)}
              </text>
            </g>
          );
        })}
        {areaPath && <path d={areaPath} className="enclave-api-usage-chart-area" />}
        {linePath && <path d={linePath} className="enclave-api-usage-chart-line" />}
        {values.map((value, index) => {
          const point = trend[index];
          const rangeLabel = point.startDay === point.endDay
            ? point.startDay
            : `${point.startDay} 至 ${point.endDay}`;
          return (
            <g key={`${point.startDay}-${point.endDay}`}>
              <circle
                cx={xAt(index)}
                cy={yAt(value)}
                r={2.8}
                className="enclave-api-usage-chart-point"
              >
                <title>{`${rangeLabel} · ${metricLabel} ${fmtMetricValue(value, metric)}`}</title>
              </circle>
              {labelIndices.has(index) && (
                <text
                  x={xAt(index)}
                  y={height - 8}
                  textAnchor="middle"
                  className="enclave-api-usage-chart-axis"
                >
                  {point.label}
                </text>
              )}
            </g>
          );
        })}
      </svg>
      {!hasCalls && (
        <div className="enclave-api-usage-chart-empty">所选区间暂无调用</div>
      )}
    </div>
  );
}

function expiryAt(value: ExpiryPreset): number | null {
  return value === 'never'
    ? null
    : Math.floor(Date.now() / 1000) + Number(value) * 86_400;
}

function limitValue(value: string): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.max(0, Math.floor(parsed)) : 0;
}

function fmtLimit(value: number, unit: string): string {
  return value > 0 ? `${value} ${unit}` : `${unit}不限`;
}

function memberRoleLabel(role: string): string {
  if (role === 'owner') return '所有者';
  if (role === 'admin') return '管理员';
  return '成员';
}

function usageDayRange(range: UsageRange): Pick<ApiShareUsageQuery, 'fromDay' | 'toDay'> {
  if (range === 'all') return { fromDay: null, toDay: null };
  const days = range === 'today' ? 1 : range === '7d' ? 7 : 30;
  const end = new Date();
  const start = new Date(end);
  start.setUTCDate(start.getUTCDate() - (days - 1));
  return {
    fromDay: start.toISOString().slice(0, 10),
    toDay: end.toISOString().slice(0, 10),
  };
}

function exposedTokenModel(sourceName: string, model: string): string {
  return `[${sourceName.trim()}]${model.trim()}`;
}

function tokenGrantSelection(
  token: ApiShareTokenView,
  sources: ApiShareSourceView[],
): Record<string, string[]> {
  return token.sources.reduce<Record<string, string[]>>((selection, tokenSource) => {
    const source = sources.find((item) => item.id === tokenSource.sourceId);
    if (!source) return selection;
    const selected = source.models.filter((model) => (
      tokenSource.models.includes(exposedTokenModel(source.name, model))
    ));
    if (selected.length > 0) selection[source.id] = selected;
    return selection;
  }, {});
}

async function copyText(text: string, success = '已复制'): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    showToast(success, 'success');
  } catch {
    showToast('复制失败（剪贴板未授权）', 'error');
  }
}

function Badge({
  children,
  tone = 'neutral',
}: {
  children: ReactNode;
  tone?: 'neutral' | 'accent' | 'success' | 'warning' | 'danger';
}) {
  const styles = {
    neutral: ['var(--bg-field)', 'var(--text-tertiary)', 'var(--border)'],
    accent: ['var(--accent-light)', 'var(--accent)', 'color-mix(in srgb, var(--accent) 28%, var(--border))'],
    success: ['var(--status-success-bg)', 'var(--status-success)', 'color-mix(in srgb, var(--status-success) 28%, var(--border))'],
    warning: ['var(--status-warning-bg)', 'var(--status-warning)', 'color-mix(in srgb, var(--status-warning) 28%, var(--border))'],
    danger: ['var(--status-danger-bg)', 'var(--status-danger)', 'color-mix(in srgb, var(--status-danger) 28%, var(--border))'],
  }[tone];
  return (
    <span
      className="inline-flex h-5 shrink-0 items-center rounded px-1.5 text-[9px] font-medium leading-none"
      style={{ backgroundColor: styles[0], color: styles[1], border: `1px solid ${styles[2]}` }}
    >
      {children}
    </span>
  );
}

function MiniButton({
  children,
  icon,
  title,
  onClick,
  disabled,
  danger,
  active,
}: {
  children?: ReactNode;
  icon: ReactNode;
  title: string;
  onClick: () => void;
  disabled?: boolean;
  danger?: boolean;
  active?: boolean;
}) {
  return (
    <button
      type="button"
      className="enclave-api-mini-button"
      data-danger={danger || undefined}
      data-active={active || undefined}
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
    >
      {icon}
      {children && <span>{children}</span>}
    </button>
  );
}

function Section({
  title,
  hint,
  icon,
  action,
  children,
}: {
  title: string;
  hint?: string;
  icon: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="enclave-api-section">
      <header className="enclave-api-section-head">
        <span className="enclave-api-section-icon">{icon}</span>
        <div className="min-w-0 flex-1">
          <h2>{title}</h2>
          {hint && <p>{hint}</p>}
        </div>
        {action}
      </header>
      <div className="enclave-api-section-body">{children}</div>
    </section>
  );
}

function Switch({
  checked,
  disabled,
  title,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  title: string;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={title}
      title={title}
      className="enclave-api-switch"
      data-active={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span />
    </button>
  );
}

function SecretBlock({
  label,
  value,
  hint,
}: {
  label: string;
  value: string;
  hint: string;
}) {
  return (
    <div className="enclave-api-secret">
      <div className="flex min-w-0 items-center gap-2">
        <span className="shrink-0">{label}</span>
        <code className="min-w-0 flex-1 truncate" title={value}>{value}</code>
        <MiniButton
          icon={<Copy size={12} />}
          title={`复制${label}`}
          onClick={() => void copyText(value)}
        />
      </div>
      <p>{hint}</p>
    </div>
  );
}

function SourceRow({
  source,
  canManage,
  saving,
  onToggle,
  onEdit,
  onDelete,
}: {
  source: ApiShareSourceView;
  canManage: boolean;
  saving: boolean;
  onToggle: (enabled: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="enclave-api-source-row">
      <div className="enclave-api-source-main">
        <span className="enclave-api-list-icon">
          <Network size={15} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <strong className="truncate" title={source.name}>{source.name}</strong>
            <Badge tone="accent">双通道</Badge>
            <Badge tone={source.enabled ? 'success' : 'neutral'}>
              {source.enabled ? '已启用' : '已停用'}
            </Badge>
          </div>
          <div className="enclave-api-source-meta">
            <span className="truncate" title={source.providerId}>{source.providerId}</span>
            <span className="truncate" title={source.models.join(', ')}>
              {source.models.length > 0 ? source.models.join(' · ') : '跟随 Provider 模型'}
            </span>
          </div>
        </div>
      </div>

      <div className="enclave-api-source-stats">
        <span>{source.tokenCount} 令牌</span>
        <span>{source.distributionCount} 分发</span>
        <span>{fmtCount(source.requestCount)} 请求</span>
      </div>

      <div className="enclave-api-source-actions">
        {canManage ? (
          <>
            <Switch
              checked={source.enabled}
              disabled={saving}
              title={source.enabled ? '停用共享源' : '启用共享源'}
              onChange={onToggle}
            />
            <MiniButton icon={<Pencil size={12} />} title="编辑共享源" onClick={onEdit} />
            <MiniButton
              icon={<Trash2 size={12} />}
              title="删除共享源"
              danger
              disabled={saving}
              onClick={onDelete}
            />
          </>
        ) : (
          <span className="enclave-api-row-note">
            {source.enabled ? '共享范围由管理员维护' : '共享已停用'}
          </span>
        )}
      </div>
    </div>
  );
}

function distributionStatus(
  distribution: ApiShareDistributionView,
  now: number,
): { label: string; tone: 'neutral' | 'accent' | 'success' | 'warning' | 'danger'; available: boolean } {
  const expired = distribution.expiresAt !== null && distribution.expiresAt <= now;
  const tokenExpired = distribution.tokenExpiresAt !== null && distribution.tokenExpiresAt <= now;
  if (distribution.revokedAt) return { label: '已撤销', tone: 'danger', available: false };
  if (expired) return { label: '已过期', tone: 'neutral', available: false };
  if (!distribution.sourceEnabled) return { label: '来源已停用', tone: 'warning', available: false };
  if (distribution.kind === 'route_token' && !distribution.tokenEnabled) {
    return { label: '令牌已撤销', tone: 'danger', available: false };
  }
  if (distribution.kind === 'route_token' && tokenExpired) {
    return { label: '令牌已过期', tone: 'neutral', available: false };
  }
  if (distribution.tokenOwnerBlocked) {
    return { label: '令牌所有者已拉黑', tone: 'danger', available: false };
  }
  if (distribution.recipientBlocked) {
    return { label: '接收成员已拉黑', tone: 'danger', available: false };
  }
  if (distribution.archivedAt) return { label: '已归档', tone: 'neutral', available: true };
  if (distribution.appliedAt) return { label: '已添加', tone: 'success', available: true };
  if (distribution.openedAt) return { label: '已领取', tone: 'success', available: true };
  if (distribution.sentAt) return { label: '已送达', tone: 'accent', available: true };
  return { label: '待发送', tone: 'warning', available: true };
}

function DistributionRow({
  distribution,
  myUserId,
  canManage,
  now,
  saving,
  onRevoke,
  onArchive,
  onDelete,
  canDelete,
  onImport,
  onSend,
}: {
  distribution: ApiShareDistributionView;
  myUserId: string;
  canManage: boolean;
  now: number;
  saving: boolean;
  onRevoke: () => void;
  onArchive: () => void;
  onDelete: () => void;
  canDelete: boolean;
  onImport: () => void;
  onSend?: () => void;
}) {
  const status = distributionStatus(distribution, now);
  const isRecipient = distribution.recipientUserId === myUserId;
  const route = distribution.kind === 'route_token';
  return (
    <div className="enclave-api-distribution-row">
      <span className="enclave-api-list-icon">
        {route ? <KeyRound size={14} /> : <Server size={14} />}
      </span>
      <div className="enclave-api-distribution-body">
        <div className="enclave-api-distribution-heading">
          <strong className="enclave-api-distribution-source" title={distribution.sourceName}>
            {distribution.sourceName}
          </strong>
          <Badge tone={route ? 'accent' : 'success'}>
            {route ? '路由令牌' : 'Provider 配置'}
          </Badge>
          <Badge tone={status.tone}>{status.label}</Badge>
          <span className="enclave-api-distribution-created">
            创建于 {fmtTime(distribution.createdAt)}
          </span>
          <div className="enclave-api-distribution-actions">
            {onSend && !distribution.sentAt && status.available && (
              <MiniButton
                icon={<Send size={11} />}
                title="发送分发连接"
                disabled={saving}
                onClick={onSend}
              />
            )}
            {isRecipient && status.available && (
              <button type="button" className="enclave-api-primary-button" onClick={onImport}>
                <KeyRound size={12} />
                {distribution.appliedAt ? '再次添加' : '领取连接'}
              </button>
            )}
            {canManage && status.available && (
              <MiniButton
                icon={<Ban size={11} />}
                title="撤销分发"
                danger
                disabled={saving}
                onClick={onRevoke}
              />
            )}
            {canManage && !distribution.archivedAt && (
              <MiniButton
                icon={<Archive size={11} />}
                title="归档分发"
                disabled={saving}
                onClick={onArchive}
              />
            )}
            {canManage && canDelete && (
              <MiniButton
                icon={<Trash2 size={11} />}
                title="删除分发记录"
                danger
                disabled={saving}
                onClick={onDelete}
              />
            )}
          </div>
        </div>
        <div className="enclave-api-distribution-meta">
          <div className="enclave-api-distribution-people">
            <span className="enclave-api-distribution-recipient">
              接收人 {distribution.recipientName || distribution.recipientUserId}
            </span>
            {route && (
              <span className="enclave-api-distribution-token">
                令牌 {distribution.tokenName || '未命名'}
              </span>
            )}
          </div>
          <div className="enclave-api-distribution-detail">
            <span className="enclave-api-distribution-models" title={distribution.models.join(', ')}>
              {distribution.models.length > 0 ? distribution.models.join(' · ') : '跟随 Provider'}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}

function ModelPicker({
  models,
  selected,
  onToggle,
  hint = '未选时沿用共享源模型',
}: {
  models: string[];
  selected: string[];
  onToggle: (model: string) => void;
  hint?: string;
}) {
  return (
    <div className="enclave-api-field">
      <span>模型范围 <small>{hint}</small></span>
      {models.length === 0 ? (
        <div className="enclave-api-line-empty">该 Provider 未声明可选模型。</div>
      ) : (
        <div className="enclave-api-model-list">
          {models.map((model) => {
            const active = selected.includes(model);
            return (
              <button
                key={model}
                type="button"
                data-active={active}
                onClick={() => onToggle(model)}
                title={model}
              >
                {active && <Check size={11} />}
                <span className="truncate">{model}</span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

function TokenGrantPicker({
  sources,
  sourceIds,
  modelsBySource,
  onToggleSource,
  onToggleModel,
}: {
  sources: ApiShareSourceView[];
  sourceIds: string[];
  modelsBySource: Record<string, string[]>;
  onToggleSource: (source: ApiShareSourceView) => void;
  onToggleModel: (sourceId: string, model: string) => void;
}) {
  return (
    <div className="enclave-api-field">
      <span>
        共享源与模型
        <small>一枚令牌可跨多个来源，所有来源必须使用相同 API Format</small>
      </span>
      {sources.length === 0 ? (
        <div className="enclave-api-line-empty">请先创建并启用共享源。</div>
      ) : (
        <div className="enclave-api-grant-picker">
          {sources.map((source) => {
            const active = sourceIds.includes(source.id);
            const selected = modelsBySource[source.id] ?? [];
            return (
              <div
                key={source.id}
                className="enclave-api-grant-source"
                data-active={active || undefined}
                data-disabled={!source.enabled || undefined}
              >
                <button
                  type="button"
                  className="enclave-api-grant-source-head"
                  aria-pressed={active}
                  disabled={!source.enabled && !active}
                  onClick={() => onToggleSource(source)}
                >
                  <span className="enclave-api-grant-check">
                    {active && <Check size={11} />}
                  </span>
                  <span className="min-w-0 flex-1 truncate" title={source.name}>
                    {source.name}
                  </span>
                  <Badge tone={source.enabled ? 'success' : 'warning'}>
                    {source.enabled ? `${source.models.length} 模型` : '已停用'}
                  </Badge>
                </button>
                {active && (
                  <div className="enclave-api-model-list">
                    {source.models.map((model) => {
                      const modelActive = selected.includes(model);
                      return (
                        <button
                          key={model}
                          type="button"
                          data-active={modelActive}
                          onClick={() => onToggleModel(source.id, model)}
                          title={exposedTokenModel(source.name, model)}
                        >
                          {modelActive && <Check size={11} />}
                          <span className="truncate">{model}</span>
                          <small>[{source.name}]</small>
                        </button>
                      );
                    })}
                  </div>
                )}
                {active && selected.length === 0 && (
                  <p className="enclave-api-grant-warning">
                    至少为该来源选择一个模型，或取消勾选此来源。
                  </p>
                )}
                {active && !source.enabled && (
                  <p className="enclave-api-grant-warning">
                    该来源已停用，保存前请取消勾选或重新启用来源。
                  </p>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function MemberPicker({
  members,
  selected,
  blockedUserIds,
  onToggle,
}: {
  members: MessengerMemberView[];
  selected: string[];
  blockedUserIds: ReadonlySet<string>;
  onToggle: (userId: string) => void;
}) {
  return (
    <div className="enclave-api-field">
      <span>接收成员 <small>可多选；离线成员上线后仍可领取</small></span>
      {members.length === 0 ? (
        <div className="enclave-api-line-empty">当前组织没有其他可分发成员。</div>
      ) : (
        <div className="enclave-api-member-picker">
          {members.map((member) => {
            const blocked = blockedUserIds.has(member.userId);
            return (
              <label key={member.userId} data-blocked={blocked || undefined}>
                <input
                  type="checkbox"
                  checked={selected.includes(member.userId)}
                  disabled={blocked}
                  onChange={() => onToggle(member.userId)}
                />
                <span className="min-w-0 flex-1">
                  <strong className="truncate">
                    {member.displayName || member.email || member.userId}
                  </strong>
                  <small>
                    {member.email || member.role}
                    {blocked ? ' · 已拉黑' : member.online ? ' · 在线' : ' · 离线'}
                  </small>
                </span>
              </label>
            );
          })}
        </div>
      )}
    </div>
  );
}

function MemberAccessList({
  members,
  myUserId,
  saving,
  onToggle,
}: {
  members: ApiShareMemberAccessView[];
  myUserId: string;
  saving: boolean;
  onToggle: (member: ApiShareMemberAccessView) => void;
}) {
  if (members.length === 0) {
    return <div className="enclave-api-line-empty">组织成员清单尚未同步。</div>;
  }
  return (
    <div className="enclave-api-member-access-list">
      {members.map((member) => {
        const protectedMember = member.role === 'owner' || member.userId === myUserId;
        return (
          <div className="enclave-api-member-access-row" key={member.userId}>
            <span
              className="enclave-api-member-access-icon"
              data-blocked={member.blocked || undefined}
            >
              {member.blocked ? <ShieldAlert size={13} /> : <ShieldCheck size={13} />}
            </span>
            <div className="min-w-0 flex-1">
              <div className="enclave-api-member-access-title">
                <strong title={member.displayName}>{member.displayName}</strong>
                <Badge tone={member.role === 'owner' ? 'accent' : 'neutral'}>
                  {memberRoleLabel(member.role)}
                </Badge>
                {member.userId === myUserId && <Badge>当前账号</Badge>}
                <Badge tone={member.blocked ? 'danger' : 'success'}>
                  {member.blocked ? '已拉黑' : '正常'}
                </Badge>
              </div>
              <div className="enclave-api-member-access-meta">
                <span title={member.email}>{member.email || member.userId}</span>
                {member.blocked && member.blockedAt && (
                  <span>拉黑于 {fmtTime(member.blockedAt)}</span>
                )}
                {member.blocked && member.reason && (
                  <span title={member.reason}>{member.reason}</span>
                )}
              </div>
            </div>
            <MiniButton
              icon={member.blocked ? <ShieldCheck size={11} /> : <Ban size={11} />}
              title={member.blocked ? '解除成员黑名单' : '加入成员黑名单'}
              danger={!member.blocked}
              disabled={saving || protectedMember}
              onClick={() => onToggle(member)}
            />
          </div>
        );
      })}
    </div>
  );
}

function LimitFields({
  rpm,
  burst,
  concurrency,
  dailyRequests,
  dailyTokens,
  onChange,
}: {
  rpm: string;
  burst: string;
  concurrency: string;
  dailyRequests: string;
  dailyTokens: string;
  onChange: (field: 'rpm' | 'burst' | 'concurrency' | 'dailyRequests' | 'dailyTokens', value: string) => void;
}) {
  return (
    <div className="enclave-api-limit-grid">
      <label className="enclave-api-field">
        <span>每分钟请求 <small>RPM</small></span>
        <input
          className="pd-field enclave-api-input"
          inputMode="numeric"
          value={rpm}
          onChange={(event) => onChange('rpm', event.target.value)}
          placeholder="0 表示不限"
        />
      </label>
      <label className="enclave-api-field">
        <span>突发请求 <small>Burst</small></span>
        <input
          className="pd-field enclave-api-input"
          inputMode="numeric"
          value={burst}
          onChange={(event) => onChange('burst', event.target.value)}
          placeholder="0 表示不限"
        />
      </label>
      <label className="enclave-api-field">
        <span>最大并发</span>
        <input
          className="pd-field enclave-api-input"
          inputMode="numeric"
          value={concurrency}
          onChange={(event) => onChange('concurrency', event.target.value)}
          placeholder="0 表示不限"
        />
      </label>
      <label className="enclave-api-field">
        <span>每日请求</span>
        <input
          className="pd-field enclave-api-input"
          inputMode="numeric"
          value={dailyRequests}
          onChange={(event) => onChange('dailyRequests', event.target.value)}
          placeholder="0 表示不限"
        />
      </label>
      <label className="enclave-api-field enclave-api-limit-grid-wide">
        <span>每日 Token 软额度</span>
        <input
          className="pd-field enclave-api-input"
          inputMode="numeric"
          value={dailyTokens}
          onChange={(event) => onChange('dailyTokens', event.target.value)}
          placeholder="0 表示不限；超过后在下一次请求开始时拒绝"
        />
      </label>
    </div>
  );
}

export function EnclaveApiSharePage() {
  const activeOrgId = useEnclaveOrgStore((state) => state.activeOrgId);
  const orgs = useEnclaveOrgStore((state) => state.orgs);
  const orgLoading = useEnclaveOrgStore((state) => state.loading);
  const detail = useEnclaveOrgStore((state) => state.detail);
  const host = useEnclaveOrgStore((state) => state.host);
  const caps = useEnclaveOrgStore((state) => state.caps);
  const account = useAccountStore((state) => state.account);
  const {
    sources,
    providers,
    tokens,
    distributions,
    members,
    usage,
    access,
    loading,
    usageLoading,
    saving,
    error,
    load,
    loadUsage,
    createSource,
    updateSource,
    deleteSource,
    blockMember,
    unblockMember,
    createToken,
    updateToken,
    copyToken,
    revokeToken,
    archiveToken,
    deleteToken,
    createDistributions,
    markDistributionSent,
    revokeDistribution,
    archiveDistribution,
    deleteDistribution,
  } = useEnclaveApiShareStore();

  const myRole = orgs.find((item) => item.orgId === activeOrgId)?.myRole
    ?? detail?.org.myRole
    ?? '';
  const canManage = myRole === 'owner' || myRole === 'admin';
  const now = useNowSeconds();
  const myUserId = caps?.identity?.userId
    ?? detail?.members.find((member) => (
      !!account?.email && member.email.toLowerCase() === account.email.toLowerCase()
    ))?.userId
    ?? '';

  const [modal, setModal] = useState<ModalKind>(null);
  const [selectedSource, setSelectedSource] = useState<ApiShareSourceView | null>(null);
  const [selectedToken, setSelectedToken] = useState<ApiShareTokenView | null>(null);
  const [importDistribution, setImportDistribution] = useState<ApiShareDistributionView | null>(null);
  const [issuedToken, setIssuedToken] = useState<IssuedApiShareToken | null>(null);

  const [providerId, setProviderId] = useState('');
  const [sourceName, setSourceName] = useState('');
  const [sourceModels, setSourceModels] = useState<string[]>([]);
  const [sourceEnabled, setSourceEnabled] = useState(true);

  const [tokenName, setTokenName] = useState('');
  const [tokenExpiry, setTokenExpiry] = useState<TokenExpiryPreset>('30');
  const [tokenSourceIds, setTokenSourceIds] = useState<string[]>([]);
  const [tokenGrantModels, setTokenGrantModels] = useState<Record<string, string[]>>({});
  const [tokenRpm, setTokenRpm] = useState('60');
  const [tokenBurst, setTokenBurst] = useState('10');
  const [tokenConcurrency, setTokenConcurrency] = useState('4');
  const [tokenDailyRequests, setTokenDailyRequests] = useState('0');
  const [tokenDailyTokens, setTokenDailyTokens] = useState('0');

  const [distributionEntry, setDistributionEntry] = useState<DistributionEntry>('standalone');
  const [distributionKind, setDistributionKind] = useState<ApiShareDistributionKind>('route_token');
  const [distributionTokenId, setDistributionTokenId] = useState('');
  const [distributionRecipients, setDistributionRecipients] = useState<string[]>([]);
  const [distributionModels, setDistributionModels] = useState<string[]>([]);
  const [distributionExpiry, setDistributionExpiry] = useState<ExpiryPreset>('30');
  const [distributing, setDistributing] = useState(false);
  const [sendingDistributionId, setSendingDistributionId] = useState('');
  const [usageTokenId, setUsageTokenId] = useState('');
  const [usageRange, setUsageRange] = useState<UsageRange>('30d');
  const [usageMetric, setUsageMetric] = useState<UsageMetric>('requests');
  const [showArchivedTokens, setShowArchivedTokens] = useState(false);
  const [showArchivedDistributions, setShowArchivedDistributions] = useState(false);
  const selectedUsageTokenId = tokens.some((token) => token.id === usageTokenId)
    ? usageTokenId
    : '';
  const usageQuery = useMemo<ApiShareUsageQuery>(() => ({
    tokenId: selectedUsageTokenId || null,
    ...usageDayRange(usageRange),
  }), [usageRange, selectedUsageTokenId]);

  const messengerMembers = useEnclaveMessengerStore((state) => state.members);
  const loadingMessengerMembers = useEnclaveMessengerStore((state) => state.loadingMembers);

  const reload = useCallback(async () => {
    if (activeOrgId) await load(activeOrgId, canManage);
  }, [activeOrgId, canManage, load]);

  const refreshPage = useCallback(async () => {
    const next = await useEnclaveOrgStore.getState().ensureOrgs(true);
    if (!next) return;
    const orgStore = useEnclaveOrgStore.getState();
    const role = orgStore.orgs.find((item) => item.orgId === next)?.myRole
      ?? orgStore.detail?.org.myRole
      ?? '';
    const canManageNext = role === 'owner' || role === 'admin';
    await load(next, canManageNext);
    if (canManageNext) await loadUsage(next, usageQuery);
  }, [load, loadUsage, usageQuery]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      if (!cancelled) await useEnclaveOrgStore.getState().ensureOrgs();
    })();
    return () => {
      cancelled = true;
    };
  }, [account?.email]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (!activeOrgId || !canManage) return;
    void loadUsage(activeOrgId, usageQuery);
  }, [activeOrgId, canManage, loadUsage, usageQuery]);

  useEffect(() => {
    if (!activeOrgId) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ orgId: string; topic?: string; type?: string }>('enclave:changed', (event) => {
      const payload = event.payload;
      if (payload?.orgId !== activeOrgId || payload.type === 'realtime') return;
      if (payload.topic === 'api-share' || payload.topic === 'org') {
        void reload();
        if (canManage) void loadUsage(activeOrgId, usageQuery);
      }
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [activeOrgId, canManage, loadUsage, reload, usageQuery]);

  const selectedProvider = providers.find((item) => item.id === providerId) ?? null;
  const enabledSources = sources.filter((source) => source.enabled);
  const visibleDistributions = distributions.filter((item) => (
    showArchivedDistributions || !item.archivedAt
  ));
  const visibleTokens = tokens.filter((token) => (
    showArchivedTokens || !token.archivedAt
  ));
  const receivedDistributions = visibleDistributions.filter((item) => (
    myUserId ? item.recipientUserId === myUserId : false
  ));
  const distributableTokens = tokens.filter((token) => (
    token.enabled
    && (!token.expiresAt || token.expiresAt > now)
    && token.sources.some((source) => source.sourceEnabled)
  ));
  const activeDistributionTokens = distributableTokens;
  const selectedDistributionToken = activeDistributionTokens.find(
    (token) => token.id === distributionTokenId,
  );
  const blockedMemberIds = new Set(
    members.filter((member) => member.blocked).map((member) => member.userId),
  );
  const blockedMemberCount = blockedMemberIds.size;
  const selectableDistributionRecipients = distributionRecipients.filter(
    (userId) => !blockedMemberIds.has(userId),
  );
  const distributionModelOptions = distributionKind === 'route_token'
    ? selectedDistributionToken?.allowedModels ?? []
    : selectedSource?.models ?? [];
  const tokenGrants: ApiShareTokenGrantInput[] = tokenSourceIds.map((sourceId) => ({
    sourceId,
    models: tokenGrantModels[sourceId] ?? [],
  }));
  const tokenGrantsValid = tokenGrants.length > 0 && tokenGrants.every((grant) => {
    const source = sources.find((item) => item.id === grant.sourceId);
    return !!source?.enabled && (grant.models?.length ?? 0) > 0;
  });
  const usageRangeLabel = USAGE_RANGE_OPTIONS.find((option) => option.value === usageRange)?.label
    ?? '近 30 天';
  const usageTokenLabel = selectedUsageTokenId
    ? tokens.find((token) => token.id === selectedUsageTokenId)?.name ?? '指定令牌'
    : '全部令牌';
  const usageGranularityLabel = usage?.granularity === 'month'
    ? '按 UTC 自然月聚合'
    : '按 UTC 日统计';
  const canCreateToken = enabledSources.length > 0;
  const canCreateDistribution = canCreateToken || distributableTokens.length > 0;

  const openCreate = () => {
    const provider = providers.find((item) => item.apiKeySet) ?? providers[0] ?? null;
    setProviderId(provider?.id ?? '');
    setSourceName(provider?.name ?? '');
    setSourceModels(provider?.models ?? []);
    setSourceEnabled(true);
    setSelectedSource(null);
    setModal('create-source');
  };

  const openEdit = (source: ApiShareSourceView) => {
    setSelectedSource(source);
    setSourceName(source.name);
    setSourceModels(source.models);
    setSourceEnabled(source.enabled);
    setProviderId(source.providerId);
    setModal('edit-source');
  };

  const openCreateToken = () => {
    setSelectedSource(null);
    setSelectedToken(null);
    setTokenName('');
    setTokenExpiry('30');
    setTokenSourceIds(enabledSources.map((source) => source.id));
    setTokenGrantModels(Object.fromEntries(
      enabledSources.map((source) => [source.id, [...source.models]]),
    ));
    setTokenRpm('60');
    setTokenBurst('10');
    setTokenConcurrency('4');
    setTokenDailyRequests('0');
    setTokenDailyTokens('0');
    setIssuedToken(null);
    setModal('create-token');
  };

  const openEditToken = (token: ApiShareTokenView) => {
    setSelectedToken(token);
    setSelectedSource(null);
    setTokenName(token.name);
    setTokenExpiry(token.expiresAt && token.expiresAt > now ? 'current' : 'never');
    const selection = tokenGrantSelection(token, sources);
    setTokenSourceIds(Object.keys(selection));
    setTokenGrantModels(selection);
    setTokenRpm(String(token.rpmLimit));
    setTokenBurst(String(token.burstLimit));
    setTokenConcurrency(String(token.maxConcurrency));
    setTokenDailyRequests(String(token.dailyRequestLimit));
    setTokenDailyTokens(String(token.dailyTokenLimit));
    setModal('edit-token');
  };

  const openCreateDistribution = () => {
    const token = distributableTokens[0] ?? null;
    const source = enabledSources[0] ?? null;
    const kind: ApiShareDistributionKind = token ? 'route_token' : 'provider_config';
    setDistributionEntry('standalone');
    setSelectedSource(source);
    setDistributionKind(kind);
    setDistributionTokenId(token?.id ?? '');
    setDistributionRecipients([]);
    setDistributionModels(token?.allowedModels ?? source?.models ?? []);
    setDistributionExpiry('30');
    setModal('distribute');
    if (activeOrgId) void useEnclaveMessengerStore.getState().loadMembers(activeOrgId);
  };

  const openDistributeToken = (token: ApiShareTokenView) => {
    setDistributionEntry('token');
    setSelectedSource(null);
    setDistributionKind('route_token');
    setDistributionTokenId(token.id);
    setDistributionRecipients([]);
    setDistributionModels(token.allowedModels);
    setDistributionExpiry('30');
    setModal('distribute');
    if (activeOrgId) void useEnclaveMessengerStore.getState().loadMembers(activeOrgId);
  };

  const closeModal = () => {
    if (saving || distributing) return;
    setModal(null);
    setDistributionEntry('standalone');
    setSelectedSource(null);
    setSelectedToken(null);
    setIssuedToken(null);
  };

  const handleProviderChange = (id: string) => {
    const provider = providers.find((item) => item.id === id) ?? null;
    setProviderId(id);
    if (provider) {
      setSourceName(provider.name);
      setSourceModels(provider.models);
    }
  };

  const toggleSourceModel = (model: string) => {
    setSourceModels((current) => (
      current.includes(model) ? current.filter((item) => item !== model) : [...current, model]
    ));
  };

  const toggleTokenSource = (source: ApiShareSourceView) => {
    setTokenSourceIds((current) => (
      current.includes(source.id)
        ? current.filter((item) => item !== source.id)
        : [...current, source.id]
    ));
    setTokenGrantModels((current) => {
      if (current[source.id]) {
        const next = { ...current };
        delete next[source.id];
        return next;
      }
      return { ...current, [source.id]: [...source.models] };
    });
  };

  const toggleTokenGrantModel = (sourceId: string, model: string) => {
    setTokenGrantModels((current) => (
      {
        ...current,
        [sourceId]: (current[sourceId] ?? []).includes(model)
          ? (current[sourceId] ?? []).filter((item) => item !== model)
          : [...(current[sourceId] ?? []), model],
      }
    ));
  };

  const toggleDistributionModel = (model: string) => {
    setDistributionModels((current) => (
      current.includes(model) ? current.filter((item) => item !== model) : [...current, model]
    ));
  };

  const selectDistributionKind = (kind: ApiShareDistributionKind) => {
    setDistributionKind(kind);
    if (kind === 'provider_config') {
      const source = selectedSource ?? enabledSources[0] ?? null;
      setSelectedSource(source);
      setDistributionModels(source?.models ?? []);
      return;
    }
    const token = activeDistributionTokens.find((item) => item.id === distributionTokenId)
      ?? activeDistributionTokens[0];
    setDistributionTokenId(token?.id ?? '');
    setDistributionModels(token?.allowedModels ?? []);
  };

  const selectDistributionToken = (tokenId: string) => {
    const token = activeDistributionTokens.find((item) => item.id === tokenId);
    setDistributionTokenId(tokenId);
    setDistributionModels(token?.allowedModels ?? []);
  };

  const selectDistributionSource = (sourceId: string) => {
    const source = enabledSources.find((item) => item.id === sourceId) ?? null;
    setSelectedSource(source);
    setDistributionModels(source?.models ?? []);
  };

  const toggleDistributionRecipient = (userId: string) => {
    if (blockedMemberIds.has(userId)) return;
    setDistributionRecipients((current) => (
      current.includes(userId) ? current.filter((item) => item !== userId) : [...current, userId]
    ));
  };

  const submitSource = async () => {
    if (!activeOrgId || !selectedProvider) return;
    const ok = modal === 'edit-source' && selectedSource
      ? await updateSource(activeOrgId, selectedSource.id, {
          name: sourceName.trim(),
          models: sourceModels,
          enabled: sourceEnabled,
        })
      : await createSource(activeOrgId, {
          providerId: selectedProvider.id,
          name: sourceName.trim() || selectedProvider.name,
          models: sourceModels,
          enabled: sourceEnabled,
        });
    if (ok) {
      showToast(modal === 'edit-source' ? '共享源已更新' : 'API 共享已发布', 'success');
      closeModal();
      void reload();
    } else {
      showToast(useEnclaveApiShareStore.getState().error ?? '保存失败', 'error');
    }
  };

  const submitToken = async () => {
    if (!activeOrgId) return;
    if (!tokenGrantsValid) {
      showToast('请选择至少一个已启用来源，并为每个来源保留模型', 'warning');
      return;
    }
    const expiresAt = tokenExpiry === 'current'
      ? selectedToken?.expiresAt ?? null
      : expiryAt(tokenExpiry);
    const limits = {
      grants: tokenGrants,
      expiresAt,
      rpmLimit: limitValue(tokenRpm),
      burstLimit: limitValue(tokenBurst),
      maxConcurrency: limitValue(tokenConcurrency),
      dailyRequestLimit: limitValue(tokenDailyRequests),
      dailyTokenLimit: limitValue(tokenDailyTokens),
    };
    if (modal === 'edit-token' && selectedToken) {
      const ok = await updateToken(activeOrgId, selectedToken.id, {
        name: tokenName.trim(),
        ...limits,
        clearExpiry: tokenExpiry === 'never',
      });
      if (ok) {
        showToast('令牌额度已更新', 'success');
        closeModal();
        void reload();
      } else {
        showToast(useEnclaveApiShareStore.getState().error ?? '更新失败', 'error');
      }
      return;
    }
    const issued = await createToken(activeOrgId, {
      name: tokenName.trim(),
      ...limits,
    });
    if (issued) {
      setIssuedToken(issued);
      void reload();
    } else {
      showToast(useEnclaveApiShareStore.getState().error ?? '创建令牌失败', 'error');
    }
  };

  const submitDistributions = async () => {
    const recipients = selectableDistributionRecipients;
    if (
      !activeOrgId
      || (distributionKind === 'provider_config' && !selectedSource)
      || recipients.length === 0
    ) {
      if (distributionRecipients.length > 0) {
        showToast('已拉黑成员不能接收新的共享连接', 'warning');
      }
      return;
    }
    if (distributionKind === 'route_token' && !distributionTokenId) {
      showToast('请选择要分发的路由令牌', 'warning');
      return;
    }
    setDistributing(true);
    const created = await createDistributions(activeOrgId, {
      kind: distributionKind,
      sourceId: distributionKind === 'provider_config' ? selectedSource?.id ?? null : null,
      tokenId: distributionKind === 'route_token' ? distributionTokenId : null,
      recipientUserIds: recipients,
      models: distributionModels,
      expiresAt: expiryAt(distributionExpiry),
    });
    if (!created) {
      setDistributing(false);
      showToast(useEnclaveApiShareStore.getState().error ?? '创建分发失败', 'error');
      return;
    }

    let sent = 0;
    const distributionTitle = distributionKind === 'route_token'
      ? selectedDistributionToken?.name ?? '路由令牌'
      : selectedSource?.name ?? 'Provider 连接';
    for (const distribution of created.items) {
      const message = await useEnclaveMessengerStore.getState().send(activeOrgId, {
        peerId: distribution.recipientUserId,
        kind: 'api-share-distribution',
        content: `${distributionKind === 'route_token' ? '路由令牌' : 'Provider 连接'} · ${distributionTitle}`,
        metadata: JSON.stringify({
          refType: 'api-share-distribution',
          distributionId: distribution.id,
          kind: distribution.kind,
          sourceId: distribution.sourceId,
          sourceName: distribution.sourceName,
          models: distribution.models,
          expiresAt: distribution.expiresAt,
        }),
        sensitive: false,
      });
      if (message) {
        sent += 1;
        await markDistributionSent(activeOrgId, distribution.id, message.id);
      }
    }
    setDistributing(false);
    if (sent === created.items.length) {
      showToast(`已向 ${sent} 位成员发送共享连接`, 'success');
      closeModal();
    } else {
      showToast(`已创建 ${created.items.length} 条分发，成功发送 ${sent} 条`, 'warning');
    }
    void reload();
  };

  const copyTokenConnection = async (token: ApiShareTokenView) => {
    if (!activeOrgId) return;
    const secret = await copyToken(activeOrgId, token.id);
    if (!secret) {
      showToast(useEnclaveApiShareStore.getState().error ?? '读取令牌失败', 'error');
      return;
    }
    const connection = [
      `令牌名称: ${token.name}`,
      `共享来源: ${token.sources.map((source) => source.sourceName).join('、') || '来源已移除'}`,
      `API Base URL: ${secret.baseUrl}`,
      `API Token: ${secret.token}`,
      `API Format: ${token.apiFormat || 'openai'}`,
      `模型: ${token.allowedModels.length > 0 ? token.allowedModels.join(', ') : '全部共享模型'}`,
      `有效期: ${secret.expiresAt ? `有效至 ${fmtTime(secret.expiresAt)}` : '长期有效'}`,
    ].join('\n');
    await copyText(connection, `「${token.name}」完整连接信息已复制`);
    void reload();
  };

  const removeSource = async (source: ApiShareSourceView) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '删除 API 共享',
      message: `删除「${source.name}」后，该来源的模型授权与 Provider 配置分发会一并移除；跨来源令牌会保留其他来源授权。此操作不可撤销。`,
      confirmText: '删除',
      danger: true,
    });
    if (!ok) return;
    const success = await deleteSource(activeOrgId, source.id);
    if (success) showToast('共享源已删除', 'success');
    else showToast(useEnclaveApiShareStore.getState().error ?? '删除失败', 'error');
  };

  const toggleSource = async (source: ApiShareSourceView, enabled: boolean) => {
    if (!activeOrgId) return;
    const ok = await updateSource(activeOrgId, source.id, { enabled });
    if (!ok) showToast(useEnclaveApiShareStore.getState().error ?? '更新失败', 'error');
  };

  const toggleMemberBlock = async (member: ApiShareMemberAccessView) => {
    if (!activeOrgId) return;
    const blocking = !member.blocked;
    const ok = await confirmDialog({
      title: blocking ? '限制成员共享 API 访问' : '恢复成员共享 API 访问',
      message: blocking
        ? `将「${member.displayName}」加入组织 API 共享黑名单后，其名下令牌和全部分发凭据会立即无法调用；解封后恢复。已经导入的 Provider 配置无法远程收回。`
        : `恢复「${member.displayName}」后，其原有令牌与分发凭据会重新按当前来源和令牌状态生效。`,
      confirmText: blocking ? '加入黑名单' : '解除限制',
      danger: blocking,
    });
    if (!ok) return;
    const success = blocking
      ? await blockMember(activeOrgId, member.userId, '管理员限制其使用组织共享 API')
      : await unblockMember(activeOrgId, member.userId);
    if (success) {
      showToast(blocking ? '成员已加入 API 共享黑名单' : '成员 API 共享访问已恢复', 'success');
      void reload();
    } else {
      showToast(useEnclaveApiShareStore.getState().error ?? '更新成员状态失败', 'error');
    }
  };

  const revokeTokenConfirm = async (token: ApiShareTokenView) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '撤销令牌',
      message: `撤销「${token.name}」后，使用该令牌的客户端将立即无法访问。`,
      confirmText: '撤销',
      danger: true,
    });
    if (!ok) return;
    const success = await revokeToken(activeOrgId, token.id);
    if (success) showToast('令牌已撤销', 'success');
  };

  const archiveTokenConfirm = async (token: ApiShareTokenView) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '归档令牌',
      message: '归档后令牌及关联分发记录默认隐藏，但不会恢复令牌有效性。',
      confirmText: '归档',
    });
    if (!ok) return;
    const success = await archiveToken(activeOrgId, token.id);
    if (success) {
      showToast('令牌已归档', 'success');
      void reload();
    } else {
      showToast(useEnclaveApiShareStore.getState().error ?? '归档失败', 'error');
    }
  };

  const deleteTokenConfirm = async (token: ApiShareTokenView) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '删除令牌',
      message: `删除「${token.name}」后，相关路由令牌分发记录会一并删除，传讯消息中的连接也将失效。此操作不可恢复。`,
      confirmText: '删除',
      danger: true,
    });
    if (!ok) return;
    const success = await deleteToken(activeOrgId, token.id);
    if (success) {
      showToast('令牌及相关分发记录已删除', 'success');
      void reload();
    } else {
      showToast(useEnclaveApiShareStore.getState().error ?? '删除失败', 'error');
    }
  };

  const revokeDistributionConfirm = async (distribution: ApiShareDistributionView) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '撤销分发',
      message: '撤销后接收方将无法继续领取连接；已经导入到本机的 Provider 不会被删除。',
      confirmText: '撤销',
      danger: true,
    });
    if (!ok) return;
    const success = await revokeDistribution(activeOrgId, distribution.id);
    if (success) showToast('分发已撤销', 'success');
  };

  const archiveDistributionConfirm = async (distribution: ApiShareDistributionView) => {
    if (!activeOrgId) return;
    const ok = await confirmDialog({
      title: '归档分发',
      message: '归档后该记录默认隐藏，不会改变连接当前的有效性。',
      confirmText: '归档',
    });
    if (!ok) return;
    const success = await archiveDistribution(activeOrgId, distribution.id);
    if (success) {
      showToast('分发已归档', 'success');
      void reload();
    } else {
      showToast(useEnclaveApiShareStore.getState().error ?? '归档失败', 'error');
    }
  };

  const deleteDistributionConfirm = async (distribution: ApiShareDistributionView) => {
    if (!activeOrgId) return;
    const providerConfig = distribution.kind === 'provider_config';
    const ok = await confirmDialog({
      title: '删除分发记录',
      message: providerConfig
        ? '删除只清理共享记录，不会收回接收方已经导入的 Provider 配置和 API Key。传讯消息中的连接卡片将失效。'
        : '删除后该分发记录及领取入口将失效，传讯消息中的连接卡片也会失效。此操作不可恢复。',
      confirmText: '删除',
      danger: true,
    });
    if (!ok) return;
    const success = await deleteDistribution(activeOrgId, distribution.id);
    if (success) {
      showToast(providerConfig ? '分发记录已删除' : '分发记录已删除', 'success');
      void reload();
    } else {
      showToast(useEnclaveApiShareStore.getState().error ?? '删除失败', 'error');
    }
  };

  const sendDistribution = async (distribution: ApiShareDistributionView) => {
    if (!activeOrgId) return;
    setSendingDistributionId(distribution.id);
    try {
      const message = await useEnclaveMessengerStore.getState().send(activeOrgId, {
        peerId: distribution.recipientUserId,
        kind: 'api-share-distribution',
        content: `${distribution.kind === 'route_token' ? '路由令牌' : 'Provider 连接'} · ${distribution.sourceName}`,
        metadata: JSON.stringify({
          refType: 'api-share-distribution',
          distributionId: distribution.id,
          kind: distribution.kind,
          sourceId: distribution.sourceId,
          sourceName: distribution.sourceName,
          models: distribution.models,
          expiresAt: distribution.expiresAt,
        }),
        sensitive: false,
      });
      if (!message) {
        showToast(useEnclaveMessengerStore.getState().error ?? '发送分发连接失败', 'error');
        return;
      }
      const marked = await markDistributionSent(activeOrgId, distribution.id, message.id);
      showToast(marked ? '分发连接已发送' : '消息已发送，但分发状态更新失败', marked ? 'success' : 'warning');
      if (marked) void reload();
    } finally {
      setSendingDistributionId('');
    }
  };

  const gatewayHealthy = !!access?.enabled;
  const successRate = usage && usage.requests > 0
    ? (usage.successRequests / usage.requests) * 100
    : 0;

  return (
    <WorkbenchAssetPage contextLabel="飞地 · API 共享" activeAsset="enclave" frameless>
      <div className="enclave-api-page">
        <div className="enclave-api-nav-wrap">
          <EnclaveNavBar
            active="api-share"
            onRefresh={refreshPage}
            refreshing={loading || orgLoading}
          />
        </div>

        <main className="enclave-api-scroll pd-scroll-stable">
          {!activeOrgId && (
            <div className="enclave-api-empty">
              <Cloud size={28} />
              <strong>先选择组织</strong>
              <span>组织确定后，才能查看该团队的 API 共享与连接。</span>
            </div>
          )}

          {activeOrgId && (
            <>
              <section className="enclave-api-gateway">
                <div className="enclave-api-gateway-state">
                  <span className="enclave-api-gateway-icon" data-healthy={gatewayHealthy}>
                    {gatewayHealthy ? <Wifi size={18} /> : <WifiOff size={18} />}
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <h1>API 共享网关</h1>
                      <Badge tone={gatewayHealthy ? 'success' : access?.disabledReason ? 'warning' : 'neutral'}>
                        {gatewayHealthy ? '可连接' : '当前不可用'}
                      </Badge>
                    </div>
                    <p>
                      {gatewayHealthy
                        ? '路由令牌与 Provider 配置可按接收成员分别分发；共享连接对消费方按云端 Provider 识别。'
                        : access?.disabledReason || '正在等待主机网关状态。'}
                      {' '}共享配置、令牌与调用记录仅在本机存储；成员经局域网访问网关，不上平台云端。
                    </p>
                  </div>
                  {canManage && (
                    <button
                      type="button"
                      className="enclave-api-gateway-action"
                      onClick={() => setModal('member-access')}
                      title="管理组织成员对共享 API 的访问"
                    >
                      <ShieldAlert size={12} />
                      <span>成员黑名单</span>
                      <strong>{blockedMemberCount}</strong>
                    </button>
                  )}
                </div>

                <div className="enclave-api-gateway-facts">
                  <div>
                    <span>连接地址</span>
                    <code title={access?.gatewayUrl}>{access?.gatewayUrl || '未生成'}</code>
                  </div>
                  <div>
                    <span>端口</span>
                    <code>{access?.port || 47822}</code>
                  </div>
                  <div>
                    <span>主机</span>
                    <strong>{host?.hostName || (host?.role === 'host' ? '本机' : '未连接')}</strong>
                  </div>
                  <div>
                    <span>对外服务</span>
                    <strong style={{ color: gatewayHealthy ? 'var(--status-success)' : 'var(--status-warning)' }}>
                      {gatewayHealthy ? '正常' : '已关闭'}
                    </strong>
                  </div>
                </div>

                <div className="enclave-api-fingerprint">
                  <Fingerprint size={14} />
                  <span>证书指纹</span>
                  <code title={access?.certificateFingerprint}>
                    {access?.certificateFingerprint || '主机证书尚未生成'}
                  </code>
                  {access?.certificateFingerprint && (
                    <button
                      type="button"
                      title="复制证书指纹"
                      onClick={() => void copyText(access.certificateFingerprint, '证书指纹已复制')}
                    >
                      <Copy size={12} />
                    </button>
                  )}
                </div>
              </section>

              {error && (
                <div className="enclave-api-error">
                  <ShieldAlert size={14} />
                  <span className="min-w-0 flex-1">{error}</span>
                  <button type="button" onClick={() => void reload()}>
                    <RefreshCw size={12} />
                    重试
                  </button>
                </div>
              )}

              <div className="enclave-api-grid">
                <div className="enclave-api-column">
                  <Section
                    title="共享来源"
                    hint={canManage
                      ? '来源只定义可共享的 Provider 与模型范围；调用凭证和分发在各自区块管理。'
                      : '共享由管理员发布；可领取的连接会通过主动分发发送给你。'}
                    icon={<Layers3 size={14} />}
                    action={canManage ? (
                      <button type="button" className="enclave-api-primary-button" onClick={openCreate}>
                        <Plus size={12} />
                        发布共享
                      </button>
                    ) : undefined}
                  >
                    {loading && sources.length === 0 ? (
                      <div className="enclave-api-loading">
                        <Loader2 size={15} className="animate-spin" />
                        正在读取共享来源
                      </div>
                    ) : sources.length === 0 ? (
                      <div className="enclave-api-inline-empty">
                        <Cloud size={20} />
                        <div>
                          <strong>{canManage ? '还没有发布 API 共享' : '当前没有可用的 API 共享'}</strong>
                          <span>
                            {canManage
                              ? '从已配置 Key 的 Provider 发布一个共享来源。'
                              : '管理员启用共享后，会在这里提供可选来源。'}
                          </span>
                        </div>
                      </div>
                    ) : (
                      <div className="enclave-api-source-list">
                        {sources.map((source) => (
                          <SourceRow
                            key={source.id}
                            source={source}
                            canManage={canManage}
                            saving={saving}
                            onToggle={(enabled) => void toggleSource(source, enabled)}
                            onEdit={() => openEdit(source)}
                            onDelete={() => void removeSource(source)}
                          />
                        ))}
                      </div>
                    )}
                  </Section>

                  <Section
                    title="我收到的连接"
                    hint="管理员主动分发的路由令牌和 Provider 配置在这里领取；导入后保存为本机 Provider。"
                    icon={<ShieldCheck size={14} />}
                  >
                    {receivedDistributions.length === 0 ? (
                      <div className="enclave-api-line-empty">
                        暂未收到共享连接。管理员通过传讯主动分发后会显示在这里。
                      </div>
                    ) : (
                      <div className="enclave-api-source-list">
                        {receivedDistributions.map((distribution) => (
                          <DistributionRow
                            key={distribution.id}
                            distribution={distribution}
                            myUserId={myUserId}
                            canManage={false}
                            canDelete={false}
                            now={now}
                            saving={saving}
                            onRevoke={() => undefined}
                            onArchive={() => undefined}
                            onDelete={() => undefined}
                            onImport={() => setImportDistribution(distribution)}
                          />
                        ))}
                      </div>
                    )}
                  </Section>

                  {canManage && (
                    <Section
                      title="分发记录"
                      hint="每次分发保留来源、模型、接收人和令牌快照；不保存 Provider Key。"
                      icon={<Send size={14} />}
                      action={(
                        <div className="enclave-api-section-actions">
                          <button
                            type="button"
                            className="enclave-api-primary-button"
                            disabled={!canCreateDistribution}
                            title={canCreateDistribution ? '创建新的共享连接分发' : '请先发布共享来源或创建路由令牌'}
                            onClick={openCreateDistribution}
                          >
                            <Send size={12} />
                            分发连接
                          </button>
                          {distributions.some((distribution) => distribution.archivedAt) && (
                            <MiniButton
                              icon={<Archive size={11} />}
                              title={showArchivedDistributions ? '隐藏已归档分发' : '显示已归档分发'}
                              active={showArchivedDistributions}
                              onClick={() => setShowArchivedDistributions((value) => !value)}
                            />
                          )}
                        </div>
                      )}
                    >
                      {visibleDistributions.length === 0 ? (
                        <div className="enclave-api-line-empty">
                          {distributions.length === 0
                            ? '尚未分发连接。点击右上角“分发连接”选择路由令牌或 Provider 配置。'
                            : '当前没有未归档分发记录。'}
                        </div>
                      ) : (
                        <div className="enclave-api-source-list">
                          {visibleDistributions.map((distribution) => {
                            const token = distribution.tokenId
                              ? tokens.find((item) => item.id === distribution.tokenId)
                              : null;
                            const knownTokenUsage = token?.hasUsage === true;
                            return (
                              <DistributionRow
                                key={distribution.id}
                                distribution={distribution}
                                myUserId={myUserId}
                                canManage={canManage}
                                canDelete={distribution.kind === 'provider_config' || !knownTokenUsage}
                                now={now}
                                saving={saving || sendingDistributionId === distribution.id}
                                onRevoke={() => void revokeDistributionConfirm(distribution)}
                                onArchive={() => void archiveDistributionConfirm(distribution)}
                                onDelete={() => void deleteDistributionConfirm(distribution)}
                                onImport={() => setImportDistribution(distribution)}
                                onSend={() => void sendDistribution(distribution)}
                              />
                            );
                          })}
                        </div>
                      )}
                    </Section>
                  )}

                </div>

                <div className="enclave-api-column">
                  {canManage && (
                    <Section
                      title="路由令牌"
                      hint="仅管理员签发；额度独立作用于每枚令牌，0 表示该项不限制。"
                      icon={<KeyRound size={14} />}
                      action={(
                        <div className="enclave-api-section-actions">
                          <button
                            type="button"
                            className="enclave-api-primary-button"
                            disabled={!canCreateToken}
                            title={canCreateToken ? '创建可跨共享源使用的路由令牌' : '请先发布并启用共享来源'}
                            onClick={openCreateToken}
                          >
                            <Plus size={12} />
                            创建令牌
                          </button>
                          {tokens.some((token) => token.archivedAt) && (
                            <MiniButton
                              icon={<Archive size={11} />}
                              title={showArchivedTokens ? '隐藏已归档令牌' : '显示已归档令牌'}
                              active={showArchivedTokens}
                              onClick={() => setShowArchivedTokens((value) => !value)}
                            />
                          )}
                        </div>
                      )}
                    >
                      {visibleTokens.length === 0 ? (
                        <div className="enclave-api-line-empty">
                          {tokens.length === 0
                            ? '还没有路由令牌。点击右上角“创建令牌”，生成后可从令牌行主动分发。'
                            : '当前没有未归档令牌。'}
                        </div>
                      ) : (
                        <div className="enclave-api-token-list">
                          {visibleTokens.map((token) => {
                          const expired = !!token.expiresAt && token.expiresAt <= now;
                          const activeSources = token.sources.filter(
                            (source) => source.sourceEnabled,
                          );
                          const sourcePaused = token.sources.length === 0
                            || activeSources.length === 0;
                          const partiallyPaused = !sourcePaused
                            && activeSources.length < token.sources.length;
                          const tokenConfigured = !token.archivedAt
                            && token.enabled
                            && !expired
                            && !token.ownerBlocked
                            && !sourcePaused;
                          const callable = tokenConfigured && access?.enabled === true;
                          const status = token.archivedAt
                            ? { label: '已归档', tone: 'neutral' as const }
                            : !token.enabled
                              ? { label: '已撤销', tone: 'danger' as const }
                              : expired
                                ? { label: '已过期', tone: 'neutral' as const }
                                : sourcePaused
                                  ? { label: '来源停用', tone: 'warning' as const }
                                  : token.ownerBlocked
                                    ? { label: '成员已拉黑', tone: 'danger' as const }
                                  : access === null
                                    ? { label: '检查中', tone: 'neutral' as const }
                                    : !access.enabled
                                      ? { label: '服务已关闭', tone: 'warning' as const }
                                      : { label: '有效', tone: 'success' as const };
                          const canMutateToken = canManage || token.ownerUserId === myUserId;
                          const editable = canMutateToken && !token.archivedAt;
                          const deletable = canMutateToken
                            && !token.enabled
                            && !token.hasUsage;
                          return (
                            <div key={token.id} className="enclave-api-token-row">
                              <span className="enclave-api-token-icon"><KeyRound size={13} /></span>
                              <div className="min-w-0 flex-1">
                                <div className="flex min-w-0 flex-wrap items-center gap-1.5">
                                  <strong className="truncate" title={token.name}>{token.name}</strong>
                                  <div
                                    className="enclave-api-token-source"
                                    data-paused={sourcePaused || partiallyPaused || undefined}
                                    title={`共享来源：${token.sources
                                      .map((source) => source.sourceName)
                                      .join('、') || '来源已移除'}`}
                                  >
                                    <Network size={10} />
                                    <span>共享来源</span>
                                    <strong>
                                      {token.sources.length === 0
                                        ? '来源已移除'
                                        : token.sources.length === 1
                                          ? token.sources[0].sourceName
                                          : `${token.sources.length} 个`}
                                    </strong>
                                  </div>
                                  <Badge tone={status.tone}>{status.label}</Badge>
                                </div>
                                <div className="enclave-api-token-meta">
                                  <code>{token.tokenPrefix}</code>
                                  <span
                                    className="truncate"
                                    title={token.allowedModels.join(', ')}
                                  >
                                    {token.allowedModels.length > 0
                                      ? `${token.allowedModels.length} 个模型 · ${token.allowedModels.slice(0, 2).join(' · ')}`
                                      : '没有可用模型'}
                                  </span>
                                  <span>{fmtLimit(token.rpmLimit, 'RPM')}</span>
                                  <span>并发 {token.maxConcurrency || '不限'}</span>
                                  <span>今日 {token.todayRequests} 请求</span>
                                </div>
                              </div>
                              <div className="flex shrink-0 items-center gap-1">
                                {tokenConfigured && (
                                  <MiniButton
                                    icon={<Copy size={11} />}
                                    title="一键复制完整连接信息"
                                    onClick={() => void copyTokenConnection(token)}
                                  />
                                )}
                                {canManage && callable && activeSources.length > 0 && (
                                  <MiniButton
                                    icon={<Send size={11} />}
                                    title="分发令牌"
                                    onClick={() => openDistributeToken(token)}
                                  />
                                )}
                                {editable && token.enabled && (
                                  <MiniButton
                                    icon={<Gauge size={11} />}
                                    title="调整令牌额度"
                                    onClick={() => openEditToken(token)}
                                  />
                                )}
                                {editable && token.enabled && (
                                  <MiniButton
                                    icon={<Ban size={11} />}
                                    title="撤销令牌"
                                    danger
                                    disabled={saving}
                                    onClick={() => void revokeTokenConfirm(token)}
                                  />
                                )}
                                {editable && !token.enabled && (
                                  <MiniButton
                                    icon={<Archive size={11} />}
                                    title="归档令牌"
                                    disabled={saving}
                                    onClick={() => void archiveTokenConfirm(token)}
                                  />
                                )}
                                {deletable && (
                                  <MiniButton
                                    icon={<Trash2 size={11} />}
                                    title="删除令牌及相关分发记录"
                                    danger
                                    disabled={saving}
                                    onClick={() => void deleteTokenConfirm(token)}
                                  />
                                )}
                              </div>
                            </div>
                          );
                          })}
                        </div>
                      )}
                    </Section>
                  )}

                  {canManage && (
                    <Section
                      title="用量概览"
                      hint={`按组织汇总 · ${usageRangeLabel} · ${usageTokenLabel}；${usageGranularityLabel}，仅统计经飞地网关完成的调用。`}
                      icon={<Activity size={14} />}
                      action={usageLoading ? <Loader2 size={13} className="animate-spin" /> : undefined}
                    >
                      <div className="enclave-api-usage-filters">
                        <label className="enclave-api-field">
                          <span>令牌</span>
                          <Select
                            value={selectedUsageTokenId}
                            onChange={setUsageTokenId}
                            options={[
                              { value: '', label: '全部令牌' },
                              ...tokens.map((token) => ({
                                value: token.id,
                                label: `${token.name} · ${token.tokenPrefix}`,
                              })),
                            ]}
                            placeholder="全部令牌"
                            searchable={tokens.length > 8}
                            style={API_SHARE_SELECT_STYLE}
                          />
                        </label>
                        <label className="enclave-api-field">
                          <span>时间区间</span>
                          <Select
                            value={usageRange}
                            onChange={(value) => setUsageRange(value as UsageRange)}
                            options={USAGE_RANGE_OPTIONS}
                            style={API_SHARE_SELECT_STYLE}
                          />
                        </label>
                      </div>
                      <div
                        className="enclave-api-usage-grid"
                        role="tablist"
                        aria-label="用量走势维度"
                      >
                        {USAGE_METRIC_OPTIONS.map((option) => {
                          const value = !usage
                            ? 0
                            : option.value === 'requests'
                              ? usage.requests
                              : option.value === 'successRate'
                                ? successRate
                                : usage[option.value];
                          const active = usageMetric === option.value;
                          return (
                            <button
                              key={option.value}
                              type="button"
                              role="tab"
                              aria-selected={active}
                              aria-controls="enclave-api-usage-trend"
                              data-active={active || undefined}
                              onClick={() => setUsageMetric(option.value)}
                            >
                              <span>{option.label}</span>
                              <strong>{fmtMetricValue(value, option.value)}</strong>
                            </button>
                          );
                        })}
                      </div>
                      <div
                        id="enclave-api-usage-trend"
                        className="enclave-api-usage-chart"
                        role="tabpanel"
                      >
                        <header>
                          <strong>
                            {USAGE_METRIC_OPTIONS.find((option) => option.value === usageMetric)?.label}
                            走势
                          </strong>
                          <span>{usage?.granularity === 'month' ? '自然月' : 'UTC 日'}</span>
                        </header>
                        <UsageTrendChart
                          trend={usage?.trend ?? []}
                          metric={usageMetric}
                          granularity={usage?.granularity ?? 'day'}
                        />
                      </div>
                    </Section>
                  )}

                  <section className="enclave-api-note">
                    <CircleDollarSign size={15} />
                    <div>
                      <strong>识别与边界</strong>
                      <p>共享连接按云端 Provider 识别；本地模型直连仍按 internal 识别。Provider Key 不进入团队数据库。</p>
                    </div>
                  </section>
                </div>
              </div>
            </>
          )}
        </main>
      </div>

      {(modal === 'create-source' || modal === 'edit-source') && (
        <ConversionDialogShell
          icon={modal === 'edit-source' ? <Pencil size={17} /> : <Network size={17} />}
          kicker="共享来源"
          title={modal === 'edit-source' ? '编辑 API 共享' : '发布 API 共享'}
          description={modal === 'edit-source'
            ? `${selectedSource?.providerId ?? ''} · Provider、协议与上游地址始终读取当前本机配置。`
            : '选择本机已配置的 Provider，定义共享模型范围；令牌由管理员创建后主动分发。'}
          busy={saving}
          onClose={closeModal}
          footer={(
            <>
              <span className="flex-1" />
              <button type="button" className="pd-btn" onClick={closeModal} disabled={saving}>
                取消
              </button>
              <button
                type="button"
                className="pd-btn pd-btn-primary"
                onClick={() => void submitSource()}
                disabled={
                  saving
                  || !selectedProvider
                  || !selectedProvider.apiKeySet
                  || sourceModels.length === 0
                }
              >
                {saving ? <Loader2 size={12} className="animate-spin" /> : <Check size={12} />}
                {modal === 'edit-source' ? '保存修改' : '发布共享'}
              </button>
            </>
          )}
        >
          <div className="enclave-api-form-grid">
            <label className="enclave-api-field">
              <span>Provider</span>
              <Select
                value={providerId}
                onChange={handleProviderChange}
                disabled={modal === 'edit-source'}
                options={providers.map((provider) => ({
                  value: provider.id,
                  label: `${provider.name}${provider.apiKeySet ? '' : ' · 未配置 Key'}`,
                  disabled: !provider.apiKeySet,
                }))}
                placeholder="选择 Provider"
                style={API_SHARE_SELECT_STYLE}
              />
            </label>
            <label className="enclave-api-field">
              <span>共享名称</span>
              <input
                className="pd-field enclave-api-input"
                value={sourceName}
                onChange={(event) => setSourceName(event.target.value)}
                placeholder="团队内可见的名称"
              />
            </label>
          </div>
          {selectedProvider && !selectedProvider.apiKeySet && (
            <div className="enclave-api-dialog-callout is-warning">
              <ShieldAlert size={14} />
              <span>该 Provider 尚未配置 API Key，请先到「设置 · 集成管理」完成配置。</span>
            </div>
          )}
          <ModelPicker
            models={selectedProvider?.models ?? []}
            selected={sourceModels}
            onToggle={toggleSourceModel}
            hint="至少选择一个模型，作为稳定的共享范围"
          />
          {selectedProvider && sourceModels.length === 0 && (
            <div className="enclave-api-dialog-callout is-warning">
              <ShieldAlert size={14} />
              <span>共享源至少需要保留一个模型，否则接收方无法稳定识别可用模型。</span>
            </div>
          )}
          <div className="enclave-api-toggle-line">
            <div>
              <strong>发布后立即启用</strong>
              <span>停用会让现有令牌暂时不可调用，也会暂停连接领取。</span>
            </div>
            <Switch checked={sourceEnabled} title="发布后立即启用" onChange={setSourceEnabled} />
          </div>
        </ConversionDialogShell>
      )}

      {(modal === 'create-token' || modal === 'edit-token')
        && (modal === 'create-token' || !!selectedToken) && (
        <ConversionDialogShell
          icon={<KeyRound size={17} />}
          kicker="路由令牌"
          title={issuedToken ? '令牌已生成' : modal === 'edit-token' ? '调整令牌额度' : '创建路由令牌'}
          description={issuedToken
            ? '下一步将连接主动分发给成员；如泄露请立即撤销。'
            : modal === 'edit-token'
              ? '调整该令牌可访问的共享源、模型和独立额度；已分发令牌不能修改名称。'
              : '已预选全部启用的共享源；可取消来源或按模型收窄授权范围。'}
          busy={saving}
          onClose={issuedToken ? () => setIssuedToken(null) : closeModal}
          footer={issuedToken ? (
            <>
              <span className="flex-1" />
              <button
                type="button"
                className="pd-btn"
                onClick={() => {
                  setIssuedToken(null);
                  closeModal();
                }}
              >
                稍后分发
              </button>
              <button
                type="button"
                className="pd-btn pd-btn-primary"
                onClick={() => {
                  const issuedView = issuedToken?.view;
                  setIssuedToken(null);
                  if (issuedView) openDistributeToken(issuedView);
                }}
              >
                <Send size={12} />
                立即分发
              </button>
            </>
          ) : (
            <>
              <span className="flex-1" />
              <button type="button" className="pd-btn" onClick={closeModal} disabled={saving}>
                取消
              </button>
              <button
                type="button"
                className="pd-btn pd-btn-primary"
                onClick={() => void submitToken()}
                disabled={saving || !tokenGrantsValid}
              >
                {saving ? <Loader2 size={12} className="animate-spin" /> : <KeyRound size={12} />}
                {modal === 'edit-token' ? '保存额度' : '生成令牌'}
              </button>
            </>
          )}
        >
          {issuedToken ? (
            <>
              <SecretBlock
                label="API Token"
                value={issuedToken.token}
                hint="放入客户端 Authorization: Bearer 头中使用。"
              />
              <SecretBlock
                label="API Base URL"
                value={issuedToken.baseUrl || access?.gatewayUrl || ''}
                hint="请求经飞地 HTTPS 网关转发，不暴露 Provider Key。"
              />
              <div className="enclave-api-dialog-callout">
                <ShieldAlert size={14} />
                <span>
                  推荐使用「立即分发」主动发送连接；路由令牌只能由管理员签发，
                  原始连接信息可在令牌列表再次复制。
                </span>
              </div>
            </>
          ) : (
            <>
              <div className="enclave-api-form-grid">
                <label className="enclave-api-field">
                  <span>令牌名称</span>
                  <input
                    className="pd-field enclave-api-input"
                    value={tokenName}
                    onChange={(event) => setTokenName(event.target.value)}
                    placeholder="例如：本机脚本 / 测试环境"
                  />
                </label>
                <label className="enclave-api-field">
                  <span>有效期</span>
                  <Select
                    value={tokenExpiry}
                    onChange={(value) => setTokenExpiry(value as TokenExpiryPreset)}
                    options={[
                      ...(modal === 'edit-token' && selectedToken?.expiresAt
                        ? [{
                            value: 'current',
                            label: `保持当前有效期（${fmtTime(selectedToken.expiresAt)}）`,
                          }]
                        : []),
                      { value: 'never', label: '长期有效' },
                      { value: '7', label: '7 天' },
                      { value: '30', label: '30 天' },
                      { value: '90', label: '90 天' },
                    ]}
                    style={API_SHARE_SELECT_STYLE}
                  />
                </label>
              </div>
              <TokenGrantPicker
                sources={sources}
                sourceIds={tokenSourceIds}
                modelsBySource={tokenGrantModels}
                onToggleSource={toggleTokenSource}
                onToggleModel={toggleTokenGrantModel}
              />
              <LimitFields
                rpm={tokenRpm}
                burst={tokenBurst}
                concurrency={tokenConcurrency}
                dailyRequests={tokenDailyRequests}
                dailyTokens={tokenDailyTokens}
                onChange={(field, value) => {
                  if (field === 'rpm') setTokenRpm(value);
                  else if (field === 'burst') setTokenBurst(value);
                  else if (field === 'concurrency') setTokenConcurrency(value);
                  else if (field === 'dailyRequests') setTokenDailyRequests(value);
                  else setTokenDailyTokens(value);
                }}
              />
              <div className="enclave-api-dialog-callout">
                <Gauge size={14} />
                <span>RPM 与 Burst 控制短时速率，最大并发控制同时请求；每日 Token 是软额度，在请求开始时检查。</span>
              </div>
            </>
          )}
        </ConversionDialogShell>
      )}

      {modal === 'distribute' && (
        distributionEntry === 'token' ? !!selectedDistributionToken : true
      ) && (
        <ConversionDialogShell
          icon={<Send size={17} />}
          kicker="连接分发"
          title="分发共享连接"
          description="创建分发记录后通过传讯发送；接收人可直接领取并添加为本机 Provider。"
          busy={distributing}
          onClose={closeModal}
          footer={(
            <>
              <span className="flex-1" />
              <button type="button" className="pd-btn" onClick={closeModal} disabled={distributing}>
                取消
              </button>
              <button
                type="button"
                className="pd-btn pd-btn-primary"
                onClick={() => void submitDistributions()}
                disabled={
                  distributing
                  || selectableDistributionRecipients.length === 0
                  || (distributionKind === 'route_token' && !selectedDistributionToken)
                  || (distributionKind === 'provider_config' && !selectedSource)
                }
              >
                {distributing ? <Loader2 size={12} className="animate-spin" /> : <Send size={12} />}
                创建并发送
              </button>
            </>
          )}
        >
          <div className="enclave-api-source-summary">
            <span
              className="enclave-api-source-mark"
              data-mode={distributionKind === 'route_token' ? 'route' : 'shared'}
            >
              {distributionKind === 'route_token'
                ? <KeyRound size={14} />
                : <Server size={14} />}
            </span>
            <div>
              <strong>
                {distributionKind === 'route_token'
                  ? selectedDistributionToken?.name ?? '路由令牌'
                  : selectedSource?.name ?? '共享来源'}
              </strong>
              <span>
                {distributionKind === 'route_token'
                  ? selectedDistributionToken?.sources
                    .map((source) => source.sourceName)
                    .join(' · ') || '没有可用来源'
                  : selectedSource?.models.join(' · ') || '跟随 Provider 模型'}
              </span>
            </div>
          </div>
          {distributionEntry === 'standalone' && (
            <div className="enclave-api-field">
              <span>分发方式</span>
              <div className="enclave-api-segmented">
                <button
                  type="button"
                  data-active={distributionKind === 'route_token'}
                  onClick={() => selectDistributionKind('route_token')}
                >
                  路由令牌
                </button>
                <button
                  type="button"
                  data-active={distributionKind === 'provider_config'}
                  onClick={() => selectDistributionKind('provider_config')}
                >
                  Provider 配置
                </button>
              </div>
            </div>
          )}
          {distributionKind === 'route_token' ? (
            activeDistributionTokens.length === 0 ? (
              <div className="enclave-api-dialog-callout is-warning">
                <ShieldAlert size={14} />
                <span>
                  {distributionEntry === 'token'
                    ? '当前令牌已没有可用共享来源，请先恢复来源或调整令牌授权。'
                    : '当前没有可用路由令牌。请先创建令牌，或改为分发 Provider 配置。'}
                </span>
              </div>
            ) : (
              <label className="enclave-api-field">
                <span>选择令牌 <small>同一令牌可分发给多位成员，共享额度</small></span>
                <Select
                  value={distributionTokenId}
                  onChange={selectDistributionToken}
                  options={activeDistributionTokens.map((token) => ({
                    value: token.id,
                    label: `${token.name} · ${token.tokenPrefix} · ${fmtLimit(token.rpmLimit, 'RPM')}`,
                  }))}
                  placeholder="选择路由令牌"
                  style={API_SHARE_SELECT_STYLE}
                />
              </label>
            )
          ) : distributionEntry === 'standalone' ? (
            enabledSources.length === 0 ? (
              <div className="enclave-api-dialog-callout is-warning">
                <ShieldAlert size={14} />
                <span>当前没有已启用的共享来源，请先发布或启用共享来源。</span>
              </div>
            ) : (
              <>
                <label className="enclave-api-field">
                  <span>选择共享来源</span>
                  <Select
                    value={selectedSource?.id ?? ''}
                    onChange={selectDistributionSource}
                    options={enabledSources.map((source) => ({
                      value: source.id,
                      label: `${source.name} · ${source.models.length} 个模型`,
                    }))}
                    placeholder="选择共享来源"
                    style={API_SHARE_SELECT_STYLE}
                  />
                </label>
                <div className="enclave-api-dialog-callout">
                  <ShieldCheck size={14} />
                  <span>Provider Key 只在接收方领取时由团队主机临时读取，不写入消息与团队数据库。</span>
                </div>
              </>
            )
          ) : (
            <div className="enclave-api-dialog-callout">
              <ShieldCheck size={14} />
              <span>Provider Key 只在接收方领取时由团队主机临时读取，不写入消息与团队数据库。</span>
            </div>
          )}
          <div className="enclave-api-form-grid">
            <label className="enclave-api-field">
              <span>领取有效期</span>
              <Select
                value={distributionExpiry}
                onChange={(value) => setDistributionExpiry(value as ExpiryPreset)}
                options={[
                  { value: '7', label: '7 天' },
                  { value: '30', label: '30 天' },
                  { value: '90', label: '90 天' },
                ]}
                style={API_SHARE_SELECT_STYLE}
              />
            </label>
            <div className="enclave-api-field">
              <span>已选接收人</span>
              <div className="enclave-api-selection-count">
                <Users size={12} />
                {distributionRecipients.length} 位成员
              </div>
            </div>
          </div>
          <MemberPicker
            members={messengerMembers}
            selected={distributionRecipients}
            blockedUserIds={blockedMemberIds}
            onToggle={toggleDistributionRecipient}
          />
          {loadingMessengerMembers && (
            <div className="enclave-api-caption">正在读取组织成员…</div>
          )}
          <ModelPicker
            models={distributionModelOptions}
            selected={distributionModels}
            onToggle={toggleDistributionModel}
          />
          <div className="enclave-api-dialog-callout">
            <ShieldAlert size={14} />
            <span>撤销分发只阻止后续领取；接收方已经保存到本机的 Provider 不会被删除。</span>
          </div>
        </ConversionDialogShell>
      )}

      {modal === 'member-access' && (
        <ConversionDialogShell
          icon={<ShieldAlert size={17} />}
          kicker="网关访问控制"
          title="API 共享成员黑名单"
          description={`按组织成员生效，不依赖分发记录。当前已限制 ${blockedMemberCount} 位成员。`}
          busy={saving}
          onClose={closeModal}
          footer={(
            <button type="button" className="pd-btn pd-btn-primary ml-auto" onClick={closeModal}>
              完成
            </button>
          )}
        >
          <div className="enclave-api-dialog-callout">
            <ShieldAlert size={14} />
            <span>被限制成员名下的路由令牌和全部分发凭据会立即失效；解除后按原有状态恢复。</span>
          </div>
          <MemberAccessList
            members={members}
            myUserId={myUserId}
            saving={saving}
            onToggle={(member) => void toggleMemberBlock(member)}
          />
          <p className="enclave-api-member-access-note">
            已导入本机的 Provider 配置包含原始 Key，无法远程收回；路由连接仍由网关持续校验。
          </p>
        </ConversionDialogShell>
      )}

      {activeOrgId && importDistribution && (
        <ApiShareConfigImportDialog
          orgId={activeOrgId}
          distribution={importDistribution}
          onClose={() => setImportDistribution(null)}
          onImported={() => {
            setImportDistribution(null);
            void reload();
          }}
        />
      )}
    </WorkbenchAssetPage>
  );
}
