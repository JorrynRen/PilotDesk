#!/usr/bin/env node
/**
 * validate-plugin.mjs — PilotDesk 插件包静态校验。
 *
 * 规则与后端 `PluginHost::validate_manifest`（src-tauri/src/plugin/mod.rs）一致：
 * 同一条失败在安装/发现阶段会被后端拒绝或让插件整体不可见，所以必须在交付前跑通。
 * 额外检查后端**不校验**但会导致插件"能装上却不能用"的项：入口 JS 语法约束、
 * 未知权限（会让插件被跳过）、面板组件是否真的注册、README.md 的可读性
 * （文件名/长度/相对路径引用/权限说明）。
 *
 * 用法: node validate-plugin.mjs <插件目录>
 * 退出码: 0 = 无 error；1 = 有 error（warning 不影响退出码）
 */

import { readFileSync, existsSync, statSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';

/** 合法权限（后端 ALL_PERMISSIONS，16 项） */
const ALL_PERMISSIONS = [
  'ui:panel', 'ui:toast', 'ui:modal',
  'session:read', 'session:write', 'session:execute',
  'data:invoke', 'storage:*', 'fs:read', 'fs:write', 'shell:exec',
  'plugin:call', 'plugin:events',
  'workflow:read', 'workflow:write', 'workflow:trigger',
];
/** 无需声明即视为可用 */
const DEFAULT_PERMISSIONS = ['ui:toast', 'storage:*'];
/** 高风险权限：安装后会在插件卡片上标注，交付时必须向用户说明用途 */
const HIGH_RISK = ['fs:read', 'fs:write', 'data:invoke', 'shell:exec', 'session:execute'];
/** manifest.json 大小上限（后端 max_manifest_size） */
const MAX_MANIFEST_BYTES = 64 * 1024;

const errors = [];
const warnings = [];
const notes = [];
const err = (m) => errors.push(m);
const warn = (m) => warnings.push(m);
const note = (m) => notes.push(m);

const dir = process.argv[2];
if (!dir) {
  console.error('用法: node validate-plugin.mjs <插件目录>');
  process.exit(1);
}
const pluginDir = resolve(dir);
if (!existsSync(pluginDir) || !statSync(pluginDir).isDirectory()) {
  console.error(`不是一个目录: ${pluginDir}`);
  process.exit(1);
}

// ── 1. manifest.json ──
const manifestPath = join(pluginDir, 'manifest.json');
if (!existsSync(manifestPath)) {
  err('缺少 manifest.json（插件目录下必须有它，否则发现阶段直接跳过）');
  report();
}
const size = statSync(manifestPath).size;
if (size > MAX_MANIFEST_BYTES) {
  err(`manifest.json 超过 ${MAX_MANIFEST_BYTES} 字节上限（当前 ${size}）`);
}
let manifest;
try {
  manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
} catch (e) {
  err(`manifest.json 不是合法 JSON: ${e.message}`);
  report();
}

// id：非空、不含 .. / \、≤128 字符
const id = manifest.id;
if (typeof id !== 'string' || id.trim() === '') {
  err('id 必填且不能为空');
} else {
  if (id.includes('..') || id.includes('/') || id.includes('\\')) {
    err(`id 不能包含路径分隔符或 ".."（当前 "${id}"）`);
  }
  if (id.length > 128) err(`id 不能超过 128 字符（当前 ${id.length}）`);
  if (!/^[a-z0-9][a-z0-9._-]*$/i.test(id)) {
    warn(`id "${id}" 建议只用字母/数字/点/下划线/短横线，且不以符号开头（目录名即 id，特殊字符易踩坑）`);
  }
}
// name：非空、≤64 字符
if (typeof manifest.name !== 'string' || manifest.name.trim() === '') {
  err('name 必填且不能为空');
} else if (manifest.name.length > 64) {
  err(`name 不能超过 64 字符（当前 ${manifest.name.length}）`);
}
// version：按 . 切分后段数 ≥2 且无空段
if (typeof manifest.version !== 'string' || manifest.version.trim() === '') {
  err('version 必填且不能为空');
} else {
  const segs = manifest.version.split('.');
  if (segs.length < 2 || segs.some((s) => s.trim() === '')) {
    err(`version 需为至少两段的点分版本号（如 1.0.0），当前 "${manifest.version}"`);
  }
}
// permissions：必须全部在合法清单内，否则整个插件不可用
const permissions = Array.isArray(manifest.permissions) ? manifest.permissions : [];
if (!Array.isArray(manifest.permissions)) {
  err('permissions 必须是数组（没有权限需求就写 []）');
}
for (const p of permissions) {
  if (!ALL_PERMISSIONS.includes(p)) {
    err(`未知权限 "${p}"：不在合法清单内 → 插件会被判定"含未授权权限"，禁止启用且不注册任何贡献点`);
  }
}
// entry.main：不含 ..、文件必须真实存在
const main = manifest.entry && manifest.entry.main;
if (typeof main !== 'string' || main.trim() === '') {
  err('entry.main 必填（一般是 "index.js"）');
} else {
  if (main.includes('..')) err(`entry.main 不能包含 ".."（当前 "${main}"）`);
  const entryPath = join(pluginDir, main);
  if (!existsSync(entryPath)) err(`entry.main 指向的文件不存在: ${main}`);
}
/**
 * 应用会投递到插件的事件（宿主 → 插件，命名空间 `动作:对象`）。
 *
 * 注意：这是**仅通知**通道 —— handler 的返回值/异常都不影响应用本身，
 * 没有「发送前拦截」这类能力（不存在 message:before-send）。
 */
const SUPPORTED_EVENTS = [
  'session:created',   // 新会话创建成功后：{ sessionId, agentType }
  'session:deleted',   // 会话删除成功后：{ sessionId }
  'message:sent',      // 消息已发出后：{ sessionId, messageId, role }
];
/** 前缀匹配的事件族：`workflow:<类型>`（类型见 WorkflowEventType，如 workflow:instance:completed） */
const SUPPORTED_EVENT_PREFIXES = ['workflow:'];

/** 判断事件名是否为应用会投递的事件 */
function isSupportedEvent(event) {
  return SUPPORTED_EVENTS.includes(event) || SUPPORTED_EVENT_PREFIXES.some((p) => event.startsWith(p));
}

// contributes：后端不校验，但写错会静默失效
const contributes = manifest.contributes || {};
const panels = contributes.panels || [];
for (const [i, p] of panels.entries()) {
  if (!p || typeof p.id !== 'string' || p.id.trim() === '') err(`contributes.panels[${i}].id 必填`);
  if (!p || typeof p.title !== 'string' || p.title.trim() === '') err(`contributes.panels[${i}].title 必填`);
}
const commands = contributes.commands || [];

/** 平台生成参数表单允许的属性类型（与后端 validate_manifest 一致） */
const INPUT_TYPES = ['string', 'number', 'integer', 'boolean', 'array', 'object'];
/** 平台自己管理的参数键，命令的 input 模式声明它们无意义 */
const RESERVED_PARAM_KEYS = ['plugin_id', 'commandId', '__input__'];

/** 校验命令的 input 模式：平台按它生成节点参数表单，写错会让字段在界面上丢失 */
function checkCommandInput(command, index) {
  const input = command.input;
  if (input === undefined) return;
  const where = `contributes.commands[${index}] "${command.id}"`;
  if (!input || typeof input !== 'object' || Array.isArray(input)) {
    err(`${where} 的 input 必须是对象`);
    return;
  }
  // input.type 可省略（省略即 object），写了就必须是 "object"
  if (input.type !== undefined && input.type !== 'object') {
    err(`${where} 的 input.type 必须是 "object"`);
    return;
  }
  const properties = input.properties || {};
  if (typeof properties !== 'object' || Array.isArray(properties)) {
    err(`${where} 的 input.properties 必须是对象`);
    return;
  }
  for (const [key, property] of Object.entries(properties)) {
    if (RESERVED_PARAM_KEYS.includes(key)) {
      warn(`${where}.input.properties.${key} 是平台管理的参数键，声明它不会出现在表单里`);
      continue;
    }
    if (!property || typeof property !== 'object' || typeof property.type !== 'string') {
      err(`${where}.input.properties.${key} 必须声明 type`);
    } else if (!INPUT_TYPES.includes(property.type)) {
      err(`${where}.input.properties.${key}.type = "${property.type}" 不支持，可选: ${INPUT_TYPES.join(', ')}`);
    }
    if (property && property.enum !== undefined && (!Array.isArray(property.enum) || property.enum.length === 0)) {
      err(`${where}.input.properties.${key}.enum 必须是非空数组`);
    }
    // enumLabels 与 enum 按下标对齐，是给界面看的中文名（值本身仍走 enum）。
    // 多写没有对应取值 → 报错；少写会按位回落为原始值 → 只提示。
    if (property && property.enumLabels !== undefined) {
      const labels = property.enumLabels;
      const enumLen = Array.isArray(property.enum) ? property.enum.length : 0;
      if (!Array.isArray(labels) || labels.some((x) => typeof x !== 'string')) {
        err(`${where}.input.properties.${key}.enumLabels 必须是字符串数组`);
      } else if (enumLen === 0) {
        err(`${where}.input.properties.${key} 写了 enumLabels 但没有 enum：下拉框按 enum 生成，没有 enum 就没有选项`);
      } else if (labels.length > enumLen) {
        err(`${where}.input.properties.${key}.enumLabels 有 ${labels.length} 项，超过 enum 的 ${enumLen} 项：多出来的没有对应取值`);
      } else if (labels.length < enumLen) {
        note(`${where}.input.properties.${key}.enumLabels 只给了 ${labels.length}/${enumLen} 项，其余项在界面上显示原始值`);
      }
    }
  }
  if (input.required !== undefined) {
    if (!Array.isArray(input.required) || input.required.some((key) => typeof key !== 'string')) {
      err(`${where}.input.required 必须是字符串数组`);
    } else {
      for (const key of input.required) {
        if (!(key in properties)) err(`${where}.input.required 里的 "${key}" 未在 properties 中声明`);
      }
    }
  }
}

const seenCommandIds = new Set();
for (const [i, c] of commands.entries()) {
  if (!c || typeof c.id !== 'string' || c.id.trim() === '') {
    err(`contributes.commands[${i}].id 必填（工作流节点按它调用插件命令）`);
    continue;
  }
  if (seenCommandIds.has(c.id)) {
    err(`contributes.commands[${i}].id "${c.id}" 重复：工作流节点无法区分这两个命令`);
  }
  seenCommandIds.add(c.id);
  checkCommandInput(c, i);
}
if (contributes.hooks && contributes.hooks.length) {
  const seenHookEvents = new Set();
  for (const [i, h] of contributes.hooks.entries()) {
    const where = `contributes.hooks[${i}]`;
    if (!h || typeof h.event !== 'string' || h.event.trim() === '') {
      err(`${where}.event 必填`);
      continue;
    }
    if (typeof h.handler !== 'string' || h.handler.trim() === '') {
      err(`${where}.handler 必填（入口 export default 里的函数名）`);
    }
    if (seenHookEvents.has(h.event)) {
      warn(`${where}.event "${h.event}" 与前面的钩子重复：同一事件会被注册多次`);
    }
    seenHookEvents.add(h.event);
    if (!isSupportedEvent(h.event)) {
      warn(`${where}.event "${h.event}" 不是应用会投递的事件，永远不会触发。当前支持: ${SUPPORTED_EVENTS.join('、')}（以及 workflow:<类型>，如 workflow:instance:completed）；事件仅通知、不可拦截`);
    }
  }
  note(`contributes.hooks 是**仅通知**：handler 的返回值/异常都不影响应用，没有「发送前拦截」能力（不存在 message:before-send）`);
}
if (contributes.node_types && contributes.node_types.length) {
  note('声明了 node_types：节点类型会注册为工作流节点，但真正执行仍走 plugin_id + command_id');
}
if (contributes.workflow_config) {
  const wf = contributes.workflow_config;
  if (!wf.component && !(wf.components && Object.keys(wf.components).length)) {
    err('contributes.workflow_config 必须声明 component 或 components 之一，否则工作流节点没有参数表单');
  }
  if (wf.component) {
    note(`contributes.workflow_config.component = "${wf.component}"：入口必须 export default 出同名 React 组件，作为所有命令的默认表单`);
  }
  for (const [commandId, componentName] of Object.entries(wf.components || {})) {
    note(`contributes.workflow_config.components["${commandId}"] = "${componentName}"：命令级表单，优先于 component`);
  }
}

// ── 2. 入口 JS ──
if (typeof main === 'string' && existsSync(join(pluginDir, main))) {
  const entryPath = join(pluginDir, main);
  const source = readFileSync(entryPath, 'utf8');

  // 运行时是 new Function('React', source.replace(/export\s+default/, 'return '))
  if (!/export\s+default\s*\{/.test(source)) {
    err('入口文件必须包含 `export default { onLoad, onUnload }`（运行时会把 export default 改写成 return）');
  }
  if (/^\s*import\s|\bimport\s+[\w{*]/m.test(source)) {
    err('入口文件不能使用 import（运行时用 new Function 求值，不是模块环境）');
  }
  if (/\brequire\s*\(/.test(source)) {
    err('入口文件不能使用 require（没有 CommonJS 环境）');
  }
  if (/<\/?[A-Z][\w.]*[\s/>]/.test(source) || /<[a-z]+[\s/>][^>]*>/.test(source)) {
    err('入口文件不能使用 JSX（用 React.createElement 构造元素）');
  }
  if (!/\bonLoad\b/.test(source)) {
    warn('入口文件里没看到 onLoad：插件不会有任何注册动作（面板会退化成默认占位面板）');
  }
  // 面板声明了、但没注册对应 id 的组件 → 用户看到的是 DefaultPluginPanel 占位
  for (const p of panels) {
    if (!p || !p.id) continue;
    if (!source.includes(`'${p.id}'`) && !source.includes(`"${p.id}"`)) {
      warn(`contributes.panels 声明了 "${p.id}"，但入口代码里没有出现该 id：面板会渲染成默认占位面板（需 api.ui.addPanel({ id: '${p.id}', component })）`);
    }
  }
  // 无权限却调用受限 API 是最常见的运行期失败
  const uses = (re) => re.test(source);
  if (uses(/api\.fs\./) && !permissions.includes('fs:read') && !permissions.includes('fs:write')) {
    err('代码调用了 api.fs.*，manifest.permissions 必须声明 fs:read / fs:write（否则拒绝执行，且沙箱默认开启时一律被拒）');
  }
  if (uses(/api\.shell\./) && !permissions.includes('shell:exec')) {
    err('代码调用了 api.shell.exec，manifest.permissions 必须声明 shell:exec');
  }
  if (uses(/api\.agent\./) && !permissions.some((p) => p.startsWith('session:'))) {
    err('代码调用了 api.agent.*，manifest.permissions 必须声明 session:read / session:write / session:execute');
  }
  if (uses(/api\.ui\.addPanel/) && !permissions.includes('ui:panel')) {
    err('代码调用了 api.ui.addPanel，manifest.permissions 必须声明 ui:panel');
  }
  if (uses(/api\.data\.invoke/) && !permissions.includes('data:invoke')) {
    err('代码调用了 api.data.invoke，manifest.permissions 必须声明 data:invoke');
  }
}

// ── 2b. 工作流可用性（contributes.commands + 参数表单来源）──
// 工作流的「插件」节点靠这几样东西工作：命令下拉来自 contributes.commands；
// 参数表单来源有三层，依次优先：命令级组件（workflow_config.components[命令id]）
// → 插件级组件（workflow_config.component）→ 按所选命令的 input 模式由平台生成；
// 运行时按 node.pluginId + node.commandId 调用 api.commands.register 注册的 handler。
// 三者都没有的插件不会出现在插件下拉里。
if (typeof main === 'string' && existsSync(join(pluginDir, main))) {
  const source = readFileSync(join(pluginDir, main), 'utf8');
  for (const c of commands) {
    if (!c || typeof c.id !== 'string' || !c.id) continue;
    if (!source.includes(`'${c.id}'`) && !source.includes(`"${c.id}"`)) {
      err(`contributes.commands 声明了 "${c.id}"，但入口代码里没有这个字符串：工作流选到该命令会报"命令未注册"`);
    } else if (!/api\.commands\.register\s*\(/.test(source)) {
      err('入口没有调用 api.commands.register：命令不会被注册，工作流节点必然失败');
    }
  }
  const wfConfig = contributes.workflow_config || {};
  const wfComp = wfConfig.component;
  const wfComponents = wfConfig.components || {};
  if (typeof wfComp === 'string' && wfComp) {
    if (!new RegExp(`\\b${wfComp}\\s*:`).test(source)) {
      err(`manifest 声明 workflow_config.component = "${wfComp}"，但入口的 export default 里没有该键：工作流节点下方的配置 UI 不会出现`);
    }
    if (!commands.length) {
      warn('声明了 workflow_config 却没有 contributes.commands：节点没有可选命令，配置 UI 也无意义');
    }
  }
  for (const [commandId, componentName] of Object.entries(wfComponents)) {
    if (!seenCommandIds.has(commandId)) {
      err(`workflow_config.components 里的 "${commandId}" 不是已声明的命令 id：这份命令级表单永远不会被渲染`);
    }
    if (typeof componentName !== 'string' || !componentName) {
      err(`workflow_config.components["${commandId}"] 的组件名不能为空`);
    } else if (!new RegExp(`\\b${componentName}\\s*:`).test(source)) {
      err(`workflow_config.components["${commandId}"] = "${componentName}"，但入口的 export default 里没有该键：该命令不会出现参数表单`);
    }
  }
  const schemaFormCommands = commands.filter(
    (c) => c && c.input && c.input.properties && Object.keys(c.input.properties).length > 0,
  );
  const hasAnyForm = Boolean(wfComp) || Object.keys(wfComponents).length > 0 || schemaFormCommands.length > 0;
  if (commands.length && !hasAnyForm) {
    warn('有 contributes.commands 但没有任何参数表单来源（命令级/插件级组件、命令的 input 模式都没有）：该插件不会出现在工作流「插件」节点的插件下拉里。若这些命令只用于插件间调用（api.events.call），可以忽略本条');
  }
  if (commands.length && !wfComp && !Object.keys(wfComponents).length && schemaFormCommands.length > 0) {
    note(`未声明 workflow_config：${schemaFormCommands.length} 个命令的参数表单由平台按 input 模式生成（零 React 代码即可接入工作流）`);
  }
  if (!wfComp && !Object.keys(wfComponents).length) {
    // 常见漏写：入口里写了配置组件，manifest 忘了声明它 —— 组件永远不会被渲染
    const m = /export\s+default\s*\{[\s\S]{0,400}?([A-Za-z_$][\w$]*Config)\s*:/.exec(source);
    if (m) {
      warn(`入口导出了 ${m[1]}（像是节点配置组件），但 manifest 没有声明它（workflow_config.component 或 components）：组件不会被渲染`);
    }
  }
  // input 模式的属性名必须与 handler 读取的 params 键一致，否则表单填了也传不到
  const schemaKeys = schemaFormCommands.flatMap((c) => Object.keys(c.input.properties));
  const missingInSource = [...new Set(schemaKeys)].filter(
    (key) => !source.includes(`params.${key}`) && !source.includes(`'${key}'`) && !source.includes(`"${key}"`),
  );
  if (missingInSource.length) {
    note(`input 模式里声明的这些属性在入口代码里没出现，核对 handler 是否真的读它们: ${missingInSource.join(', ')}`);
  }
  // 参数输入框：自写的 input 没有 {{ 变量补全，用户只能手打键名。
  // 只有当插件**自己写参数表单**（声明了 workflow_config）时才要求用 TemplateField ——
  // 没声明 workflow_config 时，平台按命令的 input 模式自动生成表单，插件根本没有输入框要写；
  // 面板等处的输入框与工作流参数无关，不该因此报警。
  const writesOwnParamForm = Boolean(contributes.workflow_config);
  const hasOwnInput = /createElement\(\s*['"](input|textarea)['"]/.test(source);
  const usesTemplateField = /TemplateField/.test(source);
  if (writesOwnParamForm && hasOwnInput && !usesTemplateField) {
    warn('入口自己写了 input/textarea：自写输入框没有 `{{` 变量补全（用户容易把键名打错）。请改用平台注入的 props.TemplateField（传 value/onChange/variables），见 SKILL「硬规则二」');
  }
  // 含 {{}} 的值一律视为合法输入：对参数做数值转换/校验时必须先豁免模板引用
  const doesNumericCheck = /(Number\s*\(|parseFloat\s*\(|isNaN\s*\()/.test(source);
  const guardsTemplate = /(indexOf|includes|startsWith)\s*\(\s*['"]\{\{/.test(source) || /\/\{\{/.test(source);
  if (doesNumericCheck && !guardsTemplate) {
    warn('入口对参数做了数值转换/校验，但没看到对 `{{变量}}` 的豁免：含变量引用的值会被判成"非数字"。含 `{{` 的值必须跳过校验（类型判定放 handler，并区分"变量引用未解析"与"不是数字"）');
  }
  // 组件里造的默认值必须落进 params：`params.x || '默认'` 只用于显示时，
  // 用户不碰控件（或再选一次已显示的默认项，值没变不触发 onChange）就会让 handler 收到空值。
  const inlineDefaults = new Map();
  for (const match of source.matchAll(/params\.([A-Za-z_$][\w$]*)\s*\|\|\s*['"]([^'"]*)['"]/g)) {
    if (match[2] !== '') inlineDefaults.set(match[1], match[2]);
  }
  const declaredProps = new Map();
  for (const c of commands) {
    for (const [key, prop] of Object.entries(c.input?.properties || {})) {
      declaredProps.set(key, prop);
    }
  }
  const unwiredDefaults = [];
  for (const [key, literal] of inlineDefaults) {
    const prop = declaredProps.get(key);
    if (prop?.default !== undefined) continue;
    // 组件若把这个默认值写回过 params，就不再是"平台不知道的默认值"
    if (new RegExp(`onParamsChange\\(\\s*['"]${key}['"]`).test(source)) continue;
    unwiredDefaults.push(`${key}（显示默认 ${JSON.stringify(literal)}）`);
  }
  if (unwiredDefaults.length) {
    warn(`组件里有"平台不知道的默认值"，且没看到写回 params: ${unwiredDefaults.join('、')}。用户不碰控件时 params 缺该键 → handler 收到空值报错（表现为"选默认项反而失败"）。请在 manifest 的 input.properties[key].default 里声明，或在组件就绪时 onParamsChange 写回`);
  }
  // 默认值只在组件挂载时写回：交互式编辑时没问题（平台切换命令会重建表单实例，
  // 挂载逻辑会重跑），但定时触发/无人打开节点面板就执行时组件根本不会挂载。
  const writesInMountEffect = [...source.matchAll(/useEffect\s*\(\s*function[\s\S]{0,2000}?\}\s*,\s*\[\s*\]\s*\)/g)]
    .map((m) => m[0])
    .some((body) => body.includes('onParamsChange') && /commandId|props\.command/.test(body));
  if (writesInMountEffect) {
    note('默认值写回写在挂载 effect（`useEffect(…, [])`）里：用户打开节点面板后能补上（平台切换命令会重建表单、重跑挂载逻辑），但**定时触发或无人打开面板直接执行**时组件不挂载，params 里就没有这些键、handler 会收到空值。要覆盖这种执行方式，请把默认值写进 manifest 的 `input.properties[key].default`（平台在渲染表单时补齐，且是命令契约的一部分）');
  }
  // 返回信封是常见坑：dispatcher 会把 handler 返回值包成 {success, data}，再返回 {success,data} 就多嵌套一层
  if (/success\s*:\s*(true|false)/.test(source) && commands.length) {
    warn('入口里出现 success: true/false：若这是命令 handler 的返回值，会与 CommandDispatcher 的信封冲突并多嵌套一层；handler 应直接返回载荷、失败时抛错');
  }
  // 数字输入框：会让用户连 {{变量}}、负号以外的表达式都打不进去
  if (/type\s*:\s*['"]number['"]/.test(source)) {
    warn('入口用了 type: "number" 输入框：用户无法输入 {{变量}} 这类模板引用（也会被 Number() 变成 NaN）。节点参数请一律用文本输入，数值校验与转换放到 handler 里');
  }
  // 命令选择器 vs 表单下拉：级联合法（命令=类别、下拉=该类别下的具体项），重复才要改。
  // 判据：表单下拉的选项字面量与命令 id 后缀重叠 → 同一个选择被表达两次。
  const opKey = /onParamsChange\(\s*['"](operation|op|action|type|mode|kind)['"]/.exec(source);
  if (opKey && commands.length > 1) {
    // 只在下拉自己的 options 数组里比对，避免把别处的同名字符串（如命令 id、标签数组）当成选项
    const optionBlocks = [...source.matchAll(/options\s*:\s*\[([\s\S]{0,800}?)\]/g)].map((m) => m[1]).join('\n');
    const suffixes = commands
      .map((c) => String(c.id).split(/[._]/).pop().toLowerCase())
      .filter((s) => s && s.length > 1);
    const overlapping = suffixes.filter((s) => new RegExp(`['"]${s}['"]`).test(optionBlocks));
    if (overlapping.length >= 2) {
      warn(`表单里的 "${opKey[1]}" 下拉选项（${overlapping.join('、')}…）与命令集合含义重叠：同一个选择被表达了两次，且可能自相矛盾（命令选减法、下拉选乘法）。二选一：拆成命令（表单里去掉该下拉），或只声明一个命令（把下拉留在表单里）`);
    } else {
      note(`表单里有 "${opKey[1]}" 下拉且声明了 ${commands.length} 个命令：若是"命令=类别、下拉=该类别的具体项"的级联，保留即可 —— 但必须随命令重置下级、并把下级参数写进各命令的 input 模式（硬规则一、三）`);
    }
  }
  if (commands.length > 1 && wfComp && !Object.keys(wfComponents).length) {
    note(`声明了 ${commands.length} 个命令但只提供一个插件级组件：组件会收到 commandId，可在里面按命令分支；也可以改用 workflow_config.components 或各命令的 input 模式，让平台按命令出表单`);
  }
}

// ── 3. 交付信息 ──
const highRisk = permissions.filter((p) => HIGH_RISK.includes(p));
if (highRisk.length) {
  note(`含高风险权限: ${highRisk.join(', ')} —— 交付时必须向用户说明用途；fs/shell 还需要用户在 设置 › 插件管理 的「沙箱」按钮里禁用沙箱才可用`);
}
const granted = permissions.filter((p) => DEFAULT_PERMISSIONS.includes(p));
if (granted.length) {
  note(`以下权限无需声明即默认可用，写了也不算错: ${granted.join(', ')}`);
}
if (!existsSync(join(pluginDir, 'README.md'))) {
  // 只差大小写时单独提示：Windows 上能读到，大小写敏感的文件系统上读不到
  const nearMatch = readdirSync(pluginDir).find((file) => file.toLowerCase() === 'readme.md');
  if (nearMatch) {
    warn(`说明文档文件名是 "${nearMatch}"：后端只读 README.md（大小写敏感），请改名`);
  } else {
    warn('缺少 README.md：插件条目的 📖 按钮会显示"作者未提供 README 文档"（用户看不出用途、权限与用法）');
  }
} else {
  const readme = readFileSync(join(pluginDir, 'README.md'), 'utf8');
  const lineCount = readme.split('\n').length;
  if (readme.trim().length < 200) {
    warn('README.md 内容过少：至少写清"做什么 / 怎么用 / 要什么权限"三件事');
  } else if (lineCount > 400) {
    warn(`README.md 有 ${lineCount} 行：📖 是 600px 宽、80vh 高的弹窗，建议压到 100 行左右`);
  }
  // 相对路径取不到插件目录（渲染器没有基准目录），图片与链接都会失效
  const relativeRefs = [...readme.matchAll(/!?\[[^\]]*\]\((?!https?:|data:|#|mailto:)([^)\s]+)\)/g)].map((m) => m[1]);
  if (relativeRefs.length) {
    warn(`README.md 里有 ${relativeRefs.length} 处相对路径引用（如 ${relativeRefs[0]}）：渲染器不会把它们解析到插件目录，图片/链接都会失效，请改用 http(s) 或 data: URL`);
  }
  // 权限用途是用户装不装这个插件的判断依据，必须逐条出现在 README 里
  const undocumented = permissions.filter((perm) => !readme.includes(perm));
  if (permissions.length && undocumented.length) {
    warn(`README.md 没提到这些权限：${undocumented.join(', ')} —— 用户靠 README 判断插件要什么权限`);
  }
}

report();

function report() {
  const name = manifest?.name ?? '?';
  const idLabel = manifest?.id ?? '?';
  if (errors.length) {
    console.log(`✗ 校验未通过: ${name} (${idLabel})`);
    for (const e of errors) console.log(`  [error] ${e}`);
  } else {
    console.log(`✓ 校验通过: ${name} (${idLabel})`);
  }
  for (const w of warnings) console.log(`  [warn ] ${w}`);
  for (const n of notes) console.log(`  [info ] ${n}`);
  console.log(`\n插件目录: ${pluginDir}`);
  process.exit(errors.length ? 1 : 0);
}
