---
name: glue-teach
description: 用 GlueSight 的示教 MCP（gluesight 服务的 teach_* 工具）完成段配方的走廊示教：看示教件帧、画走廊、取色、试测并按验收口径判断。用户要求“帮我示教”“画走廊”“示教 NEW-x”“取色”“试测这一段”时使用。
---

# 胶型检测走廊示教

接口契约见 `docs/integration/agent-teaching.md`。工具只改候选的走廊与颜色模型；示教件、分段、像素当量、检测项与公差、图纸标称值、发布都由人在界面上做。

## 1. 看现状

`teach_status` 不带参数列出候选，再带 `recipe` 取现状。下列任一情况停下来请人在界面处理，不要绕过：

- `teachingPiece` 为 null（没选示教件）；
- 某段 `frames` 是原因文字而不是 `[start, end]`（分段没确认）；
- 要示教的相机 `mmPerPx` 为空。

待办 = 每段 `cameras` 里的每台相机，在 `cameras[].segments` 里 `saved` 为假或彩色相机 `colorModel` 为假的项。

## 2. 每台相机、每段

1. **选帧**：在这一段 `frames` 的中间部分选 `seq`，避开两端各约 10% 的起胶、收胶过渡帧。
2. **看图**：`teach_view`（不带 `segment`）看整帧，找到胶条；再用 `crop` 放大胶条所在区域，按网格读坐标。
3. **读坐标**：大图上读的像素坐标常偏 10–20 px。在胶条上取 4–6 个位置，各用 80×160 左右的小 `crop`、`grid: 10` 放大，读那一点胶条的上下边取中点。
4. **画走廊**：`teach_corridor` 的 `stroke` 用这些中点，从起胶端（喷嘴一侧）排到收胶端。先用缺省的 `extract: true`；报“边缘不连续”且笔划确实贴着胶条中心时，改 `extract: false` 直接用笔划。核对返回叠图：青线应沿胶条走完整段，偏出胶条就重读那一处的坐标。
5. **取色**（`cameras[].color` 为真）：`teach_fit_color`。胶样本数很少或分离度明显低于其他段时，换一帧重画走廊后再取。
6. **试测**：`teach_trial`，`from`/`to` 取第 1 步避开过渡帧后的稳态范围。

## 3. 验收（稳态范围内）

硬性：`fallbackFrames`、`errors` 为空，`limits.gapOverLimit` 与 `limits.presentBelow` 为空（有胶的帧不能按这一段的断胶口径报断）。`states` 里非 `measured` 的帧只能是设计上不测的（`notInspected`）。

参考：`gapFrames` 里低于断胶长度的小断口、`limits.widthOutside` 里的帧要逐个看图，分清是胶本身（绕圈、堆胶、收窄）还是测偏；ToEdge 段的 `edgeMm` 中位应接近 `offsetRef.edge.nominalMm`。

不合格时，对 `worst` 里的帧用 `teach_view` 带 `segment` 看测量叠图（黄线 = 每站测到的胶宽，绿线 = 胶边距），判断是走廊偏、示教帧不典型还是图像本身（高光、遮挡）。前两种换帧或重画；同一段连续两次不合格，或判断是图像或参数问题，停下来把现象和帧号报给人。

## 4. 汇报

逐台相机、逐段列出：示教帧 `seq`、走廊长度与站数、是否取色、稳态范围、胶宽中位与 P5–P95、胶边距分布、断口帧数。再列出需要人确认的事项：未通过的 `blockers`、ToEdge 的图纸标称值、胶宽规格。不要替人发布。
