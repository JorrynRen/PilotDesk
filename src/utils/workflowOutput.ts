/**
 * 工作流最终产出的归一化。
 *
 * 后端只负责判定「取哪个节点的产出」（见 engine 的 `resolve_final_output`），
 * 取到的原始值形状千差万别：
 *
 * - Agent / 交互节点 → `{ result: "<正文>" }`（outputMapping 包的壳）
 * - End 节点 → `{ "<映射名>": "<值>" }`（inputMapping 组成的对象，可能有多个字段）
 * - API / 插件节点 → 各自返回的任意结构
 *
 * 直接 `JSON.stringify` 展示没法读，所以这里统一做三件事：
 * 1. **解包**：逐层剥掉 `output/content/text/result/value/data` 这类纯包装键；
 * 2. **识别主正文**：在所有叶子文本里取最长的一段作为正文（Markdown 渲染）；
 * 3. **分块**：其余叶子按键值对列出，长文本折叠。
 *
 * 这是纯函数（无 React 依赖），便于单测与在抽屉 / 卡片 / 通知里复用。
 */

/** 视为"纯包装"的键：单键对象遇到它们继续往下剥 */
const WRAPPER_KEYS = ['output', 'content', 'text', 'result', 'value', 'data'];

/** 数组元素全是标量时，拼成多行文本（如"热点新闻列表"） */
function isScalarArray(v: unknown): v is (string | number | boolean)[] {
  return Array.isArray(v) && v.every((x) => typeof x === 'string' || typeof x === 'number' || typeof x === 'boolean');
}

interface Leaf {
  /** 字段路径（用于生成字段名，如 `result.summary`） */
  path: string[];
  value: unknown;
}

/** 把任意结构摊平成叶子列表；标量数组作为一个整体文本叶子 */
function collectLeaves(value: unknown, path: string[], out: Leaf[]): void {
  if (isScalarArray(value)) {
    out.push({ path, value: value.map((x) => String(x)).join('\n') });
    return;
  }
  if (Array.isArray(value)) {
    if (value.length === 0) {
      out.push({ path, value });
      return;
    }
    value.forEach((item, i) => collectLeaves(item, [...path, `[${i + 1}]`], out));
    return;
  }
  if (value !== null && typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) {
      out.push({ path, value });
      return;
    }
    // 单键且键名是包装键：不下沉成字段，直接剥掉（保留外层字段名语义）
    if (entries.length === 1 && WRAPPER_KEYS.includes(entries[0][0]) && path.length > 0) {
      collectLeaves(entries[0][1], path, out);
      return;
    }
    for (const [k, v] of entries) collectLeaves(v, [...path, k], out);
    return;
  }
  out.push({ path, value });
}

/** 叶子是否算"有内容"（与后端 output_is_meaningful 同口径） */
function isMeaningful(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value === 'string') return value.trim().length > 0;
  if (typeof value === 'number' || typeof value === 'boolean') return true;
  if (Array.isArray(value)) return value.some(isMeaningful);
  if (typeof value === 'object') return Object.values(value as Record<string, unknown>).some(isMeaningful);
  return true;
}

/** 单个字段块 */
export interface OutputField {
  /** 展示用字段名 */
  label: string;
  /** 文本化后的值（字符串原样，其余 JSON 美化） */
  text: string;
  /** 是否建议按 Markdown 渲染（长文本 / 含换行或 Markdown 特征） */
  markdown: boolean;
}

export interface NormalizedOutput {
  /** 主正文（Markdown 字符串）；识别不到文本正文时为空串 */
  body: string;
  /** 其余字段 */
  fields: OutputField[];
  /** 原始产出的 JSON 字符串（"查看原始数据"用） */
  rawJson: string;
  /** 是否完全无内容 */
  empty: boolean;
}

/** 字段名美化：`result.summary` → `summary`；数组下标保留为 `items[1].title` */
function fieldLabel(path: string[]): string {
  const parts = path.filter((p) => p.length > 0);
  if (parts.length === 0) return '值';
  if (parts.length === 1) return parts[0];
  return parts.join('.');
}

/** 判断是否值得按 Markdown 渲染 */
function looksLikeMarkdown(text: string): boolean {
  if (text.includes('\n')) return true;
  return /(^|\n)\s*(#{1,6}\s|[-*+]\s|\d+\.\s|```|\|)/.test(text);
}

function toText(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value === null || value === undefined) return '';
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

/**
 * 归一化最终产出。
 *
 * @param output 实例的 `output` 字段（未归一化的原始值）
 */
export function normalizeWorkflowOutput(output: unknown): NormalizedOutput {
  const rawJson = (() => {
    try {
      return JSON.stringify(output ?? null, null, 2);
    } catch {
      return String(output);
    }
  })();

  if (!isMeaningful(output)) {
    return { body: '', fields: [], rawJson, empty: true };
  }

  // 纯字符串：直接就是正文
  if (typeof output === 'string') {
    return { body: output, fields: [], rawJson, empty: false };
  }

  const leaves: Leaf[] = [];
  collectLeaves(output, [], leaves);
  const meaningful = leaves.filter((l) => isMeaningful(l.value));
  if (meaningful.length === 0) {
    return { body: '', fields: [], rawJson, empty: true };
  }

  // 主正文判定分两档（自动识别为主；用户想改可回头调 End 的映射）：
  // 1. 包装键上的文本（`result` / `content` / `output` …）——引擎里 Agent / 交互节点的正文
  //    就在这些键上，优先认它们，免得被 `sessionId` 这类更长的元数据字符串抢走正文位；
  // 2. 没有包装键时，取最长的文本叶子（End 的 inputMapping 字段名是用户自定义的，
  //    认不出语义，只能用"最长的那段才是正文"这条经验规则）。
  const isWrapperLeaf = (leaf: Leaf): boolean => {
    const last = leaf.path[leaf.path.length - 1];
    if (!last) return false;
    return WRAPPER_KEYS.includes(last.replace(/\[\d+\]$/, ''));
  };

  const stringLeaves = meaningful.filter((l) => typeof l.value === 'string' && (l.value as string).trim());
  let bodyLeaf: Leaf | null = stringLeaves.find(isWrapperLeaf) ?? null;
  if (!bodyLeaf) {
    let bodyLen = 0;
    for (const leaf of stringLeaves) {
      const len = (leaf.value as string).trim().length;
      if (len > bodyLen) {
        bodyLen = len;
        bodyLeaf = leaf;
      }
    }
  }

  const fields: OutputField[] = [];
  for (const leaf of meaningful) {
    if (bodyLeaf && leaf === bodyLeaf) continue;
    const text = toText(leaf.value);
    if (!text.trim()) continue;
    fields.push({
      label: fieldLabel(leaf.path),
      text,
      markdown: typeof leaf.value === 'string' && looksLikeMarkdown(text),
    });
  }

  // 全部是结构化字段（没有可当正文的长文本）时，把第一个字段提升为正文更易读
  let body = bodyLeaf ? String(bodyLeaf.value) : '';
  if (!body && fields.length > 0) {
    body = fields[0].text;
    fields.shift();
  }

  return { body, fields, rawJson, empty: false };
}

/** 产出取值来源 → 展示文案（卡片上标注"结果来自哪里"） */
export const OUTPUT_SOURCE_LABEL: Record<string, string> = {
  end: '结束节点',
  'end-upstream': '结束节点的上游',
  'last-node': '最后一个产出节点',
  none: '本次无产出',
};

/** 本次无产出时的解释文案（帮助用户判断是配置问题还是运行问题） */
export function explainEmptyOutput(source?: string, status?: string): string {
  if (source === 'none' || !source) {
    if (status === 'running') return '执行中，结果会在结束后出现在这里';
    if (status === 'pending') return '等待触发，尚未开始执行';
    if (status === 'paused') return '已暂停在人工确认，答复后继续执行';
    if (status === 'failed') return '执行失败，未产生结果';
    if (status === 'cancelled') return '执行已取消，未产生结果';
    if (status === 'timeout') return '执行超时，未产生结果';
    return '本次执行没有产生内容：检查结束节点是否配置了输入映射，或上游节点是否真的输出了内容';
  }
  return '';
}
