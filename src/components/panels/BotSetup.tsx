import { useState, useEffect, useCallback } from 'react';
import { Bot, Plus, Trash2, Wifi, WifiOff, Settings } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { useSessionStore } from '../../stores/sessionStore';

interface BotChannel {
  id: string;
  agentType: string;
  platform: string;
  method: string;
  status: string;
  triggerPrefix: string;
  responseFormat: string;
  config: Record<string, unknown>;
  createdAt: number;
  updatedAt: number;
}



export function BotSetup() {
  const [channels, setChannels] = useState<BotChannel[]>([]);
  const [loading, setLoading] = useState(false);
  const [editing, setEditing] = useState<Partial<BotChannel> | null>(null);
  const [isNew, setIsNew] = useState(false);

  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  const fetchChannels = useCallback(async () => {
    setLoading(true);
    try {
      const result = await invoke<BotChannel[]>('list_bot_channels');
      setChannels(result);
    } catch (err) {
      console.error('Failed to fetch bot channels:', err);
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    fetchChannels();
  }, [fetchChannels]);

  const handleSave = async () => {
    if (!editing) return;
    try {
      const payload = {
        id: isNew ? undefined : editing.id,
        agentType: editing.agentType ?? currentSession?.agentType ?? 'claude',
        platform: editing.platform ?? 'wechat',
        method: editing.method ?? 'clawbot',
        status: editing.status ?? 'disconnected',
        triggerPrefix: editing.triggerPrefix ?? '',
        responseFormat: editing.responseFormat ?? 'markdown',
        config: editing.config ?? {},
      };
      await invoke<BotChannel>('save_bot_channel', { payload });
      await fetchChannels();
      setEditing(null);
      setIsNew(false);
    } catch (err) {
      console.error('Failed to save bot channel:', err);
    }
  };

  const handleDelete = async (id: string) => {
    if (!confirm('确定要删除此 Bot 通道吗？')) return;
    try {
      await invoke('delete_bot_channel', { id });
      await fetchChannels();
    } catch (err) {
      console.error('Failed to delete bot channel:', err);
    }
  };

  const startNew = () => {
    setEditing({
      agentType: currentSession?.agentType ?? 'claude',
      platform: 'wechat',
      method: 'clawbot',
      status: 'disconnected',
      triggerPrefix: '',
      responseFormat: 'markdown',
    });
    setIsNew(true);
  };

  return (
    <div className="h-full flex flex-col overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <h3 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
          Bot 通道配置
        </h3>
        <button
          onClick={startNew}
          className="flex items-center gap-1 px-2 py-1 rounded text-xs font-medium"
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          <Plus size={12} />
          新建
        </button>
      </div>

      <div className="flex-1 overflow-y-auto">
        {editing ? (
          /* Edit form */
          <div className="p-4 space-y-3">
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="block text-[10px] font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>Agent</label>
                <select
                  value={editing.agentType ?? 'claude'}
                  onChange={(e) => setEditing({ ...editing, agentType: e.target.value })}
                  className="w-full px-2 py-1.5 rounded text-xs outline-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                >
                  <option value="claude">Claude Code</option>
                  <option value="hermes">Hermes Agent</option>
                </select>
              </div>
              <div>
                <label className="block text-[10px] font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>平台</label>
                <select
                  value={editing.platform ?? 'wechat'}
                  onChange={(e) => setEditing({ ...editing, platform: e.target.value })}
                  className="w-full px-2 py-1.5 rounded text-xs outline-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                >
                  <option value="wechat">微信</option>
                  <option value="dingtalk">钉钉</option>
                  <option value="feishu">飞书</option>
                </select>
              </div>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="block text-[10px] font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>触发前缀</label>
                <input
                  type="text"
                  value={editing.triggerPrefix ?? ''}
                  onChange={(e) => setEditing({ ...editing, triggerPrefix: e.target.value })}
                  placeholder="如: /"
                  className="w-full px-2 py-1.5 rounded text-xs outline-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                />
              </div>
              <div>
                <label className="block text-[10px] font-medium mb-1" style={{ color: 'var(--text-secondary)' }}>响应格式</label>
                <select
                  value={editing.responseFormat ?? 'markdown'}
                  onChange={(e) => setEditing({ ...editing, responseFormat: e.target.value })}
                  className="w-full px-2 py-1.5 rounded text-xs outline-none"
                  style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
                >
                  <option value="markdown">Markdown</option>
                  <option value="text">纯文本</option>
                </select>
              </div>
            </div>

            {/* QR Code placeholder */}
            <div
              className="flex items-center justify-center h-40 rounded-lg"
              style={{ backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
            >
              <div className="text-center">
                <Bot size={32} style={{ color: 'var(--text-tertiary)' }} />
                <p className="text-xs mt-2" style={{ color: 'var(--text-secondary)' }}>
                  二维码将在此显示
                </p>
              </div>
            </div>

            <div className="flex gap-2">
              <button
                onClick={handleSave}
                className="flex-1 px-3 py-1.5 rounded text-xs font-medium"
                style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
              >
                保存
              </button>
              <button
                onClick={() => { setEditing(null); setIsNew(false); }}
                className="flex-1 px-3 py-1.5 rounded text-xs"
                style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)', border: '1px solid var(--border)' }}
              >
                取消
              </button>
            </div>
          </div>
        ) : (
          /* Channel list */
          <div>
            {loading && !channels.length ? (
              <div className="flex items-center justify-center h-24">
                <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
              </div>
            ) : channels.length === 0 ? (
              <div className="flex flex-col items-center justify-center h-24 gap-2">
                <Bot size={24} style={{ color: 'var(--text-tertiary)' }} />
                <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>暂无 Bot 通道</span>
              </div>
            ) : (
              <div className="divide-y" style={{ borderColor: 'var(--border)' }}>
                {channels.map((channel) => (
                  <div key={channel.id} className="px-4 py-3">
                    <div className="flex items-center justify-between mb-1">
                      <div className="flex items-center gap-2">
                        <Bot size={14} style={{ color: channel.agentType === 'claude' ? 'var(--claude-tag)' : 'var(--hermes-tag)' }} />
                        <span className="text-xs font-medium" style={{ color: 'var(--text-primary)' }}>
                          {channel.agentType === 'claude' ? 'Claude Code' : 'Hermes'}
                        </span>
                        <span className="text-[10px] px-1.5 py-0.5 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
                          {channel.platform}
                        </span>
                      </div>
                      <div className="flex items-center gap-2">
                        <span className="flex items-center gap-1 text-[10px]" style={{ color: channel.status === 'connected' ? '#10B981' : 'var(--text-tertiary)' }}>
                          {channel.status === 'connected' ? <Wifi size={10} /> : <WifiOff size={10} />}
                          {channel.status === 'connected' ? '已连接' : '未连接'}
                        </span>
                        <button
                          onClick={() => { setEditing(channel); setIsNew(false); }}
                          className="p-1 rounded"
                          style={{ color: 'var(--text-secondary)' }}
                        >
                          <Settings size={12} />
                        </button>
                        <button
                          onClick={() => handleDelete(channel.id)}
                          className="p-1 rounded"
                          style={{ color: 'var(--text-tertiary)' }}
                        >
                          <Trash2 size={12} />
                        </button>
                      </div>
                    </div>
                    <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
                      前缀: {channel.triggerPrefix || '(无)'} | 格式: {channel.responseFormat}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
