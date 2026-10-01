
"""
图表样式管理模块
提供图表样式的增删改查功能
"""
import json
import os
from typing import Optional, List, Dict, Any
from pathlib import Path


# 默认图表样式配置文件路径
_DEFAULT_CHART_STYLES_FILE = Path(__file__).parent / "chart_styles.json"


def _get_chart_styles_file() -> Path:
    """获取图表样式配置文件路径"""
    return _DEFAULT_CHART_STYLES_FILE


def _load_chart_styles() -> List[Dict[str, Any]]:
    """
    加载所有图表样式配置

    Returns:
        图表样式列表
    """
    file_path = _get_chart_styles_file()
    if not file_path.exists():
        # 如果文件不存在，返回默认样式
        return [
            {
                "chart_id": "default_bar",
                "description": "默认柱状图样式",
                "chart_style": {
                    "structure": {
                        "chart_type": "bar",
                        "variant": "clustered",
                        "orientation": "vertical"
                    },
                    "data_encoding": {
                        "x": "category",
                        "y": "value",
                        "series": "type"
                    },
                    "layout": {
                        "legend_position": "top",
                        "title_position": "top"
                    },
                    "style": {
                        "background": "transparent",
                        "border": "none",
                        "shadow": False,
                    },
                    "color": {
                        "palette": ["#5470C6", "#EE6666"],
                        "highlight": "#FFA800"
                    },
                    "typography": {
                        "title": "bold 20pt",
                        "label": "12pt gray"
                    },
                    "axis": {
                        "grid": "light",
                        "axis_line": False
                    }
                }
            }
        ]

    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            return json.load(f)
    except (json.JSONDecodeError, IOError) as e:
        raise ValueError(f"加载图表样式配置失败: {e}")


def _save_chart_styles(styles: List[Dict[str, Any]]) -> None:
    """
    保存图表样式配置

    Args:
        styles: 图表样式列表
    """
    file_path = _get_chart_styles_file()
    file_path.parent.mkdir(parents=True, exist_ok=True)

    try:
        with open(file_path, 'w', encoding='utf-8') as f:
            json.dump(styles, f, ensure_ascii=False, indent=2)
    except IOError as e:
        raise ValueError(f"保存图表样式配置失败: {e}")


def get_chart_style(chart_id: str) -> Optional[Dict[str, Any]]:
    """
    根据 chart_id 获取图表样式

    Args:
        chart_id: 图表样式ID

    Returns:
        图表样式配置字典，如果不存在返回 None
    """
    styles = _load_chart_styles()

    for style in styles:
        if style.get("chart_id") == chart_id:
            return style.get("chart_style")

    return None


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
    styles = _load_chart_styles()

    # 查找是否存在相同的 chart_id
    found = False
    for i, style in enumerate(styles):
        if style.get("chart_id") == chart_id:
            # 更新现有样式
            styles[i] = {
                "chart_id": chart_id,
                "description": description,
                "chart_style": chart_style
            }
            found = True
            break

    if not found:
        # 添加新样式
        styles.append({
            "chart_id": chart_id,
            "description": description,
            "chart_style": chart_style
        })

    _save_chart_styles(styles)


def get_chart_style_list() -> List[Dict[str, str]]:
    """
    获取所有图表样式的 ID 和描述列表

    Returns:
        包含 chart_id 和 description 的字典列表
    """
    styles = _load_chart_styles()

    return [
        {
            "chart_id": style.get("chart_id", ""),
            "description": style.get("description", "")
        }
        for style in styles
    ]
