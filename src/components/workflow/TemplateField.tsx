/**
 * TemplateField — 支持 `{{}}` 变量补全的文本输入（单行 input / 多行 textarea）
 *
 * 宿主字段与插件配置组件共用同一实现：光标前刚输入 `{{` 时弹出可选变量列表，
 * 选中后把刚输入的两个 `{` 替换为 `{{变量}}`。列表内容由调用方通过 `variables` 传入，
 * 语义必须与后端模板引擎一致 —— 插件/agent 节点的模板上下文是「输入映射」的键，
 * 直接写上游节点输出不会被解析，故这里也只列这些键。
 *
 * 本组件不做任何类型校验：含 `{{...}}` 的值一律视为合法输入，
 * 真实类型在后端解析出变量值之后由 handler 判定（见 skill「参数校验与变量」）。
 */

import React, { useEffect, useRef, useState } from 'react';
import { useTemplateVariables } from './templateVariables';
import type { TemplateVariableGroup } from './templateVariables';

export interface TemplateFieldProps {
  value: string;
  /** 值变更回调（补全插入也走这里） */
  onChange: (value: string) => void;
  /**
   * 可用变量；**省略时自动使用平台注入的节点变量**（见 `templateVariables.ts`），
   * 因此插件漏传也不会变成空列表。要限定候选子集时才显式传。
   */
  variables?: TemplateVariableGroup[];
  placeholder?: string;
  /** true 渲染 textarea（默认单行 input） */
  multiline?: boolean;
  rows?: number;
  style?: React.CSSProperties;
  disabled?: boolean;
  /**
   * 是否启用 `{{` 补全（默认启用）。
   * 值本身不是模板时（如 transform 节点的 JS 脚本）必须关掉：那里的 `{{` 不是变量引用。
   */
  enableCompletion?: boolean;
}

const baseFieldStyle: React.CSSProperties = {
  width: '100%',
  padding: '6px 10px',
  borderRadius: 'var(--radius-md)',
  border: '1px solid var(--border)',
  background: 'var(--bg-primary)',
  color: 'var(--text-primary)',
  fontSize: 'var(--fs-12)',
  outline: 'none',
};

const baseTextareaStyle: React.CSSProperties = {
  ...baseFieldStyle,
  resize: 'vertical',
  fontFamily: 'inherit',
};

/** 弹层坐标：多行跟随光标所在行，单行贴在输入框下方；水平居中于字段 */
function selectorPosition(el: HTMLElement, multiline: boolean): { x: number; y: number } {
  const rect = el.getBoundingClientRect();
  if (!multiline) {
    return { x: rect.left + rect.width / 2, y: rect.bottom + 6 };
  }
  const textarea = el as HTMLTextAreaElement;
  const style = getComputedStyle(textarea);
  const lineHeight = parseInt(style.lineHeight) || 20;
  const paddingTop = parseInt(style.paddingTop) || 6;
  const caretLine = textarea.value.slice(0, textarea.selectionStart).split('\n').length - 1;
  return { x: rect.left + rect.width / 2, y: rect.top + paddingTop + (caretLine + 1) * lineHeight };
}

/** 扁平化变量分组（兼容直接给 options 与只给 children 两种写法） */
function flattenGroups(groups: TemplateVariableGroup[] | undefined): Array<{ group: string; options: { value: string; label: string }[] }> {
  const result: Array<{ group: string; options: { value: string; label: string }[] }> = [];
  for (const group of groups || []) {
    if (group.options?.length) {
      result.push({ group: group.group, options: group.options });
    }
    for (const child of group.children || []) {
      if (child.options?.length) {
        result.push({ group: child.group, options: child.options });
      }
    }
  }
  return result;
}

export const TemplateField: React.FC<TemplateFieldProps> = ({
  value,
  onChange,
  variables,
  placeholder,
  multiline = false,
  rows = 4,
  style,
  disabled,
  enableCompletion = true,
}) => {
  /** 触发补全时 `{{` 之后的光标位置 */
  const triggerPosRef = useRef<number>(0);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerPos, setPickerPos] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [dragOffset, setDragOffset] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  // 未显式给变量（undefined）或给了空列表（作者常写 props.variables || []）时，
  // 都用平台注入的节点变量兜底 —— 只有非空的自定义列表才覆盖它。
  // 想彻底关掉补全请用 enableCompletion={false}，而不是传空数组。
  const contextVariables = useTemplateVariables();
  const effectiveVariables = variables && variables.length > 0 ? variables : contextVariables;

  useEffect(() => {
    if (!pickerOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setPickerOpen(false);
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [pickerOpen]);

  const handleChange = (event: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
    const next = event.target.value;
    onChange(next);
    if (!enableCompletion) return;
    const cursorPos = event.target.selectionStart || 0;
    const textBefore = next.slice(0, cursorPos);
    if (textBefore.endsWith('{{')) {
      triggerPosRef.current = cursorPos;
      setDragOffset({ x: 0, y: 0 });
      setPickerPos(selectorPosition(event.target, multiline));
      setPickerOpen(true);
    }
  };

  const insertVariable = (token: string) => {
    const pos = triggerPosRef.current;
    const wrapped = token.startsWith('{{') && token.endsWith('}}') ? token : `{{${token}}}`;
    const current = value || '';
    onChange(current.slice(0, pos - 2) + wrapped + current.slice(pos));
    setPickerOpen(false);
    triggerPosRef.current = 0;
  };

  const renderedGroups = flattenGroups(effectiveVariables);
  const hasOptions = renderedGroups.length > 0;

  return (
    <div style={{ position: 'relative', width: '100%' }}>
      {multiline ? (
        <textarea
          value={value}
          onChange={handleChange}
          placeholder={placeholder}
          rows={rows}
          disabled={disabled}
          style={{ ...baseTextareaStyle, ...style }}
        />
      ) : (
        <input
          type="text"
          value={value}
          onChange={handleChange}
          placeholder={placeholder}
          disabled={disabled}
          style={{ ...baseFieldStyle, ...style }}
        />
      )}

      {pickerOpen && (
        <>
          <div onClick={() => setPickerOpen(false)} style={{ position: 'fixed', inset: 0, zIndex: 99 }} />
          <div
            style={{
              position: 'fixed',
              left: pickerPos.x + dragOffset.x,
              top: pickerPos.y + dragOffset.y,
              transform: 'translateX(-50%)',
              zIndex: 100,
              minWidth: 200,
              maxHeight: 250,
              display: 'flex',
              flexDirection: 'column',
              borderRadius: 'var(--radius-md)',
              border: '1px solid var(--border)',
              background: 'var(--bg-secondary)',
              boxShadow: '0 8px 24px rgba(0,0,0,0.28)',
              overflow: 'hidden',
            }}
          >
            <div
              onMouseDown={(event) => {
                const startX = event.clientX;
                const startY = event.clientY;
                const origin = { ...dragOffset };
                const handleMouseMove = (moveEvent: MouseEvent) => {
                  setDragOffset({ x: origin.x + moveEvent.clientX - startX, y: origin.y + moveEvent.clientY - startY });
                };
                const handleMouseUp = () => {
                  document.removeEventListener('mousemove', handleMouseMove);
                  document.removeEventListener('mouseup', handleMouseUp);
                };
                document.addEventListener('mousemove', handleMouseMove);
                document.addEventListener('mouseup', handleMouseUp);
              }}
              style={{
                padding: '6px 8px',
                fontSize: 'var(--fs-11)',
                color: 'var(--text-secondary)',
                fontWeight: 600,
                borderBottom: '1px solid var(--border)',
                textAlign: 'center',
                cursor: 'grab',
                userSelect: 'none',
                flexShrink: 0,
              }}
            >
              请选择变量…
            </div>
            <div style={{ overflow: 'auto', flex: 1 }}>
              {!hasOptions ? (
                <div style={{ padding: '10px 12px', fontSize: 'var(--fs-11)', color: 'var(--text-tertiary)' }}>
                  暂无可用变量：先在节点的「输入映射」里定义参数名，再回到这里引用。
                </div>
              ) : (
                renderedGroups.map((group, groupIndex) => (
                  <div key={`${group.group}-${groupIndex}`}>
                    {group.group && (
                      <div style={{ padding: '3px 8px', fontSize: 'var(--fs-10)', color: 'var(--text-secondary)', fontWeight: 500, borderBottom: '1px solid var(--border)' }}>
                        {group.group}
                      </div>
                    )}
                    {group.options.map((option) => (
                      <div
                        key={option.value}
                        onClick={() => insertVariable(option.value)}
                        style={{ padding: '5px 8px 5px 16px', cursor: 'pointer', color: 'var(--text-primary)', fontSize: 'var(--fs-11)', borderBottom: '1px solid var(--border)' }}
                        onMouseEnter={(event) => { event.currentTarget.style.background = 'var(--bg-tertiary)'; }}
                        onMouseLeave={(event) => { event.currentTarget.style.background = 'transparent'; }}
                      >
                        {option.label}
                      </div>
                    ))}
                  </div>
                ))
              )}
            </div>
          </div>
        </>
      )}
    </div>
  );
};
