/**
 * 工作流入参表单：手动「运行」前按定义的 inputSchema 收集本次入参。
 *
 * 为什么需要它：顶层执行（手动 / 定时 / 事件触发）没有上游节点提供入参，
 * 定义里的 inputSchema 若只当校验用，就成了"标了 required 反而跑不起来"的摆设。
 * 这里让 schema 真正长出输入口——字段类型/说明/default 全部取自 schema，预填 default，
 * 未填且非必填的字段不下发（交给后端的 default 兜底）。
 */
import { useState } from 'react';
import { X, Play } from 'lucide-react';

/** 与定义属性里的 inputSchema 字段规格一致（见 types/workflow.ts） */
export type InputSchemaField = {
  type: string;
  description?: string;
  required?: boolean;
  /** schema 里声明的默认值，来自用户/外部 JSON，形态未知，使用处按需窄化 */
  default?: unknown;
};

interface Props {
  definitionName: string;
  schema: Record<string, InputSchemaField>;
  onRun: (input: Record<string, unknown>) => void;
  onClose: () => void;
}

type FieldValue = { text: string; bool: boolean };

/** 把 schema 的 default 预填成表单初值（对象/数组按 JSON 文本编辑） */
function initialValue(field: InputSchemaField): FieldValue {
  const def = field.default;
  if (field.type === 'boolean') return { text: '', bool: def === true };
  if (def === undefined || def === null) return { text: '', bool: false };
  if (typeof def === 'string') return { text: def, bool: false };
  return { text: JSON.stringify(def), bool: false };
}

export function WorkflowInputDialog({ definitionName, schema, onRun, onClose }: Props) {
  const fields = Object.entries(schema);

  const [values, setValues] = useState<Record<string, FieldValue>>(
    () => Object.fromEntries(fields.map(([key, field]) => [key, initialValue(field)])),
  );
  const [error, setError] = useState<string | null>(null);

  const setField = (key: string, patch: Partial<FieldValue>) =>
    setValues((prev) => ({ ...prev, [key]: { ...prev[key], ...patch } }));

  const handleRun = () => {
    const input: Record<string, unknown> = {};
    for (const [key, field] of fields) {
      const raw = values[key] ?? { text: '', bool: false };
      const text = raw.text.trim();

      if (field.type === 'boolean') {
        input[key] = raw.bool;
        continue;
      }
      if (!text) {
        // 留空即"不下发"：required 由这里拦下，其余交给后端 default 兜底
        if (field.required) {
          setError(`「${key}」为必填参数`);
          return;
        }
        continue;
      }

      switch (field.type) {
        case 'number':
        case 'integer': {
          const num = Number(text);
          if (!Number.isFinite(num) || (field.type === 'integer' && !Number.isInteger(num))) {
            setError(`「${key}」需要${field.type === 'integer' ? '整数' : '数字'}`);
            return;
          }
          input[key] = num;
          break;
        }
        case 'array':
        case 'object': {
          try {
            const parsed = JSON.parse(text);
            const ok = field.type === 'array' ? Array.isArray(parsed) : parsed !== null && typeof parsed === 'object' && !Array.isArray(parsed);
            if (!ok) {
              setError(`「${key}」需要${field.type === 'array' ? ' JSON 数组' : ' JSON 对象'}`);
              return;
            }
            input[key] = parsed;
          } catch {
            setError(`「${key}」的 JSON 格式有误`);
            return;
          }
          break;
        }
        default:
          input[key] = text;
      }
    }
    setError(null);
    onRun(input);
  };

  const inputStyle = {
    backgroundColor: 'var(--bg-tertiary)',
    color: 'var(--text-primary)',
    border: '1px solid var(--border)',
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.5)' }}
      onClick={onClose}
    >
      <div
        className="w-[480px] max-h-[80vh] rounded-xl shadow-2xl flex flex-col"
        style={{ backgroundColor: 'var(--bg-primary)', border: '1px solid var(--border)' }}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3 shrink-0" style={{ borderBottom: '1px solid var(--border)' }}>
          <div className="flex items-center gap-2 min-w-0">
            <Play size={15} style={{ color: 'var(--accent)', flexShrink: 0 }} />
            <span className="text-sm font-medium truncate" style={{ color: 'var(--text-primary)' }}>
              运行「{definitionName}」· 填写输入参数
            </span>
          </div>
          <button onClick={onClose} className="pd-btn p-1 rounded hover:opacity-80" style={{ color: 'var(--text-tertiary)' }}>
            <X size={14} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-3 pd-scroll-stable">
          {fields.map(([key, field]) => (
            <div key={key}>
              <label className="block text-xs font-medium mb-1.5" style={{ color: 'var(--text-secondary)' }}>
                {key}
                <span className="ml-1.5 text-[10px] font-normal" style={{ color: 'var(--text-tertiary)' }}>
                  {field.type}
                  {field.required ? ' · 必填' : ''}
                </span>
              </label>

              {field.type === 'boolean' ? (
                <label className="inline-flex items-center gap-2 text-xs" style={{ color: 'var(--text-primary)' }}>
                  <input
                    type="checkbox"
                    checked={values[key]?.bool ?? false}
                    onChange={(e) => setField(key, { bool: e.target.checked })}
                  />
                  是
                </label>
              ) : field.type === 'array' || field.type === 'object' ? (
                <textarea
                  value={values[key]?.text ?? ''}
                  onChange={(e) => setField(key, { text: e.target.value })}
                  rows={3}
                  placeholder={field.type === 'array' ? '["a", "b"]' : '{"k": "v"}'}
                  className="w-full px-3 py-2 text-xs rounded-lg outline-none resize-none"
                  style={{ ...inputStyle, fontFamily: 'monospace' }}
                />
              ) : (
                <input
                  value={values[key]?.text ?? ''}
                  onChange={(e) => setField(key, { text: e.target.value })}
                  type={field.type === 'number' || field.type === 'integer' ? 'number' : 'text'}
                  placeholder={field.description || ''}
                  className="w-full px-3 py-2 text-xs rounded-lg outline-none transition-colors"
                  style={inputStyle}
                  autoFocus={fields[0]?.[0] === key}
                />
              )}

              {field.description && field.type !== 'string' && (
                <div className="mt-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{field.description}</div>
              )}
            </div>
          ))}

          {error && (
            <div className="p-2 rounded text-xs" style={{ backgroundColor: 'rgba(239,68,68,0.1)', color: '#EF4444' }}>
              {error}
            </div>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 px-5 py-3 shrink-0" style={{ borderTop: '1px solid var(--border)' }}>
          <span className="mr-auto text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
            留空的非必填参数将使用定义里的 default
          </span>
          <button
            onClick={onClose}
            className="pd-btn px-4 py-1.5 text-xs rounded"
            style={{ border: '1px solid var(--border)', background: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
          >
            取消
          </button>
          <button
            onClick={handleRun}
            className="pd-btn px-4 py-1.5 text-xs rounded"
            style={{ backgroundColor: 'var(--accent)', color: '#fff' }}
          >
            运行
          </button>
        </div>
      </div>
    </div>
  );
}
