/**
 * KbModelSettings — 设置 › 知识库
 *
 * 指定知识库用的模型（投喂后的整理、以及 AI 生成）。这是**可选覆盖项**：
 * 不指定时后端回落到"第一个 OpenAI 兼容且配了 Key 的提供商及其首个模型"，
 * 所以这里重点不是"让用户必须选"，而是把"当前到底在用哪个模型"讲清楚。
 */

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Library, Loader2, Info } from 'lucide-react';
import { useKbModelStore, hasKbModelOverride, type KbModelConfig } from '../../stores/kbModelStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { Select } from '../common/Select';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

interface KbModelInfo {
  providerName: string;
  model: string;
  fromFallback: boolean;
}

export function KbModelSettings() {
  const { providers, fetchProviders } = useApiProviderStore();
  const { config, loaded, load, save } = useKbModelStore();

  const [providerId, setProviderId] = useState('');
  const [model, setModel] = useState('');
  const [saving, setSaving] = useState(false);
  const [effective, setEffective] = useState<KbModelInfo | null>(null);
  const [probing, setProbing] = useState(true);

  /**
   * 自动沉淀开关（app_settings `kb_auto_enabled`，见 `commands/knowledge.rs::KB_AUTO_ENABLED_SETTING`）。
   * **默认开启**；关闭后完全不触发，也就不额外花 token。
   * 目标库不再预设：整理时把**全部知识库**的定义交给模型，由模型逐条选库并填该库专属属性。
   */
  const [autoEnabled, setAutoEnabled] = useState(true);
  const [loadingAuto, setLoadingAuto] = useState(true);

  /** 当前实际生效的模型（不含 api_key；由后端解析后回传） */
  const refreshEffective = useCallback(async () => {
    setProbing(true);
    try {
      setEffective(await invoke<KbModelInfo | null>('kb_effective_model'));
    } catch {
      setEffective(null);
    } finally {
      setProbing(false);
    }
  }, []);

  useEffect(() => {
    void fetchProviders();
    void load();
    // 不在 effect 里同步调用：`refreshEffective` 开头就 setProbing(true)，
    // 属于"effect 体内同步 setState"（`react-hooks/set-state-in-effect` 判为级联渲染）。
    // 推到微任务 —— 同一个任务、早于绘制，行为一致。
    void Promise.resolve().then(() => refreshEffective());
  }, [fetchProviders, load, refreshEffective]);

  // 自动沉淀开关（读不到就保持默认开启，不让设置页整体失败）
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const raw = await invoke<string | null>('get_app_setting', { key: 'kb_auto_enabled' });
        if (!alive) return;
        // 缺省 / 非 "0"/"false" = 开启（与后端 `kb_auto_enabled` 同一判据）
        const t = (raw ?? '').trim();
        setAutoEnabled(!(t === '0' || t.toLowerCase() === 'false'));
      } catch {
        /* 忽略：保持默认开启 */
      } finally {
        if (alive) setLoadingAuto(false);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  const toggleAuto = async () => {
    const next = !autoEnabled;
    setAutoEnabled(next);
    try {
      await invoke('set_app_setting', { key: 'kb_auto_enabled', value: next ? '1' : '0' });
      showToast(next ? '已开启自动沉淀' : '已关闭自动沉淀', 'success');
    } catch (e) {
      setAutoEnabled(!next); // 回滚
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    }
  };

  // 配置读回后同步到表单（只在首次加载完成时灌入，避免覆盖用户正在选的草稿）。
  //
  // 用「渲染期修正」而不是 effect：在 effect 里同步 setState 会多一轮级联渲染
  // （`react-hooks/set-state-in-effect`），而"外部配置到位就把草稿对齐"正是 React 推荐的
  // adjust-during-render 场景。判据用配置对象本身，用户改草稿不会触发重灌。
  const [syncedConfig, setSyncedConfig] = useState<KbModelConfig | null>(null);
  if (loaded && syncedConfig !== config) {
    setSyncedConfig(config);
    setProviderId(config.providerId);
    setModel(config.model);
  }

  const provider = providers.find((p) => p.id === providerId);
  const models = provider?.models ?? [];
  const anthropic = (provider?.apiFormat ?? '').toLowerCase().includes('anthropic');
  const dirty = providerId !== config.providerId || model !== config.model;

  const handleSave = async () => {
    if (!providerId || !model) {
      showToast('请选择提供商与模型', 'info');
      return;
    }
    setSaving(true);
    try {
      await save({ providerId, model });
      await refreshEffective();
      showToast('已保存：知识库将使用该模型', 'success');
    } catch (e) {
      showToast(`保存失败: ${errorMessage(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  const handleClear = async () => {
    setSaving(true);
    try {
      await save({ providerId: '', model: '' });
      setProviderId('');
      setModel('');
      await refreshEffective();
      showToast('已改为自动回落（用第一个可用提供商的模型）', 'info');
    } catch (e) {
      showToast(`取消失败: ${errorMessage(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="space-y-4">
      <div
        className="rounded-lg px-3 py-2.5 text-[11px] leading-relaxed"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
      >
        <div className="flex items-center gap-1.5 mb-1" style={{ color: 'var(--text-primary)' }}>
          <Library size={12} />
          <span className="text-xs font-medium">知识库模型</span>
        </div>
        知识库有三处在调模型：<b>文件 / 网页 / 云文档投喂</b>（分块、起标题、抽标签、按专属字段推断属性）、
        <b>片段投喂与 AI 生成</b>（产出知识草稿）、<b>自动沉淀</b>（把一轮对话整理成候选）。
        <b>只有片段投喂、AI 生成、自动沉淀的产出先落「待确认」</b>等你核对；
        文件 / 网页 / 云文档是你明确点下的投喂意图，整理后<b>直接入库</b>。
        不指定时自动回落——用第一个 OpenAI 兼容、配了 Key 的提供商的第一个模型。
      </div>

      {/* 当前实际生效的模型：不指定时最容易"不知道在用哪个"，这里显式说清 */}
      <div
        className="rounded-lg px-3 py-2.5 flex items-center gap-2 text-[11px]"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      >
        <span className="shrink-0" style={{ color: 'var(--text-tertiary)' }}>当前生效</span>
        {probing ? (
          <Loader2 size={11} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
        ) : effective ? (
          <>
            <span style={{ color: 'var(--text-primary)' }}>{effective.providerName} · {effective.model}</span>
            <span
              className="px-1.5 py-[1px] rounded text-[10px]"
              style={{
                backgroundColor: effective.fromFallback ? 'var(--bg-tertiary)' : 'var(--accent-light)',
                color: effective.fromFallback ? 'var(--text-tertiary)' : 'var(--accent)',
              }}
            >
              {effective.fromFallback ? '自动回落' : '已指定'}
            </span>
          </>
        ) : (
          <span style={{ color: 'var(--status-danger, #EF4444)' }}>
            没有可用模型：请先在「设置 › API 提供商」里配置，或在上方指定
          </span>
        )}
      </div>

      <section>
        <h3 className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>指定模型（可选）</h3>
        <div className="space-y-3">
          <div>
            <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>提供商</label>
            <Select
              value={providerId}
              onChange={(v) => { setProviderId(v); setModel(''); }}
              options={[
                { value: '', label: '自动回落（不指定）' },
                ...providers.map((p) => ({ value: p.id, label: p.name })),
              ]}
              size="sm"
              className="w-full"
              placeholder="自动回落"
              title="提供知识库整理与生成所用的接口；需为 OpenAI 兼容格式"
            />
            {anthropic && (
              <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: '#F59E0B' }}>
                该提供商是 Anthropic 原生格式：没有 /chat/completions 端点，知识库调用会被拒绝。请改选 OpenAI 兼容的提供商。
              </div>
            )}
          </div>

          <div>
            <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>模型</label>
            <Select
              value={model}
              onChange={setModel}
              options={models.map((m) => ({ value: m, label: m }))}
              size="sm"
              className="w-full"
              placeholder={providerId ? (models.length ? '选择模型' : '该提供商下还没有配置模型') : '先选择提供商'}
              disabled={!providerId || models.length === 0}
              title="用于知识库整理与生成的模型"
            />
            {providerId && models.length === 0 && (
              <div className="mt-1.5 text-[10px]" style={{ color: '#F59E0B' }}>
                该提供商下还没有模型，请先到「设置 › API 提供商」里添加。
              </div>
            )}
          </div>

          <div className="flex items-center gap-2 pt-1">
            <button
              onClick={() => void handleSave()}
              disabled={saving || !dirty || !providerId || !model}
              className="pd-btn px-3 py-1.5 rounded-lg text-xs"
              style={{
                backgroundColor: 'var(--accent)',
                color: '#fff',
                border: 'none',
                opacity: saving || !dirty || !providerId || !model ? 0.5 : 1,
              }}
            >
              保存
            </button>
            <button
              onClick={() => void handleClear()}
              disabled={saving || !hasKbModelOverride(config)}
              className="pd-btn px-3 py-1.5 rounded-lg text-xs"
              style={{
                color: 'var(--text-secondary)',
                backgroundColor: 'var(--bg-field)',
                border: '1px solid var(--border)',
                opacity: saving || !hasKbModelOverride(config) ? 0.5 : 1,
              }}
              title="取消指定，改为自动回落"
            >
              改为自动回落
            </button>
            {dirty && (
              <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>有未保存的改动</span>
            )}
          </div>
        </div>
      </section>

      <div className="flex items-start gap-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
        <Info size={11} className="shrink-0 mt-[1px]" />
        整理与生成都会真的发起网络请求并计入用量；整理失败的条目不会被删除，只是少了标签与属性，可稍后重试。
      </div>

      {/* 自动沉淀：轮末把新增对话整理成候选（默认开启） */}
      <section className="pt-3" style={{ borderTop: '1px solid var(--border)' }}>
        <h3 className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>自动沉淀（可选）</h3>
        <div className="flex items-center gap-3">
          <button
            onClick={() => void toggleAuto()}
            disabled={loadingAuto}
            className="relative rounded-full transition-colors shrink-0 disabled:opacity-50"
            style={{
              width: 36,
              height: 20,
              backgroundColor: autoEnabled ? '#22c55e' : 'var(--bg-field)',
              border: '1px solid var(--border)',
            }}
            title={autoEnabled ? '点击关闭' : '点击开启'}
          >
            <div
              className="absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform shadow-sm"
              style={{ left: autoEnabled ? '18px' : '2px' }}
            />
          </button>
          <span className="text-xs" style={{ color: autoEnabled ? '#22c55e' : 'var(--text-tertiary)' }}>
            {loadingAuto ? '读取中…' : autoEnabled ? '已开启' : '已关闭'}
          </span>
        </div>
        <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
          开启后，每轮对话结束会把新增的消息交给上面的模型整理成候选，进对应知识库的「待确认」等你核对
          —— 不会直接入库，也不会改写你的原文。目标库不再预设：全部知识库的定义会一并交给模型，
          由模型按内容逐条选库，专属属性也在同一次整理里填好。至少要新增 2 条消息才触发，
          同一会话两次之间至少间隔 3 分钟；没有知识库时不触发。
          沉淀消耗的用量计入「用量统计 › 按归因 › 知识库 › 自动沉淀」。
        </div>
      </section>
    </div>
  );
}
