import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import {
  Building2,
  Check,
  Lightbulb,
  Loader2,
  RefreshCw,
  Search,
} from 'lucide-react';
import { ConversionDialogShell } from '../common/ConversionDialogShell';
import { Select } from '../common/Select';
import { UpgradePrompt } from '../common/UpgradePrompt';
import { CAP_TEAM_SHARED_SPACE, useAccountStore } from '../../stores/accountStore';
import { errorMessage } from '../../utils/errorMessage';
import {
  canManageOrganization,
  orgSupportsSharedSpace,
  type OrganizationSummary,
} from '../../utils/orgSharedSpace';
import { showToast } from '../../utils/toast';

interface SharedInspiration {
  id: number;
  name: string;
  description: string;
  icon: string;
  tags: string[];
  creatorLabel: string;
  updatedAt: string;
  revision: number;
}

interface InspirationImportResult {
  created: number;
  updated: number;
  skipped: number;
  errors: string[];
}

interface InspirationOrgImportDialogProps {
  onClose: () => void;
  onImported: () => void | Promise<void>;
}

export function InspirationOrgImportDialog({
  onClose,
  onImported,
}: InspirationOrgImportDialogProps) {
  const openOrganization = useAccountStore((state) => state.openOrganization);
  const [organizations, setOrganizations] = useState<OrganizationSummary[] | null>(null);
  const [organizationId, setOrganizationId] = useState('');
  const [items, setItems] = useState<SharedInspiration[] | null>(null);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [loadingItems, setLoadingItems] = useState(false);
  const [importing, setImporting] = useState(false);
  const [query, setQuery] = useState('');
  const [error, setError] = useState('');
  const loadSequence = useRef(0);

  const selectedOrganization =
    organizations?.find((organization) => String(organization.id) === organizationId) ?? null;
  const capabilityBlocked = Boolean(
    selectedOrganization && !orgSupportsSharedSpace(selectedOrganization),
  );
  const selectedItem = items?.find((item) => item.id === selectedId) ?? null;

  const visibleItems = useMemo(() => {
    if (!items) return [];
    const keyword = query.trim().toLowerCase();
    if (!keyword) return items;
    return items.filter((item) =>
      [item.name, item.description, item.creatorLabel, ...item.tags]
        .join('\n')
        .toLowerCase()
        .includes(keyword),
    );
  }, [items, query]);

  const loadInspirations = useCallback(async (orgId: number) => {
    const sequence = ++loadSequence.current;
    setLoadingItems(true);
    setItems(null);
    setSelectedId(null);
    setQuery('');
    setError('');
    try {
      const shared = await invoke<SharedInspiration[]>('org_list_shared_inspirations', {
        orgId,
      });
      if (sequence !== loadSequence.current) return;
      setItems(shared);
    } catch (err) {
      if (sequence !== loadSequence.current) return;
      setItems([]);
      setError(errorMessage(err));
    } finally {
      if (sequence === loadSequence.current) {
        setLoadingItems(false);
      }
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const loadOrganizations = async () => {
      setOrganizations(null);
      setError('');
      try {
        const result = await invoke<OrganizationSummary[]>('org_list_mine');
        if (cancelled) return;
        setOrganizations(result);
        const first = result[0];
        if (!first) return;
        setOrganizationId(String(first.id));
        if (orgSupportsSharedSpace(first)) {
          await loadInspirations(first.id);
        } else {
          setItems([]);
        }
      } catch (err) {
        if (cancelled) return;
        setOrganizations([]);
        setError(errorMessage(err));
      }
    };
    void loadOrganizations();
    return () => {
      cancelled = true;
    };
  }, [loadInspirations]);

  const handleOrganizationChange = (value: string) => {
    loadSequence.current += 1;
    setOrganizationId(value);
    setSelectedId(null);
    setItems(null);
    setQuery('');
    setError('');
    const next = organizations?.find((organization) => String(organization.id) === value);
    if (!next || !orgSupportsSharedSpace(next)) {
      setItems([]);
      return;
    }
    void loadInspirations(next.id);
  };

  const handleImport = async () => {
    if (!selectedOrganization || selectedId === null || capabilityBlocked) return;
    setImporting(true);
    setError('');
    try {
      const result = await invoke<InspirationImportResult>('org_import_inspiration', {
        input: {
          orgId: selectedOrganization.id,
          resourceId: selectedId,
        },
      });
      await onImported();
      const name = selectedItem?.name || '共享灵感';
      if (result.updated > 0) {
        showToast(`已用组织共享版本更新「${name}」`, 'success');
      } else {
        showToast(`已从「${selectedOrganization.name}」导入「${name}」`, 'success');
      }
      onClose();
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      showToast(`导入失败：${message}`, 'error');
    } finally {
      setImporting(false);
    }
  };

  return (
    <ConversionDialogShell
      icon={<Building2 size={18} />}
      kicker="团队共享"
      title="从组织导入灵感"
      description="选择组织及其共享内容，导入后会与本地灵感按稳定 ID 合并。"
      size="compact"
      busy={importing}
      onClose={onClose}
      bodyClassName="pd-org-import-body"
      footer={
        <>
          <span className="pd-org-import-footnote">
            {selectedItem ? `已选择：${selectedItem.name}` : '请选择一条共享灵感'}
          </span>
          <span className="pd-org-import-footer-spacer" />
          <button
            type="button"
            className="pd-btn"
            disabled={importing}
            onClick={onClose}
          >
            取消
          </button>
          <button
            type="button"
            className="pd-btn pd-btn-primary"
            disabled={importing || selectedId === null || capabilityBlocked}
            onClick={() => void handleImport()}
          >
            {importing ? <Loader2 size={13} className="pd-animate-spin" /> : <Check size={13} />}
            {importing ? '导入中…' : '导入'}
          </button>
        </>
      }
    >
      <div className="pd-org-import-field">
        <div className="pd-org-import-label" id="inspiration-org-label">
          来源组织
        </div>
        {organizations === null ? (
          <div className="pd-org-import-loading">
            <Loader2 size={13} className="pd-animate-spin" />
            正在获取组织列表…
          </div>
        ) : organizations.length === 0 ? (
          <div className="pd-org-import-empty pd-org-import-empty-org">
            <Building2 size={18} />
            <div>
              <strong>还没有可用的组织</strong>
              <span>加入团队后，即可在这里读取组织共享的灵感。</span>
            </div>
            <button
              type="button"
              className="pd-btn"
              onClick={() => void openOrganization({ source: 'inspiration-org-import' })}
            >
              查看组织
            </button>
          </div>
        ) : (
          <div aria-labelledby="inspiration-org-label">
            <Select
              value={organizationId}
              onChange={handleOrganizationChange}
              options={organizations.map((organization) => ({
                value: String(organization.id),
                label: organization.name,
              }))}
              placeholder="选择组织"
              disabled={importing}
              className="w-full"
            />
          </div>
        )}
        {selectedOrganization && (
          <div className="pd-org-import-meta">
            共 {selectedOrganization.memberCount} 位成员 · {selectedOrganization.role === 'owner'
              ? '所有者'
              : selectedOrganization.role === 'admin'
                ? '管理员'
                : '成员'}
          </div>
        )}
      </div>

      {capabilityBlocked ? (
        <UpgradePrompt
          capability={CAP_TEAM_SHARED_SPACE}
          plan="team"
          title="团队共享空间尚未开通"
          description={
            canManageOrganization(selectedOrganization)
              ? `为“${selectedOrganization?.name}”开通团队版后，即可读取组织共享的灵感。`
              : `“${selectedOrganization?.name}”当前档位不含共享空间，请联系组织所有者或管理员。`
          }
          source="inspiration-org-import"
          forceShow
          actionLabel={canManageOrganization(selectedOrganization) ? '查看组织订阅' : '查看组织'}
          onUpgrade={() =>
            void openOrganization({
              organizationId: selectedOrganization?.id,
              capability: CAP_TEAM_SHARED_SPACE,
              source: 'inspiration-org-import',
            })
          }
        />
      ) : (
        <>
          <div className="pd-org-import-list-head">
            <div>
              <span>共享灵感</span>
              <span className="pd-org-import-count">{items?.length ?? 0}</span>
            </div>
            {items && items.length > 0 && (
              <label className="pd-org-import-search">
                <Search size={12} />
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="筛选共享灵感"
                  disabled={importing}
                />
              </label>
            )}
          </div>

          <div className="pd-org-import-list" role="listbox" aria-label="共享灵感列表">
            {loadingItems ? (
              <div className="pd-org-import-loading pd-org-import-loading-list">
                <RefreshCw size={16} className="pd-animate-spin" />
                正在读取共享灵感…
              </div>
            ) : items && items.length === 0 ? (
              <div className="pd-org-import-empty">
                <Lightbulb size={20} />
                <div>
                  <strong>该组织暂无共享灵感</strong>
                  <span>成员共享后，会自动出现在这里。</span>
                </div>
              </div>
            ) : visibleItems.length === 0 ? (
              <div className="pd-org-import-empty">
                <Search size={20} />
                <div>
                  <strong>没有匹配的共享灵感</strong>
                  <span>换个关键词再试。</span>
                </div>
              </div>
            ) : (
              visibleItems.map((item) => {
                const selected = item.id === selectedId;
                return (
                  <button
                    key={item.id}
                    type="button"
                    role="option"
                    aria-selected={selected}
                    className="pd-org-import-item"
                    data-selected={selected ? 'true' : 'false'}
                    disabled={importing}
                    onClick={() => setSelectedId(item.id)}
                  >
                    <span className="pd-org-import-item-icon" aria-hidden>
                      {item.icon || '💡'}
                    </span>
                    <span className="pd-org-import-item-copy">
                      <span className="pd-org-import-item-name" title={item.name}>
                        {item.name}
                      </span>
                      {item.description && (
                        <span className="pd-org-import-item-description">
                          {item.description}
                        </span>
                      )}
                      {item.tags.length > 0 && (
                        <span className="pd-org-import-item-tags">
                          {item.tags.slice(0, 3).map((tag) => (
                            <span key={tag} className="pd-org-import-item-tag">
                              {tag}
                            </span>
                          ))}
                        </span>
                      )}
                      <span className="pd-org-import-item-meta">
                        {item.creatorLabel || '未知成员'} · 第 {Math.max(1, item.revision)} 版
                        {item.updatedAt
                          ? ` · ${new Date(item.updatedAt).toLocaleString()}`
                          : ''}
                      </span>
                    </span>
                    <span className="pd-org-import-item-check" aria-hidden>
                      {selected && <Check size={13} />}
                    </span>
                  </button>
                );
              })
            )}
          </div>
        </>
      )}

      {error && !capabilityBlocked && (
        <div className="pd-conversion-error" role="alert">
          {error}
        </div>
      )}
    </ConversionDialogShell>
  );
}
