# 美观图表添加指南
**添加图表时必须遵循以下流程：**
1. 调用 `get_chart_style_list()` 获取可用样式列表
2. 根据图表类型选择合适的 `chart_id`
3. 调用 `get_chart_style(chart_id)` 获取样式配置
4. 按样式配置创建并美化图表
**禁止**直接硬编码颜色、字体等样式参数。

## 图表样式管理

```python
# 导入图表样式管理脚本
import sys
sys.path.insert(0, "skills/pptx/scripts")
from chart import get_chart_style, get_chart_style_list, set_chart_style

def get_chart_style_list() -> List[Dict[str, str]]:
    """
    获取所有图表样式的 ID 和描述列表

    Returns:
        包含 chart_id 和 description 的字典列表
    """
def get_chart_style(chart_id: str) -> Optional[Dict[str, Any]]:
    """
    根据 chart_id 获取图表样式

    Args:
        chart_id: 图表样式ID

    Returns:
        图表样式配置字典，如果不存在返回 None
    """

def set_chart_style(
    chart_id: str,
    description: str,
    chart_style: Dict[str, Any]
) -> None:
    """
    设置或更新图表样式

    Args:
        chart_id: 图表样式ID
        description: 图表样式描述
        chart_style: 图表样式配置字典，包含以下字段：
            - structure: 图表结构配置
                - chart_type: 图表类型（如 "bar", "line", "pie" 等）
                - variant: 变体类型（如 "clustered", "stacked" 等）
                - orientation: 方向（"vertical" 或 "horizontal"）
            - data_encoding: 数据编码配置
                - x: X轴数据映射字段
                - y: Y轴数据映射字段
                - series: 系列数据映射字段
            - layout: 布局配置
                - legend_position: 图例位置（如 "top", "bottom", "left", "right"）
                - title_position: 标题位置
            - style: 样式配置
                - background: 背景色（如 "transparent", "#FFFFFF"）
                - border: 边框样式（如 "none", "solid"）
                - shadow: 是否显示阴影（布尔值）
            - color: 颜色配置
                - palette: 调色板列表（颜色代码数组）
                - highlight: 高亮颜色
            - typography: 字体配置
                - title: 标题字体样式（如 "bold 20pt"）
                - label: 标签字体样式（如 "12pt gray"）
            - axis: 坐标轴配置
                - grid: 网格线样式（如 "light", "dark", "none"）
                - axis_line: 是否显示坐标轴线（布尔值）
    """
```


## 如何应用图表样式
先了解当前可用的图表样式
```python
import sys
sys.path.insert(0, "skills/pptx/scripts")
from chart import get_chart_style, get_chart_style_list

# 使用print 打印获取的图表样式 ID 和描述列表，来选择合适的图表
print(get_chart_style_list())
```

然后，获取图表样式详情
```python
# 使用print打印获取的图表样式配置字典，来了解图表样式的详情
print(get_chart_style({chart_id}))
```


```python
# 必须在你 读取到图表样式的详情 之后再执行这一步。
from pptx.dml.color import RGBColor
from pptx.util import Pt

# 示例调色板
palette = ["示例颜色1", "示例颜色2"] # 根据获取的图表样式设置, 可以结合当前ppt 的颜色、风格进行调整

# 遍历所有 series 应用颜色
for i, series in enumerate(chart.series):
    color_hex = palette[i % len(palette)]
    rgb = RGBColor.from_string(color_hex.replace("#", ""))

    fill = series.format.fill
    fill.solid()
    fill.fore_color.rgb = rgb
	# 对于折线图， 可用设置线宽
	line = series.format.line
    line.color.rgb = rgb
    line.width = Pt({线宽})

# 控制是否显示 legend
chart.has_legend = True/False

legend = chart.legend

# 设置位置
position = xx # 根据获取的图表样式设置

if position == "top":
    legend.position = XL_LEGEND_POSITION.TOP
elif position == "bottom":
    legend.position = XL_LEGEND_POSITION.BOTTOM
elif position == "left":
    legend.position = XL_LEGEND_POSITION.LEFT
elif position == "right":
    legend.position = XL_LEGEND_POSITION.RIGHT
```


