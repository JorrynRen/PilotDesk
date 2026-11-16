import { useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { invoke } from '@tauri-apps/api/core';
import { save as saveDialog, open as openDialog } from '@tauri-apps/plugin-dialog';
import {
  AlertCircle,
  ArrowDownUp,
  Building2,
  Download,
  Lightbulb,
  Plus,
  RefreshCw,
  Search,
  SearchX,
  Star,
  Store,
  Upload,
  X,
} from 'lucide-react';
import { WorkbenchAssetPage } from '../layout';
import { PageToolbar } from '../common/PageToolbar';
import { useInspirationStore, type InspirationItem } from '../../stores/inspirationStore';
import { useSessionStore } from '../../stores/sessionStore';
import { usePendingInputStore } from '../../stores/pendingInputStore';
import { InspirationCard } from './InspirationCard';
import { InspirationForm } from './InspirationForm';
import { InspirationExportDialog } from './InspirationExportDialog';
import { InspirationOrgImportDialog } from './InspirationOrgImportDialog';
import { TagFilter } from './TagFilter';
import { confirmDialog } from '../../stores/confirmStore';
import { useAccountStore } from '../../stores/accountStore';
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
  const navigate = useNavigate();
  const {
    inspirations,
    loading,
    error,
    searchQuery,
    activeTag,
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
    clearFilters,
  } = useInspirationStore();

  const [showForm, setShowForm] = useState(false);
  const [editingInspiration, setEditingInspiration] = useState<InspirationItem | null>(null);
  const [prefillContent, setPrefillContent] = useState('');
  /** 导出 JSON 文件的目标条目（非空即弹窗） */
  const [exportTarget, setExportTarget] = useState<InspirationItem | null>(null);
  /** 全量导出（备份）进行中 */
  const [exportingZip, setExportingZip] = useState(false);
  const [showOrgImport, setShowOrgImport] = useState(false);
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

  const openOrgImport = async () => {
    if (!useAccountStore.getState().account) {
      const shouldLogin = await confirmDialog({
        title: '登录后使用团队共享',
        message: '登录后可选择所属组织，并导入该组织共享的灵感。',
        confirmText: '登录',
        cancelText: '稍后',
        danger: false,
      });
      if (shouldLogin) {
        await useAccountStore.getState().login();
      }
      return;
    }
    setShowOrgImport(true);
  };

  const hasFilters = Boolean(searchQuery.trim() || activeTag || favoriteOnly);

  return (
    <WorkbenchAssetPage contextLabel="灵感库" activeAsset="inspiration">
      <div className="pd-inspiration-page h-full flex flex-col overflow-hidden">
        <div className="pd-page-head pd-inspiration-header shrink-0">
          <div className="pd-inspiration-title">
            <span className="pd-inspiration-title-icon">
              <Lightbulb size={15} />
            </span>
            <span>灵感库</span>
            <span className="pd-inspiration-count">{inspirations.length}</span>
          </div>

          <PageToolbar
            className="pd-inspiration-actions"
            actions={[
              {
                key: 'favorite',
                label: favoriteOnly ? '显示全部' : '只看收藏',
                title: favoriteOnly ? '显示全部灵感' : '只看收藏的灵感',
                icon: <Star size={13} fill={favoriteOnly ? 'currentColor' : 'none'} />,
                priority: 1,
                active: favoriteOnly,
                tone: favoriteOnly ? '#D99A0B' : undefined,
                onClick: () => setFavoriteOnly(!favoriteOnly),
              },
              {
                key: 'market',
                label: '资源市集',
                title: '前往资源市集浏览灵感',
                icon: <Store size={13} />,
                priority: 2,
                onClick: () => navigate('/market?tab=inspiration'),
              },
              {
                key: 'transfer',
                label: '导入/导出',
                title: '导入或导出灵感',
                icon: <ArrowDownUp size={13} />,
                priority: 1,
                alwaysVisible: true,
                menuItems: [
                  {
                    key: 'file-import',
                    label: '文件导入',
                    title: '导入 JSON 灵感或 ZIP 备份',
                    icon: <Download size={13} />,
                    onClick: () => void handleImport(),
                  },
                  {
                    key: 'org-import',
                    label: '从组织导入',
                    title: '从组织共享空间导入灵感',
                    icon: <Building2 size={13} />,
                    onClick: () => void openOrgImport(),
                  },
                  {
                    key: 'export-all',
                    label: exportingZip ? '正在导出…' : '全量导出',
                    title: '把全部灵感导出为 ZIP 备份',
                    icon: <Upload size={13} />,
                    disabled: exportingZip || inspirations.length === 0,
                    onClick: () => void handleExportZip(),
                  },
                ],
              },
              {
                key: 'new',
                label: '新建灵感',
                icon: <Plus size={13} />,
                priority: 0,
                alwaysVisible: true,
                variant: 'primary',
                onClick: openNewForm,
              },
            ]}
          />
        </div>

        <div className="pd-inspiration-filterbar shrink-0">
          <div className="pd-inspiration-search">
            <Search size={13} className="pd-inspiration-search-icon" />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => handleSearch(e.target.value)}
              placeholder="搜索标题、正文或标签"
              className="search-input"
            />
            {searchQuery && (
              <button
                type="button"
                className="pd-inspiration-search-clear pd-btn"
                title="清除搜索"
                aria-label="清除搜索"
                onClick={() => handleSearch('')}
              >
                <X size={12} />
              </button>
            )}
          </div>
          <div className="pd-inspiration-tags">
            <TagFilter />
          </div>
        </div>

        <div className="pd-inspiration-content flex-1 overflow-y-auto pd-scroll-stable">
          {error && (
            <div className="pd-inspiration-error" role="alert">
              <AlertCircle size={16} />
              <div className="min-w-0 flex-1">
                <div className="pd-inspiration-error-title">灵感加载失败</div>
                <div className="pd-inspiration-error-copy">{error}</div>
              </div>
              <button
                type="button"
                className="pd-btn pd-inspiration-error-retry"
                onClick={() => void fetchInspirations()}
              >
                <RefreshCw size={12} />
                重试
              </button>
            </div>
          )}

          {loading && !inspirations.length ? (
            <div className="pd-inspiration-loading">
              <RefreshCw size={15} className="pd-animate-spin" />
              <span>正在读取灵感</span>
            </div>
          ) : inspirations.length === 0 ? (
            <div className="pd-inspiration-empty">
              <div className="pd-inspiration-empty-icon" data-tone={hasFilters ? 'muted' : 'accent'}>
                {hasFilters ? <SearchX size={23} /> : <Lightbulb size={23} />}
              </div>
              <h3>{hasFilters ? '没有匹配的灵感' : '还没有保存的灵感'}</h3>
              <p>
                {hasFilters
                  ? '换个关键词，或清除当前筛选后再看看。'
                  : '把可复用的提示词、方法和片段收进来，之后可以随时发送到会话。'}
              </p>
              <div className="pd-inspiration-empty-actions">
                {hasFilters ? (
                  <>
                    <button
                      type="button"
                      className="pd-btn pd-inspiration-empty-primary"
                      onClick={() => void clearFilters()}
                    >
                      清除筛选
                    </button>
                    <button type="button" className="pd-btn pd-inspiration-empty-secondary" onClick={openNewForm}>
                      <Plus size={12} />
                      新建灵感
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      type="button"
                      className="pd-btn pd-inspiration-empty-primary"
                      onClick={openNewForm}
                    >
                      <Plus size={12} />
                      新建灵感
                    </button>
                    <button
                      type="button"
                      className="pd-btn pd-inspiration-empty-secondary"
                      onClick={() => navigate('/market?tab=inspiration')}
                    >
                      <Store size={12} />
                      浏览资源市集
                    </button>
                    <button
                      type="button"
                      className="pd-btn pd-inspiration-empty-secondary"
                      onClick={() => void openOrgImport()}
                    >
                      <Building2 size={12} />
                      从组织导入
                    </button>
                  </>
                )}
              </div>
            </div>
          ) : (
            <div className="pd-inspiration-grid">
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

        {exportTarget && (
          <InspirationExportDialog
            inspiration={exportTarget}
            onClose={() => setExportTarget(null)}
          />
        )}

        {showOrgImport && (
          <InspirationOrgImportDialog
            onClose={() => setShowOrgImport(false)}
            onImported={async () => {
              await fetchInspirations();
              await fetchTags();
            }}
          />
        )}
      </div>
    </WorkbenchAssetPage>
  );
}
