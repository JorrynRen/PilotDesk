---
name: plugin-authoring
description: 把用户需求落成可用的 PilotDesk 插件包（manifest.json 与 index.js 插件目录），含 manifest 校验规则、入口 JS 约束、权限声明、安装位置、校验脚本与最小模板；适用于用户要求做一个插件、加侧栏面板、让工作流能调用某个插件时。
---

# PilotDesk 插件开发

插件 = 一个目录，里面至少 `manifest.json` + 入口 JS（默认 `index.js`）。应用启动与刷新时扫描插件目录、校验 manifest、在前端运行时执行入口 JS，于是插件能贡献**侧栏面板**、**命令**（可被工作流与其他插件调用）、**工作流节点类型**，并能调用应用能力（存储、agent 会话、fs、shell 等）。

## 安装位置与生效

- 插件根目录：`<PilotDesk 配置根>/plugins/`，Windows 上是 `%APPDATA%\PilotDesk\plugins\`。
- 单插件目录名 = `manifest.id`，即 `<配置根>/plugins/<id>/manifest.json`。
- 用 `write_file` 直接写进该目录即可，不需要打包；分发才需要 zip（根目录或单层子目录含 `manifest.json`，在插件列表头部用「+安装」按钮选包）。
- **生效方式（不要在交付说明里写错入口）**：这些都在**右侧边栏的「插件」标签页**（Package 图标）—— **不在设置页**：
  1. 打开「插件」标签页（切到该页时会自动扫描一次）；
  2. 「已安装插件」区块右上角点 **刷新**（`刷新插件列表`）；
  3. 插件条目下若出现红色 `加载错误: ...`，说明入口 JS 执行失败（更细的堆栈看 DevTools 控制台，`[PluginRegistry]` 前缀）；出现黄色"包含未授权权限声明"说明 `permissions` 里有非法项；
  4. **需要用到插件面板时，条目必须是"已启用"**（禁用状态下面板入口不显示）；命令不走这个开关，见下一节。
- 终端模式下右侧栏不显示「插件」标签页，需切回会话模式。

## manifest.json

```json
{
  "id": "demo-panel",
  "name": "示例面板",
  "version": "1.0.0",
  "description": "演示插件：面板 + 可被工作流调用的命令",
  "author": "用户",
  "minAppVersion": "0.1.0",
  "permissions": ["ui:panel"],
  "entry": { "main": "index.js" },
  "contributes": {
    "panels": [{ "id": "demo-panel-main", "title": "示例面板" }],
    "commands": [{ "id": "demo.uppercase", "title": "转大写" }]
  }
}
```

**后端会强制校验的字段（写错会被拒绝或让插件整体不可用）**

| 字段 | 规则 |
|---|---|
| `manifest.json` 大小 | ≤ 64 KB |
| `id` | 非空；不含 `..`、`/`、`\`；≤ 128 字符。目录名必须与它一致 |
| `name` | 非空；≤ 64 字符 |
| `version` | 非空；按 `.` 切分后**段数 ≥ 2 且无空段**（`1.0` 可以，`bad` 不行） |
| `permissions` | 每一项都必须在合法清单内，否则判定"含未授权权限"→ **禁止启用且不注册任何贡献点** |
| `entry.main` | 非空；不含 `..`；指向的文件必须真实存在 |

`description` / `author` / `minAppVersion` / `icon` 不校验（`minAppVersion` 当前完全未使用）。`contributes` 里的面板/钩子不校验，写错只会静默失效；命令 id 重复、`workflow_config` 与命令 `input` 模式的写法**会被校验并拒绝加载**（见「工作流插件贡献规范」）。

**contributes 各贡献点的字段**

- `panels[]`：`{ id, title, icon? }`。`id` 必须与代码里 `api.ui.addPanel({ id })` 一致，否则面板显示为默认占位面板。
- `commands[]`：`{ id, title, input?, output? }`。`id` 就是工作流节点里的 `commandId`，约定 `<插件名>.<动作>`（如 `demo.uppercase`），后端不强制前缀；**工作流节点的命令下拉只认这里的声明**，详见「工作流插件贡献规范」。`input` 是**契约**：平台按它在插件没提供自定义组件时生成参数表单。
- `node_types[]`：`{ type_id, name, config_schema?, permissions? }`。非标准路径，只影响节点面板里是否出现该类型；实际执行仍走 `pluginId` + `commandId`。没有特别需求不要用它。
- `workflow_config`：`{ component?, components? }`，两者至少写一个。`component` 是入口 `export default` 里导出的组件名，作为该插件所有命令的默认参数表单；`components` 是命令级表单映射（键为 `contributes.commands[].id`，值为导出的组件名），**优先于 `component`**。名字必须与导出键逐字一致。
- `hooks[]`：`{ event, handler }` —— **声明式订阅应用事件**：`handler` 是入口 `export default` 里的**函数名**。等价于在 `onLoad` 里写 `api.events.on(event, handler)`；支持的事件见「可用 API 与权限」的**应用事件**表（仅通知、不可拦截）。

## index.js（最容易翻车的地方）

运行时不打包、不转译，实现是：

```js
const src = await invoke('plugin_read_entry', { pluginId });   // 读入口文件原文
// 危险全局（window/globalThis/document/fetch/localStorage/Function...）
// 会被"以同名参数"遮蔽，并以 'use strict' 执行
const module = new Function('window', 'globalThis', /* ... */ 'navigator', 'React',
  "'use strict';\n" + src.replace(/export\s+default\s*/, 'return '))(
    undefined, undefined, /* ... */ navigatorStandin, React);
```

因此入口文件必须满足：

- **必须有且只有一个 `export default { onLoad, onUnload }`**（它会被改写为 `return`）。
- **不能用 `import` / `require` / JSX / TypeScript**：`new Function` 不是模块环境，出现即整体抛错、插件加载失败（控制台只见一条 warn）。UI 用 `React.createElement`（`React` 是注入进来的参数，别自己 import）。
- **不要依赖 `window` / `globalThis` / `document` / `fetch` / `localStorage` / `Function` 等全局**：它们已被"以同名参数"遮蔽为 `undefined`，引用会直接抛错。跨生命周期共享状态用**模块作用域变量**；网络/存储请走 `api.storage` 或声明的权限能力。`navigator` 只提供一个最小只读替身（`userAgent`、`clipboard.writeText`）。
  - `eval` / `arguments` **没法**被遮蔽：函数体是严格模式，这两个名字不允许作形参名（放进去会让 `new Function` 直接抛 `SyntaxError`，插件全部加载失败）。影响很小 —— 直接 `eval('window')` 仍在**当前函数作用域**求值，`window` 照样命中被遮蔽的形参。
- 这只是**纵深防御**，不是绝对隔离 —— 插件与宿主仍在同一 JS realm，语言层面无法彻底阻断逃逸；真正的安全边界在 Rust 侧（命令层的权限与白名单校验）。
- 生命周期：`onLoad(api)` 里做注册；`onUnload()` 里清理自己创建的资源（`setInterval`、模块作用域里的订阅变量）。`api.events.on` 返回退订函数；命令、事件订阅、全局订阅、面板组件由宿主在卸载时自动清理。
- 顶层代码会在加载时执行，但注册动作放 `onLoad` 里，否则拿不到 `api`。

最小可用入口：

```js
// 面板组件：React.createElement，无 JSX
function DemoPanel(props) {
  var state = React.useState(0);
  var count = state[0];
  var setCount = state[1];
  return React.createElement('div', { style: { padding: '12px' } },
    React.createElement('p', null, '点击次数: ', count),
    React.createElement('button', { onClick: function () { setCount(count + 1); } }, '点我')
  );
}

export default {
  onLoad: function (api) {
    // 面板：id 必须与 manifest.contributes.panels[].id 一致
    api.ui.addPanel({ id: 'demo-panel-main', title: '示例面板', component: DemoPanel });

    // 命令：工作流与其他插件按 pluginId + commandId 调用
    api.commands.register('demo.uppercase', async function (params) {
      var text = (params && params.text) || '';
      return { text: String(text).toUpperCase(), length: String(text).length };
    });
  },
  onUnload: function () {
    // 只清理自建资源；注册物由宿主回收
  },
};
```

## 可用 API 与权限

`api` 上按需声明权限，未声明的调用会失败：

| API | 作用 | 需要的权限 |
|---|---|---|
| `ui.addPanel({id,title,component})` / `ui.removePanel(id)` | 在右侧面板注册 UI 组件 | `ui:panel` |
| `ui.showToast(message, type)` | 提示（当前只写控制台） | `ui:toast`（默认可用） |
| `storage.get/set/delete(key)` | 插件私有存储（localStorage，按插件 id 隔离） | `storage:*`（默认可用） |
| `commands.register(id, handler)` / `commands.execute(id, params)` | 注册/调用本插件命令，handler 返回值为调用结果 | 无 |
| `global.on/emit/call(targetPluginId, commandId, params)` | 跨插件通信，`call` 返回 `{success,data,error}` | 无（对方需已注册该命令） |
| `data.invoke(cmd, params)` | 调用**命令白名单**内的只读命令（当前仅 `list_api_providers`、`get_api_provider`）；`get_api_key` / `write_text_file` / `plugin_install_zip` / `terminal_*` / `set_app_setting` 等一律不可达 | `data:invoke`（高风险） |
| `events.on(name, handler)` | **订阅应用事件**（宿主 `session:*` / `message:*` / `workflow:instance:started`，以及其它插件的 `global.emit` 广播）；返回退订函数 | 无（订阅不加权限门；可用 `plugin:events` 表达意图） |
| `hooks.on(name, handler)` | 与 `events.on` **完全等价**（同一实现），保留旧名以兼容 | 无 |
| `agent.createSession/sendMessage/getHistory/listSessions/deleteSession/listAgents` | 驱动应用内 agent 会话 | `session:write` / `session:execute` / `session:read`（**必须声明**） |
| `fs.readText/writeText/delete/exists/readDir(path)` | 文件系统，**路径必须落在本插件目录内** | `fs:read` / `fs:write`（高风险，**必须声明**） |
| `shell.exec(command, {timeout_ms, working_dir})` | 执行命令；`timeout_ms` 默认 30000 | `shell:exec`（高风险，**必须声明**） |

**应用事件（宿主 → 插件，仅通知）**：`api.events.on(name, handler)`（与 `api.hooks.on`、`contributes.hooks` 同一实现）订阅的是宿主事件总线；下列事件由应用在"操作**成功之后**"投递，载荷**只含标识**（不含消息正文 / API Key / 文件内容）。handler 的返回值与异常都**不影响**应用本身 —— 这是纯通知，**不存在** `message:before-send` 这类"发送前拦截/改写"能力：

| 事件 | 触发时机 | 载荷 |
|---|---|---|
| `session:created` | 新会话创建成功后 | `{ sessionId, agentType }` |
| `session:deleted` | 会话删除成功后 | `{ sessionId }` |
| `message:sent` | 消息已发出后（`role` 区分 `user` / `assistant` / `system` / `tool`） | `{ sessionId, messageId, role }` |
| `workflow:instance:started` | 工作流实例成功启动后 | `{ instanceId, nodeId?, data?, timestamp }` |
| `workflow:<其它类型>` | **目前不会触发**：实例的完成/失败、节点与阶段级状态真身在 Rust 侧，前端只做轮询读取，没有投递点。**别依赖它们** | — |

声明式写法：manifest 里 `"hooks": [{ "event": "session:created", "handler": "onSessionCreated" }]`，`handler` 是入口 `export default` 里的函数名（宿主加载插件时自动接线）。`api.events.on` 与其它插件的 `api.global.emit` 共用同一条总线，所以它也能收到跨插件广播。

权限清单（16 项，写别的会被判定未授权）：`ui:panel` `ui:toast` `ui:modal` `session:read` `session:write` `session:execute` `data:invoke` `storage:*` `fs:read` `fs:write` `shell:exec` `plugin:call` `plugin:events` `workflow:read` `workflow:write` `workflow:trigger`。默认可用（无需声明）：`ui:toast`、`storage:*`。高风险（安装后会在插件卡片上标注）：`fs:read`、`fs:write`、`data:invoke`、`shell:exec`、`session:execute`。

**权限是硬边界（Rust 侧强制）**：`fs` / `shell` / `agent` / `data.invoke` 都在后端按 `manifest.permissions` 逐项校验，**未声明就报"未声明权限 'xxx'，调用被拒绝"**。权限校验与沙箱开关**解耦**：关闭沙箱**不会**授予任何权限，只额外解除 `fs` / `shell` 的沙箱门。

**哪些权限是真门禁、哪些只是声明**：真正被强制的是 `fs:read` / `fs:write` / `shell:exec` / `data:invoke` / `session:*`（在 Rust 的调用点逐项校验）。其余如 `plugin:events` / `plugin:call` / `ui:modal` / `workflow:*` 目前**只是意图声明**，没有校验点 —— 插件与宿主同域运行，任何前端校验都能被绕过，所以我们不在前端做"假门槛"。写它们的作用是让用户与工具知道插件想做什么；`plugin:events` 尤其不构成实际暴露：订阅不改动应用状态，事件载荷也只含标识（不含消息正文 / 密钥 / 文件内容）。

**沙箱**：默认开启，此时 `fs.*` 与 `shell.exec` 一律被拒（返回"沙箱已启用…"）。要用必须由用户在 **设置 › 插件管理** 点「沙箱」按钮 → 沙箱信息里点「禁用」（脚本运行时无法自行关闭）。`agent.*` / `data.invoke` 不受沙箱开关影响，只看权限声明。

## 贡献点在界面上的使用方法（交付说明里要交代这些）

| 贡献点 | 用户在哪里看到 / 怎么用 | 前提 |
|---|---|---|
| `contributes.panels` | 会话模式 → 右侧面板「插件」标签页 → 直接渲染该面板（多个面板时顶部有切换条；终端模式下不显示该标签页） | 插件**已启用**；入口调了 `api.ui.addPanel({ id })` 且 id 与声明一致；权限 `ui:panel` |
| `contributes.commands` | ① 工作流：新增节点 → 选「插件」节点 → 选该插件 → 选命令；② 其他插件：`api.global.call('<插件id>', '<命令id>', params)` | 入口用 `api.commands.register('<命令id>', handler)` 注册了同一 id |
| `contributes.workflow_config` | 上面那个插件节点配置面板的**下方**，选好插件/命令后出现（插件自己的参数表单） | 入口 `export default` 里导出同名组件（`component` 或 `components` 里的名字） |
| `contributes.node_types` | 工作流节点面板里多出一种节点类型 | 非标准路径、不推荐；实际执行仍要 `pluginId` + `commandId` |
| `contributes.hooks` | 没有界面入口；加载插件时把 `handler`（入口同名函数）接到应用事件总线 | 入口 `export default` 里有同名函数；事件名见「应用事件」表 |
| 能力类（`storage` / `fs` / `shell` / `agent` / `data.invoke`） | 不是"给用户看"的贡献点，是插件自身能力 | 见权限表（**必须先在 `permissions` 里声明**）；`fs` / `shell` 还需用户关闭沙箱 |

插件条目上的徽标可用来核对交付结果：「N 面板」「N 命令」「N 钩子」对应上面的声明，「工作流可用」表示该插件能在工作流的「插件」节点里选到 —— 没有这个徽标，说明它既没写 `workflow_config`，也没有任何命令声明 `input` 模式，**在工作流里根本选不到它**。

**启用/禁用的真实影响**（容易误解，交付时别写错）：禁用**不会**阻止入口 JS 执行、也不会注销已注册的命令（工作流仍能调用该插件的命令）；它只让 **设置 › 插件管理** 里的插件条目变暗，并让该插件的面板**不再出现在会话右侧面板的「插件」标签页**。所以"给用户用的面板类插件"必须提醒用户启用。

## README.md（📖 说明文档）编写指引

**它是插件唯一面向用户的说明书**：插件条目右侧的 📖 按钮（title「查看 README」）弹出的就是这个文件。不写的话，用户点开只看到"作者未提供 README 文档" —— 面板类插件尤其不能缺。

**读取规则（决定文件名与配图怎么写）**

- 文件名必须是插件目录根部的 **`README.md`**（后端硬编码这个名字，大小写敏感；其他名字读不到）；
- 上传到在线插件库（store）时，会自动从 `{插件 baseUrl}/README.md` 下载到插件目录，所以发布时记得一起传；
- 渲染器就是应用内的 Markdown 渲染器：支持 GFM 表格 / 任务列表 / 删除线、代码块语法高亮、外链（新窗口打开）、图片（点击放大）；
- **图片必须用 `http(s)` 或 `data:` URL**：`![图](./icon.png)` 这类相对路径不会解析到插件目录，渲染不出来（链接同理）；
- 本地读取没有大小限制，但 📖 是 600px 宽、80vh 高的弹窗；控制在 100 行左右最好读。

**推荐结构**（按用户关心的顺序写，不是按实现顺序）

1. **一句话做什么**：解决什么问题、适合谁用；
2. **怎么用**：面板类写"进入会话模式 → 右侧面板「插件」标签页"；工作流类写"新增「插件」节点 → 选本插件 → 选哪个命令 → 参数在哪填"；
3. **权限逐条说明**：`manifest.permissions` 里每一项用来做什么；含 `fs` / `shell` 的必须写清"要先在「沙箱」按钮里禁用沙箱"；
4. **参数说明**：键名、含义、取值范围、是否支持 `{{变量}}` 引用（含引用方式）；
5. **输出字段**：工作流节点输出的字段名，供下游写 `{{节点ID.字段}}`；
6. **常见问题与限制**：把与用户有关的限制写进来（不要只写在代码注释里）。

**写法要求**

- 中文、短句、列表；不写实现细节（用了哪个 API、改了什么），只写用户看得见的契约；
- 命令 id、参数名、输出字段名必须与 manifest / handler **逐字一致** —— 用户会直接照抄；
- 不要承诺未实现的能力：应用事件（`contributes.hooks` / `api.events.on`）是**仅通知**，没有"发送前拦截/改写"能力（不存在 `message:before-send`）；
- README 与交付说明是两件事：README 放在插件目录里长期可查，交付说明是本次对话里给用户的操作步骤，两份都要有。

## 工作流插件贡献规范（要让工作流用得上，必须照这个写）

工作流的「插件」节点是这样跑起来的：

```
工作流定义里的节点              前端                          后端
{"type": "plugin",      →  CommandDispatcher.execute   →  PluginExecutor 发
 "pluginId": "<插件id>",     (pluginId, commandId,         `workflow:plugin-execute`
 "commandId": "<命令id>",     params)                      事件交给前端执行，
 "params": { ... }}                                        再把结果回传后端
```

**两件套 + 一个表单来源**

1. **`contributes.commands[]`**（必需）：节点里"选择命令"下拉的内容**只来自** manifest 这里（不是来自 `api.commands.register`）。命令 `id` 必须与注册时用的字符串逐字一致，且**不能重复**。
2. **入口 `api.commands.register('<命令id>', handler)`**（必需）：真正执行的地方。不一致 → 节点报"命令未注册"。
3. **参数表单来源**（三选一，都没有则该插件在工作流插件下拉里**选不到**）：
   - **命令级组件** `workflow_config.components: { "<命令id>": "<导出组件名>" }` —— 每个命令一套表单，最适合"一个操作一个命令"；
   - **插件级组件** `workflow_config.component: "<导出组件名>"` —— 所有命令共用一套表单，组件会收到 `commandId` 可自行分支；
   - **命令的 `input` 模式** —— 不写任何 React 组件，平台按所选命令的 `input.properties` 自动生成表单（零代码接入，见下）。

> **`input` 是契约不是文档**：平台会用它生成参数表单，并在切换命令时据此判断哪些参数已失效（`output` 仍只是文档，界面不消费）。写成平台不认识的样子会被 manifest 校验拒绝加载：`input.type` 可省略（省略即视为 `object`），写了必须是 `"object"`；每个属性必须声明 `type`（`string`/`number`/`integer`/`boolean`/`array`/`object`）；`required` 必须是 `properties` 的子集；`enum` 必须是非空数组；`enumLabels`（可选）必须是与 `enum` 按下标对齐的字符串数组，**不得多写**（多出来的没有对应取值，会被拒）。

**零 React 表单的最小写法**（推荐没有特殊 UI 需求时用）：manifest 里给每个命令写 `input.properties`，**不写 `workflow_config`**；节点里会按所选命令自动出现这些字段（`string`/`number`/`integer` → 文本输入，`boolean` → 勾选框，带 `enum` → 下拉选择，`array`/`object` → JSON 文本框）。属性名必须与 handler 读取的 `params` 键**逐字一致**，`description` 会作为输入提示；`number`/`integer` 属性里填的能解析成数字、其余按字符串写出，含 `{{变量}}` 时一律按字符串交给后端解析。需要给布尔值写模板引用时，把该属性声明为 `string`（勾选框写不了 `{{}}`）。

> **下拉选项想显示中文，用 `enumLabels`，不要改 `enum`**：`enum` 里是**机器值**（会原样写进工作流定义，handler 按它分支），`enumLabels` 是同一个数组的显示名，**按下标一一对应**：
> ```json
> "op": { "type": "string", "default": "add",
>         "enum": ["add", "sub"],
>         "enumLabels": ["add（相加）", "sub（相减）"] }
> ```
> 下拉里显示 `add（相加）`，但节点参数里存的仍是 `add`。只给前几项起中文名也可以，剩下的按原值显示。

节点字段（工作流定义 JSON / 编辑器）：

| 字段 | 含义 | 谁写 |
|---|---|---|
| `pluginId` | 插件 id | 编辑器"选择插件"下拉（同时把 `plugin_id` 写进 `params`） |
| `commandId` | 命令 id | 编辑器"选择命令"下拉 |
| `params` | 传给 handler 的参数对象 | 内置字段 + 插件自己的配置组件（`onParamsChange`）或平台按 `input` 模式生成的表单 |

**执行契约（最容易踩的两处）**

- handler 收到的 `params` = 节点 `params`（表单里的 `{{变量}}` 先被解析） + 上游输出：上游输出是对象时**只补进 `params` 里还没有的键**；不是对象时放进 `params.__input__`。
- **handler 的返回值就是该节点的输出**，下游用 `{{节点ID.键名}}` 引用。`CommandDispatcher` 会自动把它包成 `{success, data, duration}`，**所以 handler 不要再返回 `{success, data}` 信封**（会让节点输出多嵌套一层）；失败请直接 `throw new Error(...)`。
- 超时：节点超时默认 30 秒、下限 5 秒；未选命令 / 命令未注册 / 抛错 / 超时都会让该节点失败。

**配置组件契约**

```js
function MyNodeConfig(props) {
  // props = { params, onParamsChange, api, commandId, command, variables, TemplateField }
  var params = props.params || {};
  var onParamsChange = props.onParamsChange;  // onParamsChange(key, value) 写回节点 params
  var commandId = props.commandId;            // 当前选中的命令 id；props.command.input 是它的 input 模式
  var TemplateField = props.TemplateField;    // 平台输入组件：自带 {{ 变量补全
  var variables = props.variables;            // 当前可引用的变量（输入映射的键名）
  // 组件内只能通过 onParamsChange 写 params；pluginId / commandId 由内置字段管理，不要在组件里改
}
```

组件必须在 `export default` 里导出，名字与 `workflow_config.component`（或 `components` 里的值）逐字一致。

**硬规则一：命令选择器与表单下拉只能"级联"，不能"重复"**

节点配置里已经有一个内置的**「选择命令」下拉**（内容 = `contributes.commands`）。它与表单里的下拉只能是上下级关系：

- **级联（合法，常见）**：命令 = 上级"类别"，表单下拉 = 该类别下的**具体项**，选项由命令决定。例如命令 `math.basic`（基础运算）＋表单里选 `加法/减法/乘法/除法`；命令 `math.trigonometry` ＋表单里选 `sin/cos/tan`。这种写法要满足三点：
  1. 下拉选项随命令变化（用 `commandId` 或 `props.command.input` 决定）；
  2. **切命令时把下级重置为新命令下的合法值**：下级是 `enum` 参数时，平台会自动清掉"不在新命令取值范围内"的旧值（再由默认值/表单补上），所以只要把下级写进各命令的 `input` 模式就基本自动了；非 `enum` 的下级需要组件自己重置并写回；
  3. 下级参数写进**每个命令的 `input` 模式**（平台据此补齐默认值、核对键名）。
- **重复（要改）**：命令集合与表单下拉**表达同一件事** —— 例如声明了 `math.add` / `math.subtract` 两个命令，表单里又放一个"加法/减法"下拉。用户在两处表达同一个选择，还可能自相矛盾（命令选减法、下拉选乘法），最终以 handler 收到的 `params` 为准，预期与实际不一致。二选一：拆成命令（表单里不放该下拉），或只声明一个命令（把下拉留在表单里）。

判定口径一句话：**看表单下拉的选项集合是否与命令集合含义重叠** —— 重叠 = 重复；只是"某个命令下的具体项" = 级联。级联的默认值同样必须落进 `params`（见硬规则三）。

另外要清楚：**插件级表单组件对该插件的所有命令共用**，它会收到 `commandId`，请在里面按命令分支渲染（或改用命令级组件 / 各命令的 `input` 模式），否则表单会显示"别的命令才需要的字段"。

**硬规则二：参数输入框一律用平台的 `props.TemplateField`，且不得对值做类型校验**

平台会把「当前可引用的变量」与一个输入组件一起传给配置组件：

- **`props.TemplateField`**：平台提供的模板输入组件，**自带 `{{` 触发变量补全**（输入 `{{` 即弹出可选变量，选中后插入 `{{变量}}`）。参数输入框一律用它；**不要自己写 `<input>` / `<textarea>`** —— 自写的输入框没有补全，用户只能靠手打变量名，极易写错键名。
- **`props.variables`**：可省略 —— 省略时平台自动注入**当前节点的变量**（该节点「输入映射」的键名），所以 `{{` 补全会正常弹出；只有想限定候选子集（例如只列本命令用得到的键）时才显式传。节点还没配任何输入映射时列表为空，弹层会提示"先在输入映射里定义参数名"。
- 传参与原生控件一致：`value`、`onChange(nextValue)`、`multiline`（多行）、`rows`、`placeholder`、`style`、`disabled`。**不要传数字控件**（如 `type="number"`）。

**参数值的三种形态 —— 决定校验怎么做**

| 形态 | 例子 | 前端能否校验类型 |
|---|---|---|
| 常量（无 `{{}}`） | `6`、`加法` | **能**：按字面值校验（数字、范围、枚举…） |
| 变量（被 `{{}}` 包裹） | `{{a}}` | **不能**：一律视为合法输入，无论该字段期望字符串还是数字 |
| 混合文本 | `共 {{a}} 个` | **不能**：结果一定是字符串，别按数字校验 |

**类型校验的正确位置是 handler**：引擎在调用 handler **之前**已经把 `{{}}` 解析成真实值并推断类型（`{{a}}` 且 a = 6 → 数字 `6`），此时类型才是确定的。所以：

- 前端**只提示、不拦截**：不要在 `onChange` 里拒绝、纠正或清空输入（否则用户连 `{{` 都打不进去）；要提示就放在失焦/预览态，且**含 `{{` 的值必须跳过类型校验**。
- 数字字段用文本输入承载：`1e3`、负数、`{{变量}}` 都要能输入。
- handler 里把两种失败分开报：值里仍含 `{{` → 说明"变量引用未解析"（键名写错或上游没这个字段）；否则才是"不是数字"。

```js
// handler 里的正确写法（校验放这里，且区分"引用未解析"与"不是数字"）
function requireNumber(value, name) {
  if (typeof value === 'string' && value.indexOf('{{') >= 0) {
    throw new Error('参数 ' + name + ' 的变量引用未解析: ' + value + '（检查节点的输入映射里是否有该键）');
  }
  var n = Number(value);
  if (!Number.isFinite(n)) throw new Error('参数 ' + name + ' 不是数字: ' + value);
  return n;
}
```

**硬规则三：控件里"显示出来的默认值"必须落进 `params`**

最常见的失败形态是"**选默认项反而报错，选别的选项就正常**"：表单显示着 `sin`/`add`，但 `params.func` 是空串或根本不存在 —— 用户不碰控件（或"再选一次已经显示的默认项"，值没变、不触发 `onChange`）时，handler 收到空值直接抛"无效的函数: "。

选一种把默认值写到位：

- **首选**：在 manifest 的 `input.properties[key].default` 里声明默认值 —— 平台打开节点表单时会把"缺失或空串"的参数补成该默认值并写回 `params`；
- 或组件就绪时自己写回：

```js
React.useEffect(function () {
  if (params.func == null || params.func === '') onParamsChange('func', 'sin');
}, []);   // 只做一次：不要写成依赖 params 的循环
```

**同一个组件服务多个命令时，写回的默认值必须属于当前命令**。`func` 这种键常被多个命令共用（三角函数 `sin/cos/tan…`、对数 `log/log10/log2`、取整 `ceil/floor/round/trunc`），无条件写 `'sin'` 会把平台刚按新命令清掉的非法值**填回来**，切到对数命令后 handler 直接报"无效的对数类型: sin"。按下级所属命令取默认值：

```js
var DEFAULTS = {
  'math.trigonometry': { func: 'sin' },
  'math.logarithm':    { func: 'log' },   // 默认值必须是本命令 enum 里的取值
};
React.useEffect(function () {
  var defaults = DEFAULTS[commandId] || {};
  Object.keys(defaults).forEach(function (key) {
    var current = params[key];
    if (current == null || current === '') onParamsChange(key, defaults[key]);
  });
}, []);
```

**平台会在切换命令时重建表单实例**（同一个命令内打字不会重建）。所以"挂载时写回默认值"在**用户打开过节点面板**的前提下是可靠的 —— 但**定时触发、或流程里直接跑而没人打开过这个节点**时组件根本不会挂载，`params` 里就没有这些键，handler 收到空值就报错。所以**首选仍然是把默认值写进 manifest 的 `default`**：它是命令契约的一部分，与谁触发无关。

**禁止**只在组件里造默认值（`params.func || 'sin'` 只用来显示、从不写回）—— 平台与 handler 都看不到它。

**级联选择器（上级决定下级的选项）额外要求**：切换上级时必须**同时把下级重置为该上级下的合法值并写回**，例如：

```js
onChange: function (next) {
  onParamsChange('operation', next);          // 上级
  onParamsChange('func', FUNCS[next][0]);     // 下级：立刻换成新上级下的第一个合法值
}
```

否则下级会残留上一个上级的取值（如 `operation` 已是 `trigonometry`、`func` 还是 `round` 的 `ceil`），handler 报"无效的函数"。

**变量从哪来（务必写进交付说明）**：引擎先解析节点的**输入映射**（`resolve_node_input` 对 `{{节点ID.字段}}` 求值），再把它的结果当作上下文解析表单参数里的 `{{}}`。所以：

1. 引用上游/全局变量 → 在节点的**「输入映射」**里写 `键名 = {{节点ID.字段}}`；
2. 表单参数可以引用输入映射的**键名**：写 `{{键名}}`（或 `{{__input__.键名}}`）。命中时按内容推断类型（`{{a}}` 而 a = 6 → 数字 6，不是字符串 `"6"`），周围有其它文字则保持字符串（`共 {{a}} 个` → `"共 6 个"`）；
3. 表单里手写的字面值**不做类型推断**：填 `6` 传下去仍是字符串 `"6"`；
4. **同名键以表单为准**：表单里写过 `a`，输入映射的 `a` 不会覆盖它；但仍可在别的参数里用 `{{a}}` 引用输入映射解析出的值；
5. 引用**写错不报错**：键名不存在（或上游没这个字段）时 `{{...}}` 原样保留并传给 handler，节点不会失败 —— 交付说明里要提醒用户核对变量名，handler 也应对拿到 `{{...}}` 的情况给出可读错误。

**参数卫生（平台会自动清理，但要按约定写才清得干净）**：切换命令时，平台删除"旧命令 `input` 模式声明、新命令未声明"的参数键；**新命令声明了、但取值不在其 `enum` 内（或为空串）的键也会被清掉**（视为未设置，交给默认值/表单重填）；切换插件时，旧插件的参数会被整体清空（只保留 `plugin_id`）；切换命令时平台还会**重建表单实例**，组件的挂载逻辑会重跑一遍。因此：

- 声明了 `input` 模式的字段，切换命令时会被精确清理，不会串到新命令的 handler —— 级联的下级（`enum`）因此能自动切干净；
- 自写组件写入的**模式外键**（`input` 里没声明的键）无法归属，切命令时保留、切插件时才清空 —— 需要被清理的字段请一并写进 `input` 模式；
- 在定时触发等"没人打开节点面板"的执行路径下，组件不会挂载，参数只能靠 `input` 模式的 `default` 与节点里已保存的 `params`。

**工作流插件最小完整示例**

`manifest.json`（只保留工作流需要的部分）：

```json
{
  "id": "demo-node",
  "name": "示例节点",
  "version": "1.0.0",
  "description": "把输入文本转大写的插件节点",
  "author": "用户",
  "minAppVersion": "0.1.0",
  "permissions": [],
  "entry": { "main": "index.js" },
  "contributes": {
    "commands": [
      {
        "id": "demo-node.uppercase",
        "title": "转大写",
        "input": { "type": "object", "properties": { "text": { "type": "string", "description": "待转换文本" } }, "required": ["text"] },
        "output": { "type": "object", "properties": { "text": { "type": "string", "description": "大写结果" } } }
      }
    ],
    "workflow_config": { "component": "DemoNodeConfig" }
  }
}
```

`index.js`：

```js
// 节点配置组件：props = { params, onParamsChange, api, commandId, command, variables, TemplateField }
// 注意：只放"参数"，不要放"操作/类型"下拉 —— 操作已由节点的「选择命令」表达
function DemoNodeConfig(props) {
  var params = props.params || {};
  var onParamsChange = props.onParamsChange || function () {};
  var TemplateField = props.TemplateField;   // 平台输入组件：输入 {{ 自动弹出可引用变量
  return React.createElement('div', { style: { display: 'flex', flexDirection: 'column', gap: 8 } },
    React.createElement('label', { style: { fontSize: 12 } }, '待转换文本'),
    React.createElement(TemplateField, {
      value: params.text || '',
      onChange: function (next) { onParamsChange('text', next); },   // 不做校验、不做类型转换
      // variables 可省略（平台注入当前节点变量）；要限定候选子集时才传 props.variables
      placeholder: '固定值，或输入映射的变量引用，如 {{text}}',
      style: { padding: '6px 10px' }
    })
  );
}

export default {
  DemoNodeConfig: DemoNodeConfig,   // 名字与 manifest 的 workflow_config.component 一致

  onLoad: function (api) {
    api.commands.register('demo-node.uppercase', async function (params) {
      var text = (params && params.text) || (params && params.__input__) || '';
      if (!String(text).trim()) throw new Error('缺少输入文本');   // 校验与类型转换都放在这里
      return { text: String(text).toUpperCase() };   // 直接返回载荷，不要包 success/data
    });
  },

  onUnload: function () {},
};
```

**用户侧配置步骤（交付时必须写清）**

1. **设置 › 插件管理** → 点「刷新」，确认插件出现在「已安装插件」里且状态为「已启用」；
2. 工作流里新增节点 → 选「插件」节点；
3. 节点配置里先选插件、再选命令（命令下拉即 `contributes.commands`）；
4. 节点下方会出现参数表单，填好（值写进 `params`）；
5. 运行后，后续节点用 `{{该节点ID.text}}` 引用它的输出。

> 上面这个示例同时写了组件和 `input` 模式：**渲染以组件为准**，`input` 模式仍有用 —— 平台用它判断切换命令时要清理哪些参数，校验脚本也按它核对字段名与 handler 是否对得上。不需要自定义 UI 时，删掉 `workflow_config` 只留 `input` 模式即可，节点里照样出现输入框。

**节点报错对照**

| 现象 | 原因 |
|---|---|
| 报"缺少命令 ID" | 节点 `commandId` 未选，且 `params.commandId` 也没有 |
| 报"命令未注册: <插件id>:<命令id>" | 入口没注册该 id，或注册字符串与 manifest / 节点里选的不一致 |
| 插件下拉里找不到该插件 | 既没写 `workflow_config`（component/components），也没有任何命令声明 `input` 模式 |
| 能选到插件但"选择命令"下拉为空 | `contributes.commands` 缺失或为空 |
| 节点下方提示"该命令没有参数表单" | 该命令既没有可用组件，其 `input.properties` 也是空的 |
| 节点下方没有参数表单 | `workflow_config` 声明的组件名在入口 `export default` 里不存在（控制台有对应 warn） |
| 命令的参数模式不给渲染 | `input` 写法不合契约（类型不支持、`required` 不在 `properties` 里等）→ manifest 校验失败，插件整体不加载 |
| 节点输出多嵌套一层 `success` / `data` | handler 返回了 `{success,data}` 信封（应直接返回载荷） |
| 节点超时 | 默认 30 秒、下限 5 秒；长任务请拆分节点或调大节点超时 |
| 输入框中输入 `{{` 不弹出变量列表 | 该输入框不是平台的 `TemplateField`（自写的 `<input>` 没有补全）；或该节点还没有任何输入映射参数（此时弹层会提示先在输入映射里定义） |
| 填了 `{{变量}}` 却被提示"不是数字" | 前端输入框里做了类型校验：含 `{{` 的值必须跳过校验，类型判定放 handler（见硬规则二） |
| 选默认项报错、选别的选项就正常 | 默认值只用于显示、没落进 `params`（`params.func \|\| 'sin'` 且 manifest 无 `default`）：按硬规则三写回，或改为在 `input.properties[key].default` 里声明 |
| 切换上级选项后报"无效的 XX" | 级联选择器没有在下级重置为合法值（见硬规则三），下级残留了上一个上级的取值 |

**交付前自检（涉及工作流的插件逐条核对，缺一项就算没写完）**

- [ ] `manifest.contributes.commands` 里每个命令 id 唯一，且入口都有 `api.commands.register('<同一字符串>', handler)`；
- [ ] **表单来源三选一已落实**：命令级组件（`workflow_config.components`）／插件级组件（`component`）／命令的 `input` 模式 —— 组件的名字在入口 `export default` 里**逐字存在**；
- [ ] 用组件时按 `{ params, onParamsChange, api, commandId, command, variables, TemplateField }` 写，用户要填的每个字段都通过 `onParamsChange('<键>', 值)` 写回，键名与 handler 读的 `params.xxx` 一致；
- [ ] 用 `input` 模式时，属性名与 handler 读取的 `params` 键逐字一致、类型在允许清单内、`required` 是 `properties` 子集；
- [ ] **表单下拉与「选择命令」不是同一个选择**：级联（命令=类别、下拉=该类别的具体项）可以保留；若下拉选项与命令集合含义重叠，改成"拆命令"或"只留一个命令"；
- [ ] **参数输入框都用 `props.TemplateField`**（`{{` 能弹补全），没有自写 `<input>`/`type="number"`；
- [ ] **没有对含 `{{` 的值做类型校验**（不拒绝、不纠正、不清空；类型校验在 handler 里，且区分"变量引用未解析"与"不是数字"）；
- [ ] **每个会给 handler 读的参数都有值来源**：要么在 `input.properties[key].default` 里声明默认值，要么组件就绪时写回 `onParamsChange`——不能只写 `params.x || '默认'` 显示而从不写回（见硬规则三）；
- [ ] **级联选择器**：切换上级时下级已重置为新上级下的合法值并写回；
- [ ] handler 直接返回载荷（不包 `{success,data}` 信封），失败用 `throw`；
- [ ] 已按「README.md 编写指引」写好插件目录下的 `README.md`（做什么 / 怎么用 / 权限逐条 / 参数 / 输出字段）；
- [ ] 已跑 `validate-plugin.mjs`，无 `[error]`；
- [ ] 交付说明写清：在哪刷新、是否要启用、节点里选哪个插件/命令、参数在哪填。

## 标准流程

1. **先问清形态**：面板（人工点）、工作流可调用的命令（自动化），还是两者都要？只要涉及工作流，就按「工作流插件贡献规范」逐条核对：`contributes.commands` + `api.commands.register` 是**必需的两件**，参数表单来源（命令级组件 / 插件级组件 / `input` 模式）**至少落实一个** —— 一个都没有的插件在工作流里根本选不到，不要以"用户可以先只选命令"收尾。
2. 定 `id`（小写、含点或短横线，作目录名），在 `<配置根>/plugins/<id>/` 下写 `manifest.json` 与 `index.js`。
3. 按上面模板落地；需要的能力逐项写进 `permissions`，并核对不是"未声明的 API 调用"。
4. 写插件目录下的 `README.md`（内容按「README.md 编写指引」，权限要逐条说明用途）。
5. **跑校验脚本**（技能根目录见本次加载结果第一行）：

   ```sh
   node "<技能根目录>/validate-plugin.mjs" "<配置根>/plugins/<id>"
   ```

   有 `[error]` 必须修到通过；`[warn]` 逐条判断。
6. 把需要用户做的事说清楚：到 **设置 › 插件管理** 刷新并确认已启用；面板类插件另说"进入会话模式后打开右侧面板的「插件」标签页"；含高风险权限或需要 fs/shell 时，明确告知用途，并说明 fs/shell 需要先在「沙箱」按钮里禁用沙箱。
7. 让用户回报现象，失败时按下一节定位。

## 故障排查

| 现象 | 原因 |
|---|---|
| 插件列表里根本不出现 | **设置 › 插件管理** 点刷新后仍无：`manifest.json` 缺失/不是合法 JSON/超 64KB，或 `id`/`name`/`version`/`entry.main` 校验失败 |
| 出现了但"含未授权权限"、无法启用 | `permissions` 里有非法项（拼错、大小写、或用了清单外的名字） |
| 插件在列表里但没有出现在右栏「插件」标签页 | 插件处于「已禁用」（点一下切换为「已启用」），或 `contributes.panels` 没写 |
| 面板显示成占位内容 | `api.ui.addPanel` 的 `id` 与 `contributes.panels[].id` 不一致，或入口加载/执行报错 |
| 入口没生效，控制台只有一条 warn | 入口用了 `import`/`require`/JSX，或没有 `export default` |
| 工作流插件节点失败 | `command_id` 未在 `api.commands.register` 注册；handler 抛错；超时（下限 5 秒）；插件未被加载 |
| 📖 弹窗显示"作者未提供 README 文档" | 插件目录里没有 `README.md`（或文件名不是它，如 `readme.md`/`Readme.md`） |
| README 里图片/链接点不开、不显示 | 用了相对路径（`./x.png`）：渲染器没有基准目录，改 `http(s)` / `data:` URL |
| README 里写的权限和插件条目标注不一致 | 以 `manifest.permissions` 为准；README 必须逐条对齐 |
| `fs`/`shell` 返回"沙箱已启用" | 用户还没在「插件」标签页的「沙箱」按钮里禁用沙箱；`fs` 路径还必须在本插件目录内 |
| 调 API 报权限不足 | 该 API 需要的权限没写进 `permissions` |

## 已知限制（不要向用户承诺这些）

- 应用事件是**仅通知、不可拦截**：`api.events.on` / `api.hooks.on` / `contributes.hooks` 能收到宿主投递的事件（见「应用事件」表），但 handler 的返回值与异常都不影响应用；**没有** `message:before-send` 这类"发送前拦截/改写"能力。宿主事件与跨插件广播（`api.global.emit`）共用同一条总线，故 `api.events.on` 也能收到别的插件的广播。当前宿主只投递 `session:created` / `session:deleted` / `message:sent` 与 `workflow:<类型>`。
- 启用/禁用只影响插件条目外观与面板入口，不阻止入口 JS 执行、也不注销已注册的命令（详见「贡献点在界面上的使用方法」）。
- 插件列表目录是 `%APPDATA%\PilotDesk\plugins\`。
- 面板只能出现在右侧面板区；不能新增顶部页签（顶部"自定义页签"是用户手工配置的 iframe，与插件无关）。
- `minAppVersion` 未生效；`contributes` 里只有命令 id 唯一性、`workflow_config`、命令 `input` 模式会被校验（面板/钩子等字段写错不报错、只是不工作）。
- 插件 JS 与宿主同处一个渲染进程 realm：危险全局（`window` / `globalThis` / `document` / `fetch` / `localStorage` / `Function` 等）已被"以同名参数"遮蔽为 `undefined`（`eval` / `arguments` 受严格模式限制无法遮蔽），但这是**纵深防御**，不是绝对隔离 —— 语言层面仍有逃逸手段。真正的安全边界在 Rust 侧（命令层的权限 + 白名单校验），因此**不要**把它当作可运行不可信代码的沙箱。写插件时不要尝试读取明文密钥、发起外网请求或写入插件目录以外的位置，也不要声明用不到的权限。
- 在线插件库（store）安装时会校验入口文件的 sha256（索引字段由 CI 生成）：哈希不匹配或索引缺 `sha256` 都会被拒绝安装。
