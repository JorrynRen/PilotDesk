import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { X, Download } from 'lucide-react';
import type { InspirationItem } from '../../stores/inspirationStore';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';

/** 市场 id 规则：与后端 `validate_market_inspiration_id`、索引生成器同源，不得漂移 */
const MARKET_ID_PATTERN = /^[a-z0-9][a-z0-9-]*$/;
const MARKET_ID_MAX_LEN = 64;

/** 本地即时校验：返回错误文案，合规则返回 null（最终仍以后端命令的校验为准） */
function validateMarketId(id: string): string | null {
  if (!id) return '市场 id 不能为空';
  if (id.length > MARKET_ID_MAX_LEN) return `长度 ${id.length} 超过上限 ${MARKET_ID_MAX_LEN}，请缩短`;
  if (!MARKET_ID_PATTERN.test(id)) return '仅允许小写字母 / 数字 / 短横线（-），且必须以字母或数字开头';
  return null;
}

interface InspirationExportDialogProps {
  inspiration: InspirationItem;
  onClose: () => void;
}

/**
 * 「导出为市场 JSON（投稿用）」弹窗。
 *
 * 市场 id 预填后端由标题派生的合规默认值，用户可改；改动即实时校验。
 * 确认后用系统保存对话框选路径，再调 `export_inspiration_json` 落盘。
 * 导出内容**只含市场格式字段**（id/title/icon/tags/content），不含本地元数据。
 */
export function InspirationExportDialog({ inspiration, onClose }: InspirationExportDialogProps) {
  const [marketId, setMarketId] = useState('');
  const [exporting, setExporting] = useState(false);

  // 预填默认 id（后端派生）；失败就留空由用户自填
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const id = await invoke<string>('suggest_market_inspiration_id', { title: inspiration.title });
        if (alive) setMarketId(id);
      } catch {
        /* 预填失败不阻断，用户手动填写即可 */
      }
    })();
    return () => {
      alive = false;
    };
  }, [inspiration.title]);

  const error = validateMarketId(marketId);

  const handleExport = async () => {
    if (error) return;
    setExporting(true);
    try {
      const destPath = await saveDialog({
        defaultPath: `${marketId}.json`,
        filters: [{ name: 'JSON', extensions: ['json'] }],
      });
      if (!destPath) {
        setExporting(false);
        return;
      }
      await invoke('export_inspiration_json', {
        id: inspiration.id,
        marketId,
        destPath,
      });
      showToast('已导出，可在市场投稿页上传该文件', 'success');
      onClose();
    } catch (e) {
      showToast(`导出失败：${errorMessage(e)}`, 'error');
      setExporting(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="w-[520px] max-h-[80vh] rounded-2xl overflow-hidden flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3" style={{ borderBottom: '1px solid var(--border)' }}>
          <h3 className="text-sm font-medium" style={{ color: 'var(--text-primary)' }}>
            导出为市场 JSON（投稿用）
          </h3>
          <button onClick={onClose} className="pd-btn p-1 rounded" style={{ color: 'var(--text-secondary)' }}>
            <X size={16} />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-3">
          <p className="text-xs leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
            将「{inspiration.title}」导出为符合灵感市场格式的 JSON，之后可在市场投稿页上传该文件。
          </p>

          <div>
            <label className="block text-xs mb-1.5" style={{ color: 'var(--text-secondary)' }}>
              市场 id
            </label>
            <input
              type="text"
              value={marketId}
              onChange={(e) => setMarketId(e.target.value)}
              placeholder="例如 code-review-checklist"
              className="w-full px-3 py-2 rounded-lg text-sm outline-none"
              style={{
                backgroundColor: 'var(--bg-tertiary)',
                color: 'var(--text-primary)',
                border: `1px solid ${error ? 'var(--danger, #EF4444)' : 'var(--border)'}`,
              }}
            />
            <p className="text-[10px] mt-1.5 leading-relaxed" style={{ color: error ? 'var(--danger, #EF4444)' : 'var(--text-tertiary)' }}>
              {error ?? `规则：仅小写字母 / 数字 / 短横线（-），以字母或数字开头，长度 ≤ ${MARKET_ID_MAX_LEN}；id 即市场目录名，一旦发布不可更改。`}
            </p>
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-end gap-2 px-5 py-3" style={{ borderTop: '1px solid var(--border)' }}>
          <button
            onClick={onClose}
            className="pd-btn px-4 py-2 rounded-lg text-sm"
            style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)' }}
          >
            取消
          </button>
          <button
            onClick={handleExport}
            disabled={!!error || exporting}
            className="pd-btn px-4 py-2 rounded-lg text-sm flex items-center gap-1.5 disabled:opacity-50"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            <Download size={13} />
            {exporting ? '导出中...' : '选择位置并导出'}
          </button>
        </div>
      </div>
    </div>
  );
}
