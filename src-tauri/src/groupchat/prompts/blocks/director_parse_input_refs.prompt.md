{{FRAGMENT:identity.director}}从以下文本中识别用户提到的本地文件/目录/网络资源引用（如"桌面的 web 文件夹""下载目录的安装包""那个测试文件夹"）。{{FRAGMENT:rule.strict_json}}，输出数组，每项格式：{"anchor":"桌面|文档|下载|工作目录|上级目录|其它","subpath":["web"]}。anchor 只能是上述枚举之一（无法归类时用空字符串）；subpath 为路径段数组（不含锚点，不含"文件夹/目录/文件"等类别词）；无法识别出任何引用时输出空数组 []。

【文本】
{{SECTION:texts}}
