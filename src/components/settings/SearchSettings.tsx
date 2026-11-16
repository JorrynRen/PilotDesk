import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Save, Loader2 } from 'lucide-react';
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
    <div className="settings-page-stack">
      <SettingsSection
        title="联网搜索"
        description="配置 API Agent 的 search_web / fetch_web 工具所使用的搜索后端。"
      >
        <label className="pd-settings-field-label">搜索后端</label>
        <Select
          value={config.provider}
          onChange={(v) =>
            setConfig((prev) => ({ ...prev, provider: v as SearchConfig['provider'] }))
          }
          options={PROVIDERS.map((p) => ({ value: p.value, label: p.label }))}
          className="w-full"
        />
        <p className="pd-settings-note mt-2">
          默认使用 Bing 中国版（无需 API Key），在国内可直连；如遇反爬限制会自动降级到本机无头浏览器。
        </p>
      </SettingsSection>

      <SettingsSection
        title="API Key（可选）"
        description="仅当切换为 Tavily 或 Bing API 后端时需要填写。留空则相关后端不可用。"
        actions={
          <SettingsButton
            variant="primary"
            onClick={handleSave}
            icon={saving ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
            disabled={saving}
          >
            保存
          </SettingsButton>
        }
      >
        <div className="space-y-3">
          <div>
            <label className="pd-settings-field-label">Tavily API Key</label>
            <input
              type="password"
              value={config.tavilyApiKey}
              onChange={(e) => setConfig((prev) => ({ ...prev, tavilyApiKey: e.target.value }))}
              placeholder="tvly-..."
              className="pd-settings-input w-full"
            />
          </div>
          <div>
            <label className="pd-settings-field-label">Bing Web Search API Key</label>
            <input
              type="password"
              value={config.bingApiKey}
              onChange={(e) => setConfig((prev) => ({ ...prev, bingApiKey: e.target.value }))}
              placeholder="Azure 订阅密钥（Ocp-Apim-Subscription-Key）"
              className="pd-settings-input w-full"
            />
          </div>
        </div>
      </SettingsSection>

      <p className="pd-settings-note px-1">
        API Key 保存在本地 SQLite 数据库，不会上传。保存后将在下一次 API Agent 会话时生效。
      </p>
    </div>
  );
}
