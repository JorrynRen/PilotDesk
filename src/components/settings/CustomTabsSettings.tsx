import { useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Plus, Trash2, Pencil, Check, X, GripVertical, Globe, Link2, ExternalLink } from 'lucide-react';
import {
  DndContext,
  closestCenter,
  PointerSensor,
  KeyboardSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from '@dnd-kit/core';
import {
  SortableContext,
  useSortable,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { useCustomTabsStore, validateCustomTabInput, TITLEBAR_LIMIT_MIN, TITLEBAR_LIMIT_MAX, type CustomTab } from '../../stores/customTabsStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/**
 * 自定义标签管理（设置页 tab 内容）。
 * CRUD + @dnd-kit 拖动排序，数据持久化到 app_settings（复用现有 KV 表）。
 * 录入校验与 store 双层执行（以 store 为准），错误就地展示。
 */

/** 可拖拽排序的标签行 */
function SortableTabRow({
  tab,
  index,
  isEditing,
  editLabel,
  editUrl,
  error,
  onEditLabelChange,
  onEditUrlChange,
  onSave,
  onCancelEdit,
  onStartEdit,
  onRemove,
  onOpenInBrowser,
}: {
  tab: CustomTab;
  index: number;
  isEditing: boolean;
  editLabel: string;
  editUrl: string;
  /** 该行编辑/校验失败时的中文原因（非编辑态为空） */
  error?: string;
  onEditLabelChange: (v: string) => void;
  onEditUrlChange: (v: string) => void;
  onSave: (id: string) => void;
  onCancelEdit: () => void;
  onStartEdit: (id: string, label: string, url: string) => void;
  onRemove: (id: string) => void;
  onOpenInBrowser: (url: string) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: tab.id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : 1,
    backgroundColor: isDragging ? 'var(--accent)' + '22' : 'var(--bg-secondary)',
    border: '1px solid var(--border)',
  };

  return (
    <div ref={setNodeRef} style={style} className="p-3 rounded-xl transition-all">
      <div className="flex items-center gap-2">
        {/* 拖动把手 */}
        <span
          className="cursor-grab active:cursor-grabbing shrink-0"
          style={{ color: 'var(--text-tertiary)' }}
          title="拖动排序"
          {...attributes}
          {...listeners}
        >
          <GripVertical size={15} />
        </span>
        <span className="w-5 text-center text-xs shrink-0" style={{ color: 'var(--text-tertiary)' }}>
          {index + 1}
        </span>
        {isEditing ? (
          <>
            <input
              value={editLabel}
              onChange={(e) => onEditLabelChange(e.target.value)}
              className="flex-1 min-w-[90px] px-2 py-1.5 rounded text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
            />
            <input
              value={editUrl}
              onChange={(e) => onEditUrlChange(e.target.value)}
              className="flex-[2] min-w-[140px] px-2 py-1.5 rounded text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
              onKeyDown={(e) => { if (e.key === 'Enter') onSave(tab.id); }}
            />
            <button onClick={() => onSave(tab.id)} className="shrink-0 p-1.5 rounded transition-colors" style={{ color: '#22c55e' }} title="保存">
              <Check size={14} />
            </button>
            <button onClick={onCancelEdit} className="shrink-0 p-1.5 rounded transition-colors" style={{ color: 'var(--text-secondary)' }} title="取消">
              <X size={14} />
            </button>
          </>
        ) : (
          <>
            <span className="flex-1 min-w-[90px] text-xs truncate" style={{ color: 'var(--text-primary)' }} title={tab.label}>
              {tab.label}
            </span>
            <span className="flex-[2] min-w-[140px] text-xs truncate flex items-center gap-1" style={{ color: 'var(--text-tertiary)' }} title={tab.url}>
              <Link2 size={11} className="shrink-0" />
              {tab.url}
            </span>
            {/* 在浏览器打开：录入后立刻验证地址是否可用（复用 open_path：http(s) 交系统默认浏览器） */}
            <button
              onClick={() => onOpenInBrowser(tab.url)}
              className="shrink-0 p-1.5 rounded transition-colors"
              style={{ color: 'var(--text-secondary)' }}
              title="在浏览器中打开"
            >
              <ExternalLink size={14} />
            </button>
            <button
              onClick={() => onStartEdit(tab.id, tab.label, tab.url)}
              className="shrink-0 p-1.5 rounded transition-colors"
              style={{ color: 'var(--text-secondary)' }}
              title="编辑"
            >
              <Pencil size={14} />
            </button>
            <button
              onClick={() => onRemove(tab.id)}
              className="shrink-0 p-1.5 rounded transition-colors"
              style={{ color: 'var(--danger, #EF4444)' }}
              title="删除"
            >
              <Trash2 size={14} />
            </button>
          </>
        )}
      </div>
      {/* 校验错误就地展示（编辑态） */}
      {isEditing && error && (
        <div className="mt-1.5 pl-7 text-[11px] leading-relaxed" style={{ color: 'var(--danger, #EF4444)' }}>
          {error}
        </div>
      )}
    </div>
  );
}

/**
 * 自定义标签管理（设置页 tab 内容）。
 * CRUD + 拖动排序，数据持久化到 app_settings（复用现有 KV 表）。
 */
export function CustomTabsSettings() {
  const { tabs, addTab, updateTab, removeTab, reorderTabs, titleBarLimit, setTitleBarLimit } = useCustomTabsStore();

  // 顶栏平铺显示个数（0–3）：拖动 / 键盘调整时只更新本地态，松手（或失焦）后才落库，
  // 避免拖动过程中每次 onChange 都写库。
  const [limitDraft, setLimitDraft] = useState(titleBarLimit);
  const commitLimit = useCallback(() => {
    if (limitDraft !== titleBarLimit) void setTitleBarLimit(limitDraft);
  }, [limitDraft, titleBarLimit, setTitleBarLimit]);

  // 新增表单
  const [label, setLabel] = useState('');
  const [url, setUrl] = useState('');
  // 录入错误（就地展示，中文）
  const [formError, setFormError] = useState('');
  // 行内编辑
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editLabel, setEditLabel] = useState('');
  const [editUrl, setEditUrl] = useState('');
  // 行内编辑错误（仅当前编辑行展示）
  const [editError, setEditError] = useState('');

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates })
  );

  const handleAdd = useCallback(async () => {
    // 第一层：页面即时校验（快速反馈）；第二层：store 写入时再校验（最终把关）
    const checked = validateCustomTabInput(label, url, tabs, null);
    if (!checked.ok) { setFormError(checked.error); return; }
    const res = await addTab(label, url);
    if (!res.ok) { setFormError(res.error); return; }
    setFormError('');
    setLabel('');
    setUrl('');
  }, [label, url, tabs, addTab]);

  const startEdit = useCallback((id: string, l: string, u: string) => {
    setEditingId(id);
    setEditLabel(l);
    setEditUrl(u);
    setEditError('');
  }, []);

  const cancelEdit = useCallback(() => {
    setEditingId(null);
    setEditError('');
  }, []);

  const saveEdit = useCallback(async (id: string) => {
    const checked = validateCustomTabInput(editLabel, editUrl, tabs, id);
    if (!checked.ok) { setEditError(checked.error); return; }
    const res = await updateTab(id, { label: editLabel.trim(), url: editUrl.trim() });
    if (!res.ok) { setEditError(res.error); return; }
    setEditError('');
    setEditingId(null);
  }, [editLabel, editUrl, tabs, updateTab]);

  // 在浏览器打开：复用 open_path（http(s) 交系统默认浏览器；本地路径交系统默认程序）
  const openInBrowser = useCallback(async (targetUrl: string) => {
    try {
      await invoke('open_path', { path: targetUrl });
    } catch (e) {
      showToast(`打开失败：${errorMessage(e)}`, 'error');
    }
  }, []);

  const handleDragEnd = useCallback(async (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const ids = tabs.map((t) => t.id);
    const oldIndex = ids.indexOf(active.id as string);
    const newIndex = ids.indexOf(over.id as string);
    if (oldIndex === -1 || newIndex === -1) return;
    await reorderTabs(oldIndex, newIndex);
  }, [tabs, reorderTabs]);

  return (
    <div>
      <div className="mb-4">
        <h3 className="text-sm font-medium mb-1" style={{ color: 'var(--text-primary)' }}>自定义标签</h3>
        <p className="text-xs" style={{ color: 'var(--text-tertiary)' }}>
          自定义标签会直接加入标题栏组合开关（工作流 / 会话 / 群聊 / 终端之后），点击即可快速切换访问常用页面。
          <br />
          支持三种地址：网络地址（http/https 网页）、本地 HTML 文件（任意文件完整路径，如 E:\doc\index.html，经 asset 协议加载）、本地目录（如 E:\doc，自动生成文件列表页，可进入子目录 / 打开文件）。
          <br />
          未带协议的域名（如 www.example.com）会自动补 https://；仅支持 http/https 与本地文件/目录路径，其它协议会被拒绝。
          <br />
          提示：部分站点通过 X-Frame-Options / CSP 禁止被内嵌，此类页面可能无法在标签中显示。
          <br />
          按住左侧手柄可拖动排序，顺序会持久化保存。
        </p>
      </div>

      {/* 顶栏平铺显示个数：设置组合开关里平铺几个自定义标签，其余收进「更多」下拉 */}
      <div
        className="mb-4 p-3 rounded-xl"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      >
        <div className="flex items-center justify-between gap-3">
          <label htmlFor="custom-tabs-titlebar-limit" className="text-xs" style={{ color: 'var(--text-primary)' }}>
            顶部显示个数
          </label>
          <span className="text-xs font-medium tabular-nums" style={{ color: 'var(--accent)' }}>{limitDraft}</span>
        </div>
        <input
          id="custom-tabs-titlebar-limit"
          type="range"
          min={TITLEBAR_LIMIT_MIN}
          max={TITLEBAR_LIMIT_MAX}
          step={1}
          value={limitDraft}
          onChange={(e) => setLimitDraft(Number(e.target.value))}
          onMouseUp={commitLimit}
          onTouchEnd={commitLimit}
          onBlur={commitLimit}
          className="w-full mt-2"
          style={{ accentColor: 'var(--accent)' }}
        />
        <p className="mt-1.5 text-[11px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
          0 表示不在顶栏显示自定义标签（仍可从「更多」进入）；最多 3 个。
        </p>
      </div>

      {/* 新增表单 */}
      <div
        className="flex items-center gap-2 p-3 rounded-xl"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
      >
        <input
          value={label}
          onChange={(e) => { setLabel(e.target.value); if (formError) setFormError(''); }}
          placeholder="标签名称"
          className="flex-1 min-w-[100px] px-3 py-2 rounded-lg text-xs outline-none"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
        />
        <input
          value={url}
          onChange={(e) => { setUrl(e.target.value); if (formError) setFormError(''); }}
          placeholder="URL / 本地文件 / 本地目录（如 https://example.com、E:\doc\index.html 或 E:\doc）"
          className="flex-[2] min-w-[180px] px-3 py-2 rounded-lg text-xs outline-none"
          style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}
          onKeyDown={(e) => { if (e.key === 'Enter') handleAdd(); }}
        />
        <button
          onClick={handleAdd}
          className="flex items-center gap-1 px-3 py-2 rounded-lg text-xs font-medium transition-all active:scale-[.98]"
          style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
        >
          <Plus size={13} />
          添加
        </button>
      </div>
      {/* 新增校验错误：就地展示 */}
      {formError && (
        <div className="mt-1.5 px-1 text-[11px] leading-relaxed" style={{ color: 'var(--danger, #EF4444)' }}>
          {formError}
        </div>
      )}

      {/* 标签列表 */}
      <div className="mt-4">
      {tabs.length === 0 ? (
        <div className="flex flex-col items-center justify-center gap-3 py-12">
          <Globe size={26} style={{ color: 'var(--text-tertiary)' }} />
          <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>暂无自定义标签，在上方添加</p>
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
            <SortableContext items={tabs.map((t) => t.id)} strategy={verticalListSortingStrategy}>
              {tabs.map((t, index) => (
                <SortableTabRow
                  key={t.id}
                  tab={t}
                  index={index}
                  isEditing={editingId === t.id}
                  editLabel={editLabel}
                  editUrl={editUrl}
                  error={editingId === t.id ? editError : ''}
                  onEditLabelChange={(v) => { setEditLabel(v); if (editError) setEditError(''); }}
                  onEditUrlChange={(v) => { setEditUrl(v); if (editError) setEditError(''); }}
                  onSave={saveEdit}
                  onCancelEdit={cancelEdit}
                  onStartEdit={startEdit}
                  onRemove={removeTab}
                  onOpenInBrowser={openInBrowser}
                />
              ))}
            </SortableContext>
          </DndContext>
        </div>
      )}
      </div>
    </div>
  );
}
