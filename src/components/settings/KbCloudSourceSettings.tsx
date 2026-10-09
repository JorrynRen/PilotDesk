/**
 * KbCloudSourceSettings — 设置 › 知识库 › 云文档账号
 *
 * 管理云文档平台的账号（第一期：语雀）。一个账号 = 一个平台 + 一套凭证；
 * **账号是复数** —— 同一个人可以有个人号与团队号，或将来接入多个平台，各配一条即可。
 * 配好之后，投喂页的「云文档」模式会让用户从这些账号里挑一个来拉文档。
 *
 * 三条口径必须在这里讲清楚，否则用户会按错误预期使用：
 *   ① Token 只用于**读取**，在平台侧勾「只读」权限就够；
 *   ② 投喂的是**正文纯文本** —— 文档里的图片、附件、画板取不到；
 *   ③ 这是**拉取快照**，不是双向同步：云端改动不会自动跟随，需要重新拉取。
 *
 * 凭证由后端加密存储（AES-256-GCM + DPAPI），前端只能读到"有哪几个账号"。
 */

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Cloud, Info, Loader2, Plus, Trash2 } from 'lucide-react';
import { Select } from '../common/Select';
import { showToast } from '../../utils/toast';
import { errorMessage } from '../../utils/errorMessage';
import {
  CLOUD_SOURCE_LABEL,
  cloudSourceLabel,
  type CloudAccount,
  type CloudSourceId,
} from '../../types/knowledge';

/** 可选平台：加飞书时在这里补一项（后端 `CloudSource` 同步加） */
const CLOUD_SOURCES: CloudSourceId[] = ['yuque'];

/** 语雀个人 Token 的生成入口 */
const YUQUE_TOKEN_URL = 'https://www.yuque.com/settings/tokens';

export function KbCloudSourceSettings() {
  const [accounts, setAccounts] = useState<CloudAccount[] | null>(null);
  const [source, setSource] = useState<CloudSourceId>('yuque');
  const [label, setLabel] = useState('');
  const [token, setToken] = useState('');
  const [adding, setAdding] = useState(false);
  const [removingId, setRemovingId] = useState('');

  const refresh = useCallback(async () => {
    try {
      setAccounts(await invoke<CloudAccount[]>('kb_cloud_accounts_list'));
    } catch (e) {
      showToast(`读取云文档账号失败：${errorMessage(e)}`, 'error');
      setAccounts([]);
    }
  }, []);

  // 推一个微任务再读：避免在 effect 体内同步 setState（会多一轮级联渲染）
  useEffect(() => {
    void Promise.resolve().then(() => refresh());
  }, [refresh]);

  const handleAdd = async () => {
    if (!token.trim()) {
      showToast('请先填写 API Token', 'info');
      return;
    }
    setAdding(true);
    try {
      await invoke<CloudAccount>('kb_cloud_account_add', {
        source,
        label: label.trim() || null,
        token: token.trim(),
      });
      setToken('');
      setLabel('');
      await refresh();
      showToast('账号已添加并通过验证', 'success');
    } catch (e) {
      // 验证不过就不会落盘（后端先验证再写入），所以这里只需把原因说清楚
      showToast(`添加失败：${errorMessage(e)}`, 'error');
    } finally {
      setAdding(false);
    }
  };

  const handleRemove = async (id: string, name: string) => {
    setRemovingId(id);
    try {
      await invoke('kb_cloud_account_remove', { id });
      await refresh();
      showToast(`已删除账号「${name}」`, 'info');
    } catch (e) {
      showToast(`删除失败：${errorMessage(e)}`, 'error');
    } finally {
      setRemovingId('');
    }
  };

  return (
    <div className="space-y-4">
      <div
        className="rounded-lg px-3 py-2.5 text-[11px] leading-relaxed"
        style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)', color: 'var(--text-secondary)' }}
      >
        <div className="flex items-center gap-1.5 mb-1" style={{ color: 'var(--text-primary)' }}>
          <Cloud size={12} />
          <span className="text-xs font-medium">云文档账号</span>
        </div>
        配好账号后，在「知识库 › 投喂 › 云文档」里挑一个账号拉取知识库与文档列表，勾选后直接投喂。
        <b>可以配多个账号</b>（例如个人号与团队号）。Token 只用于<b>读取</b>，在平台侧勾「只读」权限即可；
        投喂的是<b>正文纯文本</b>（图片 / 附件 / 画板取不到），且是<b>拉取快照</b>
        ——云端后续改动不会自动跟随，需重新拉取。
      </div>

      <section>
        <h3 className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>添加账号</h3>

        <div className="space-y-3">
          <div className="flex items-end gap-2">
            <div className="w-[120px] shrink-0">
              <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>平台</label>
              <Select
                value={source}
                onChange={(v) => setSource(v as CloudSourceId)}
                options={CLOUD_SOURCES.map((s) => ({ value: s, label: CLOUD_SOURCE_LABEL[s] }))}
                size="sm"
                className="w-full"
                title="云文档平台；目前支持语雀"
              />
            </div>
            <div className="flex-1 min-w-0">
              <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                账号名（可选）
              </label>
              <input
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="留空则用平台账号名"
                className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
                style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
              />
            </div>
          </div>

          <div>
            <label className="block text-[11px] mb-1.5" style={{ color: 'var(--text-secondary)' }}>
              {CLOUD_SOURCE_LABEL[source]} API Token
            </label>
            <input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="粘贴 Token"
              className="w-full px-2.5 py-1.5 rounded-lg text-xs outline-none"
              style={{ backgroundColor: 'var(--bg-field)', color: 'var(--text-primary)', border: '1px solid var(--border)' }}
            />
            <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
              {CLOUD_SOURCE_LABEL[source]}：登录后到「账户设置 › Token」新建一个即可（
              <a href={YUQUE_TOKEN_URL} target="_blank" rel="noreferrer" style={{ color: 'var(--accent)' }}>
                {YUQUE_TOKEN_URL}
              </a>
              ）。点「添加」时会先验证，验证不通过不会写入。
            </div>
          </div>

          <div className="flex items-center gap-2 pt-1">
            <button
              onClick={() => void handleAdd()}
              disabled={adding || !token.trim()}
              className="pd-btn flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs"
              style={{
                backgroundColor: 'var(--accent)',
                color: '#fff',
                border: 'none',
                opacity: adding || !token.trim() ? 0.5 : 1,
              }}
            >
              {adding ? <Loader2 size={11} className="animate-spin" /> : <Plus size={11} />}
              {adding ? '验证中…' : '添加'}
            </button>
          </div>
        </div>
      </section>

      <section className="pt-3" style={{ borderTop: '1px solid var(--border)' }}>
        <h3 className="text-xs font-medium mb-3" style={{ color: 'var(--text-primary)' }}>
          已配置账号{accounts ? `（${accounts.length}）` : ''}
        </h3>

        {!accounts ? (
          <div className="flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            <Loader2 size={11} className="animate-spin" />
            读取中…
          </div>
        ) : accounts.length === 0 ? (
          <div className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
            还没有云文档账号；投喂页的「云文档」模式会提示先到这里添加。
          </div>
        ) : (
          <div className="space-y-1.5">
            {accounts.map((a) => (
              <div
                key={a.id}
                className="flex items-center gap-2 px-2.5 py-1.5 rounded-lg"
                style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
              >
                <span className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
                  {cloudSourceLabel(a.source)}
                </span>
                <span className="flex-1 min-w-0 truncate text-xs" style={{ color: 'var(--text-primary)' }} title={a.label}>
                  {a.label}
                </span>
                <button
                  onClick={() => void handleRemove(a.id, a.label)}
                  disabled={removingId === a.id}
                  className="pd-btn shrink-0 p-1 rounded"
                  style={{ color: 'var(--status-danger, #EF4444)', opacity: removingId === a.id ? 0.5 : 1 }}
                  title="删除这个账号（已投喂进知识库的内容不受影响）"
                >
                  {removingId === a.id ? <Loader2 size={11} className="animate-spin" /> : <Trash2 size={11} />}
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      <div className="flex items-start gap-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
        <Info size={11} className="shrink-0 mt-[1px]" />
        Token 由后端加密存储（AES-256-GCM + DPAPI），不会回传到界面；投喂时逐篇拉取并留有间隔，
        避免触发平台限流。删除账号只影响之后的拉取，已经入库的知识不受影响。
      </div>
    </div>
  );
}
