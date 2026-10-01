# ECharts 使用规范
- 图表容器必须用具有明确像素宽高的父 div 包裹
- 每列最多一个图表
- 图表颜色从 design.css 的 Design Token 派生，去除不必要的网格线和背景，保持极简风格
- 标签字号必须足够大（>= 18px），确保投影清晰
- **图例防跑版铁律**：必须显式指定 legend 的位置和间距，并确保图表容器宽度足够，防止图例文字因拥挤而换行错位。
- **禁止**生成虚构数据；所有数字必须来自可验证来源，并标注数据源

## Grid & 图例防截断配置
```javascript
grid: {
    containLabel: true,   // 必须设置
    left: 60,             // 最小值 60px，约容纳 3-4 个数字+百分号（如 "10.5%"）；有 Y 轴名称时加到 80-100
    right: 60,            // 最小值 60px，X 轴名称或次 Y 轴标签，约容纳 3-4 个中文字；更长时加到 80-100
    top: 40,              // 最小值 40px，轴名称或图例高度；有图例时加到 60-80
    bottom: 40            // 最小值 40px，X 轴标签高度，约容纳 2-3 个中文字；类别名较长或旋转时加到 60-80
}
legend: {
    top: '3%', left: 'center',  // 必须显式指定位置，禁止省略
    orient: 'horizontal',
    itemGap: 20,                 // 必须 >= 16，防图例换行错位
    textStyle: { fontSize: 14 }
}
```

图表容器最小尺寸：**宽 >= 400px，高 >= 300px**。

## 通用配置约束
以下规则适用于所有图表类型，不因图表类型不同而例外：

- 分类轴必须设置 `axisLabel.interval: 0`，默认 'auto' 会在空间不足时隐藏标签。标签密集时用 rotate 旋转并同步加大 grid.bottom。
- 使用 `axisLabel.rotate` 时须同步加大 grid.bottom（45°旋转约需 18%），禁止仅旋转不加边距。
- 禁止使用 `dataZoom`、`timeline`、`tooltip` 等交互组件，这些在 PPTX 中无效。
- 禁止使用 `markLine` 绘制参考线，转换后参考线可能与图表元素重叠且标签错位。需要标注参考值时在图表外的正文中说明。
- **series.label 慎用**：密集图表（雷达/散点/折线/多系列柱状）**禁止**开启 label——标签会叠在图形上，严重破坏可读性；系列身份用 legend 区分，精确数值放进图表外的正文中说明。仅数据点稀疏且需常驻标注时启用 label（如饼图扇区名、单系列柱顶数值）。
- formatter 规范：
  - 禁止在 formatter 中使用 JavaScript 反引号模板字符串（` 包裹的字符串），会导致 ReferenceError。
  - formatter 模板变量分两种场景：
    - **轴标签**（axisLabel.formatter）：使用 `{value}`，如 `formatter: "{value}%"`
    - **数据标签**（series.label.formatter）：使用 `{c}`（数值）、`{a}`（系列名）、`{b}`（数据名）、`{d}`（百分比，仅饼图/环形图可用），如 `formatter: "{c}%"`
  - 禁止在 formatter 字符串中使用 `${value}`、`${c}`、`${b}` 等形式：
    `${...}` 会被 PPTX 转换器识别为 JavaScript 模板字面量插值，导致 ReferenceError。
  - 若需要在数值前加 $ 等前缀，使用函数 formatter。函数式 formatter 回调参数签名因回调位置不同而不同，必须区分使用：
    - `axisLabel.formatter`：参数是原始值（number/string），不是对象。
      - 正确：`formatter: function(v) { return '$' + v; }`
      - 错误：`formatter: function(v) { return '$' + v.value; }`（v.value 为 undefined）
    - `series.label.formatter`：参数是 params 对象，通过 `.value` 取数值。
      - 正确：`formatter: function(p) { return '$' + p.value; }`
- `label.position` 限制：ECharts 丰富的 label.position 值（如 'top'、'right'）在转换为 PPTX 原生图表时会丢失语义，导致标签错位。因而只能使用 `'outside'` 和 `'inside'`，禁止使用以下 position 值：
  - `top`、`bottom`、`left`、`right`、`position: [x, y]`——无法正确映射
  - `inner`——ECharts 已弃用
- 当图表已有 title 覆盖了系列含义，或只有一个系列时，禁止重复设置 legend。单系列图表不应携带图例，避免信息冗余。

## 各图规范

### 柱状图
- 负值处理：
  - 数据传入：负值数据直接传入负数数值，ECharts 自动将负值柱体显示在 X 轴下方。禁止拆分为正负两个 series、禁止使用 stack + 透明基底等瀑布图手法来模拟负值效果。
  - 数值轴下限：含负值时**必须**显式设置数值轴（`type: 'value'`）的 `min`，取值 <= `最小负值 - interval` 且为 interval 整数倍，为负值标签预留空间，避免转换后与分类轴重叠；建议同步设置 `interval` 锁定刻度密度。
- 密度控制：柱体宽度 `barWidth` 须 <= 类别可用垂直空间的 70%，且必须设置 `barGap >= '20%'` 确保柱间有视觉间隙。类别数 >= 8 时，`barWidth` 不超过 22px。

### 热力图
- 数据格式必须为 `[x, y, value]` 三元组，禁止携带第 4 个额外元素。
- `visualMap` 仅在热力图中可用（用于颜色映射），其他图表禁止使用。使用 visualMap 时必须设置 `show: false` 隐藏颜色条，热力图的数值通过 `series.label` 展示，无需重复显示颜色条。
- **热力图标签对比度**：`series.label.color` 与 `visualMap.inRange.color` 色域两端（最浅色、最深色）对比度均须 >= 4.5:1；色域跨明暗时单一固定色无法兼顾两端，须按背景明度动态切换标签颜色（深底浅字、浅底深字）。
- 热力图标签必须保持简洁，仅展示数值（如 $120），禁止在标签中添加额外说明文字。
- 禁止在热力图上叠加 scatter 等额外 series 做标注，会导致元素重叠。特殊单元格的标注应通过 heatmap 自身的 `label.formatter` 和 `itemStyle` 实现。

### 散点图
- 使用对象形式 `{ value: [...], itemStyle: {...} }` 时，公司名/类别名必须放在 `name` 字段，value 数组只放数值（[x, y, ...]）。禁止将字符串放入 value 数组，否则数据点不会显示。
- 散点图仅适用于双数值轴场景（X 轴和 Y 轴均为 `type: 'value'` 的连续数值轴）。当某一轴为分类轴时，应改用条形图或柱状图。
- 标签与防重叠策略：
  - **标签全局关闭**：数据点 > 8 个或呈密集簇状分布时，强制 `label: { show: false }`。实体名称与精确数值交由图表外的正文区域陈述。
  - **极值点点睛**：全局关闭的前提下，可在 data 中挑选 1-2 个关键极值点（最大/最小/异常），单独开启 `label: { show: true }` 作为视觉锚点。
  - **自动防重叠**：只要图表中存在任何开启的标签，必须在同级配置 `labelLayout: { hideOverlap: true }`。

### 环形图（饼图）
- 图例展示扇区名称，标签仅显示百分比，扇区名称与数值不混排。必须配置 legend 确保图例可见。
- 标签统一策略：`label: { show: true, formatter: '{d}%', position: 'inside' }`
- 关闭引导线：`labelLine: { show: false }`
- 标签对比度：标签颜色与色板中最深色和最浅色扇区对比度均须 >= 4.5:1。若无法同时满足，则收窄色板明度范围，使色板集中在深色或浅色区间。

### 甘特图
- 实现方式：必须使用 `type: 'custom'` 自定义系列，禁止用"堆叠柱状图 + 透明占位柱"模拟。
- 坐标轴：X 轴 `type: 'value'` 或 `'time'`（连续轴），Y 轴 `type: 'category'`（任务名）。禁止两轴均为 category。
- 数据结构：`value: [yIndex, start, end]` 绝对坐标格式，`encode: { x: [1, 2], y: 0 }`。同一 yIndex 可传多段数据表示不连续任务。
- 渲染逻辑：必须配置 `renderItem` 函数，使用 `api.coord([x, y])` 精确计算任务起点和终点的画布坐标，并返回 `type: 'rect'` 矩形对象。
- 系列设置 `clip: true`，防矩形溢出 grid。

### Combo 图（双 Y 轴）
- 左右边距各留 80-100px 以上，禁止左右不对称导致次轴标签被截断。
- 禁止两个系列都使用 bar 类型，否则柱体会重叠。至少一个系列须使用 line 类型区分。
- combo 图最多只允许一个 series 开启 label。

## 禁用图表
以下图表类型在 PPTX 场景中表现不佳，**禁止使用 ECharts 实现**，应改用其他方式：
- **tree / graph**（树状图、网络拓扑图）：表现会很糟糕，须使用 SVG 进行绘制。
- **瀑布图、非零基线柱**：实现起来太过复杂，用纯粹的柱状图足够满足 pptx 场景的需求。
