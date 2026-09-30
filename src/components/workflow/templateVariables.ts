/**
 * 模板变量的 React 上下文：平台渲染插件参数表单时提供「该节点可引用的变量」。
 *
 * 存在的理由：插件作者常常只写 `props.TemplateField` 而忘了传 `variables`，
 * 结果 `{{` 补全弹层是空列表。变量本来就属于宿主（节点上下文），
 * 因此平台在这里兜底注入，插件显式传 `variables` 时才覆盖（例如只想给候选子集）。
 */

import { createContext, useContext } from 'react';

/** 变量分组（与工作流编辑器内部结构一致，可直接传入） */
export interface TemplateVariableGroup {
  group: string;
  options?: { value: string; label: string }[];
  children?: TemplateVariableGroup[];
}

/** 节点可引用变量；undefined 表示当前上下文没有变量（或不在节点表单内） */
export const TemplateVariablesContext = createContext<TemplateVariableGroup[] | undefined>(undefined);

/** 读取当前节点可引用的变量（TemplateField 在未显式收到 variables 时用它） */
export function useTemplateVariables(): TemplateVariableGroup[] | undefined {
  return useContext(TemplateVariablesContext);
}
