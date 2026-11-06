---
name: glue-teach
description: 用 GlueSight 的示教 MCP（gluesight 服务的 teach_* 工具）完成段配方示教：看示教件帧、画走廊（按胶色吸附）、取色、试测、配检测项与零件边参考、用历史件整件复测，并按验收口径判断。用户要求“帮我示教”“画走廊”“示教 NEW-x”“取色”“试测这一段”“配胶偏”“复测历史件”时使用。
---

# 胶型检测示教

接口契约见 `docs/integration/agent-teaching.md`。工具只改候选。示教件、分段、像素当量、发布由人在界面上做；检测项开关、公差与图纸标称值能用 `teach_segment` 写，但取值只用人在对话里给出的，不自行决定。

## 1. 看现状

`teach_status` 不带参数列出候选，再带 `recipe` 取现状。下列任一情况停下来请人在界面处理，不要绕过：

- `teachingPiece` 为 null（没选示教件）；
- 某段 `frames` 是原因文字而不是 `[start, end]`（分段没确认）；
- 要示教的相机 `mmPerPx` 为空。

待办 = 每段 `cameras` 里的每台相机，在 `cameras[].segments` 里 `saved` 为假或彩色相机 `colorModel` 为假的项。

## 2. 每台相机、每段画走廊

1. **选帧**：在这一段 `frames` 的中间部分选 `seq`，避开两端各约 10% 的起胶、收胶过渡帧。
2. **看图**：`teach_view`（不带 `segment`）看整帧，找到胶条，按网格读出沿胶条的 3–6 个点（弯道多取几个），从起胶端（喷嘴一侧）排到收胶端。
3. **画走廊**：`teach_corridor`，`extract` 用缺省 `auto`。
   - 彩色帧按胶色吸附：点离胶中心在 `radiusPx`（缺省 30）以内就行，整幅图上目测的点就够，不用放大逐点读。胶条旁边有别的同色物（另一条胶、胶丝）时把 `radiusPx` 调小，免得吸到旁边。返回里已含沿新走廊重新取色的结果（`colorModel`），不用再调 `teach_fit_color`。
   - 灰度帧按边缘提取：大图上读的坐标常偏 10–20 px，在胶条上取 4–6 处用 80×160 左右的小 `crop`、`grid: 10` 放大，读上下边取中点。报“边缘不连续”且笔划确实贴着胶条中心时，改 `extract: "none"` 直接用笔划。
4. **核对**：看返回叠图，青线应沿胶条中心走完整段，没有台阶；偏出胶条就把那一处的点重读后再画。彩色帧再看 `colorModel.quality`：`usable` 为真，`centerShift` 在 1 px 左右（走廊居中）。分离度明显低于其他段或胶样本很少时，换一帧再画。
5. **试测**：`teach_trial`，`from`/`to` 取第 1 步避开过渡帧后的稳态范围。

## 3. 零件边参考与检测项（只在人给了值时）

- 人给了图纸胶边距、要开胶偏检测时：`teach_segment` 写 `offsetRef`，如 `{"measure": "toEdge", "camera": "cam1", "edge": {"polarity": "brightToDark", "searchMm": 18, "contrastMin": 70, "nominalMm": <人给的值>}}`，`checks.offset.enabled` 设真。
- 找边参数（`polarity`、`searchMm`、`contrastMin`）可以自己试：改一项后 `teach_trial` 看 `edgeMm` 是否稳定、中位是否接近标称值，再用 `teach_view` 带 `segment` 看绿线是否落在零件边上。拉丝、高反光底材上 `contrastMin` 常要 70 左右。
- 人给了胶宽规格、断胶长度等时，用 `teach_segment` 的 `checks` 补丁写入；没给就保持现状。

## 4. 验收（稳态范围内）

硬性：`fallbackFrames`、`errors` 为空，`limits.gapOverLimit` 与 `limits.presentBelow` 为空（有胶的帧不能按这一段的断胶口径报断）。`states` 里非 `measured` 的帧只能是设计上不测的（`notInspected`）。

参考：`gapFrames` 里低于断胶长度的小断口、`limits.widthOutside` 里的帧要逐个看图，分清是胶本身（绕圈、堆胶、收窄）还是测偏；ToEdge 段的 `edgeMm` 中位应接近 `offsetRef.edge.nominalMm`。

不合格时，对 `worst` 里的帧用 `teach_view` 带 `segment` 看测量叠图（黄线 = 每站测到的胶宽，绿线 = 胶边距），判断是走廊偏、示教帧不典型还是图像本身（高光、遮挡）。前两种换帧或重画；同一段连续两次不合格，或判断是图像或参数问题，停下来把现象和帧号报给人。

## 5. 整件复测

所有段都过了第 4 步后，`teach_replay` 不带 `records` 列出最近的历史件，请人指出哪些是好件、哪些是缺陷件（或人已说明），再带 `records` 复测。好件应判合格；缺陷件应判不合格，且 `regions` 里报错的段与缺陷位置对得上。对不上时看那一段的 `reason` 与帧组范围，回到第 2–4 步。

## 6. 汇报

逐台相机、逐段列出：示教帧 `seq`、走廊长度与站数、取色质量、稳态范围、胶宽中位与 P5–P95、胶边距分布、断口帧数；整件复测每件的原判定与复测判定。再列出需要人确认的事项：未通过的 `blockers`、还没给的图纸标称值与胶宽规格。不要替人发布。
