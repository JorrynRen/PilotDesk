# 图标获取指南
从 https://www.flaticon.com/ 获取图标。 只允许获取 白名单 icon pack中的图标。

```python
import sys
sys.path.insert(0, "{技能目录}/skills/pptx/scripts")

# 导入图标搜索 和 图标下载的函数
from icon import search_icons, download_icon
def search_icons(
    query: str,
    limit: int = 20,
) -> list[dict]:
    """
    搜索 Iconify 图标，仅返回白名单图标包中的结果。

    调用 Iconify Search API，并自动过滤掉不在白名单内的图标包，
    适合 AI 用于选择合适的开源图标。

    参数:
        query: 搜索关键词，例如 "home"、"arrow-right"
        limit: 期望搜索返回的最大图标总数（含未过滤结果），默认 20。
               实际返回条数 ≤ limit，且只含白名单图标包中的图标。

    返回:
        图标信息列表，每项为一个字典：
        {
            "id":      str,   # 图标 ID，例如 "tabler:home"
            "prefix":  str,   # 图标包前缀，例如 "tabler"
            "name":    str,   # 图标名称，例如 "home"
            "pack_name": str, # 图标包显示名称，例如 "Tabler Icons"
            "license": str,   # 许可证，例如 "MIT"
        }

    示例:
        >>> results = search_icons("home")
        >>> for r in results:
        ...     print(r["id"], r["license"])
        tabler:home MIT
        lucide:home ISC
        ...

    抛出:
        RuntimeError: 网络请求失败时
    """

def download_icon(
    icon_id: str,
    save_path: str,
    width: Optional[int] = None,
    height: Optional[int] = None,
    color: Optional[str] = None,
) -> str:
    """
    从 Iconify 下载 SVG 图标到本地文件，仅允许白名单内的图标包。

    参数:
        icon_id:   图标 ID，格式为 "prefix:name"，例如 "tabler:home"
        save_path: 保存路径，例如 "/tmp/home.svg" 或 "icons/home.svg"
                   若目录不存在，会自动创建。
        width:     SVG 宽度（像素），可选。仅指定一个时另一维度自动计算。
        height:    SVG 高度（像素），可选。
        color:     图标颜色，支持以下格式：
                   - 十六进制颜色（带或不带 #），例如 "#ff0000" 或 "ff0000"
                   - CSS 颜色名，例如 "red"、"blue"
                   仅对单色（无硬编码调色板）图标生效。可选。

    返回:
        已保存的 SVG 文件的绝对路径。

    抛出:
        PermissionError: 图标包不在白名单中时。
        ValueError:      icon_id 格式不正确时。
        RuntimeError:    网络请求失败或写文件失败时。

    示例:
        >>> path = download_icon("tabler:home", "icons/home.svg", height=24, color="#333333")
        >>> print(path)
        /abs/path/to/icons/home.svg

        >>> # 尝试下载非白名单图标包
        >>> download_icon("noto:home", "icons/home.svg")
        PermissionError: 禁止下载：图标包 'noto' 不在白名单中 ...
    """
```

import puresvg
from PIL import Image

#将svg转成png
arr = puresvg.render("input.svg", width={width}, height={height})

img = Image.fromarray(arr)
img.save("output.png")