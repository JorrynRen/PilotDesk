// 确认块的纯函数工具：解析确认请求、把回复格式化为回传文本。
// 从 ConfirmationCard.tsx 拆出，避免组件文件混出非组件导出（react-refresh/only-export-components）。

import type {
  GroupChatConfirmationRequest,
  GroupChatConfirmationResponseInput,
} from '../../types/groupchat';

/** 解析确认请求消息/事件的 JSON（items/options 补齐安全数组）。 */
export function parseConfirmation(raw: string | undefined | null): GroupChatConfirmationRequest | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    if (!v || typeof v !== 'object' || typeof v.replyMode !== 'string') return null;
    if (!Array.isArray(v.items)) v.items = [];
    for (const it of v.items) {
      if (it && !Array.isArray(it.options)) it.options = [];
    }
    return v as GroupChatConfirmationRequest;
  } catch {
    return null;
  }
}

/** 把确认回复格式化为可读文本（label：value，仅含被勾选项），供会话模式回传模型。 */
export function formatResponsesToText(
  items: GroupChatConfirmationRequest['items'],
  responses: GroupChatConfirmationResponseInput[],
): string {
  return responses
    .map((r) => {
      const label = items.find((it) => it.id === r.itemId)?.label?.trim();
      return `${label || r.itemId}：${r.value}`;
    })
    .join('\n');
}
