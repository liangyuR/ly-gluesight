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
- 要示教的相机 `mmPerPx` 为空；
- `engine.loaded=false`，先按返回的运行库来源和错误恢复引擎；
- 开启逐帧定向的相机有 `headingIssues`，先处理 `window`、出胶点等前提；
- `coverage` 未知、边界不可信、出现 `zeroCoverageSegments` 或直线段没有有效胶偏窗口，先核对真实覆盖，不用单段试测替代生产覆盖检查。

待办 = 每段 `cameras` 里的每台相机，在 `cameras[].segments` 里 `saved` 为假，或彩色相机的 `modelUsable` 为假的项。`modelPresent` / `colorModel` 只表示字段存在；查看 `quality` 和 `modelIssues`，背景样本为 0、过曝或质量未知不能当作已就绪。配置检查通过也不代表路径正确或实测完成。

保存本次 `snapshot` 的项目、候选实例、修订号和示教轮次。写工具都带上读到的 `expectedRevision`，冲突后重新读状态并重新应用最小补丁，不取最新 revision 来覆盖旧文档。读测结果 `snapshot.stale=true` 时归档为原快照证据，重测当前快照后才能验收；共享相机参数、整件测量和结构修改由一个执行者协调。

## 2. 每台相机、每段画走廊

1. **选帧**：在这一段 `frames` 的中间部分选 `seq`，避开两端各约 10% 的起胶、收胶过渡帧。
2. **看图**：`teach_view`（不带 `segment`）看整帧，找到胶条，按网格读出沿胶条的 3–6 个点（弯道多取几个），从起胶端（喷嘴一侧）排到收胶端。
   - **走廊要覆盖整段胶**：吸附只校正横向位置，走廊的起止就是笔划的首尾。彩色段的颜色模型沿走廊取样，走廊缩短一截，模型就少见一段胶；在外移缺陷件上，这样做曾让整件判定从“超宽、跑偏”变成“断续”。所以第一点放大到喷嘴处，点在胶刚出胶嘴的位置；最后一点放在胶条离开画面或这一段结束的地方。
   - **出胶点**：`teach_status` 里这台相机的 `nozzle.calibrated` 为假时，出胶点由各定向段（`heading` 不为 null）的走廊起点推出，检测窗口按距出胶点的毫米数截取；同一相机各段的起点要一致，否则报“出胶点不一致”。
3. **画走廊**：`teach_corridor`，带当前 `expectedRevision`，`extract` 用缺省 `auto`。
   - 彩色帧按胶色吸附：点离胶中心在 `radiusPx`（缺省 30）以内就行，整幅图上目测的点就够，不用放大逐点读。胶条旁边有别的同色物（另一条胶、胶丝）时把 `radiusPx` 调小，免得吸到旁边。返回里已含沿新走廊重新取色的结果（`colorModel`），不用再调 `teach_fit_color`。
   - 灰度帧按边缘提取：大图上读的坐标常偏 10–20 px，在胶条上取 4–6 处用 80×160 左右的小 `crop`、`grid: 10` 放大，读上下边取中点。报“边缘不连续”且笔划确实贴着胶条中心时，改 `extract: "none"` 直接用笔划。
   - 新走廊、示教帧和颜色模型一次提交；提取、最终越界/站数校验、取色失败或模型不可用时保留旧状态。成功读取 `commit` 和新 revision。错误若明确“已保存但刷新视图失败”，先重新读 `teach_status`，不要盲目重试。独立 `teach_fit_color` 可保存不可用模型用于诊断，成功返回仍须检查 `modelUsable`。
4. **核对**：看叠图，青线应沿新胶中心走完整段，没有台阶或跳到旧胶/相邻分叉。彩色帧看 `modelUsable`、`quality`、样本数、中心移动和分离度；数值可用不能证明路径正确。背景样本缺失时核对曝光与取样范围，不重复拟合同一张无信息图，也不复制其他段的模型。
5. **试测**：`teach_trial` 的 `from`/`to` 限制实际量测区间，读取返回的 `requestedWindow`、`window`、`range`。可先用稳态范围诊断，再核对完整段与整件；不能把短胶的末帧通过当作完整工艺验收。

## 3. 零件边参考与检测项（只在人给了值时）

- 人给了图纸胶边距、要开胶偏检测时：`teach_segment` 写 `offsetRef`，如 `{"measure": "toEdge", "camera": "cam1", "edge": {"polarity": "brightToDark", "searchMm": 18, "contrastMin": 70, "nominalMm": <人给的值>}}`，`checks.offset.enabled` 设真。
- 找边参数（`polarity`、`searchMm`、`contrastMin`）可以自己试：改一项后看 `measurement.edge` 的有效帧数、有效率、缺失帧号，再看 `edgeMm` 分布，并用 `teach_view` 带 `segment` 核对代表帧、缺失帧与最差帧的绿线确实落在零件边。稳定中位数或降低门限后有数值都不足以确认边参考；无法确认真实边时保留失败证据，不把相邻胶/高光边当作标称。弯道不开胶偏。
- 人给了胶宽规格、断胶长度等时，用带 `expectedRevision` 的 `teach_segment` 补丁写入，检查返回 `impact` 和新 revision；没给就保持现状。联合核对胶宽标称/公差与独立溢胶上限，处理 `parameterWarnings` 中的矛盾。示教件中位数不能自动变成图纸标称，也不自动放宽溢胶或断胶规则。

## 4. 验收（稳态范围内）

硬性：快照没有 stale，`measurement.measuredFrames == expectedFrames`，逐项核对所有 `nonMeasuredFrames`；只有 `expected=false` 的计划不测可以排除，Error、Missing、Unmeasured、OutOfCorridor 均不能因 `errors=[]` 被漏掉。`fallbackFrames`、`limits.gapOverLimit`、`limits.presentBelow` 为空（有胶的帧不能按这一段的断胶口径报断）。窄区间通过只说明该范围。

参考：`gapFrames` 中小断口、`limits.widthOutside` 与 `limits.overflowFrames` 要逐个看图，分清胶本身（绕圈、堆胶、收窄）与测偏。平均宽度在公差带内也可能因最大宽度判溢胶；开了偏宽只预警（`wideWarnOnly`）的段，偏宽与溢胶只记预警，重点核对偏窄与断口。ToEdge 须联合核对实际测量范围的有效率、缺失帧、真实边落点及图纸值；首尾剔除后的可判窗口留到整件结果单独核对。

不合格时，对 `worst` 里的帧用 `teach_view` 带 `segment` 看测量叠图（黄线 = 每站测到的胶宽，绿线 = 胶边距），判断是走廊偏、示教帧不典型还是图像本身（高光、遮挡）。前两种换帧或重画；同一段连续两次不合格，或判断是图像或参数问题，停下来把现象和帧号报给人。

## 5. 整件复测

所有段都过了第 4 步后，`teach_replay` 不带 `records` 列出历史件，使用人已确认的好件与缺陷件；未说明时请人指出。逐件保存 `comparisonId`、`snapshot`，先核对 `measurement.cameras[].segments[]` 的应测/实测、全部未测帧、`coverage` 和 `flags`；再核对最终 `regions[].offsetUnjudged`、`offsetEvidence` 的参考相机、首尾剔除后窗口及缺失帧。方向细化后的判定区域可能与原图量测的比例计划不同，以最终区域的胶偏证据为准。

好件应判合格；缺陷件应判不合格且报错区域与缺陷位置对应。`verdict=OK` 与检测完整是独立条件，有 `flags.incomplete`、零覆盖段或应测未测帧时不能宣称示教通过。同一示教件自测也不能代替独立好件和缺陷件验收。对不上时带帧号回到第 2–4 步，不通过增加 skip、缩短走廊或放宽门限隐藏问题。

## 6. 汇报

逐台相机、逐段列出：项目/候选实例/修订、示教帧、走廊长度与站数、模型可用性及原因、实际试测范围、胶宽分布、边距有效率和缺失帧、guard 后覆盖与胶偏窗口；整件逐件列出原判定/复测判定、flags、全部未测成帧和未判原因。配置检查、局部试测、整件覆盖、测量完整性、生产结论分别说明。再列出未解决问题及需要人确认的规格；不要替人发布。
