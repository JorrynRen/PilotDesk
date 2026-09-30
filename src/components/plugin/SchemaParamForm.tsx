/**
 * SchemaParamForm — 按所选命令的 input 模式生成的通用参数表单
 *
 * 插件没有提供 workflow_config 组件时，工作流节点的参数表单由平台据此渲染：
 * 字段名、类型、说明、默认值、枚举都取自所选命令的 `input.properties`。
 * 文本输入统一用 TemplateField（输入 `{{` 弹出可引用变量），与插件自写组件遵守同一约定：
 * 不做输入校验、必须能写 `{{变量}}`；含模板引用时保持字符串，真实类型由后端解析后交 handler 判定。
 */

import React from 'react';
import type { CommandContribution, CommandInputProperty } from '../../types/plugin';
import { TemplateField } from '../workflow/TemplateField';
import type { TemplateVariableGroup } from '../workflow/templateVariables';
import { containsTemplate } from '../../utils/templateValue';
import { Select } from '../common/Select';

/** 平台自己管理的参数键，命令模式声明这些键无意义 */
const RESERVED_PARAM_KEYS = ['plugin_id', 'commandId', '__input__'];

/**
 * 插件参数值：来源不可信——插件在 input 模式里声明的 default/enum 可以是任意 JSON，
 * 用户输入与模板引用也未经校验，因此统一按 unknown 处理，真实类型由后端解析后交 handler 判定。
 */
type ParamValue = unknown;

interface SchemaParamFormProps {
  /** 节点当前选中的命令贡献点 */
  command: CommandContribution;
  params: Record<string, ParamValue>;
  onParamsChange: (key: string, value: ParamValue) => void;
  /** 该节点可引用的变量（输入映射的键名），用于 `{{` 补全 */
  variables?: TemplateVariableGroup[];
}

/** 按模式类型写出参数值：含模板引用一律按字符串写出，由后端解析 */
function toParamValue(property: CommandInputProperty, text: string): string | number {
  if (containsTemplate(text)) return text;
  if (property.type === 'number' || property.type === 'integer') {
    const trimmed = text.trim();
    if (trimmed === '') return '';
    const parsed = Number(trimmed);
    return Number.isFinite(parsed) ? parsed : text;
  }
  return text;
}

/** 输入框显示值：参数未填写时回落到模式声明的 default */
function toDisplayValue(property: CommandInputProperty, raw: ParamValue): string {
  const value = raw === undefined || raw === null ? property.default : raw;
  if (value === undefined || value === null) return '';
  return typeof value === 'object' ? JSON.stringify(value) : String(value);
}

const inputStyle: React.CSSProperties = {
  width: '100%',
  padding: '6px 10px',
  borderRadius: 'var(--radius-md)',
  border: '1px solid var(--border)',
  background: 'var(--bg-primary)',
  color: 'var(--text-primary)',
  fontSize: 'var(--fs-12)',
  outline: 'none',
};

const labelStyle: React.CSSProperties = {
  fontSize: 'var(--fs-11)',
  color: 'var(--text-tertiary)',
  display: 'block',
  marginBottom: 4,
};

export const SchemaParamForm: React.FC<SchemaParamFormProps> = ({ command, params, onParamsChange, variables }) => {
  const properties = Object.entries(command.input?.properties || {}).filter(
    ([key]) => !RESERVED_PARAM_KEYS.includes(key),
  );
  if (properties.length === 0) return null;
  const requiredKeys = command.input?.required || [];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      {properties.map(([key, property]) => {
        const label = `${key}${requiredKeys.includes(key) ? ' *' : ''}`;
        const displayValue = toDisplayValue(property, params[key]);

        if (Array.isArray(property.enum) && property.enum.length > 0) {
          return (
            <div key={key}>
              <label style={labelStyle}>{label}</label>
              <Select
                className="w-full"
                value={displayValue}
                onChange={(v) => {
                  const option = property.enum!.find((item) => String(item) === v);
                  onParamsChange(key, option);
                }}
                placeholder="（未设置）"
                options={[
                  { value: '', label: '（未设置）' },
                  // enumLabels 与 enum 按下标对齐；缺失/项数不足时回落为原始值（向后兼容）
                  ...property.enum.map((item, i) => ({
                    value: String(item),
                    label: property.enumLabels?.[i] ?? String(item),
                  })),
                ]}
              />
              {property.description && (
                <div style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)', marginTop: 2 }}>
                  {property.description}
                </div>
              )}
            </div>
          );
        }

        if (property.type === 'boolean') {
          return (
            <label key={key} style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 'var(--fs-12)', color: 'var(--text-secondary)' }}>
              <input
                type="checkbox"
                checked={params[key] === true || params[key] === 'true'}
                onChange={(e) => onParamsChange(key, e.target.checked)}
              />
              {label}
              {property.description && (
                <span style={{ fontSize: 'var(--fs-10)', color: 'var(--text-tertiary)' }}>{property.description}</span>
              )}
            </label>
          );
        }

        if (property.type === 'array' || property.type === 'object') {
          return (
            <div key={key}>
              <label style={labelStyle}>{label}</label>
              <TemplateField
                multiline
                rows={3}
                style={{ ...inputStyle, minHeight: 60, fontFamily: 'var(--font-mono)' }}
                value={displayValue}
                variables={variables}
                placeholder={property.description ? `${property.description}（JSON 文本，插件负责解析）` : 'JSON 文本，插件负责解析'}
                onChange={(text) => onParamsChange(key, text)}
              />
            </div>
          );
        }

        return (
          <div key={key}>
            <label style={labelStyle}>{label}</label>
            <TemplateField
              style={inputStyle}
              value={displayValue}
              variables={variables}
              placeholder={property.description || '固定值，或输入映射的变量引用，如 {{text}}'}
              onChange={(text) => onParamsChange(key, toParamValue(property, text))}
            />
          </div>
        );
      })}
    </div>
  );
};
