# Agent 示教接口（MCP）

GlueSight 内嵌一个 MCP 服务，让 Agent（Claude Code 等）直接完成段配方的示教：看示教件的帧、画走廊、取色、试测、配检测项与零件边参考、用历史件整件复测。Agent 走的是示教页背后同一套后端函数，校验、留痕与界面一致；示教流程与验收口径写在项目 Skill `.claude/skills/glue-teach/SKILL.md`。

## 范围与边界

- 只改**候选**（草稿工作区）：走廊、颜色模型、各段的检测项与零件边参考。发布、生效、切换生产配方、PLC、相机参数、项目切换都不提供工具，仍由人在界面上操作。
- 示教件、分段确认、像素当量在界面上由人确认，工具只读出现状。检测项开关、公差与 ToEdge 的图纸标称值可以经 `teach_segment` 写入，但取值要由人给出，Agent 不自行决定。
- 工具与界面共用候选的修订号。每次写入后发 `workspace://changed`，界面里打开着同一候选且没有未保存改动时随即重新载入；界面里有未保存改动时，它的下一次保存会因修订号过期失败。示教期间不要在界面编辑同一候选。
- 测量类工具（试测、带测量叠图的看图、带 `records` 的整件复测）与界面试测同一规则：有在途件或正在采集时拒绝。

## 启用与连接

服务默认关闭。启动 GlueSight 前设置环境变量：

| 变量 | 含义 |
|---|---|
| `GLUESIGHT_MCP_TOKEN` | 访问令牌，至少 16 个字符；不设或太短则不启动服务 |
| `GLUESIGHT_MCP_PORT` | 端口，缺省 47821 |

服务只监听 `127.0.0.1`，地址为 `http://127.0.0.1:<端口>/mcp`。启动结果写应用日志（标题“示教 MCP”）；设置页“关于”里的“示教 MCP”一项只读显示服务状态：未启用、监听地址或启动失败的原因。

Claude Code 里注册（令牌取同一个环境变量）：

```bash
claude mcp add --transport http gluesight http://127.0.0.1:47821/mcp --header "Authorization: Bearer $GLUESIGHT_MCP_TOKEN"
```

## 传输

MCP Streamable HTTP 的无状态子集：

- 只接受 `POST /mcp`，请求体为单个 JSON-RPC 2.0 消息，响应 `application/json`；不开 SSE 流，`GET` 返回 405，不发 `Mcp-Session-Id`。
- 必须带 `Authorization: Bearer <令牌>`，否则 401；带 `Origin` 头的请求（浏览器发出）一律 403，防止网页借本机地址调用。
- 支持的方法：`initialize`、`ping`、`tools/list`、`tools/call`；通知（无 `id`）返回 202。
- 工具执行失败返回 `isError: true` 的结果，文本为业务原因（与界面提示同一措辞）；参数或协议错误返回 JSON-RPC 错误。

## 工具

所有工具都以 `recipe`（候选的配方编号，如 `NEW-1`）指定候选，作用于当前项目；帧以示教件轮次里的帧组号 `seq` 指定，坐标一律是原图像素。

| 工具 | 读 / 写 | 用途 |
|---|---|---|
| `teach_status` | 读 | 不给 `recipe` 时列出当前项目的候选；给了时返回示教现状：下一步、示教件、各段的帧组范围与检测相机、各相机各段的走廊 / 颜色模型 / 示教帧、发布前未通过的检查 |
| `teach_view` | 读 | 示教件一帧的 PNG：叠加原图坐标网格；给 `segment` 时按已保存走廊测这一帧，叠加走廊、胶两侧边与胶边距线，并附这一帧的指标 |
| `teach_corridor` | 写 | 按 Agent 给的笔划（原图像素点列）在第 `seq` 帧上示教这台相机这一段的走廊：冻结示教帧 → 由笔划得出走廊（彩色帧按胶色吸附）→ 保存 → 彩色帧沿新走廊重新取色。返回走廊点数、长度、取色结果与叠图 |
| `teach_fit_color` | 写 | 沿已保存的走廊取色，写入这一段的颜色模型（只对彩色示教帧）；返回胶色、背景色与分离度 |
| `teach_trial` | 读 | 用已保存的走廊试测示教件里这一段的全部帧，返回状态计数、胶宽 / 胶边距 / 偏移分布、断胶与缺胶帧、最差的几帧 |
| `teach_segment` | 写 | 按补丁改一段的检测项（`checks`）与零件边参考（`offsetRef`） |
| `teach_replay` | 读 | 按候选现在的配置从原图复测历史件并整件判定；不给记录时列出最近的历史件 |

### `teach_status`

参数：`recipe?: string`。

返回 JSON 文本。给了 `recipe` 时字段为：`recipe`、`revision`、`next`（进度的下一步）、`teachingPiece`（`roundId`、`recordId`、`groupCount`；没选为 null）、`segments[]`（`index`、`kind`、`cameras` 检测相机、`frames` 示教件里的帧组范围或不能确定的原因、`checks`、`offsetRef`、`heading` 逐帧定向参数，null 为不定向）、`cameras[]`（`camera`、`color`、`mmPerPx`、`calibMmPerPx`、`ready`、`nozzle` 出胶点（`point`、`calibrated`），`segments[]`：`segment`、`seq`、`corridorPoints`、`saved`、`colorModel`）、`blockers[]`（未通过的发布前检查）。

### `teach_view`

参数：`recipe`、`camera`、`seq`；可选 `segment`（按这一段的已保存走廊测量并叠图）、`crop: [x, y, w, h]`（原图像素）、`maxSide`（输出长边上限，缺省 1280）、`grid`（网格间距 px，缺省按视野自动取 50/100/200）。

返回 PNG 与说明文本（原图尺寸、裁剪框、缩放、这一帧所在段、测量指标或不能测量的原因）。网格线标原图坐标，Agent 据此读出笔划点。

### `teach_corridor`

参数：`recipe`、`camera`、`segment`、`seq`（须在这一段的帧组范围内）、`stroke: [[x, y], ...]`（至少 2 点，原图像素，从起胶端排到收胶端）、`radiusPx`（缺省 30）、`extract`（`auto` / `color` / `edges` / `none`，缺省 `auto`）。

- `color`（彩色帧的 `auto`）：按胶色吸附。笔划插点后作示教线跑 `glue.color_model`（取色图 `color_teach`，`shiftMax` = `radiusPx`），每站在卡尺上找胶色段重新居中、五站中值；取它输出的胶中心线（overlay 的 `center`，站距 4 px）平滑（±20 px 滑动平均）后，以偏移上限 10 px 再居中一遍、再平滑，每 5 点留 1 点；两端沿切线补到笔划首尾在切线上的投影。吸附只校正横向位置，走廊沿胶路的起止就是笔划的首尾。彩色段的颜色模型沿走廊取样，走廊应覆盖这一段整条胶（从出胶处起）；出胶点未标定时，开启逐帧定向的段还由走廊起点推出出胶点。笔划离胶中心不超过 `radiusPx` 即可。
- `edges`（灰度帧的 `auto`）：同示教页刷选提取，提取范围为笔划外接框外扩 `radiusPx` 加 40 px。提取靠灰度上的胶两侧边，拉丝、高反光底材上常报边缘不连续。
- `none`：同示教页“笔划直接生成走廊”，笔划按 20 px 插点直接作走廊。走廊只决定 ROI 与测站，测量在搜索带内找胶，笔划贴着胶条中心即可。

走廊按站距不足 3 站时拒绝保存（与示教页同一口径）。保存后，彩色示教帧随即沿新走廊重新取色（同 `teach_fit_color`），结果在返回的 `colorModel` 里；取色失败只记在这一项，走廊照样保存。示教帧在提取前就已换成第 `seq` 帧，提取失败时这一段的走廊仍是原来的。

### `teach_fit_color`

参数：`recipe`、`camera`、`segment`。前置：这一段已保存走廊，示教帧是彩色。

### `teach_trial`

参数：`recipe`、`camera`、`segment`；可选 `from`、`to`（统计用的帧组范围，缺省整段，用来排除起胶 / 收胶的过渡帧）。

返回 JSON 文本：`range`、`states`（各状态帧数）、`width`（mm：中位、P5、P95、最小、最大）、`edgeMm`、`xMm`、`gapFrames`（帧内有断口的帧与最长断口 mm）、`fallbackFrames`（逐帧定向没找到胶条的帧与原因）、`errors`、`worst`（按断口长度、有胶站占比排序的前 5 帧），以及 `limits`：按这一段启用的检测项逐帧比对的结果（`gapOverLimit` 帧内断口长于 `maxGapMm`（与生产判定同口径）、`presentBelow` 有胶站占比低于 `minPresent`、`widthOutside` 平均胶宽出了公差带 `widthBand`）。整件判定还看跨帧的连续长度，`limits` 只是逐帧线索。

### `teach_segment`

参数：`recipe`、`segment`；可选 `checks`、`offsetRef`。两者都是补丁，深合并进这一段现有的配置：对象逐键合并，其余值（含 null）直接替换。`offsetRef: null` 关掉这一段的胶偏参考；这一段原来没有 `offsetRef` 时补丁要给全（`measure`、`camera`，ToEdge 另加 `edge`）。合并后按段配置的格式与候选校验（同界面改段），`xRef` 等由示教件测得的量不能直接改。

返回这一段合并后的 `checks` 与 `offsetRef`。

### `teach_replay`

参数：`recipe`；可选 `records`（历史记录编号）。

- 不给 `records`：返回最近 30 件历史记录（`id`、`ts`、`sn`、`recipeId`、`verdict`、`reason`、`mode`）。
- 给了：逐件用候选现在的配置从原图复测并整件判定（同界面“对照”的复测，对照记录照常保存），返回每件的 `originalVerdict`、`verdict`、`reason` 与各区域（`kind` 区域类型原样：段内为 `{type: "straight" | "corner", segment}`，段首尾跳过的帧为 `{type: "skipped", segment}`，未分段区域为 `{type: "unsegmented", firstSegment, lastSegment, reason}`；帧组范围、`verdict`、`reason`、`width`、`offset`）。单件失败只记在这一件的 `error`。

## 实现

- 服务：`src-tauri/src/agent_mcp.rs`（hyper，JSON-RPC 分派、鉴权），随应用启动。
- 工具：`src-tauri/src/workspace/agent.rs`，直接调用示教页命令的同一组函数；叠图绘制在 `src-tauri/src/agent_mcp/canvas.rs`。
- 按胶色吸附用取色图 `src-tauri/graphs/color_teach.lyflow.json`：它另外输出 `n_model.overlay`，并把 `shiftMax` 作为图参数。
