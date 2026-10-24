# PLC 通讯页重排与对接参数迁移 · 设计文档

状态：设计已定（结论见第 8 节），已按此实现。

> 2026-10-23 现状（分段连拍，依据 [segment-burst.md](../decisions/segment-burst.md) D-SB-6、D-SB-25 与第八节）：握手协议已升为 6（点表布局同协议 5：S7-1200/1500，PLC 区默认 DB4、PC 区默认 DB2，握手点位仍为 28 个），本文第 1–3 节的线框示例值、阶段“现在在等”文案和计数行已按协议 6 改写。第 4 节的对接参数分两套：段配方（新建配方）用计划导出 `PlanExport`（`WorkspaceView.plan.productionExport / candidateExport`，面板 `PlanExportPanel`，`shotCount` = `deviceShots` = ΣN_exp）；原文的 `PlcParams`、拍照顺序 → Pose 表只服务**旧拍照点模型（保留、停用）**，见第 4 节开头的说明。

## 0 职责与边界

| 页面 | 使用者 | 职责 | 不做什么 |
| --- | --- | --- | --- |
| PLC 通讯页 `/plc` | 现场工程师 | 建站（连接参数、点表）与联调（看握手位、看阶段、手动写自定义点位） | 不复位、不展示配方对接参数、不承载操作员日常操作 |
| 在线检测页 `/inspect` | 操作员 | 看工件、看故障、复位 | 不改 PLC 配置 |
| 配方总览 `/recipe/progress`、配方库 `/recipe` | 工艺 / PLC 工程师 | 查看配方的 PLC 对接参数 | 不读写 PLC |
| 验证与发布页 `/recipe/validation` | 工艺工程师 | 发布配方，并列出这次发布 PLC 要同步的变化 | 不读写 PLC |

数据来源都是现有 hook/API，本设计不新增后端接口；需要前端派生的量在各节标注“派生”。

## 1 PLC 通讯页布局

### 1.1 桌面（1280–1440）线框

页面外层沿用 `PlcSettingsPage`（`ReturnButton` 与离线 `OfflineManagedNotice` 不变），下面是 `PlcPage` 的四个区域，单列、按联调时的查看频率自上而下排列。

```
┌─ A 顶部状态条（sticky，沿用 .plc-actionbar）──────────────────────────────────────────────┐
│ ●已连接   S7 · S7-1200 · 192.168.2.10:102             周期 48 ms · 轮询 12 345 · 错误 0 · 最近 14:02:31 │
│                                                                   [未保存] [还原] [保存配置]  │ [断开] │
│ ⚠ 生产或结果交付尚未结束，PLC 配置和手动写入已锁定                          还有 1 条提示 ▾ │
└──────────────────────────────────────────────────────────────────────────────────────────┘
┌─ B 握手监视 · 只读 ─────────────────────────────────────────────────────────────────────┐
│ 阶段  采集中 (acquiring)          现在在等  PLC：相机使能为高时连续发脉冲……写最终计数并置 partEnd │
│                                                                                            │
│ PLC → PC   partStart ●1   partEnd ○0   resultAck ○0   faultReset ○0   plcHeartbeat ◐ 0.4 s 前翻转  │
│ PC → PLC   visionReady ○0  armed ●1   busy ●1   done ○0   visionFault ○0   pcHeartbeat ◐ 0.2 s 前翻转 │
│ ────────────────────────────────────────────────────────────────────────────────────────── │
│ requestSeq 17   acceptedSeq 17   resultSeq —   ackSeq 0  │ partSn A24-000123   resultSn —           │
│ resultCode —    faultCode —                               │ 脉冲计数（参考） deviceTriggers 38（累计脉冲，参考） · deviceShots 40（预期总帧数 ΣN_exp） │
│ 后端说明：（handshake.message，没有时整行不显示）                                              │
└──────────────────────────────────────────────────────────────────────────────────────────┘
┌─ C 连接配置 ────────────────────────────────────────────────────────────────── [编辑] ───┐
│ S7-1200 · 192.168.2.10:102 · Rack 0 / Slot 1 · OP · TSAP 自动/自动 · PDU 480                    │
│ 轮询 50 ms · 超时 1000 ms · 重连 3000 ms · 心跳 上位机心跳 (DB2.DBX0.5) / 500 ms · 日志保留 30 天 · 启动自动连接 关 │
└──────────────────────────────────────────────────────────────────────────────────────────┘
┌─ D 地址表 ──────────────────────────────────────────────────────────────────────────────┐
│ 握手点位  28 个 · 协议 6 · 🔒 模板生成，不可手动写入            [载入一期 S7 点表] [导入/导出] │
│ ┌──────────┬──────────────┬──────┬────┬────┬────────────────────┬────┬──────────┬────┐  │
│ │ 名称     │ 地址         │ 类型 │读写│边沿│ 标签               │记录│ 当前值   │操作│  │
│ │ 工件开始 │ DB4.DBX0.0   │ Bool │只读│上升│ 工件开始（PLC→PC） │ 是 │ 1        │ ✎  │  │
│ │ …        │              │      │    │    │                    │    │          │    │  │
│ └──────────┴──────────────┴──────┴────┴────┴────────────────────┴────┴──────────┴────┘  │
│ ▸ 自定义点位  3 个                                                              [新增点位] │
│   （默认收起；展开后同列表格，操作列有 写入 / 编辑 / 删除）                                   │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

区域 A 吸顶，B–D 随页滚动。原来的“S7 握手状态”卡、连接卡里的状态徽章与 `status.message`、`conn-footer` 的运行指标、“PLC 对接参数”卡、“复位故障”按钮全部不再出现在本页（迁移/删除清单见第 6 节）。

### 1.2 窄屏（< 900）折叠

- A：三行。第 1 行 状态徽章 + 连接摘要（摘要 `ellipsis`，悬停全文）；第 2 行 运行指标（`stats-inline` 已 `flex-wrap`）；第 3 行 操作按钮靠右换行，[断开]/[连接] 始终在最右。提示条独占一行。
- B：两侧握手位各自改为一列两行的网格（每行 3 个位，`grid-template-columns: repeat(3, minmax(0, 1fr))`），序号/SN/结果码的两栏并为一栏。SN 用 `.sn` 样式允许断行（r2-ui U-7）。
- C：摘要行按 `·` 分隔自然换行；编辑态内部 `ConnectionForm` 已有容器查询（760/500 px 两档）。
- D：表格外层 `.table-wrap` 横向滚动；标题行的按钮换到第二行靠右。不隐藏列，联调时每一列都要看。
- 页面本身不再出现第二个 sticky 元素，避免与 A 叠加遮挡。

## 2 各区域内容、数据来源、交互

### 2.1 A 顶部状态条

| 元素 | 内容 / 文案 | 来源 |
| --- | --- | --- |
| 状态徽章 | `PlcStatusBadge`（未连接 / 连接中 / 已连接 / 错误） | `usePlcStatus().state` |
| 连接摘要 | `S7 · {CPU 预设名或“自定义”} · {host}:{port}` | `saved.connection`（已保存配置，不是草稿）；CPU 预设名复用 `ConnectionForm` 的 `matchesCpu` 逻辑（需导出为 `cpuPresetLabel(connection)`，派生） |
| 运行指标 | `周期 48 ms · 轮询 12 345 · 错误 0 · 最近 14:02:31`；未连接时周期/最近显示 `--` | `status.cycleMs / pollCount / errorCount / lastPoll`，`formatClock` |
| 草稿组 | `[未保存]` 徽章（warn）、`[还原]`、`[保存配置]`；只在 `dirty` 时渲染整组 | `dirty`、`invalid`、`blockedReason` 沿用现有判定 |
| 连接按钮 | 已连接：`[断开]`（tonal + danger）；未连接：`[连接]`（filled）；错误/连接中：`[重连]`；锁定且未连接：`[使用已保存配置重连]` | 现有 `toggleConnection`；与草稿组之间用 `Divider vertical` 隔开，“断开”是本页唯一危险按钮 |
| 提示条 | 只显示一条，规则见 2.1.1 | 见下 |

#### 2.1.1 提示合并规则

同一时刻只渲染一个 `Alert`；其余提示折到 `还有 n 条提示 ▾`（`<details>`，展开后按同一顺序列出，`Alert` 逐条）。优先级从高到低，取第一个非空：

| 序 | 条件 | tone | 文案 |
| --- | --- | --- | --- |
| 1 | `actionError`（操作失败，4 s 自动消失） | ng | 后端原文 |
| 2 | `loadError` | ng | `无法读取 PLC 配置：{原文}` + `[重新加载]` |
| 3 | `status.state === "error"` | ng | `{status.message}`；有 `lastError` 时附 `最近错误：{lastError.message}` |
| 4 | `blockedReason` 非空 | warn | 原文（生产锁 / 生产状态不可用 / 正在核对生产状态） |
| 5 | `handshake.phase` 为 `fault` 或 `resetRequired` | warn | `握手待复位：{handshake.message}。复位在在线检测页进行` + `[前往在线检测]`（`Link to="/inspect"`） |
| 6 | `dirty && invalid` | warn | `{invalid}`（参数校验原文，保存前必须修正） |
| 7 | `readOnly` | info | `离线期间 PLC 由站点模式管理，本页只读`（页面顶部已有 `OfflineManagedNotice`，此条只进“还有 n 条”，不作首条） |
| 8 | 非 Tauri | info | `浏览器预览模式：PLC 连接、保存、写入需在桌面端运行` |

`handshake.message` 本身不再做提示条，只显示在 B 区。`status.message` 在 `connected` 时不显示（信息已在徽章里）。

### 2.2 B 握手监视（只读）

`Card`，标题 `握手监视 · 只读`，右侧 `actions` 放一个 `Chip` 显示协议版本 `协议 6`（来源：点表中 `protocolVersion` 标签点位的当前值；未连接时显示 `协议 —`）。

| 行 | 内容 | 来源 |
| --- | --- | --- |
| 阶段 | `{handshakePhaseLabels[phase]} ({phase})` | `usePlcOperationState().handshake.phase`，标签表沿用现有 `handshakePhaseLabels` |
| 现在在等 | `PLC：…` 或 `PC：…`，文案见 2.2.1 | 由 `phase` 派生；`idle` 且 `message` 以 `S7 空闲：` 开头时改为该 message |
| PLC → PC 位 | 5 个握手位，每个为 `StatusBadge`：值 1 → tone `ok`、显示 `●1`；值 0 → tone `off`、`○0`；读错误 → tone `ng`、`!`；未配置该标签 → tone `neutral`、`未配置`；未连接 → tone `off`、`—` | `saved.points.find(p => p.tags.includes(tag))` → `values[point.id]`（派生 `tagValue(tag)`）。只用已保存点表，草稿里改了地址的点位不算 |
| 心跳 | `plcHeartbeat` / `pcHeartbeat`：显示当前值和 `x.x s 前翻转`；超过 3 s 未翻转 → tone `warn` 并写 `失活 >3 s`（协议 §1 失活阈值） | `values[id].ts` 为该点最后一次更新时间；若实现核对后发现 `ts` 是最后一次轮询而非最后一次变化，改为前端记录值翻转时刻（派生） |
| PC → PLC 位 | 6 个握手位，同上 | 同上 |
| 序号 | `requestSeq / acceptedSeq / resultSeq / ackSeq`，等宽字体；`requestSeq`、`resultSeq` 优先用 `handshake.requestSeq / resultSeq`，其余读点位 | `handshake` + `tagValue` |
| SN | `partSn / resultSn`，`.sn` 样式 | `tagValue("partSn")`、`tagValue("resultSn")` |
| 结果 | `resultCode / faultCode`；`done=1` 时后面加 `Chip` 显示码名（如 `1 OK`、`90 ERR_INSPECT · 94 预期总数与计划不符（旧配方：拍照点数不符）`，表来自协议 §6，前端常量 `src/features/plc/handshake.ts`；91 / 94 / 96 先写协议 6 段配方的含义，括号里保留旧拍照点配方的含义） | `tagValue` |
| 脉冲计数（参考） | `deviceTriggers {n}（累计脉冲，参考） · deviceShots {m}（预期总帧数 ΣN_exp）`；协议 6 下 `deviceTriggers` 只作参考，不要求等于 `deviceShots` | `tagValue` |
| 后端说明 | `handshake.message`，`Text variant="label" muted`；为空不渲染 | `handshake.message` |

B 区在 `handshake === null`（没有 S7 会话，例如预览模式）时整卡显示 `EmptyState title="没有 S7 握手会话" description="连接 PLC 并载入一期 S7 点表后，这里显示双方握手位和当前阶段"`。

#### 2.2.1 handshake.phase → “现在在等谁”

按协议 §5 阶段表与 §8 复位流程推导：

| phase | 中文 | 等待方 | 文案（“现在在等”） |
| --- | --- | --- | --- |
| `resetRequired` | 待复位 | PLC（操作员） | `PLC：清 partStart/partEnd/resultAck，faultReset 低 ≥1 s 后置高；高 ≥500 ms 且看到 visionFault=1 后拉低，再等 visionFault=0；也可在在线检测页点“复位故障”` |
| `idle` | 空闲 | PLC | `PLC：写 requestSeq、partSn、productCode、shotCount 与 deviceShots（预期总帧数 ΣN_exp）、planVersion，最后置 partStart`；若 `message` 以 `S7 空闲：` 开头，直接显示该 message（PLC 仍持有 end/ACK，或 PC 设备未就绪撤了 ready） |
| `validating` | 校验请求 | PC | `PC：核对请求、撤 visionReady、写 acceptedSeq，置 busy，最后置 armed（PLC 此时禁止触发）` |
| `acquiring` | 采集中 | PLC | `PLC：相机使能为高时连续发脉冲（deviceTriggers 只作参考，不要求等于预期总帧数），运动结束、脉冲停发后写最终计数并置 partEnd` |
| `draining` | 等待帧处理结束 | PC | `PC：收尾收帧、等待测量完成，写结果码/异常码/resultSn/resultSeq，最后置 done` |
| `awaitAck` | 等待结果确认 | PLC | `PLC：核对 resultSeq=requestSeq、resultSn=partSn 与结果码，写 ackSeq，再置 resultAck` |
| `releasing` | 等待握手释放 | PLC → PC | `PLC：看到 done=busy=0 后清 partStart/partEnd/resultAck；PLC 清完后 PC 落盘并置 visionReady` |
| `fault` | 故障 | PLC（操作员） | `PLC：停止触发并处置实物；PC 不会自行恢复，需 faultReset 上升沿或在线检测页复位。原因：{message}` |

### 2.3 C 连接配置

- 默认摘要态：`Card` 标题 `连接配置`，`actions` 为 `[编辑]`（`Button variant="tonal" leadingIcon=<Pencil>`）。正文一行摘要（2.1 线框第二块），全部来自 `draft`（草稿）；草稿与已保存不同的字段加下划线并在悬停显示 `已保存：…`（派生 diff，比较 `saved` 与 `draft` 对应字段）。
- 编辑态：点 `[编辑]` 后卡内展开现有 `ConnectionForm` + `心跳与日志`（心跳点位 `Select`、心跳周期 `NumberField`、日志保留天数 `NumberField`）+ `启动时自动连接 Checkbox`，`actions` 变为 `[收起]`。保存/还原不在卡内，统一在 A 区草稿组（保存后草稿组消失，卡保持展开直到用户点收起）。
- 编辑态以 `useState` 保存，`dirty` 变为 true 且用户没展开过时自动展开（例如“载入一期 S7 点表”改了 CPU 预设时让用户看到）。
- `readOnly`、`blockedReason` 非空、预览模式：`[编辑]` 禁用，`title` 分别为 `离线期间只读` / `{blockedReason}` / `预览模式不能修改`。
- 心跳点位下拉只列 `access === "readWrite"` 的点位（现状），载入模板后默认指向 `pcHeartbeat` 点位（模板已带）。

### 2.4 D 地址表

一张 `Card`，标题 `地址表`，内部两个分组。

**握手点位组**（始终展开）

- 组标题行：`握手点位 · {n} 个 · 协议 {v}`，`Chip` 🔒 `模板生成`；右侧按钮 `[载入一期 S7 点表]`、`[导入/导出]`。
- 行范围：`point.tags.some(t => inspectionHandshakeTags.has(t))` 的点位（派生 `isHandshakePoint`）。
- 列：名称（含心跳 `Chip`）、地址、类型、读写、边沿、标签（中文 `tagPresets` 标签，协议方向已在标签名里）、记录、当前值、操作。
- 操作列只有 `✎ 编辑`：打开 `PointEditor`，但握手点位在编辑器内锁定 名称 / 地址 / 类型 / 读写 / 边沿 / 标签（禁用并提示 `握手点位由模板定义，改地址请重新载入一期 S7 点表`），只允许改 描述 和 记录变化（第 8 节结论 3）。无 写入、无 删除。
- 当前值列沿用现状逻辑（待保存 / 未连接 / 值 / 读错误标红）。
- 空态（还没载入模板）：`EmptyState title="尚未载入握手点表" description="点「载入一期 S7 点表」按 CPU 型号生成 28 个握手点位" action=[载入一期 S7 点表]`。

**自定义点位组**（默认收起）

- `Accordion`（`collapsible`，单项 `value="custom"`），标题 `自定义点位 · {n} 个`，标题右侧 `[新增点位]`（点击不触发折叠切换，`stopPropagation`）。有未保存的新增/改动自定义点位时标题加 `[未保存]` `StatusBadge`，并自动展开。
- 行范围：非握手点位。
- 列同上；操作列 `写入`（`WriteDialog`，仅 `readWrite` 且已连接且已保存）、`编辑`、`删除`。心跳点位可以是自定义点位（不走 `protectedWrite` 的 tag 分支，但 `heartbeat.pointId` 仍受保护，现状不变）。
- 空态：`EmptyState title="没有自定义点位" description="联调需要监视或手动写入的额外地址在这里添加；握手点位不在此列"`.

**按钮归属**

| 按钮 | 位置 | 禁用条件（title 给原因） |
| --- | --- | --- |
| 载入一期 S7 点表 | 握手组标题行 | `readOnly` / `blockedReason` / `busy` / 非 Tauri（`一期 S7 点表需要桌面后端生成`） |
| 导入/导出 | 握手组标题行（导入导出的是整张表，含两组） | `readOnly` / `blockedReason` / `busy`（导出在预览模式也可用） |
| 新增点位 | 自定义组标题行 | 同上 |
| 还原 / 保存配置 | A 区草稿组 | 现状 |
| 断开 / 连接 | A 区最右 | 现状 |

地址示例文案（`S7 地址示例：DB1.DBX0.0 …`）移到 `PointEditor` 内地址字段的 `hint`，不再占表头空间。

## 3 状态矩阵

“操作”列中 ✓ 可用、✗ 禁用（按钮保留并 `title` 给原因）、— 不渲染。

| 状态 | 判定 | A 徽章/按钮 | 首条提示 | B 握手监视 | C 连接配置 | D 握手组 | D 自定义组 | 写入 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 未连接 | `state=disconnected`，`blockedReason=""` | 未连接；`[连接]`（草稿无效时 ✗）；dirty 时草稿组 | 无（dirty+invalid 时第 6 条） | 阶段按 `handshake` 显示（通常 `resetRequired`），所有位 `—`，心跳 `—` | 编辑 ✓ | 当前值 `未连接`；载入/导入 ✓ | 新增/编辑/删除 ✓ | ✗ `PLC 未连接` |
| 连接中 | `state=connecting` | 连接中（warn）；`[重连]` | 第 3 条不触发；无 | 同未连接 | 编辑 ✓（保存后需重连才生效，第 8 节结论 2） | 同未连接 | 同未连接 | ✗ |
| 连接错误 | `state=error` | 错误（ng）；`[重连]` | 第 3 条：`status.message` + `lastError` | 同未连接 | 编辑 ✓ | 同未连接 | 同未连接 | ✗ |
| 已连接空闲 | `connected` 且 `phase=idle` | 已连接；`[断开]`；运行指标实时 | 无；`message` 以 `S7 空闲：` 开头时不作提示，只在 B 区 | 位实时；“现在在等 PLC 置 partStart”；心跳计时 | 编辑 ✓ | 当前值实时；载入/导入 ✓（载入后变 dirty，保存会触发后端重新校验点表并进入待复位，提示条第 5 条随之出现） | ✓ | ✓（仅自定义 readWrite、已保存） |
| 事务进行中 | `connected` 且 phase ∈ validating/acquiring/draining/awaitAck/releasing；后端 `plcLocked`（`host.busy()`）为 true 或 cycle phase 非 IDLE/FAULT → `blockedReason` 非空 | 已连接；`[断开]` ✗（`blockedReason`） | 第 4 条生产锁 | 位实时；“现在在等”按 2.2.1 | 编辑 ✗ | 载入/导入 ✗ | 新增/编辑/删除 ✗ | ✗ `{blockedReason}` |
| 故障 / 待复位 | phase ∈ fault/resetRequired，且 `blockedReason=""`（后端仍报 `plcLocked` 时按“生产锁定”行处理） | 已连接；`[断开]` ✓ | 第 5 条：`握手待复位：{message}` + `[前往在线检测]` | 阶段标红（`fault` 用 `StatusBadge tone="ng"`）；“现在在等 PLC 复位”；位实时（可看到 visionFault=1） | 编辑 ✓ | ✓ | ✓ | ✓（自定义点位） |
| 生产锁定 | `blockedReason` 非空（含“正在核对生产状态”和“生产状态不可用”） | 按连接态；未连接时 `[使用已保存配置重连]` ✓，其余 ✗ | 第 4 条 | 正常显示 | 编辑 ✗ | 载入/导入 ✗ | 新增/编辑/删除 ✗ | ✗ |
| 离线只读 | `readOnly=true`（站点离线） | 按连接态；所有按钮 ✗（整页 `fieldset disabled`，现状） | 页顶 `OfflineManagedNotice`（现状）；第 7 条只进“还有 n 条” | 正常显示（离线虚拟 PLC 的握手同样可看） | 编辑 ✗ | 载入/导入 ✗ | 全部 ✗ | ✗ |
| 浏览器预览 | `!isTauri()` | 未连接；`[连接]` ✗（`PLC 操作需要桌面后端`）；指标 `--` | 第 8 条 | `handshake=null` → `EmptyState` | 编辑 ✓（可改草稿，保存 ✗） | 空态；载入 ✗；导入 ✓ 导出 ✓ | 新增/编辑/删除 ✓（只在内存） | ✗ |

B 区在全部状态下都是只读，没有任何按钮。

## 4 PLC 对接参数：配方总览完整面板 + 配方库摘要 + 发布页变化清单

> **段配方（现行，2026-10-23）**：三处分工不变，数据与内容换成计划导出（D-SB-25；`src/features/workspace/PlcParams.tsx`）。
>
> - 配方总览 `/recipe/progress`：段配方挂载 `PlanProgress`，其中 `PlanExportPanel` 读 `WorkspaceView.plan.productionExport`（未发布时读 `candidateExport`，标“尚未发布，以下为候选计划”）。开始请求参数表为 productCode、planVersion（未分配时写“发布时分配”）、shotCount 与 deviceShots（都填 ΣN_exp，未标定写“待标定”）；下方是“段序列与预期帧数”表（段、类型、长度、N_exp 或“待标定”、容差、检测区间）和 ΣN_exp ± Σtol 说明；提供“复制 JSON”“另存 JSON”。没有拍照顺序 → Pose 表。已发布且 `plan.shotCountChanged` 时标题右侧显示“候选发布后将变化”。
> - 配方库卡片：段配方写 `PLC：产品 {code} · 计划 {planVersion} · shotCount {ΣN_exp}`（未标定写“shotCount 待标定”）。
> - 发布确认框：段配方用 `PlanPublishBody`，先列候选与生产的对比（ΣN_exp ± Σtol、段序列、共用触发的相机），计划身份变化时在“计划已变，生效后 PLC 要同步”里列 planVersion、shotCount / deviceShots（ΣN_exp）、productCode、段序列、相机顺序中变了的项（`planSyncItems`），并写明不同步时 PC 返回 100 / 94 / 95。
>
> 下文 4.1–4.4 的 `PlcParams`（`shotCount` = 触发次数）、`productionPlc / candidatePlc`、拍照顺序 → Pose 表与“三目设备的三个视角不分别计数”只服务**旧拍照点模型（保留、停用）**：`segments` 为空的旧配方仍由 `ProgressPage` 的 `PlcParamsPanel` 与 `plcSyncItems` 按此显示，段配方这些字段为 null。

对接参数是配方的属性（已发布配方就有一组 productCode / planVersion / shotCount），所以放在配方内部；“验证与发布”只关心这次发布让 PLC 要改什么。三处分工：

| 位置 | 内容 |
| --- | --- |
| 配方总览 `/recipe/progress` | 完整面板「PLC 对接参数」（锚点 `#plc-params`）：当前生产版本的参数表 + 拍照顺序→Pose 表 |
| 配方库 `/recipe` 卡片 | 已发布配方加一行摘要，PLC 工程师不进工作台即可扫完全部产品 |
| 验证与发布 `/recipe/validation` | 不放完整表；右栏 KV「PLC 计划版本」保留；发布确认框直接列出 PLC 要同步的变化 |

不加“复制对接参数”按钮。

### 4.1 数据来源（后端统一编计划，前端不推算）

`WorkspaceView` 新增两项，均由后端用与接件核对相同的触发分组（`plc_plan::trigger_groups`）编出：

| 字段 | 含义 |
| --- | --- |
| `productionPlc: PlcParams \| null` | 生产配方的参数；未发布为 null |
| `candidatePlc: PlcParams \| null` + `candidatePlcError: string \| null` | 候选会编出的参数（不要求已分配 planVersion）；编不出时给原因 |

`PlcParams = { productCode, planVersion, shotCount, shots: [{ shotId, poseId }] }`，`shotCount` 即触发次数（deviceShots 同值）；候选的 `planVersion` 为候选文件当前值，计划变化时以“发布时分配新号”显示，不展示该值。

配方库用 `cycle_recipes`（`RecipeSummary`）新增的 `planVersion`、`triggerCount`（编不出时为 null）。

### 4.2 配方总览面板

```
┌─ PLC 对接参数 · 当前生产 v3 ─────────────────────── [候选发布后将变化 →] ─┐
│ PLC 开始请求要填的值；PC 接件时逐项核对，不一致则拒绝布防。                    │
│ ┌──────────────┬──────┬──────────────────────────────────────────────────┐ │
│ │ PLC 标签     │ 值   │ 说明                                             │ │
│ │ productCode  │ 7    │ 产品代码；不一致返回 95                          │ │
│ │ planVersion  │ 41   │ 计划版本，拍照顺序或 Pose 变化后会换新号；不一致返回 100 │ │
│ │ shotCount    │ 4    │ 触发次数，deviceShots 填同一值；不一致返回 94    │ │
│ └──────────────┴──────┴──────────────────────────────────────────────────┘ │
│ 拍照顺序与 Pose：顺序 / 拍照点 / Pose                                        │
│ 机器人按上表顺序经过各拍照点，每个拍照点发一次触发；同一 Pose 可被多个拍照点引用， │
│ 三目设备的三个视角不分别计数。                                               │
└────────────────────────────────────────────────────────────────────────────┘
```

- `planChanged && productionPlc` 时标题右侧 `Badge tone="warn"`「候选发布后将变化」，链接到验证与发布页。
- 未发布：面板正文 `尚未发布：发布生效后，这里显示 PLC 要填的参数`。
- 离线站点：沿用原提示“离线发布版本不代表真机可用”。

### 4.3 配方库卡片

已发布配方的卡片在徽章行下加一行 `产品 7 · 计划 41 · 4 次触发`（`Text variant="label" muted`，数字等宽）；`triggerCount` 为 null 时写 `计划 41 · 触发数无法计算`。未发布不显示。

### 4.4 发布确认框

`planChanged=true` 时 `Notice`（warn）改为直接列出变化，去掉“查看 PLC 对接参数”链接：

```
拍照计划已变，生效后 PLC 要同步
  • planVersion：41 → 发布时分配新号（生效后在配方总览“PLC 对接参数”查看）
  • shotCount / deviceShots：4 → 5
  • productCode：7（不变）
PLC 未同步前 PC 拒绝布防：planVersion 不一致返回 100，shotCount 不一致返回 94，productCode 不一致返回 95。
```

首次发布（无 `productionPlc`）列出候选值本身；`candidatePlcError` 非空时显示该原因。`planChanged=false` 保持现文案。

### 4.5 其他链接

- `OperationGuidePage` 的「核对 PLC 计划」改为「核对 PLC 对接参数」，指向 `/recipe`（配方库摘要）。
- `/plc?panel=s7-plan` 不再有对应面板；`S7RecipePlanPanel`、前端 `plcApi.recipePlan / recipeChoices` 与后端 `plc_recipe_plan` 命令删除。

## 5 在线检测页：复位按钮旁显示不能复位的原因

`InspectPage` 顶栏在 `phase === "FAULT"` 时已有 `ContextHelp` 与 `[复位故障]`。改动：从 `usePlcOperationState().handshake.resetBlockedReason`（真机）取原因；`resetBlockedReason` 为非空字符串时按钮禁用，按钮右侧用 `Text variant="label" muted` 显示 `不能复位：{原因}`（后端原文已是完整句，如“尚未观察到 PLC 心跳翻转，不能复位”“设备尚未就绪，不能复位并置视觉就绪：…”）；为 `null` 时按钮可用，不显示文字；为 `undefined`（状态未知）时按钮可用、文字 `复位条件核对中`。离线模式沿用现状（由离线产线自动复位，不显示原因）。

`ContextHelp` 文案改为：`真机：由 PLC 发 faultReset 复位；PLC 已连接、心跳在翻转、设备与追溯就绪且无在途工件时也可点此复位。不满足时右侧显示原因`。`resetError`（提交失败）的 `Alert` 保留。PLC 页原有的 `resetPending / resetSubmitted / resetError / resetStartMessage` 状态机不迁移到在线检测页——在线检测页已有 `action === "reset"` 的 loading 处理，够用。

## 6 删除 / 迁移清单与组件映射

### 6.1 PLC 页删除

| 元素 | 处理 |
| --- | --- |
| 悬浮操作栏里的 `[复位故障]`、`当前不能复位：…`、`复位请求已提交…`、`resetError Alert` 及 `resetFault/resetPending/resetSubmitted/resetStartMessage/resetting` | 删除；`cycleApi` import 随之去掉 |
| `S7 握手状态` Card | 由 B 区替代 |
| 连接卡标题里的 `PlcStatusBadge` + `status.message` | 并入 A 区 |
| `conn-footer` 里的 `stats-inline` 运行指标 | 并入 A 区；`.conn-footer` 样式可删 |
| 独立的 `{blockedReason && <Alert>}` | 并入 2.1.1 提示合并 |
| 悬浮栏里的 `[载入一期 S7 点表] [导入/导出] [新增点位]` | 移入 D 区各组标题行 |
| `planActionsTarget` 与 `S7RecipePlanPanel` 挂载、`panel=s7-plan` hash 滚动 | 删除；组件迁移见 6.2 |
| 地址表表头的 `S7 地址示例` 文案 | 移到 `PointEditor` 地址字段 hint |
| 地址表删除/写入对握手点位的入口 | 删除（握手点位只剩编辑，且编辑器内锁定关键字段） |

### 6.2 迁移

| 元素 | 从 | 到 |
| --- | --- | --- |
| `S7RecipePlanPanel` | `src/features/plc/components/` | 改写为配方总览的 `PlcParamsPanel`（`src/features/workspace/`），数据改用 `WorkspaceView.productionPlc`；原组件与 `plc_recipe_plan` 删除 |
| 发布确认框「查看 PLC 对接参数」链接 | `ValidationPage` 对话框 | 改为 4.3 的同步清单 |
| 「核对 PLC 计划」链接 | `OperationGuidePage` | `/recipe`（配方库摘要） |
| 协议文档 §4 第一段 “PLC 通讯页的‘PLC 对接参数’卡” | `docs/integration/plc-communication-protocol.md` | 改为 “配方总览的‘PLC 对接参数’面板” |
| 复位阻塞原因展示 | PLC 页 | 在线检测页（第 5 节） |

### 6.3 qing-steel 组件映射（均为仓库已在用）

| 区域 / 元素 | 组件 |
| --- | --- |
| A 容器 | 现有 `.plc-actionbar`（sticky），内部 `Stack` |
| 状态徽章、握手位、未保存 | `StatusBadge`（tone：ok / off / ng / warn / neutral） |
| 协议版本、心跳标记、模板锁、结果码名 | `Chip` |
| 提示条 | `Alert`（tone ng / warn / info，`action` 放 `[前往在线检测]`、`[重新加载]`）；“还有 n 条” 用原生 `<details>`（工作台 `wp-check-passed` 已用此模式） |
| 断开与草稿组分隔 | `Divider vertical` |
| B / C / D 卡 | `Card`（`title` 用 `Heading level={3}`，`actions` 放按钮） |
| B 文本 | `Text`（`mono`、`.sn`、`variant="label" muted`） |
| B 空态、D 空态 | `EmptyState` |
| C 摘要态 → 编辑态 | `Button variant="tonal"` 切换 + 现有 `ConnectionForm`、`Select`、`NumberField`、`Checkbox` |
| D 自定义点位折叠 | `Accordion`（`collapsible`，`ignoreFieldsetDisabled` 让只读态仍可展开查看） |
| D 表格 | `Table` + `TableColumn`，操作列 `IconButton variant="text"` |
| 对话框 | 现有 `PointEditor / WriteDialog / PointsJsonDialog / S7Phase1Dialog`（`Modal`） |
| 验证页对接参数 | 工作台封装 `Panel / KV / Notice / Badge` + `Table`，`Link` 锚点 |
| 在线检测页原因文字 | `Text variant="label" muted`，`ContextHelp`（cycle 组件） |

库里有 `Tabs`，但仓库未使用，本设计不引入（两组点位用分组 + `Accordion` 已够）。不新增依赖、不新增全局 CSS 变量；新增样式限于 `plc.css`（握手位网格、摘要行 diff 下划线）。

## 7 实现提示（不改变设计）

- `tagValue(tag)` 建议在 `PlcPage` 内用 `useMemo` 从 `saved.points` 建 `Map<tag, pointId>`，避免每次渲染遍历 28 个点位 × 11 个位。
- A 区 sticky 高度会随提示条出现变化；D 区表格很长，不需要像原 `scrollToPlan` 那样计算 inset。
- B 区心跳“x s 前翻转”每 500 ms 刷新一次即可（`setInterval`），不要跟随每次 `plc://values` 事件重渲染整卡。
- 验证页读取生产计划只在 `doc.id`、`data.productionVersion` 变化时触发，和右栏 KV 共用同一份 `planVersion` 判定，避免两处不一致。

## 8 已定结论（2026-10-20）

1. **保存 / 还原**放在 A 区吸顶状态条，`dirty` 时出现。
2. **已连接时改连接参数**：本轮不改后端保存行为；C 区编辑态在已连接时显示 `已连接时保存，需断开重连后新参数才生效`。自动重连另行评估。
3. **握手点位可改字段**：只允许改“描述”和“记录变化”，其余锁定。
4. **对接参数位置**：配方总览完整面板 + 配方库卡片一行摘要 + 发布确认框只列变化（见第 4 节）。
5. **不加**“复制对接参数”按钮。
