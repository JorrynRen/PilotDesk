import { useCallback, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { Plus, Trash2, Pencil, Check, X, Globe, Link2, ExternalLink, Play, Info } from 'lucide-react';
import {
  useCustomTabsStore,
  validateCustomTabInput,
  isWebUrl,
  TAB_ICON_OPTIONS,
  type CustomTab,
} from '../../stores/customTabsStore';
import { Select } from '../common/Select';
import { TabIcon } from '../common/TabIcon';
import { useTerminal } from '../../TerminalManager';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/**
 * 内嵌 Web（集成配置子页签）= 在线版 Agent 站点的**承载类接入**（设计文档 §1.1 的 L2）。
 *
 * 关键取舍：**不新建一套存储，复用门户标签**。
 * 内嵌 Web 与门户标签的内核完全相同（都是 iframe 承载一个网址、同一套 LRU 保活外壳），
 * 另建一套只会让同一网址出现两份配置。这里只管理 `kind === 'agent'` 的条目；
 * 普通书签不进本页，「门户」页则两者都能看到、都能打开。
 *
 * 它不是「可调用的 Agent」：不可程序化调用，故**不进入会话选择器**，只提供"在应用内打开并用"的容器。
 */
export function EmbeddedWebSettings() {
  const navigate = useNavigate();
  const { setMode } = useTerminal();
  const { tabs, addTab, updateTab, removeTab, setActiveTab } = useCustomTabsStore();

  // 只看本类目：缺省 kind 的通用书签不在此页出现
  const sites = useMemo(() => tabs.filter((t) => t.kind === 'agent'), [tabs]);

  // 新增表单
  const [label, setLabel] = useState('');
  const [url, setUrl] = useState('');
  const [icon, setIcon] = useState('');
  const [formError, setFormError] = useState('');
  // 行内编辑
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editLabel, setEditLabel] = useState('');
  const [editUrl, setEditUrl] = useState('');
  const [editIcon, setEditIcon] = useState('');
  const [editError, setEditError] = useState('');

  /**
   * 内嵌 Web 的额外约束：**只接受网络地址**。
   * 本地文件/目录不属于"在线版 Agent"，允许它会把本页变成门户标签的重复入口。
   * 仅接受 `agent` 类目也由此保证：不会误把本地书签改造成内嵌 Web。
   */
  const checkWebUrl = useCallback((rawUrl: string): string | null => {
    const trimmed = rawUrl.trim();
    if (!trimmed) return null; // 空值由 validateCustomTabInput 统一报"地址不能为空"
    // 无协议域名会被自动补 https://（与门户标签一致），故先看是否已是本地路径/其它协议
    if (/^file:\/\//i.test(trimmed) || /^[a-zA-Z]:[\\/]/.test(trimmed) || /^\\\\|^\/\//.test(trimmed)) {
      return '内嵌 Web 只支持网络地址（http:// 或 https://），不支持本地文件/目录';
    }
    return null;
  }, []);

  const handleAdd = useCallback(async () => {
    const urlError = checkWebUrl(url);
    if (urlError) { setFormError(urlError); return; }
    // 第一层：页面即时校验（快速反馈）；第二层：store 写入时再校验（最终把关）
    const checked = validateCustomTabInput(label, url, tabs, null);
    if (!checked.ok) { setFormError(checked.error); return; }
    if (!isWebUrl(checked.url)) {
      setFormError('内嵌 Web 只支持网络地址（http:// 或 https://），不支持本地文件/目录');
      return;
    }
    const res = await addTab(label, url, undefined, icon, 'agent');
    if (!res.ok) { setFormError(res.error); return; }
    setFormError('');
    setLabel('');
    setUrl('');
  }, [label, url, icon, tabs, addTab, checkWebUrl]);

  const startEdit = useCallback((t: CustomTab) => {
    setEditingId(t.id);
    setEditLabel(t.label);
    setEditUrl(t.url);
    setEditIcon(t.icon ?? '');
    setEditError('');
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditError('');
  }, []);

  const saveEdit = useCallback(async (id: string) => {
    const urlError = checkWebUrl(editUrl);
    if (urlError) { setEditError(urlError); return; }
    const checked = validateCustomTabInput(editLabel, editUrl, tabs, id);
    if (!checked.ok) { setEditError(checked.error); return; }
    if (!isWebUrl(checked.url)) {
      setEditError('内嵌 Web 只支持网络地址（http:// 或 https://），不支持本地文件/目录');
      return;
    }
    const res = await updateTab(id, { label: editLabel.trim(), url: editUrl.trim(), icon: editIcon });
    if (!res.ok) { setEditError(res.error); return; }
    setEditError('');
    setEditingId(null);
  }, [editLabel, editUrl, editIcon, tabs, updateTab, checkWebUrl]);

  /** 在应用内打开：切到门户（custom 模式）并激活该标签——iframe 由 CustomTabHost 挂载 */
  const openInApp = useCallback((id: string) => {
    setActiveTab(id);
    setMode('custom');
    navigate('/', { replace: true });
  }, [setActiveTab, setMode, navigate]);

  /** 在浏览器打开：复用 open_path（http(s) 交系统默认浏览器） */
  const openInBrowser = useCallback(async (targetUrl: string) => {
    try {
      await invoke('open_path', { path: targetUrl });
    } catch (e) {
      showToast(`打开失败：${errorMessage(e)}`, 'error');
    }
  }, []);

  const inputCls = 'px-3 py-2 rounded-lg text-xs outline-none';

  return (
    <div>
      <div className="mb-4">
        <h3 className="text-sm font-medium mb-1" style={{ color: 'var(--text-primary)' }}>内嵌 Web</h3>
        <p className="text-xs leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
          把在线版 Agent 的网页（如 chat.deepseek.com）嵌进应用内承载——属于"承载类接入（L2）"，需人工操作。
          <br />
          它们**不会**出现在会话选择器里（不能被自动调用），只提供"在应用内打开并用"的容器；添加后同时出现在「门户」页。
          <br />
          提示：部分站点通过 X-Frame-Options / CSP 禁止被内嵌，此类页面可能白屏，可在浏览器中打开。
        </p>
      </div>

      {/* 新增表单 */}
      <div
        className="p-3 rounded-xl"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center gap-2">
          <input
            value={label}
            onChange={(e) => { setLabel(e.target.value); if (formError) setFormError(''); }}
            placeholder="名称（如 DeepSeek）"
            className={`flex-1 min-w-[100px] ${inputCls}`}
            style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)' }}
          />
          <input
            value={url}
            onChange={(e) => { setUrl(e.target.value); if (formError) setFormError(''); }}
            placeholder="网址（如 https://chat.deepseek.com）"
            className={`flex-[2] min-w-[180px] ${inputCls}`}
            style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)' }}
            onKeyDown={(e) => { if (e.key === 'Enter') void handleAdd(); }}
          />
          <button
            onClick={() => void handleAdd()}
            className="flex items-center gap-1 px-3 py-2 rounded-lg text-xs font-medium transition-all active:scale-[.98]"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            <Plus size={13} />
            添加
          </button>
        </div>
        {/* 图标（可选）：用于门户卡片与顶栏展示，默认地球 */}
        <div className="flex items-center gap-2 mt-2">
          <Select
            value={icon}
            onChange={setIcon}
            options={TAB_ICON_OPTIONS}
            placeholder="图标（默认地球）"
            size="md"
            className="min-w-[140px]"
          />
          <span className="flex items-center gap-1 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            <Info size={11} />
            仅支持 http:// 或 https:// 网址
          </span>
        </div>
      </div>
      {/* 新增校验错误：就地展示 */}
      {formError && (
        <div className="mt-1.5 px-1 text-[11px] leading-relaxed" style={{ color: 'var(--danger, #EF4444)' }}>
          {formError}
        </div>
      )}

      {/* 站点列表 */}
      <div className="mt-4">
        {sites.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-3 py-12">
            <Globe size={26} style={{ color: 'var(--text-tertiary)' }} />
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>暂无内嵌 Web 站点，在上方添加</p>
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {sites.map((t) => {
              const isEditing = editingId === t.id;
              return (
                <div
                  key={t.id}
                  className="p-3 rounded-xl"
                  style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
                >
                  {isEditing ? (
                    <div className="flex flex-col gap-2">
                      <div className="flex items-center gap-2">
                        <input
                          value={editLabel}
                          onChange={(e) => { setEditLabel(e.target.value); if (editError) setEditError(''); }}
                          placeholder="名称"
                          className={`flex-1 min-w-[90px] ${inputCls}`}
                          style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)' }}
                        />
                        <input
                          value={editUrl}
                          onChange={(e) => { setEditUrl(e.target.value); if (editError) setEditError(''); }}
                          placeholder="网址"
                          className={`flex-[2] min-w-[140px] ${inputCls}`}
                          style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)' }}
                          onKeyDown={(e) => { if (e.key === 'Enter') void saveEdit(t.id); }}
                        />
                      </div>
                      <div className="flex items-center gap-2">
                        <Select
                          value={editIcon}
                          onChange={setEditIcon}
                          options={TAB_ICON_OPTIONS}
                          placeholder="图标"
                          size="sm"
                          className="min-w-[104px] shrink-0"
                        />
                        <div className="flex-1" />
                        <button onClick={() => void saveEdit(t.id)} className="shrink-0 p-1.5 rounded transition-colors" style={{ color: '#22c55e' }} title="保存">
                          <Check size={14} />
                        </button>
                        <button onClick={cancelEdit} className="shrink-0 p-1.5 rounded transition-colors" style={{ color: 'var(--text-secondary)' }} title="取消">
                          <X size={14} />
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div className="flex items-center gap-2">
                      <span className="shrink-0 flex items-center" style={{ color: 'var(--text-secondary)' }}>
                        <TabIcon icon={t.icon} size={14} />
                      </span>
                      <span className="flex-1 min-w-[90px] text-xs truncate" style={{ color: 'var(--text-primary)' }} title={t.label}>
                        {t.label}
                      </span>
                      <span className="flex-[2] min-w-[140px] text-xs truncate flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }} title={t.url}>
                        <Link2 size={11} className="shrink-0" />
                        {t.url}
                      </span>
                      {/* 在应用内打开：切到门户并激活该标签（复用门户外壳，不新建容器） */}
                      <button
                        onClick={() => openInApp(t.id)}
                        className="shrink-0 flex items-center gap-1 px-2 py-1 rounded text-[11px] transition-all active:scale-[.98]"
                        style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
                        title="在应用内打开"
                      >
                        <Play size={11} />
                        打开
                      </button>
                      <button
                        onClick={() => void openInBrowser(t.url)}
                        className="shrink-0 p-1.5 rounded transition-colors"
                        style={{ color: 'var(--text-secondary)' }}
                        title="在浏览器中打开"
                      >
                        <ExternalLink size={14} />
                      </button>
                      <button
                        onClick={() => startEdit(t)}
                        className="shrink-0 p-1.5 rounded transition-colors"
                        style={{ color: 'var(--text-secondary)' }}
                        title="编辑"
                      >
                        <Pencil size={14} />
                      </button>
                      <button
                        onClick={() => void removeTab(t.id)}
                        className="shrink-0 p-1.5 rounded transition-colors"
                        style={{ color: 'var(--danger, #EF4444)' }}
                        title="删除"
                      >
                        <Trash2 size={14} />
                      </button>
                    </div>
                  )}
                  {isEditing && editError && (
                    <div className="mt-1.5 text-[11px] leading-relaxed" style={{ color: 'var(--danger, #EF4444)' }}>
                      {editError}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
