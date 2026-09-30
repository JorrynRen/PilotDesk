/**
 * VoiceInputSettings — 设置 › 语音识别
 *
 * 指定语音输入用的**专用转写模型**（可选）。不配置时，语音输入沿用「当前会话所选模型」，
 * 而多数对话模型没有 /audio/transcriptions 能力 —— 所以这里给出的是一句"什么时候需要配"的说明，
 * 而不是一堆开关。
 */

import { useEffect, useState } from 'react';
import { Mic, Loader2 } from 'lucide-react';
import { useVoiceInputStore, hasVoiceOverride } from '../../stores/voiceInputStore';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { Select } from '../common/Select';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

export function VoiceInputSettings() {
  const { providers, fetchProviders } = useApiProviderStore();
  const { config, loaded, load, save } = useVoiceInputStore();

  const [providerId, setProviderId] = useState('');
  const [model, setModel] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void fetchProviders();
    void load();
  }, [fetchProviders, load]);

  // 配置读回后同步到表单（只在首次加载完成/配置变化时灌入，避免覆盖用户正在选的草稿）。
  // 用「渲染期修正」（React adjust-during-render）而不是 effect——在 effect 里同步 setState
  // 会多一轮级联渲染（`react-hooks/set-state-in-effect`），而"外部值变了就同步本地状态"正是其适用场景。
  // 判据取 (loaded, config) 拼出的签名，等价于原 effect 的依赖数组。
  const formSyncSignature = `${loaded}\u0000${config.providerId}\u0000${config.model}`;
  /**
   * ⚠️ 前值哨兵初值必须是 `null`，**不能**写成 `useState(formSyncSignature)`。
   * 配置来自跨页共享的 store，本组件挂载时往往已经 `loaded`；若初值就等于当帧签名，
   * 判据第一帧就不成立 → 表单永远灌不进当前值 → 两个下拉框停在默认项
   * （用户看到的就是"始终显示默认值而非当前值"）。
   * 原 `useEffect(..., [loaded, config...])` 在挂载时也会跑一次，这里是补回那一次。
   */
  const [syncedFormSignature, setSyncedFormSignature] = useState<string | null>(null);
  if (loaded && syncedFormSignature !== formSyncSignature) {
    setSyncedFormSignature(formSyncSignature);
    setProviderId(config.providerId);
    setModel(config.model);
  }

  const provider = providers.find((p) => p.id === providerId);
  const models = provider?.models ?? [];
  const dirty = providerId !== config.providerId || model !== config.model;

  const handleSave = async () => {
    if (!providerId || !model) {
      showToast('请选择提供商与转写模型', 'info');
      return;
    }
    setSaving(true);
    try {
      await save({ providerId, model });
      showToast('已保存：语音输入将使用该转写模型', 'success');
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
      showToast('已改为跟随当前会话所选模型', 'info');
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
          <Mic size={12} />
          <span className="text-xs font-medium">语音输入（输入框麦克风按钮）</span>
        </div>
        默认用<b>当前会话所选的模型</b>转写（调用提供商的
        <code className="px-1 mx-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>/audio/transcriptions</code>
        接口）。会话模型不一定提供该接口 —— 若你常用的会话模型调不通，可以在这里指定一个专用转写模型
        （例如 <code className="px-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)' }}>whisper-1</code>）。
        留空则跟随会话模型。
      </div>

      <section>
        <h3 className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>专用转写模型</h3>
        <div className="space-y-3">
          <div>
            <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>提供商</label>
            <Select
              value={providerId}
              onChange={(v) => { setProviderId(v); setModel(''); }}
              options={[
                { value: '', label: '跟随当前会话模型（不使用覆盖）' },
                ...providers.map((p) => ({ value: p.id, label: p.name })),
              ]}
              size="sm"
              className="w-full"
              placeholder="跟随当前会话模型"
              title="语音转写调用的提供商；需为 OpenAI 兼容格式（Anthropic 原生格式没有转写端点）"
            />
            {providerId && provider && provider.apiFormat && provider.apiFormat.toLowerCase() === 'anthropic' && (
              <p className="text-[10px] mt-1" style={{ color: 'var(--status-warning, #f59e0b)' }}>
                该提供商是 Anthropic 原生格式，没有 /audio/transcriptions 端点，语音输入会失败
              </p>
            )}
          </div>

          <div>
            <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>转写模型</label>
            <Select
              value={model}
              onChange={setModel}
              options={models.map((m) => ({ value: m, label: m }))}
              size="sm"
              className="w-full"
              placeholder={providerId ? '选择该提供商下的模型' : '先选择提供商'}
              disabled={!providerId || models.length === 0}
              title="原样取自该提供商的模型清单；不确定就填 whisper-1 一类的转写模型"
            />
            {providerId && models.length === 0 && (
              <p className="text-[10px] mt-1" style={{ color: 'var(--text-tertiary)' }}>
                该提供商没有登记模型：请先到 设置 › API集成配置 里补全模型清单
              </p>
            )}
          </div>

          <div className="flex items-center gap-2">
            <button
              onClick={() => void handleSave()}
              disabled={saving || !providerId || !model || !dirty}
              className="pd-btn px-3 py-1.5 rounded-lg text-xs flex items-center gap-1.5"
              style={{
                backgroundColor: 'var(--accent)',
                color: '#fff',
                border: 'none',
                opacity: saving || !providerId || !model || !dirty ? 0.6 : 1,
                cursor: saving || !providerId || !model || !dirty ? 'not-allowed' : 'pointer',
              }}
            >
              {saving && <Loader2 size={12} className="animate-spin" />}
              保存
            </button>
            <button
              onClick={() => void handleClear()}
              disabled={saving || !hasVoiceOverride(config)}
              className="pd-btn px-3 py-1.5 rounded-lg text-xs"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-secondary)',
                border: '1px solid var(--border)',
                opacity: saving || !hasVoiceOverride(config) ? 0.6 : 1,
              }}
              title="清除后语音输入改用当前会话所选模型"
            >
              改为跟随会话模型
            </button>
            <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              {hasVoiceOverride(config) ? `当前：${config.model}` : '当前：跟随会话模型'}
            </span>
          </div>
        </div>
      </section>
    </div>
  );
}
