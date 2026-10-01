# 提示词最佳实践

这些提示词原则适用于 `generate_image` 工具。

## 结构
- 使用以下格式编写：

```text
{scene_description}（场景+主体+细节）
{subject_placement}
no text, no letters, no typography, no QR codes, no words of any kind. {additional_constraints}
```

- `{scene_description}`：主体描述要注重抽象与氛围，不应直接点明具体物品。应以意境、象征或情感氛围的表达为主，避免照搬问题中的事物本身，否则生成画面会缺少美感和联想空间
- `{subject_placement}`：正向描述主体所在画面中的位置，使主体信息明确
- `{additional_constraints}`：可根据实际场景补充其他限制

## 构图与布局
- 优先用主体的聚集、消散、边缘化、纵深和光线方向来形成留白；文字区应表现为画面自然空场，而不是孤立的版式命令。
- 避免使用诸如“留白给文字”、“for text overlay”、“给标题预留空间”等与用途相关的描述，这样容易导致图片出现用户不能接受的效果。
- 仅在实质性有帮助时指定取景和视角（特写、全景、俯视）。
- 对于人物，在有意义时描述身体取景、比例、视线方向和物体交互（`全身可见`、`低头看书`、`双手自然握住车把`）。
