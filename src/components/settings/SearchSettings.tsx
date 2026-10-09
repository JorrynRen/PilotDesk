import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Save, Loader2, Search } from 'lucide-react';
import { SettingsSection, SettingsButton } from './index';
import { Select } from '../common/Select';

// 与后端 api_agent/web.rs 的 SearchConfig / SearchProvider 对齐
interface SearchConfig {
  provider: 'bing_cn' | 'tavily' | 'bing_api';
  tavilyApiKey: string;
  bingApiKey: string;
}

const PROVIDERS = [
  { value: 'bing_cn', label: 'Bing 中国版（免 Key，默认）' },
  { value: 'tavily', label: 'Tavily Search API（需 Key）' },
  { value: 'bing_api', label: 'Bing Web Search API（需 Key）' },
] as const;

export function SearchSettings() {
  const [config, setConfig] = useState<SearchConfig>({
    provider: 'bing_cn',
    tavilyApiKey: '',
    bingApiKey: '',
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        const cfg = await invoke<SearchConfig>('get_search_config');
        setConfig({
          provider: cfg.provider ?? 'bing_cn',
          tavilyApiKey: cfg.tavilyApiKey ?? '',
          bingApiKey: cfg.bingApiKey ?? '',
        });
      } catch (e) {
        console.error('加载搜索配置失败:', e);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  const handleSave = useCallback(async () => {
    setSaving(true);
    try {
      await invoke('set_search_config', { config });
    } catch (e) {
      console.error('保存搜索配置失败:', e);
    } finally {
      setSaving(false);
    }
  }, [config]);

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 size={20} className="animate-spin" style={{ color: 'var(--text-tertiary)' }} />
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <Search size={14} style={{ color: 'var(--text-secondary)' }} />
        <div>
          <h3 className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
            联网搜索
          </h3>
          <p className="text-[10px] mt-0.5" style={{ color: 'var(--text-tertiary)' }}>
            配置 API Agent 的 search_web / fetch_web 工具所使用搜索后端
          </p>
        </div>
      </div>

      <SettingsSection title="搜索后端">
        <Select
          value={config.provider}
          onChange={(v) =>
            setConfig((prev) => ({ ...prev, provider: v as SearchConfig['provider'] }))
          }
          options={PROVIDERS.map((p) => ({ value: p.value, label: p.label }))}
          className="w-full"
        />
        <p className="text-xs mt-2" style={{ color: 'var(--text-secondary)' }}>
          默认使用 Bing 中国版（无需 API Key），在国内可直连；如遇反爬限制会自动降级到本机无头浏览器。
        </p>
      </SettingsSection>

      <SettingsSection
        title="API Key（可选）"
        description="仅当切换为 Tavily 或 Bing API 后端时需要填写。留空则相关后端不可用。"
      >
        <div className="space-y-3">
          <div>
            <label className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              Tavily API Key
            </label>
            <input
              type="password"
              value={config.tavilyApiKey}
              onChange={(e) => setConfig((prev) => ({ ...prev, tavilyApiKey: e.target.value }))}
              placeholder="tvly-..."
              className="w-full mt-0.5 px-2 py-1.5 rounded text-xs outline-none"
              style={{
                backgroundColor: 'var(--bg-field)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
            />
          </div>
          <div>
            <label className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
              Bing Web Search API Key
            </label>
            <input
              type="password"
              value={config.bingApiKey}
              onChange={(e) => setConfig((prev) => ({ ...prev, bingApiKey: e.target.value }))}
              placeholder="Azure 订阅密钥（Ocp-Apim-Subscription-Key）"
              className="w-full mt-0.5 px-2 py-1.5 rounded text-xs outline-none"
              style={{
                backgroundColor: 'var(--bg-field)',
                color: 'var(--text-primary)',
                border: '1px solid var(--border)',
              }}
            />
          </div>
        </div>
      </SettingsSection>

      <div className="flex items-center justify-end">
        <SettingsButton
          onClick={handleSave}
          icon={saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
          disabled={saving}
        >
          保存
        </SettingsButton>
      </div>

      <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
        API Key 保存在本地 SQLite 数据库，不会上传。保存后将在下一次 API Agent 会话时生效。
      </p>
    </div>
  );
}
