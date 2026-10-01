#!/usr/bin/env bash
# PPT 幻灯片工具集
#
# 用法:
#   pptx.sh check-layout <htmls...> [--mode full|static]  提交布局检测任务（异步，输出 task_id）
#   pptx.sh html-to-pptx <htmls...> -o <output>            提交 PPTX 转换任务（异步，输出 task_id）
#   pptx.sh screenshot   <htmls...> [-o <dir>]             提交截图任务（异步，输出 task_id）
#   pptx.sh query-task <task_id> [-o <output>]             查询任务状态 / 下载结果
#   pptx.sh cancel-task <task_id>                          取消任务
#
# 注意: 由于 bash 工具可能在 10s 后自动转入后台，check-layout、html-to-pptx 和 screenshot
#       仅完成"提交"即返回 task_id。模型需手动调用 query-task 获取最终结果。
SCRIPT_DIR="$(dirname "$0")"
python "$SCRIPT_DIR/pptx_cli.py" "$@" 2>&1
