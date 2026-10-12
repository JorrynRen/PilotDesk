import { useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { save as saveDialog, open as openDialog } from '@tauri-apps/plugin-dialog';
import { Search, Plus, Star, Package, Lightbulb, Upload } from 'lucide-react';
import { TitleBar, StatusBar } from '../layout';
import { useTerminal, type ViewMode } from '../../TerminalManager';
import { useInspirationStore, type InspirationItem } from '../../stores/inspirationStore';
import { useSessionStore } from '../../stores/sessionStore';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { InspirationCard } from './InspirationCard';
import { InspirationForm } from './InspirationForm';
import { InspirationExportDialog } from './InspirationExportDialog';
import { TagFilter } from './TagFilter';
import { confirmDialog } from '../../stores/confirmStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/** 后端导入结果（与 `src-tauri` 的 `InspirationImportResult` 同构） */
interface InspirationImportResult {
  created: number;
  updated: number;
  skipped: number;
  errors: string[];
}

/**
 * 灵感库（独立路由 /inspirations）。
 *
 * 原名「灵感市集」：名字里的"市集"是空头承诺 —— 这里全是**本地**保存的灵感，
 * 没有任何在线内容。在线灵感将由「资源市集 › 灵感」提供，两者是"我的"与"获取"的关系。
 */
export function InspirationLibraryPage() {
  const { viewMode, setMode } = useTerminal();
  const navigate = useNavigate();
  const {
    inspirations,
    loading,
    error,
    searchQuery,
    favoriteOnly,
    fetchInspirations,
    fetchTags,
    searchInspirations,
    createInspiration,
    updateInspiration,
    deleteInspiration,
    toggleFavorite,
    setSearchQuery,
    setFavoriteOnly,
  } = useInspirationStore();

  const [showForm, setShowForm] = useState(false);
  const [editingInspiration, setEditingInspiration] = useState<InspirationItem | null>(null);
  const [prefillContent, setPrefillContent] = useState('');
  /** 导出为市场 JSON 的目标条目（非空即弹窗） */
  const [exportTarget, setExportTarget] = useState<InspirationItem | null>(null);
  /** 全量导出（备份）进行中 */
  const [exportingZip, setExportingZip] = useState(false);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const currentSession = useSessionStore((s) => {
    const cs = s.sessions.find((ses) => ses.id === s.currentSessionId);
    return cs;
  });

  useEffect(() => {
    fetchInspirations();
    fetchTags();
  }, [fetchInspirations, fetchTags]);

  const handleSearch = useCallback(
    (query: string) => {
      setSearchQuery(query);
      if (debounceRef.current) clearTimeout(debounceRef.current);
      debounceRef.current = setTimeout(() => {
        if (query.trim()) {
          searchInspirations(query);
        } else {
          fetchInspirations();
        }
      }, 300);
    },
    [setSearchQuery, searchInspirations, fetchInspirations]
  );

  const handleSendToSession = useCallback(
    (content: string) => {
      usePendingInputStore.getState().set(content);
    },
    []
  );

  const handleEdit = (insp: InspirationItem) => {
    setEditingInspiration(insp);
    setShowForm(false);
  };

  const handleSaveNew = async (data: Parameters<typeof createInspiration>[0]) => {
    await createInspiration(data);
    setShowForm(false);
  };

  const handleUpdate = async (data: Parameters<typeof updateInspiration>[0]) => {
    await updateInspiration(data);
    setEditingInspiration(null);
  };

  const handleDelete = async (id: string) => {
    const ok = await confirmDialog({
      title: '确认删除',
      message: '确定要删除这条灵感吗？',
      confirmText: '删除',
    });
    if (ok) {
      await deleteInspiration(id);
    }
  };

  /** 全量导出：把全部灵感打包成 zip（目录布局与市场同构，供备份） */
  const handleExportZip = async () => {
    try {
      const destPath = await saveDialog({
        defaultPath: 'inspirations-backup.zip',
        filters: [{ name: 'ZIP', extensions: ['zip'] }],
      });
      if (!destPath) return;
      setExportingZip(true);
      const count = await invoke<number>('export_inspirations_zip', { destPath });
      showToast(`已导出 ${count} 条灵感到备份文件`, 'success');
    } catch (e) {
      showToast(`导出失败：${errorMessage(e)}`, 'error');
    } finally {
      setExportingZip(false);
    }
  };

  /**
   * 导入灵感：一个入口同时吃市场格式的 `.json`（可多选）与本工具导出的 `.zip` 备份 ——
   * 用户不必先分辨格式，按扩展名分派即可。落库语义由后端统一（同 `market_id` 覆盖更新，否则新建），
   * 与「资源市集 › 灵感」的导入保持一致。
   */
  const handleImport = async () => {
    try {
      const picked = await openDialog({
        multiple: true,
        filters: [{ name: '灵感文件（.json / .zip）', extensions: ['json', 'zip'] }],
      });
      const paths = (Array.isArray(picked) ? picked : picked ? [picked] : []).filter(
        (p): p is string => typeof p === 'string',
      );
      if (paths.length === 0) return;

      let created = 0;
      let updated = 0;
      let skipped = 0;
      const errors: string[] = [];
      const merge = (r: InspirationImportResult) => {
        created += r.created;
        updated += r.updated;
        skipped += r.skipped;
        errors.push(...r.errors);
      };
      const jsonPaths = paths.filter((p) => p.toLowerCase().endsWith('.json'));
      const zipPaths = paths.filter((p) => p.toLowerCase().endsWith('.zip'));
      if (jsonPaths.length > 0) {
        merge(await invoke<InspirationImportResult>('import_inspirations_from_files', { paths: jsonPaths }));
      }
      for (const zipPath of zipPaths) {
        merge(await invoke<InspirationImportResult>('import_inspirations_zip', { zipPath }));
      }

      await fetchInspirations();
      await fetchTags();
      showToast(
        `导入完成：新增 ${created} 条，更新 ${updated} 条${skipped > 0 ? `，跳过 ${skipped} 条` : ''}`,
        skipped > 0 ? 'warning' : 'success',
      );
      // 跳过原因取第一条（通常是 id 不合规 / 缺 title）—— 比只报"跳过 N 条"更能指导用户改文件
      if (errors.length > 0) showToast(errors[0], 'warning');
    } catch (e) {
      showToast(`导入失败：${errorMessage(e)}`, 'error');
    }
  };

  const openNewForm = () => {
    setEditingInspiration(null);
    setPrefillContent('');
    setShowForm(true);
  };

  // 组合开关点击：切模式并回到主布局（与设置页/知识库同一套处理）
  const handleModeChange = useCallback((mode: ViewMode) => {
    setMode(mode);
    navigate('/');
  }, [setMode, navigate]);

  return (
    <div className="h-full flex flex-col overflow-hidden" style={{ backgroundColor: 'var(--bg-canvas)' }}>
      {/* 页头：与知识库/资源市集一致 —— 走 TitleBar（模式段 + 右侧图标区），
          画在壳层那片"底"上、不在内容面板内，因此不画下边框（栏与面板靠底色差 + 圆角区分）。 */}
      <TitleBar
        mode={viewMode}
        onModeChange={handleModeChange}
        noActiveSegment
        inspirationsOpen
        onOpenInspirations={() => navigate('/')}
        onOpenMarket={() => navigate('/market')}
        onOpenKnowledge={() => navigate('/knowledge')}
        onOpenSettings={() => navigate('/settings')}
      />

      <div className="flex-1 min-h-0 px-2">
        <div className="h-full rounded-lg overflow-hidden flex flex-col" style={{ backgroundColor: 'var(--bg-content)' }}>

      {/* 标题行 + 操作：与「资源市集」首行同构 —— 容器 pt-1、标题带强调色图标、下边框收口。
          放在**内容面板内**（而不是 TitleBar 与面板之间那条飘在画布上的工具条），两页才是同级观感。 */}
      <div className="shrink-0 px-4 pt-1 pb-1 flex items-center gap-3" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="flex items-center gap-2 shrink-0">
          <Lightbulb size={14} style={{ color: 'var(--accent)' }} />
          <span className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>灵感库</span>
          <span className="text-xs px-2 py-0.5 rounded-full" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-tertiary)' }}>
            {inspirations.length}
          </span>
        </div>
        <div className="flex-1" />
        <div className="flex items-center gap-1.5 shrink-0">
          <button
            onClick={() => void handleImport()}
            className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs"
            style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)' }}
            title="导入灵感：市场格式的 .json（可多选）或本工具导出的 .zip 备份"
          >
            <Upload size={12} />
            导入
          </button>
          <button
            onClick={() => void handleExportZip()}
            disabled={exportingZip || inspirations.length === 0}
            className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs disabled:opacity-50"
            style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-secondary)' }}
            title="把全部灵感导出为 zip 备份（目录布局与市场同构）"
          >
            <Package size={12} />
            {exportingZip ? '导出中…' : '全量导出'}
          </button>
          <button
            onClick={() => setFavoriteOnly(!favoriteOnly)}
            className="p-1.5 rounded-lg transition-colors"
            style={{
              backgroundColor: favoriteOnly ? '#FBBF2422' : 'var(--bg-field)',
              color: favoriteOnly ? '#FBBF24' : 'var(--text-secondary)',
            }}
            title={favoriteOnly ? '显示全部' : '只看收藏'}
          >
            <Star size={14} fill={favoriteOnly ? '#FBBF24' : 'none'} />
          </button>
          <button
            onClick={openNewForm}
            className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs "
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            <Plus size={12} />
            新建
          </button>
        </div>
      </div>

      {/* Search + Tags */}
      <div className="shrink-0 px-4 py-2 space-y-2" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="relative">
          <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2" style={{ color: 'var(--text-tertiary)' }} />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => handleSearch(e.target.value)}
            placeholder="搜索灵感..."
            className="w-full pl-9 pr-3 py-1.5 rounded-lg text-xs outline-none"
            style={{
              backgroundColor: 'var(--bg-field)',
              color: 'var(--text-primary)',
              border: '1px solid var(--border)',
            }}
          />
        </div>
        <TagFilter />
      </div>

      {/* Grid */}
      <div className="flex-1 overflow-y-auto p-4 pd-scroll-stable">
        {error && (
          <div className="mb-3 px-3 py-2 rounded-lg text-xs" style={{ backgroundColor: 'var(--danger-bg)', color: 'var(--danger)' }}>
            {error}
          </div>
        )}

        {loading && !inspirations.length ? (
          <div className="flex items-center justify-center h-32">
            <span className="text-xs" style={{ color: 'var(--text-secondary)' }}>加载中...</span>
          </div>
        ) : inspirations.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-32 gap-2">
            <span className="text-3xl">💡</span>
            <p className="text-sm" style={{ color: 'var(--text-secondary)' }}>
              {searchQuery ? '未找到匹配的灵感' : '还没有灵感，点击新建开始收集'}
            </p>
          </div>
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {inspirations.map((insp) => (
              <InspirationCard
                key={insp.id}
                inspiration={insp}
                onToggleFavorite={toggleFavorite}
                onSendToSession={handleSendToSession}
                onDelete={handleDelete}
                onEdit={handleEdit}
                onExport={setExportTarget}
              />
            ))}
          </div>
        )}
      </div>

      {/* Form Modal */}
      {(showForm || editingInspiration) && (
        <InspirationForm
          initialData={editingInspiration}
          prefill={prefillContent}
          sourceAgent={currentSession?.agentType}
          onSave={handleSaveNew}
          onUpdate={handleUpdate}
          onCancel={() => {
            setShowForm(false);
            setEditingInspiration(null);
            setPrefillContent('');
          }}
        />
      )}

      {/* 导出为市场 JSON（投稿用） */}
      {exportTarget && (
        <InspirationExportDialog
          inspiration={exportTarget}
          onClose={() => setExportTarget(null)}
        />
      )}
        </div>
      </div>

      {/* 全局状态栏：独立路由也要保留（与会话/知识库等模式一致），否则底部少一条、页面像被截断 */}
      <StatusBar
        onOpenSettings={() => navigate('/settings')}
        onOpenEnvSettings={() => navigate('/settings?tab=environment')}
      />
    </div>
  );
}
