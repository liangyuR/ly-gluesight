# Agent 示教接口（MCP）

GlueSight 内嵌一个 MCP 服务，让 Agent（Claude Code 等）直接完成段配方的示教：看示教件的帧、画走廊、取色、试测、配检测项与零件边参考、用历史件整件复测。Agent 走的是示教页背后同一套后端函数，校验、留痕与界面一致；示教流程与验收口径写在项目 Skill `.claude/skills/glue-teach/SKILL.md`。

## 范围与边界

- 只改**候选**（草稿工作区）：走廊、颜色模型、各段的检测项与零件边参考。发布、生效、切换生产配方、PLC、相机参数、项目切换都不提供工具，仍由人在界面上操作。
- 示教件、分段确认、像素当量在界面上由人确认，工具只读出现状。检测项开关、公差、本段搜索半宽与胶宽范围、测量区间与 ToEdge 的图纸标称值可以经 `teach_segment` 写入，但取值要由人给出，Agent 不自行决定。
- 工具与界面共用候选的修订号。三个写工具都接受可选 `expectedRevision`；不匹配时拒绝写入，省略则以工具入口读取的修订号提交。入口读取的候选实例也在实际准备/提交处核对，删除后重建出同编号、同修订号的候选不能接收旧实例的操作。不能拿最新修订号保存旧文档；冲突后重新读取并重新应用补丁。每次成功写入发 `workspace://changed`，界面无未保存改动时随即重新载入，有未保存改动时下一次保存会因过期修订失败。
- 测量类工具（试测、带测量叠图的看图、带 `records` 的整件复测）与界面试测同一规则：有在途件或正在采集时拒绝。

## 启用与连接

桌面应用启动时默认启用 MCP，包括正式版、`pnpm dev`、`pnpm tauri dev` 和 `pnpm dev:joint`，无需配置或传递访问令牌。`pnpm dev:web` 仅启动前端，不提供 MCP。

| 变量 | 含义 |
|---|---|
| `GLUESIGHT_MCP_PORT` | 端口，缺省 47821 |

服务只监听 `127.0.0.1`，地址为 `http://127.0.0.1:<端口>/mcp`。启动结果写应用日志（标题“示教 MCP”）；设置页“关于”里的“示教 MCP”一项只读显示监听地址或启动失败的原因。端口被占用时启动失败；同时运行多个实例时，应为每个实例设置不同端口。

客户端无需鉴权头，可直接连接。注册 Claude Code：

```powershell
claude mcp add --transport http gluesight http://127.0.0.1:47821/mcp
```

若配置了 `GLUESIGHT_MCP_PORT`，客户端地址需同步修改。

## 传输

MCP Streamable HTTP 的无状态子集：

- 只接受 `POST /mcp`，请求体为单个 JSON-RPC 2.0 消息，响应 `application/json`；不开 SSE 流，`GET` 返回 405，不发 `Mcp-Session-Id`。
- 不要求 `Authorization` 头；带 `Origin` 头的请求一律返回 403，阻止携带网页来源的调用。
- 支持的方法：`initialize`、`ping`、`tools/list`、`tools/call`；通知（无 `id`）返回 202。
- 工具执行失败返回 `isError: true` 的结果，文本为业务原因（与界面提示同一措辞）；参数或协议错误返回 JSON-RPC 错误。

## 工具

所有工具都以 `recipe`（候选的配方编号，如 `NEW-1`）指定候选，作用于当前项目；帧以示教件轮次里的帧组号 `seq` 指定，坐标一律是原图像素。

候选详情、看图、试测与成功写入结果携带 `snapshot`：`project`、`recipe`、`instanceId`、`revision`、`roundId` 记录本次所用快照，`current` 是结束时的项目和候选身份，`stale` 表示执行期间候选被修改、重建、删除或项目改变。`stale=true` 的测量只代表原快照，不可当作当前候选的验收结果。写入另返回 `baseRevision` 与提交后的 `revision`。整件复测逐件冻结所用身份，实际取比较快照时同时核对实例与修订号；全部记录完成后一次核对当前状态，所以后续记录执行期间的修改也会使早完成项标记 stale，不会伪装成同一个最新快照。

| 工具 | 读 / 写 | 用途 |
|---|---|---|
| `teach_status` | 读 | 不给 `recipe` 时列出当前项目的候选；给了时返回示教现状：下一步、示教件、各段的帧组范围与检测相机、各相机各段的走廊 / 颜色模型 / 示教帧、发布前未通过的检查 |
| `teach_view` | 读 | 示教件一帧的 PNG：叠加原图坐标网格；给 `segment` 时按已保存走廊测这一帧，叠加走廊、胶两侧边与胶边距线，并附这一帧的指标 |
| `teach_corridor` | 写 | 由笔划提取并校验最终走廊，按胶色测量的段完成取色，再一次提交示教帧、走廊和模型。返回提交状态、长度、质量与叠图 |
| `teach_fit_color` | 写 | 沿已保存的走廊取色，写入这一段的颜色模型（只对彩色示教帧、没选按灰度的段）；返回胶色、背景色与分离度 |
| `teach_trial` | 读 | 用已保存走廊量测指定区间，返回实际范围、全部未测成帧、边距有效率、分布和阈值诊断 |
| `teach_segment` | 写 | 按补丁改一段的检测项（`checks`）、零件边参考（`offsetRef`）、搜索半宽与胶宽范围覆盖（`searchMm`、`widthRange`）与逐帧定向的测量区间覆盖（`headingWindow`） |
| `teach_replay` | 读 | 按候选现在的配置从原图复测历史件并整件判定；不给记录时列出最近的历史件 |

### `teach_status`

参数：`recipe?: string`。

返回 JSON 文本。不带候选时返回 `project`、`candidates` 与 `engine`。带候选时返回：

- `project`、`recipe`、`revision`、`snapshot`、`next`，以及 `teachingPiece`（`roundId`、`recordId`、`groupCount`；没选为 null）。
- `segments[]`：段号、类型、检测相机、帧组范围或不能确定的原因、`checks`、`offsetRef`、`heading`、`detect`（本段实际用的检测参数：搜索半宽、极性、胶宽范围）、`detectOverride`（本段的 `searchMm`、`widthRange` 覆盖，没设为 null）、`skip`、`skipHead`、`skipTail`、`parameterWarnings`。
- `cameras[]`：彩色模式、`mmPerPx`、`calibMmPerPx`、`ready`、`cameraWindow`（这台相机各段共用的测量区间 `startMm`、`lengthMm`，段没设覆盖时用它；没设为 null，这时用默认跳过 5 mm、测到 20 mm）、`nozzle`（点、`calibrated`、`source: calibrated | corridorStarts`、`warning`）与 `headingIssues`（`segment`、`message`）。`warning` 是推算出胶点时有走廊起点偏离超过 1 mm 的提示，不阻断；`headingIssues` 按段指出区间换算后不足 8 px 等前置问题。
- 每台相机的 `segments[]`：`segment`、`window`（开启逐帧定向的段实际用的测量区间：`startMm`、`lengthMm`、`endMm` 即界面“测量到”、`source: segment | camera | default`，即本段覆盖、相机的测量区间或默认值；没开定向为 null）、`seq`、`corridorPoints`、`saved`、`measure`（实际测量方式 `gray` / `color`，[color](../decisions/color.md) CL-3；还没有走廊项为 null）、`modelPresent`、`modelUsable`、`quality`、`modelIssues`。`colorModel` 仍表示字段存在，不能用它判断可用。`quality` 保留引擎的 `glueCount`、`bgCount`、`centerShift`、`separation`、高光占比等；非法 JSON、缺少质量证据、胶样本不足、背景样本或分量缺失分别给出原因。`modelUsable=true` 也不证明走廊位于新胶上。
- `engine`：加载状态、版本、运行库目录、`overridden`、`source`（`bundledRuntime` / `developmentOverride`）、`requiredSeries`、`requiredOperators` 与失败原因。`loaded=true` 包含必需算子校验；为 false 时从 `message` 核对缺失算子或运行库问题，解决后再逐段量测。
- `passes`：示教件的笔结构（D-SB-62）。`count`、`frames`（各笔帧组数）、`skip`（配方每笔开头 `head`、结尾 `tail` 跳过帧组数）、`inRecipe`（笔结构已随 N_exp 写进配方，生产按笔分段）、`skipped`（示教件上被跳过的帧组：`pass`、`frames: [first, last]`、`short` 为整笔不检）与 `suggestion`（每笔起涂建议：`current`、`suggested`、逐笔 `passes[]` 的起涂帧组数或不参与原因、`pending`）。没有示教件为 null。每笔首尾跳过只能由人在界面设置。
- `coverage`：按示教件总帧数预览 guard、段与每笔首尾跳过后的真实检测范围，有笔结构时按笔分段，否则按比例边界；`coverageSource=teachingPieceProportional`。包括逐段逐相机应测/可用帧数、被屏蔽帧、零覆盖段和胶偏首尾剔除后的窗口。没有示教件或无法读取时为 null；没有可信边界时相关范围/计数为 null，不能当作完整覆盖。
- `blockers`：未通过的发布配置检查，包含 `advisory`；配置检查不代替实测覆盖和产品验收。

### `teach_view`

参数：`recipe`、`camera`、`seq`；可选 `segment`（按这一段的已保存走廊测量并叠图）、`crop: [x, y, w, h]`（原图像素）、`maxSide`（输出长边上限，缺省 1280）、`grid`（网格间距 px，缺省按视野自动取 50/100/200）。

返回 PNG 与说明文本（`snapshot`、原图尺寸、裁剪框、缩放、这一帧所在段、测量指标或不能测量的原因）。网格线标原图坐标，Agent 据此读出笔划点；失败帧也保留可核对的原图。

### `teach_corridor`

参数：`recipe`、`camera`、`segment`、`seq`（须在这一段的帧组范围内）、`stroke: [[x, y], ...]`（至少 2 点，原图像素，从起胶端排到收胶端）、可选 `expectedRevision`、`radiusPx`（缺省 30）、`extract`（`auto` / `color` / `edges` / `none`，缺省 `auto`）。

- `color`（彩色帧、这一段没选按灰度时的 `auto`；选了按灰度的段报错）：按胶色吸附。笔划插点后作示教线跑 `glue.color_model`（取色图 `color_teach`，`shiftMax` = `radiusPx`），每站在卡尺上找胶色段重新居中、五站中值；取它输出的胶中心线（overlay 的 `center`，站距 4 px）平滑（±20 px 滑动平均）后，以偏移上限 10 px 再居中一遍、再平滑，每 5 点留 1 点；两端沿切线补到笔划首尾在切线上的投影。吸附只校正横向位置，走廊沿胶路的起止就是笔划的首尾。彩色段的颜色模型沿走廊取样，走廊应覆盖这一段整条胶（从出胶处起）；出胶点未标定时，开启逐帧定向的段还由走廊起点推出出胶点。笔划须压在胶条上（取色以笔划处为胶样本），离胶中心不超过 `radiusPx`。示教页在彩色示教帧上刷选时走同一吸附与提交（`workspace_snap_corridor`，偏移上限为笔刷半径），实现在 `workspace::snap`。
- `edges`（灰度帧或选了按灰度的段的 `auto`）：同示教页灰度帧的刷选提取，提取范围为笔划外接框外扩 `radiusPx` 加 40 px。提取靠灰度上的胶两侧边，胶比背景暗还是亮自动判断（[r2-algorithm](../decisions/r2-algorithm.md) RA-6）；拉丝、高反光底材上常报边缘不连续。
- `none`：同示教页“笔划直接生成走廊”，笔划按 20 px 插点直接作走廊。走廊只决定 ROI 与测站，测量在搜索带内找胶，笔划贴着胶条中心即可。

最终走廊必须全部位于像素中心范围内、按站距至少 3 站，不静默裁剪吸附后的越界点。提取、几何检查、彩色模型拟合和叠图均在提交前完成；取色失败或 `quality.usable=false` 拒绝整次提交，保留原来的示教帧、走廊、模型与修订号。

成功只推进一次修订，返回 `commit: {frameUpdated: true, corridorSaved: true, colorFitted: <是否取了色>}`、`baseRevision`、新 `revision`、`snapshot`、走廊长度/站数及 `colorModel` 质量摘要。提交后视图刷新失败时，错误明确指出已保存的修订号，应重新调用 `teach_status` 核对，不能把它当作准备阶段失败后盲目重试。

### `teach_fit_color`

参数：`recipe`、`camera`、`segment`，可选 `expectedRevision`。前置：这一段已保存走廊，示教帧是彩色，且没选按灰度测量（操作员在示教页“测量方式”卡片切换）。返回胶色、背景色、质量、`modelPresent`、`modelUsable`、`modelIssues` 与提交身份。这个独立取色入口保持与界面相同的保存行为：引擎成功输出但质量不可用的模型也可保存用于诊断，调用成功不能视为模型可用；`teach_corridor` 的一次提交则要求新模型可用。

### `teach_trial`

参数：`recipe`、`camera`、`segment`；可选 `from`、`to`（实际读取和量测的帧组范围，缺省整段）。与段范围取交集后才读图和量测，无交集拒绝。窄区间仅用于诊断，不改变生产检测范围，也不能替代整段和整件验收。

返回 JSON 文本：

- `range` 是全段边界，`requestedWindow` 是调用参数，`window` 是实际量测范围；`snapshot`、`camera`、`segment`、`checks`、`offsetRef` 记录本次实际使用的配置。`measurementPlan=selectedSegment` 表示此工具量测选定段，不应用整件 guard/skip 区域来缩减范围。
- `states`、`width`（mm：中位、P5、P95、最小、最大）、`edgeMm`、`xMm`、`gapFrames`、`fallbackFrames`、`errors` 与 `worst`（前 5 帧）保留量测分布和看图线索。
- `measurement`：`expectedFrames`、`measuredFrames`、`states`、`plannedSkipFrames`、`unmeasuredFrames` 以及不截断的 `nonMeasuredFrames`（`seq`、`state`、`expected`、`reason`）。Missing、Error、Unmeasured、OutOfCorridor 都不能因为 `errors=[]` 而被忽略；按计划不测单独标记。
- 配置为这台相机测 ToEdge 时，`measurement.edge` 给出实际量测区间的 `expectedFrames`、`validFrames`、`validRate`（无应测帧为 null）及 `missingFrames`。分母包含应测但失败的帧；计划不测不进入分母。有效数值不保证绿线落在真实零件边，仍需 `teach_view` 核对。这里未按整件胶偏规则剔除区域首尾。
- `limits` 给出 `maxGapMm` / `gapOverLimit`、`minPresent` / `presentBelow`、`widthBand` / `widthOutside` / `widthBelow`（平均宽度低于下限）、`wideWarnOnly`，以及独立的 `overflowMaxWidthMm` / `overflowFrames`（按每帧最大胶宽）。每笔首尾跳过的帧生产不判，不进这些列表，另列在 `passSkipped`。`widthOutside` 只比较一帧的平均宽度，是看图线索；生产胶宽判定使用逐站超差累计长度等规则。平均胶宽在公差带内也可能因局部超宽判溢胶；段开了偏宽只预警（`checks.width.wideWarnOnly`，D-SB-61）时，超出上限与溢胶上限只记预警（`flags.wideWarning` / `overflowWarning`），不判 NG。`parameterWarnings` 提示标称或公差带高于溢胶上限、胶偏参考缺失等矛盾，不自动改参数。局部诊断不代替完整生产判定。

### `teach_segment`

参数：`recipe`、`segment`；可选 `checks`、`offsetRef`、`searchMm`、`widthRange`、`headingWindow`、`expectedRevision`。配置补丁合并进同一快照：`checks`、`offsetRef` 对象逐键合并，其余值（含 null）直接替换。`offsetRef: null` 关掉胶偏参考；原来没有参考时补丁要给全（`measure`、`camera`，ToEdge 另加 `edge`）。`searchMm`（搜索半宽，mm）、`widthRange`（胶宽范围 `[下限, 上限]`，mm，上限要小于 2 × 搜索半宽）是本段的检测参数覆盖，本段各检测相机共用；`null` 去掉覆盖、用配方默认。改了示教件测量失效，历史件要原图复测。`headingWindow` 整体替换本段的测量区间覆盖 `{startMm, lengthMm}`（距出胶点只测 [startMm, startMm + lengthMm]，界面“跳过胶嘴附近”= `startMm`、“测量到”= `startMm + lengthMm`），本段各检测相机共用；`null` 去掉覆盖、用相机的测量区间（相机没设时用默认 5 → 20 mm）；本段没开逐帧定向时拒绝。合并后按段配置与候选校验，`xRef` 等由示教件测得的量不能直接改。此工具不开放段结构、检测相机或逐帧定向开关修改。

返回这一段合并后的 `checks`、`offsetRef`、`searchMm`、`widthRange`、`heading`、提交身份和 `impact`（是否结构改变、移除的示教、待示教项、测量是否失效、覆盖预览）。界面的段结构修改仍应先查看影响预览；普通检测项补丁不等于结构修改。

### `teach_replay`

参数：`recipe`；可选 `records`（历史记录编号）。

- 不给 `records`：返回最近 30 件历史记录（`id`、`ts`、`sn`、`recipeId`、`verdict`、`reason`、`mode`）。
- 给了：逐件从原图复测并整件判定，对照记录照常保存。每件返回 `comparisonId`、`cycleId`、实际 `snapshot`、`originalVerdict`、`verdict`、`reason`、`flags`、`alarms`、`uninspected`、`unchecked` 和没有可用走廊等原因造成的 `unmeasuredSegments`；失败项带 `error` 与 `attemptedSnapshot`。
- `regions` 保留 `kind` 和帧组范围；段内为 `{type: "straight" | "corner", segment}`，跳过为 `{type: "skipped", segment}`（每笔首尾跳过另带 `pass`，整笔不检带 `short: true`），未分段为 `{type: "unsegmented", firstSegment, lastSegment, reason}`。每区同时返回判定、宽度、胶偏、`offsetUnjudged`、`incompleteGroups`、帧数比较与相机帧数。直线区 `offsetEvidence` 按最终判定区域和相同的胶偏规则剔除首尾，给出参考相机、有效窗口、应测/有效帧数、缺失帧及未判原因；不与量测全范围的边距有效率混同。
- `measurement.cameras[]` 和其中 `segments[]` 给出各相机/段的应测、实测、全部非 measured 帧及原因、计划不测帧、ToEdge 有效率。`basis=measurementPlan` 明确它复用实际原图复测的分段（按笔或按比例）、guard、每笔首尾跳过和走廊选择；`coverage` 是相同规则的覆盖预览。

这些信息不改变生产 OK/NG 策略：`flags.incomplete`、`flags.passCountAnomaly`、胶宽偏宽与溢胶预警、`offsetUnjudged` 等仍是告警；`uninspected` 里带 `pass` 的是每笔首尾跳过的帧（D-SB-62）。生产结论为 OK 与测量完整是两个独立结论；任一应测帧未测成时，不可宣称示教验收完成。

逐帧诊断当前随响应内联返回，没有分页或独立诊断文件入口。大录制宜按记录分别复测并保留完整 JSON；工具未截断不表示客户端能显示全部内容。

## 实现

- 服务：`src-tauri/src/agent_mcp.rs`（hyper，JSON-RPC 分派、请求准入），随应用启动。
- 工具：`src-tauri/src/workspace/agent.rs`，直接调用示教页命令的同一组函数；叠图绘制在 `src-tauri/src/agent_mcp/canvas.rs`。
- 按胶色吸附用取色图 `src-tauri/graphs/color_teach.lyflow.json`：它另外输出 `n_model.overlay`，并把 `shiftMax` 作为图参数。

## 彩色宽胶测量证据

`teach_trial` 的 `measurementEvidence` 按帧返回版本、精确/截断/无胶/未知站数、可空 `presentRatio` 与可选 `maxWidthLowerBoundMm`。`limits.measurementIncomplete` 列出截断/未知帧，`widthLowerBounds` 列可靠下界，`overflowFrames` 包含下界已超过上限的帧；生产跳过不进入这些判定线索。`presentBelow` 与 `worst` 不把全未知帧当成 0% 有胶。

`measurement` 新增 `completeFrames`、`incompleteMeasurementFrames`、`noEvidenceFrames`；`measuredFrames` 只表示产出测量结果，不能单独证明完整量测。全未知不按断胶处理，但也不是可接受的完整合格。宽度统计只含精确站，下界和未知须另行核对。`widthPolicy=extent` 与 `flow-3` 的兼容和发布要求见[宽胶测量契约](../architecture/wide-bead-measurement-plan.md)。
