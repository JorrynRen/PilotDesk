/**
 * ModelManifestSettings — 设置 › 运行与策略 › 模型清单
 *
 * 工具（视频 / 图片）的模型差异与传输描述由外置清单决定，清单按三层解析：
 *   本地编辑（SQLite） → 市场缓存（文件） → 内置基线（随包）
 *
 * P2 只管理视频清单（`video-models.json`）；后续图片清单复用同一套命令与页面结构。
 * 编辑采用整份 JSON 文本，保存前由后端整份校验，通过才落库并立即生效。
 */

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { AlertCircle, FileJson, Loader2, Pencil, RefreshCw, RotateCcw, Trash2 } from 'lucide-react';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import { SettingsButton, SettingsSection } from './index';

type ManifestSource = 'builtin' | 'cache' | 'remote' | 'local';

interface RuleSummary {
  match: string;
  note?: string | null;
  responseMode: 'url' | 'binary';
  defaults: boolean;
  slots: string[];
  limits: {
    referenceImages: number | null;
    durations: number[];
    aspectRatios: string[];
    resolutions: string[];
  };
}

interface ManifestInfo {
  version: number;
  schemaVersion: number;
  updatedAt: string | null;
  ruleCount: number;
  source: ManifestSource;
  fetchedAt: number | null;
  cachePath: string;
  isBuiltin: boolean;
  edited: boolean;
  baseVersion: number | null;
  baseSource: string | null;
  localUpdatedAt: number | null;
  latestVersion: number | null;
  rules: RuleSummary[];
}

interface ManifestSourceText {
  text: string;
  source: ManifestSource;
  baseVersion: number | null;
  updatedAt: number | null;
}

const SOURCE_LABEL: Record<ManifestSource, string> = {
  builtin: '内置基线',
  cache: '市场缓存',
  remote: '已从市场更新',
  local: '本地编辑',
};

const SOURCE_COLOR: Record<ManifestSource, string> = {
  builtin: 'var(--text-secondary)',
  cache: 'var(--accent)',
  remote: 'var(--accent)',
  local: '#F59E0B',
};

function fmtTime(secs: number | null): string {
  if (!secs) return '—';
  return new Date(secs * 1000).toLocaleString();
}

function Badge({ text, color }: { text: string; color: string }) {
  return (
    <span
      className="inline-flex items-center rounded px-1.5 py-0.5 text-[11px]"
      style={{ color, border: `1px solid ${color}`, backgroundColor: 'transparent' }}
    >
      {text}
    </span>
  );
}

export function ModelManifestSettings() {
  const [info, setInfo] = useState<ManifestInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [resetting, setResetting] = useState<'local' | 'all' | null>(null);
  const [editorOpen, setEditorOpen] = useState(false);
  const [editorText, setEditorText] = useState('');
  const [editorError, setEditorError] = useState('');
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    try {
      setInfo(await invoke<ManifestInfo>('video_manifest_info'));
    } catch (e) {
      showToast(errorMessage(e), 'error');
      setInfo(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // effect 体内不允许同步 setState：推迟一个微任务，仍在同一帧内完成首次加载
    queueMicrotask(() => { void load(); });
  }, [load]);

  const refresh = async () => {
    setRefreshing(true);
    try {
      setInfo(await invoke<ManifestInfo>('refresh_video_manifest'));
      showToast('模型清单已刷新', 'success');
    } catch (e) {
      showToast(errorMessage(e), 'error');
    } finally {
      setRefreshing(false);
    }
  };

  const openEditor = async () => {
    try {
      const source = await invoke<ManifestSourceText>('video_manifest_source');
      setEditorText(source.text);
      setEditorError('');
      setEditorOpen(true);
    } catch (e) {
      showToast(errorMessage(e), 'error');
    }
  };

  const save = async () => {
    setSaving(true);
    setEditorError('');
    try {
      setInfo(await invoke<ManifestInfo>('save_video_manifest', { text: editorText }));
      setEditorOpen(false);
      showToast('本地清单已保存并立即生效', 'success');
    } catch (e) {
      // 校验失败时保持编辑器打开，错误原样展示（含具体规则/字段）
      setEditorError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const reset = async (scope: 'local' | 'all') => {
    const ok = await confirmDialog({
      title: scope === 'local' ? '放弃本地修改' : '恢复内置基线',
      message:
        scope === 'local'
          ? '将删除本地编辑，回到市场缓存或内置基线。此操作不可撤销。'
          : '将删除本地编辑与市场缓存，回落到随包内置清单。此操作不可撤销。',
      confirmText: '确认',
      danger: true,
    });
    if (!ok) return;
    setResetting(scope);
    try {
      setInfo(await invoke<ManifestInfo>('reset_video_manifest', { scope }));
      showToast(scope === 'local' ? '已放弃本地修改' : '已恢复内置基线', 'success');
    } catch (e) {
      showToast(errorMessage(e), 'error');
    } finally {
      setResetting(null);
    }
  };

  if (loading) {
    return (
      <SettingsSection title="模型清单" description="视频工具的模型差异与接口描述">
        <div className="flex items-center gap-2 py-6 text-sm" style={{ color: 'var(--text-secondary)' }}>
          <Loader2 size={14} className="animate-spin" />
          正在读取清单…
        </div>
      </SettingsSection>
    );
  }

  if (!info) {
    return (
      <SettingsSection title="模型清单" description="视频工具的模型差异与接口描述">
        <div className="flex items-center gap-2 py-6 text-sm" style={{ color: 'var(--text-secondary)' }}>
          <AlertCircle size={14} />
          清单状态不可用，请查看日志后重试。
        </div>
      </SettingsSection>
    );
  }

  const hasNewer = info.edited && info.latestVersion != null && info.baseVersion != null
    && info.latestVersion > info.baseVersion;

  return (
    <SettingsSection
      title="模型清单"
      description="视频工具的模型差异、端点与参数映射由这份清单决定；保存后立即生效，无需重启"
    >
      <div className="space-y-4">
        {/* 概要 */}
        <div
          className="rounded-lg p-3"
          style={{ backgroundColor: 'var(--bg-field)', border: '1px solid var(--border)' }}
        >
          <div className="flex flex-wrap items-center gap-3">
            <div className="flex items-center gap-2 text-sm" style={{ color: 'var(--text-primary)' }}>
              <FileJson size={15} />
              <span>video-models.json</span>
            </div>
            <Badge text={SOURCE_LABEL[info.source]} color={SOURCE_COLOR[info.source]} />
            {info.edited && <Badge text="含本地修改" color="#F59E0B" />}
          </div>
          <div className="mt-3 grid grid-cols-2 gap-x-4 gap-y-2 text-xs md:grid-cols-3">
            <div>
              <span style={{ color: 'var(--text-secondary)' }}>数据版本：</span>
              <span style={{ color: 'var(--text-primary)' }}>v{info.version}</span>
            </div>
            <div>
              <span style={{ color: 'var(--text-secondary)' }}>格式版本：</span>
              <span style={{ color: 'var(--text-primary)' }}>schema {info.schemaVersion}</span>
            </div>
            <div>
              <span style={{ color: 'var(--text-secondary)' }}>规则数：</span>
              <span style={{ color: 'var(--text-primary)' }}>{info.ruleCount}</span>
            </div>
            <div>
              <span style={{ color: 'var(--text-secondary)' }}>数据日期：</span>
              <span style={{ color: 'var(--text-primary)' }}>{info.updatedAt || '—'}</span>
            </div>
            <div>
              <span style={{ color: 'var(--text-secondary)' }}>最近拉取：</span>
              <span style={{ color: 'var(--text-primary)' }}>{fmtTime(info.fetchedAt)}</span>
            </div>
            <div>
              <span style={{ color: 'var(--text-secondary)' }}>本地编辑：</span>
              <span style={{ color: 'var(--text-primary)' }}>
                {info.edited
                  ? `基于 v${info.baseVersion ?? '?'}（${info.baseSource === 'remote' ? '市场' : '内置'}），${fmtTime(info.localUpdatedAt)}`
                  : '无'}
              </span>
            </div>
          </div>
          <div className="mt-2 break-all text-[11px]" style={{ color: 'var(--text-secondary)' }}>
            缓存：{info.cachePath}
          </div>
        </div>

        {hasNewer && (
          <div
            className="rounded-lg p-3 text-xs"
            style={{ backgroundColor: 'var(--status-warning-bg)', color: 'var(--status-warning)' }}
          >
            本地修改基于 v{info.baseVersion}，市场已有 v{info.latestVersion}。本地编辑优先，如需采用市场版本请先「放弃本地修改」再刷新。
          </div>
        )}

        {/* 操作 */}
        <div className="flex flex-wrap items-center gap-2">
          <SettingsButton variant="primary" onClick={refresh} disabled={refreshing} icon={
            refreshing ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />
          }>
            立即刷新
          </SettingsButton>
          <SettingsButton onClick={openEditor} icon={<Pencil size={13} />}>
            编辑
          </SettingsButton>
          <SettingsButton
            onClick={() => reset('local')}
            disabled={!info.edited || resetting !== null}
            icon={resetting === 'local' ? <Loader2 size={13} className="animate-spin" /> : <RotateCcw size={13} />}
          >
            放弃本地修改
          </SettingsButton>
          <SettingsButton
            variant="danger"
            onClick={() => reset('all')}
            disabled={resetting !== null}
            icon={resetting === 'all' ? <Loader2 size={13} className="animate-spin" /> : <Trash2 size={13} />}
          >
            恢复内置基线
          </SettingsButton>
          <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
            刷新失败不会影响当前清单
          </span>
        </div>

        {/* 编辑器 */}
        {editorOpen && (
          <div
            className="rounded-lg p-3"
            style={{ border: '1px solid var(--border)', backgroundColor: 'var(--bg-field)' }}
          >
            <div className="mb-2 flex items-center justify-between">
              <div className="text-sm" style={{ color: 'var(--text-primary)' }}>
                编辑整份清单
              </div>
              <div className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                保存前会做整份校验；失败不落库
              </div>
            </div>
            <textarea
              value={editorText}
              onChange={(e) => setEditorText(e.target.value)}
              spellCheck={false}
              className="w-full rounded p-2 font-mono text-xs"
              style={{
                height: 420,
                backgroundColor: 'var(--bg-primary)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
                resize: 'vertical',
              }}
            />
            {editorError && (
              <div className="mt-2 flex items-start gap-1.5 text-xs" style={{ color: 'var(--status-danger)' }}>
                <AlertCircle size={13} className="mt-0.5 shrink-0" />
                <span className="whitespace-pre-wrap break-all">{editorError}</span>
              </div>
            )}
            <div className="mt-3 flex items-center gap-2">
              <SettingsButton variant="primary" onClick={save} disabled={saving} icon={
                saving ? <Loader2 size={13} className="animate-spin" /> : undefined
              }>
                保存
              </SettingsButton>
              <SettingsButton onClick={() => setEditorOpen(false)} disabled={saving}>
                取消
              </SettingsButton>
            </div>
          </div>
        )}

        {/* 规则摘要 */}
        <div className="space-y-2">
          <div className="text-xs font-medium" style={{ color: 'var(--text-secondary)' }}>
            当前生效规则（{info.ruleCount}）
          </div>
          {info.rules.map((rule) => (
            <div
              key={rule.match}
              className="rounded-lg p-2.5"
              style={{ border: '1px solid var(--border)' }}
            >
              <div className="flex flex-wrap items-center gap-2">
                <code className="text-xs" style={{ color: 'var(--text-primary)' }}>{rule.match}</code>
                <Badge text={rule.responseMode === 'binary' ? '二进制' : 'URL'} color="var(--text-secondary)" />
                {rule.defaults && <Badge text="通用兜底" color="#F59E0B" />}
                <span className="text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                  {rule.slots.length > 0 ? `支持 ${rule.slots.join(' / ')}` : '仅提示词'}
                </span>
              </div>
              {rule.note && (
                <div className="mt-1 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                  {rule.note}
                </div>
              )}
              {(rule.limits.durations.length > 0 || rule.limits.aspectRatios.length > 0
                || rule.limits.resolutions.length > 0 || rule.limits.referenceImages != null) && (
                <div className="mt-1 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                  {rule.limits.durations.length > 0 && <span>时长 {rule.limits.durations.join('/')}s{'  '}</span>}
                  {rule.limits.aspectRatios.length > 0 && <span>画幅 {rule.limits.aspectRatios.join('/')}{'  '}</span>}
                  {rule.limits.resolutions.length > 0 && <span>分辨率 {rule.limits.resolutions.join('/')}{'  '}</span>}
                  {rule.limits.referenceImages != null && <span>参考图上限 {rule.limits.referenceImages} 张</span>}
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
    </SettingsSection>
  );
}
