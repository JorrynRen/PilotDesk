import { useCallback, useEffect, useState } from 'react';
import { Download } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../../utils/toast';
import { useAgentRegistry } from '../../hooks/useAgentRegistry';
import { errorMessage } from '../../utils/errorMessage';

import type { AgentConfig } from '../../types';
import { SettingsSection, SettingsCard, SettingsButton } from '../settings';

/**
 * AgentConfigMarket — CLI Agent 配置市场（在线拉取）。
 *
 * 原先是「设置 › Agent」里内嵌的一段区块（AgentManager 的下半部分）。
 * 资源市集把各「获取」入口统一收拢后，这里独立成一个组件，供
 * 「资源市集 › CLI Agent 配置」tab 使用；设置页只保留本地 Registry 的管理。
 *
 * 行为与抽出前完全一致：后端 `fetch_agents_config` 负责服务器源/降级/重试，
 * 前端只做「本地已注册 vs 市场版本」的比对与安装/覆盖。
 */
export function AgentConfigMarket() {
  const { agents, fetchAgents } = useAgentRegistry();
  const [marketAgents, setMarketAgents] = useState<AgentConfig[]>([]);
  const [marketLoading, setMarketLoading] = useState(false);
  const [saving, setSaving] = useState(false);

  /** 从 Rust 后端获取 Agent 市场配置（服务器源/降级/重试由 market.rs 统一管理） */
  const fetchMarket = useCallback(async () => {
    setMarketLoading(true);
    try {
      const data = await invoke<{ agents?: AgentConfig[] }>('fetch_agents_config');
      setMarketAgents(data.agents || []);
    } catch (err) {
      showToast(`无法连接 Agent 市场，请检查网络连接 (${errorMessage(err)})`, 'warning');
    }
    setMarketLoading(false);
  }, []);

  /**
   * 进入市场即自动拉取一次。
   * 原先是「点刷新才显示」，但市场 tab 的预期就是打开即见货架 —— 手动刷新只该用于重试/更新。
   */
  useEffect(() => {
    // 不在 effect 体内同步 setState（fetchMarket 开头就 setMarketLoading(true)，
    // 会被 `react-hooks/set-state-in-effect` 判为级联渲染）：推到微任务，同一个任务、早于绘制。
    void Promise.resolve().then(() => fetchMarket());
  }, [fetchMarket]);

  /** 比较两个版本号（支持 v1, v1.1, v1.1.1 格式），返回 true 表示 a > b */
  const isNewerVersion = (a: string, b: string): boolean => {
    const normalize = (v: string) => {
      const parts = v.replace(/^v/i, '').split('.').map(Number);
      // 补零对齐：v1 → [1,0,0], v1.1 → [1,1,0]
      while (parts.length < 3) parts.push(0);
      return parts;
    };
    const pa = normalize(a), pb = normalize(b);
    for (let i = 0; i < 3; i++) {
      if (pa[i] > pb[i]) return true;
      if (pa[i] < pb[i]) return false;
    }
    return false; // equal
  };

  /** 从市场更新（强制覆盖本地，不检查版本） */
  const handleRestoreFromMarket = async (agent: AgentConfig) => {
    setSaving(true);
    try {
      await invoke('update_agent', {
        payload: {
          agentType: agent.agentType,
          displayName: agent.displayName,
          description: agent.description,
          cliCommand: agent.cliCommand,
          npmPackage: agent.npmPackage,
          pipPackage: agent.pipPackage,
          installCmd: agent.installCmd,
          uninstallCmd: agent.uninstallCmd,
          updateCmd: agent.updateCmd,
          versionCmd: agent.versionCmd,
          latestVersionCmd: agent.latestVersionCmd,
          runCmdTemplate: agent.runCmdTemplate,
          outputParser: agent.outputParser,
          outputFilterRegex: agent.outputFilterRegex,
          versionPattern: agent.versionPattern,
          sessionIdSource: agent.sessionIdSource,
          sessionIdEventType: agent.sessionIdEventType,
          sessionIdField: agent.sessionIdField,
          resumeArgTemplate: agent.resumeArgTemplate,
          skillsDir: agent.skillsDir,
          skillEntryFile: agent.skillEntryFile,
          skillDisplayMode: agent.skillDisplayMode,
          color: agent.color,
          icon: agent.icon,
          sortOrder: agent.sortOrder,
          isEnabled: agent.isEnabled,
          version: agent.version,
        },
      });
      showToast(`已恢复 ${agent.displayName} 初始配置 (v${agent.version})`, 'success');
      fetchAgents();
    } catch (err) {
      showToast(`恢复失败: ${errorMessage(err)}`, 'error');
    }
    setSaving(false);
  };

  const handleInstallFromMarket = async (agent: AgentConfig) => {
    setSaving(true);
    try {
      const local = agents.find(a => a.agentType === agent.agentType);
      if (local) {
        // 已存在 → 检查版本
        if (!isNewerVersion(agent.version, local.version)) {
          showToast(`${agent.displayName} 已是最新版本 (${local.version})`, 'info');
          setSaving(false);
          return;
        }
        // 更新
        await invoke('update_agent', {
          payload: {
            agentType: agent.agentType,
            displayName: agent.displayName,
            description: agent.description,
            cliCommand: agent.cliCommand,
            npmPackage: agent.npmPackage,
            pipPackage: agent.pipPackage,
            installCmd: agent.installCmd,
            uninstallCmd: agent.uninstallCmd,
            updateCmd: agent.updateCmd,
            versionCmd: agent.versionCmd,
            latestVersionCmd: agent.latestVersionCmd,
            runCmdTemplate: agent.runCmdTemplate,
            outputParser: agent.outputParser,
            outputFilterRegex: agent.outputFilterRegex,
            versionPattern: agent.versionPattern,
            sessionIdSource: agent.sessionIdSource,
            sessionIdEventType: agent.sessionIdEventType,
            sessionIdField: agent.sessionIdField,
            resumeArgTemplate: agent.resumeArgTemplate,
            skillsDir: agent.skillsDir,
            skillEntryFile: agent.skillEntryFile,
            skillDisplayMode: agent.skillDisplayMode,
            color: agent.color,
            icon: agent.icon,
            sortOrder: agent.sortOrder,
            isEnabled: agent.isEnabled,
            version: agent.version,
          },
        });
        showToast(`已更新 ${agent.displayName} (${local.version} → ${agent.version})`, 'success');
      } else {
        // 新安装
        await invoke('add_agent', { payload: { ...agent, version: agent.version } });
        showToast(`已安装 ${agent.displayName} (${agent.version})`, 'success');
      }
      fetchAgents();
    } catch (err) {
      showToast(`操作失败: ${errorMessage(err)}`, 'error');
    }
    setSaving(false);
  };

  return (
    <div className="h-full overflow-y-auto p-4 pd-scroll-stable">
      <SettingsSection
        title="Agent 配置市场"
        actions={
          <SettingsButton
            variant="secondary"
            icon={<Download size={11} />}
            onClick={() => { fetchMarket(); }}
            disabled={marketLoading}
          >
            {marketLoading ? '加载中...' : '加载市场'}
          </SettingsButton>
        }
      >
        <div className="space-y-2">
          {marketAgents.length === 0 ? (
            <div className="text-xs py-2" style={{ color: 'var(--text-secondary)' }}>
              {marketLoading ? '正在加载市场配置...' : '市场暂无可用的 Agent 配置，可点「加载市场」重试'}
            </div>
          ) : (
            marketAgents.map((agent) => {
              const local = agents.find(a => a.agentType === agent.agentType);
              const isInstalled = !!local;
              const hasUpdate = isInstalled && isNewerVersion(agent.version, local.version);
              return (
                <SettingsCard key={`market-${agent.agentType}`}>
                  <div className="flex items-center gap-3 w-full">
                    <div className="flex items-center gap-2 min-w-0 flex-1">
                      <div className="w-3 h-3 rounded-full shrink-0" style={{ backgroundColor: agent.color }} />
                      <div className="min-w-0">
                        <div className="flex items-center gap-1.5">
                          <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                            {agent.displayName}
                          </span>
                          <span className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                            v{agent.version}
                          </span>
                        </div>
                        <div className="text-[10px]" style={{ color: 'var(--text-secondary)' }}>
                          {agent.description}
                        </div>
                      </div>
                    </div>
                    <div className="flex items-center gap-1.5">
                      {hasUpdate ? (
                        <SettingsButton
                          variant="primary"
                          onClick={() => handleInstallFromMarket(agent)}
                          disabled={saving}
                        >
                          更新 (v{local.version} → v{agent.version})
                        </SettingsButton>
                      ) : isInstalled ? (
                        <SettingsButton
                          variant="secondary"
                          onClick={() => handleRestoreFromMarket(agent)}
                          disabled={saving}
                          title="用市场配置覆盖本地修改，恢复出厂配置"
                        >
                          从市场更新
                        </SettingsButton>
                      ) : (
                        <SettingsButton
                          variant="primary"
                          onClick={() => handleInstallFromMarket(agent)}
                          disabled={saving}
                        >
                          拉取并注册
                        </SettingsButton>
                      )}
                    </div>
                  </div>
                </SettingsCard>
              );
            })
          )}
        </div>
      </SettingsSection>
    </div>
  );
}
