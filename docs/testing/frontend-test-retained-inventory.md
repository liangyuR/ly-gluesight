# 前端保留测试清单

更新时间：2026-10-23。按实际业务保护价值将前端默认回归从 **332 项、50 个文件收缩至 103 项、32 个文件**。数量按 Vitest 实际执行和参数化展开计；默认静态列表会遗漏展开并可能误识别同名局部调用，不作为执行数量。

本轮只修改测试、夹具与文档，产品运行逻辑及 Vitest 收集配置未改。删除旧拍照点专属、只核对默认值/文案/mock 调用和重复覆盖的用例；未新增 skip、todo 或排除配置。

## 数量与验证

| 范围 | 精简前执行用例 | 保留执行用例 | 保留文件 |
| --- | ---: | ---: | ---: |
| 配方、采集、示教与发布 | 150 | 45 | 12 |
| 生产周期、就绪与历史 | 113 | 32 | 11 |
| 相机、标定、PLC与配置 | 69 | 26 | 9 |
| 合计 | 332 | 103 | 32 |

```powershell
pnpm test
pnpm typecheck
```

2026-10-23 实跑：32 个文件、103 项通过，0 失败、0 跳过，耗时 41.48 秒；应用与测试 TypeScript 类型检查通过。JSON 执行结果与下表逐项匹配，无遗漏或重复。

测试通过真实页面、Provider 或公开业务函数保护规则，Tauri/设备/文件接口在边界模拟；不据此证明真实相机、PLC、lyFlow 算法或后端落盘。桌面集成保持独立手动入口，见[测试说明](TESTING.md)。

## 逐项保护规则

### 配方、采集、示教与发布

| 序号 | 源码 | 实际执行用例 | 防止的回归 |
| ---: | --- | --- | --- |
| 1 | [library.test.tsx](../../tests/library.test.tsx) | `配方库操作 > 创建已成功但选用失败，重试仅打开已有候选，避免二次创建` | 创建已成功但选择失败时重试打开同一候选并进入配置流程，不能再次创建重复配方 |
| 2 | [recipe-calibration.test.tsx](../../tests/recipe-calibration.test.tsx) | `多轮标定 > 普通轮次 7 轮，配置 10 轮 → 应用标定禁用并说明还差 3 轮` | 应用标定必须满足普通采集轮数，标准件轮次不能凑足普通标定门槛 |
| 3 | [recipe-calibration.test.tsx](../../tests/recipe-calibration.test.tsx) | `多轮标定 > 普通轮次 9 轮加 1 轮标准件 → 标准件不计入轮数，仍还差 1 轮` | 应用标定必须满足普通采集轮数，标准件轮次不能凑足普通标定门槛 |
| 4 | [recipe-calibration.test.tsx](../../tests/recipe-calibration.test.tsx) | `多轮标定 > 应用标定后容差输入框预填建议值 → 应用本身不提交容差，点确认容差才按输入值提交；非整数不提交` | 建议容差仅预填，只有操作员确认合法整数后才向当前修订提交，不能自动改变配方 |
| 5 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `三台相机中一台未就绪 → 仍默认勾选全部，开始接收禁用并点名未就绪相机` | 任一选中相机未就绪时禁止开始整圈采集，避免静默漏采该相机 |
| 6 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `新轮次有缺帧和丢帧告警 → 显示帧组数、各相机帧数、不完整帧组与告警，PLC 预期总数只作参考，不出现数量核对` | 当前段采集按实际帧组和相机缺帧展示结果，不把预留PLC计数或不完整组误判成整轮异常 |
| 7 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `异常轮保留原因，不能用于示教或标定` | 失败采集轮次不得进入示教或标定，故障原因保持可见 |
| 8 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `示教或标定用过的轮次不允许同时作为独立验证样本 · 段配方：本轮已有相机用作示教帧` | 用于示教或标定的轮次不能作为独立验证样本，防止用训练数据自证通过 |
| 9 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `示教或标定用过的轮次不允许同时作为独立验证样本 · 段配方：本轮已加入多轮标定` | 用于示教或标定的轮次不能作为独立验证样本，防止用训练数据自证通过 |
| 10 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `本轮已作为独立验证样本入库 → 不能再用于走廊示教或标定` | 已入独立验证样本库的轮次不能反向用于示教或标定，保持样本独立性 |
| 11 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `PLC 未连接 →开始接收禁用并给出 PLC 通讯入口` | PLC断开时禁止开始依赖开始/结束握手的整圈采集，并提供恢复入口 |
| 12 | [recipe-capture.test.tsx](../../tests/recipe-capture.test.tsx) | `Software 相机参与整圈采集 → 阻止启动并要求切换 Line0` | Software单帧调试相机不得参与Line0高电平门控整圈采集 |
| 13 | [recipe-create-flow.test.tsx](../../tests/recipe-create-flow.test.tsx) | `创建配方流程 > 在配方库填写新候选，选中创建结果并继续进入该候选的整圈采集` | 通过真实WorkspaceProvider创建当前段配方后，选中同一候选并进入其采集流程，用户填写字段不能丢失 |
| 14 | [workbench-draft.test.ts](../../tests/workbench-draft.test.ts) | `分段标定工作台的边界编辑`（参数化 3 例：插入、删除、末帧组之后不插入） | 插入边界时所在段一分为二且类型相同，删除时与后一段合并、保留前一段类型，边界个数始终等于段数 − 1（D-SB-44，2026-10-26 取代 segment-edit.test.ts） |
| 15 | [workbench-draft.test.ts](../../tests/workbench-draft.test.ts) | `工作台保存的段列表 > 段的类型序列不变 → 原样保存…` | 只改边界时段参数、N_exp、标定与 x_ref 原样保留 |
| 16 | [workbench-draft.test.ts](../../tests/workbench-draft.test.ts) | `工作台保存的段列表 > 段的类型序列变了 → 序号从 1 连续，N_exp、标定与 x_ref 清空…` | 段结构或类型改变时清除全部帧数标定与胶偏参考并维持连续段编号，同位置同类型的段沿用参数 |
| 16a | [workbench-draft.test.ts](../../tests/workbench-draft.test.ts) | `工作台保存的段列表 > 只改检测相机 → …` | 只改检测相机时标定保留；选全存为空；胶偏参考相机随本段相机修正并清空 x_ref（D-SB-41） |
| 16b | [workbench-draft.test.ts](../../tests/workbench-draft.test.ts) | `工作台保存的段列表 > 套用到同类段 → …` | 套用到同类段只改检测项与帧数容差，相机、跳过帧数、长度与标定不变；新建段沿用同类型已有段的检测项与容差（2026-10-26） |
| 17a | [align-review.test.tsx](../../tests/align-review.test.tsx) | `其余轮次的对齐复核 > → 微调不越过下一个边界；两行都确认后才保存，且一次提交整轮边界` | 复核微调保持边界严格递增，全部边界确认后才写入且一次提交（D-SB-44 P2） |
| 17 | [segment-workbench.test.tsx](../../tests/segment-workbench.test.tsx) | `分段标定工作台改变段结构 > 已标定且另有一轮已确认边界 → 保存前列出失效范围，确认后一次提交…` | 改变段结构前先列出会失效的轮次与标定，确认后一次提交段列表与模板轮边界 |
| 18 | [segments-page-leave.test.tsx](../../tests/segments-page-leave.test.tsx) | `防抖没到期就离开段列表页 > 换了配方 → 编辑提交到原配方，新配方照常加载出来` | 防抖未到期切换配方时，旧编辑提交给原配方且不得打断新配方加载 |
| 19 | [segments-page-leave.test.tsx](../../tests/segments-page-leave.test.tsx) | `防抖没到期就离开段列表页 > 仍在这个配方 → 提交后工作区拿到返回的新修订` | 防抖未到期仅离开当前页面时，工作区必须接受保存返回的新修订，避免后续保存冲突 |
| 20 | [segments-page.test.tsx](../../tests/segments-page.test.tsx) | （2026-10-26 删除：段类型改由分段标定工作台编辑，规则见第 16、17 项） | — |
| 21 | [segments-page.test.tsx](../../tests/segments-page.test.tsx) | `已标定的配方，帧数容差改为 2 → 各段预期帧数与胶偏参考保留，保存的段列表带这项改动` | 非结构编辑必须带实际改动保存，同时保留各段预期帧数与胶偏参考 |
| 22 | [segments-page.test.tsx](../../tests/segments-page.test.tsx) | `已标定的配方，长度改为 130 mm → 各段预期帧数与胶偏参考保留，保存的段列表带这项改动` | 非结构编辑必须带实际改动保存，同时保留各段预期帧数与胶偏参考 |
| 23 | [segments-page.test.tsx](../../tests/segments-page.test.tsx) | `已标定的配方，关掉溢胶检测 → 各段预期帧数与胶偏参考保留，保存的段列表带这项改动` | 非结构编辑必须带实际改动保存，同时保留各段预期帧数与胶偏参考 |
| 24 | [segments-page.test.tsx](../../tests/segments-page.test.tsx) | `离开段列表页 > 上一份编辑还在提交时又改了容差并离开 → 等上一份完成，用它返回的新修订提交新编辑` | 保存进行中继续编辑并离页时必须等上次完成，再按返回的新修订提交最新编辑，防止丢改动 |
| 25 | [teach-draft.test.ts](../../tests/teach-draft.test.ts) | `走廊示教草稿 > 草稿与已保存的段走廊只差 f32 精度 → 视为没改；点位、当量或检测参数不同 → 视为已改` | 后端f32往返精度差异不能让走廊永远处于未保存状态，真实点位或检测参数改动仍需识别 |
| 25a | [teach-draft.test.ts](../../tests/teach-draft.test.ts) | `走廊示教草稿 > 段走廊草稿 → 只写这一段的走廊…` | 段走廊草稿不覆盖相机像素当量、检测参数和其他段；清空即从配方去掉（D-SB-42/45） |
| 25b | [teach-draft.test.ts](../../tests/teach-draft.test.ts) | `走廊示教草稿 > 段帧组范围：%s` | 段示教帧可选范围按已确认边界、含两端不差一帧；只存草稿的轮次不给范围（D-SB-42） |
| 26 | [teaching-flow.test.tsx](../../tests/teaching-flow.test.tsx) | `手工落点并自动保存 → 保存期间保留缩放与走廊、不能再改点，保存后草稿并入配方且清空` | 手工走廊经真实Provider自动保存时禁止再改点，保存后保留图像视角并只更新对应相机走廊 |
| 27 | [teaching-flow.test.tsx](../../tests/teaching-flow.test.tsx) | `沿胶路刷选后自动提取走廊 → 先保存草稿，再按刷选引导、笔刷半径与点距在这一段的示教帧上提取一次` | 刷选提取前保存草稿，按这一段这台相机冻结帧、刷选引导与点距提取一次（D-SB-45） |
| 28 | [teaching-flow.test.tsx](../../tests/teaching-flow.test.tsx) | `自动提取失败后按笔划生成 → 定位原因并保留原线，人工草稿保留转弯且走廊待保存` | 自动提取失败必须保留原走廊及失败位置，操作员仍可按笔划生成保留转弯的待保存走廊 |
| 29 | [teaching-flow.test.tsx](../../tests/teaching-flow.test.tsx) | `段已有旧示教帧时点击用于走廊示教 → 直接在本轮这一段内选帧，确认后显示新帧并保持编辑` | 选帧只能在已确认分段轮次的本段帧组内，提交的就是所选（段, 轮次, 帧组）（D-SB-45） |
| 29a | [teaching-flow.test.tsx](../../tests/teaching-flow.test.tsx) | `只指定相机、停留选帧页时采集由接收到收尾再完成 → 定位到该相机第一条未示教的段…` | 只带相机的链接定位到它第一条未示教的段；未完成轮次不能选，完成后自动列出（D-SB-45） |
| 30 | [teaching.test.tsx](../../tests/teaching.test.tsx) | `走廊示教 > 段走廊有未保存的修改 → 其他段与相机参数不能切换并说明先保存，当前段可继续编辑` | 当前段走廊未保存时禁止切换到其他段或相机参数，防止草稿被丢 |
| 31 | [teaching.test.tsx](../../tests/teaching.test.tsx) | `走廊示教 > 相机缺像素当量 → 段走廊保存禁用并说明原因，不写配方` | 像素当量随相机，缺了不得把段走廊保存为已示教 |
| 32 | [teaching.test.tsx](../../tests/teaching.test.tsx) | `示教帧选择 > 链接指定相机与段、所选帧组这台相机缺帧 → …` | 链接指定的段要定位上；所选帧组缺帧时不得读取或冻结不存在的图像 |
| 33 | [validation.test.tsx](../../tests/validation.test.tsx) | `验证与发布页面 > 发布失败后冻结样本配置并重试同一待生效版本` | 发布失败后候选样本与验证被冻结，只能重试同一待生效修订，不能另发新版本 |
| 34 | [validation.test.tsx](../../tests/validation.test.tsx) | `验证与发布页面 > 生效失败 → 确认后放弃同一修订的待生效版本；状态不确定时不能放弃` | 只有明确失败的待生效版本才能经确认放弃，状态不确定时不得暴露放弃入口 |
| 35 | [validation.test.tsx](../../tests/validation.test.tsx) | `验证与发布页面 > 段配方 > 整件预期总帧数未标定 → 发布不可用并列出待标定的段号` | 整件预期总帧数尚未标定时，即使验证通过也不能发布生产配方 |
| 36 | [validation.test.tsx](../../tests/validation.test.tsx) | `验证与发布页面 > 段配方 > 多段配方整件已标定、各段预期帧数待标定 → 发布不可用并列出待标定的段号` | 多段配方各段 N_exp 都已标定才能发布（D-SB-45 修订 D-SB-21） |
| 37 | [validation.test.tsx](../../tests/validation.test.tsx) | `验证与发布页面 > 段配方 > 整件预期总帧数 12 → 14 → 发布确认只列 planVersion 换新号和 shotCount / deviceShots 的变化` | 整件标定帧数变化时发布确认须提示新的计划号与PLC预期总数同步，不能混入未变身份项 |
| 38 | [validation.test.tsx](../../tests/validation.test.tsx) | `验证与发布页面 > 没有样本验证结果、发布前检查全部满足 → 可以发布；检查有未满足项 → …` | 发布不要求独立样本验证；发布前检查有未满足项时不能发布并说明原因（D-SB-46） |
| 39 | [validation.test.tsx](../../tests/validation.test.tsx) | `验证与发布页面 > 确认期间候选修订变更必须重新核对发布前检查和确认` | 发布确认期间候选修订变化必须关闭旧确认、按新修订重新核对发布前检查，旧修订不被发布 |
| 40 | [workspace-context.test.tsx](../../tests/workspace-context.test.tsx) | `候选工作台的主流程保护 > 选择仅在当前请求真正接受后返回视图，旧请求返回 null` | 配方选择请求乱序完成时只接受最新请求，旧响应不能改当前候选或持久化偏好 |
| 41 | [workspace-context.test.tsx](../../tests/workspace-context.test.tsx) | `候选工作台的主流程保护 > 刷新请求发出后用户开始编辑，也不能覆盖草稿` | 刷新请求发出后开始编辑时，迟到刷新不得覆盖本地草稿 |
| 42 | [workspace-context.test.tsx](../../tests/workspace-context.test.tsx) | `候选工作台的主流程保护 > 操作途中离开再选回同一配方，旧操作结果也不能覆盖新选择` | 离开后重新选择同一配方时，旧操作响应不能覆盖新选择的新修订 |
| 43 | [workspace-context.test.tsx](../../tests/workspace-context.test.tsx) | `候选工作台的主流程保护 > 操作响应配方不一致时保留当前候选并允许重试` | 操作响应携带其他配方身份时拒绝接受，保留当前候选并允许正确响应重试 |
| 44 | [workspace-context.test.tsx](../../tests/workspace-context.test.tsx) | `候选工作台的主流程保护 > 某段换了示教帧 → 只清除这一段的旧走廊草稿，同相机别的段的草稿保留且仍算未保存` | 更换某段示教帧只丢弃这一段在旧帧上画的草稿 |
| 45 | [workspace-context.test.tsx](../../tests/workspace-context.test.tsx) | `候选工作台的主流程保护 > 保存失败保留走廊草稿，重试成功后才显示已保存；草稿并入这台相机这一段的走廊整份提交` | 走廊自动保存失败保留草稿可重试，提交只改这一段这台相机的走廊 |

### 生产周期、就绪与历史

| 序号 | 源码 | 实际执行用例 | 防止的回归 |
| ---: | --- | --- | --- |
| 46 | [app-log.test.tsx](../../tests/app-log.test.tsx) | `查无记录后工件入库、新日志插到最前 → 该行可点“无记录，重查”得到历史链接` | 日志出现早于工件落库时允许重查；刷新插入新行后，历史链接仍归属于原cycle的日志行。 |
| 47 | [burst-view.test.tsx](../../tests/burst-view.test.tsx) | `段配方帧组条 > 某台相机缺帧 → 该组的缺帧格为灰色缺帧状态、其余相机格照常，整组标不完整` | 一个相机缺帧只将对应格标为缺帧，其他相机格保持检测结果并将该帧组标为不完整。 |
| 48 | [cycle-isolation.test.tsx](../../tests/cycle-isolation.test.tsx) | `在线周期身份与迟到响应隔离 > 同 SN 新周期已经开始 → 迟到的旧周期快照不能覆盖当前件` | 同SN进入新段采集周期后，即使旧周期快照的时间戳更晚，也不得覆盖当前工件。 |
| 49 | [cycle-isolation.test.tsx](../../tests/cycle-isolation.test.tsx) | `在线周期身份与迟到响应隔离 > 初始快照请求迟到不会覆盖已收到的新周期事件` | 订阅已收到新周期事件时，迟到的初始API快照不得将状态退回旧周期。 |
| 50 | [cycle-timeline.test.tsx](../../tests/cycle-timeline.test.tsx) | `段配方选中一帧 → 勾选只看选中帧后只留该相机该帧组的事件，点事件的相机 / 帧组按钮回调选帧` | 选中相机/seq后时间线只展示该帧事件，不能混入同seq其他相机或同相机其他seq。 |
| 51 | [history-detail.test.tsx](../../tests/history-detail.test.tsx) | `历史详情（段配方记录） > 按区域展示结果，原图按 (相机, seq) 读取；缺帧的相机没有原图可读` | 段历史原图按historyId、camera、seq选取；所选相机缺帧时显示缺帧并停止读取不存在的原图。 |
| 52 | [history-detail.test.tsx](../../tests/history-detail.test.tsx) | `历史详情（段配方记录） > 逐帧追溯每台相机每个帧组一行、缺帧也有 → 只看异常帧后只剩缺帧的那一行` | 逐帧追溯不得省略缺帧行；异常筛选后精确保留CAM-2/seq5这一缺帧。 |
| 53 | [history-detail.test.tsx](../../tests/history-detail.test.tsx) | `历史详情（段配方记录） > 按候选规则重判得到区域结果 → 逐区域比对表标出与原结果不同的区域` | 候选规则重判改变当前区域结论时，区域比对表展示原结果与新结果并标注差异。 |
| 54 | [history-list.test.tsx](../../tests/history-list.test.tsx) | `换筛选后查询失败 → 不把上一次条件的记录当成新条件的结果` | 切换到离线筛选后查询失败时，不得将上个真机条件的记录与统计当成新查询结果。 |
| 55 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 重判结果弯道区域结论改变 → 只有实际变化的区域标为差异` | 弯道区域由OK重判为NG_WIDTH时，仅将弯道区域标为差异，不误标直线区域。 |
| 56 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 重判结果直线区域胶偏均值差超过 0.01 mm → 只有实际变化的区域标为差异` | 直线区域胶偏均值变化超过0.01mm时，必须将该直线区域标为差异。 |
| 57 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 重判结果直线区域胶偏差在 0.01 mm 容差内 → 只有实际变化的区域标为差异` | 直线区域胶偏均值变化不超过0.01mm时，不得将数值噪声误标为结果差异。 |
| 58 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 重判结果弯道区域不完整组变化 → 只有实际变化的区域标为差异` | 弯道区域缺测帧组集合变化时，即使结论相同也必须标为区域差异。 |
| 59 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 计划身份段类型变了 → 区域无法对应，不做逐区域比对` | 同段号的直线/弯道类型变更后，原区域不能与重判区域逐项对应。 |
| 60 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 计划身份某段预期帧数变了 → 区域无法对应，不做逐区域比对` | 任一段的预期帧数变化后计划身份改变，不得继续展示伪造的逐区域对应。 |
| 61 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 计划身份相机顺序变了 → 区域无法对应，不做逐区域比对` | 相机顺序变化会改变帧的相机归属，区域比对必须拒绝不同相机序列。 |
| 62 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 计划身份整件预期总数变了 → 区域无法对应，不做逐区域比对` | 整件预期帧组总数改变时必须拒绝逐区域比对，避免将不同采集计划当作同一计划。 |
| 63 | [history-replay.test.ts](../../tests/history-replay.test.ts) | `段配方按区域比对 > 只改限值、容差、胶偏参考与检测区间 → 计划身份不变，仍可逐区域比对` | 仅调整检测限值、容差、参考或区间时仍属同一计划，允许原区域与重判区域对应。 |
| 64 | [history-scope.test.tsx](../../tests/history-scope.test.tsx) | `切到候选调试历史 → 调试数量明确标注，切回真机不混入调试统计` | 候选离线调试历史与真机历史的数量和来源分开，切回真机不沿用调试统计。 |
| 65 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `在线检测切型锁定 > 切型确认期间开始接收工件，禁止切换并允许取消` | 确认切型期间开始采集则禁用切型并允许取消，不能在本件在途时改变配方。 |
| 66 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `在线检测离线产线控件 > 真机握手待复位但复位条件未满足 → 复位按钮禁用并显示后端原因` | 真实PLC明确缺少复位条件时，前端必须禁用复位并展示后端阻塞原因。 |
| 67 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `在线检测离线产线控件 > 真机模式 → 不显示离线运行控件` | 真机模式不能提供离线启动和候选试运行操作入口。 |
| 68 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `在线检测段配方 > 整件结果标记为检测不完整、胶偏未判、含未分段 → 判定卡只挂对应徽标，结论标签不变` | 检测不完整、胶偏未判与未分段告警必须显示，但不能将真实OK结论改写为NG或ERR。 |
| 69 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `在线检测段配方 > 整件结果标记为帧数异常、疑似提前终止 → 判定卡只挂对应徽标，结论标签不变` | NG_WIDTH伴随帧数异常和提前终止时，仅显示对应徽标并保留胶宽NG结论，不混入其他告警。 |
| 70 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `在线检测段配方 > 默认跟随最新帧组；点帧组条 → 按 (相机, seq) 读各相机原图且缺帧的相机不读，返回当前帧组恢复跟随` | 当前段图像默认跟随最新seq；手动选帧后按相机/seq读取，缺帧相机不读且能恢复实时跟随。 |
| 71 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `候选配方离线试运行 > 未发布候选请求被拒绝 → 保留真实原因并允许按同一修订重试，准备中不重复启动` | 候选试运行准备中不能重复启动；拒绝后保留真实失败原因并允许同一修订重试，不走生产版本入口。 |
| 72 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `候选配方离线试运行 > site 仍在运行 → 禁止从候选入口重复启动` | 离线产线尚在运行时，即使周期空闲也不能从候选入口启动第二次运行。 |
| 73 | [inspect.test.tsx](../../tests/inspect.test.tsx) | `候选配方离线试运行 > cycle 仍在运行 → 禁止从候选入口重复启动` | 当前检测周期仍在采集时，即使离线产线running为false也不能从候选入口重复启动。 |
| 74 | [inspection-diagnostics.test.tsx](../../tests/inspection-diagnostics.test.tsx) | `段配方选中的帧组缺一台相机 → 采集与测量按帧组列出每台相机，缺帧的相机写缺帧而不是被略过` | 当前段诊断保留同帧组每个相机的状态与帧号，缺帧相机不能被列表省略。 |
| 75 | [inspection-readiness.test.tsx](../../tests/inspection-readiness.test.tsx) | `在线检测设备状态 > 离线样本仅生产配方未发布 → 豁免发布条件，报告离线设备条件已通过但不宣告就绪` | 候选离线调试可豁免生产配方发布条件，但不能冒充生产就绪。 |
| 76 | [inspection-readiness.test.tsx](../../tests/inspection-readiness.test.tsx) | `在线检测设备状态 > 离线样本仍有 PLC 阻塞 → 只计 PLC 一项阻塞并给出 PLC 通讯入口` | 离线调试只豁免发布条件，真实PLC阻塞仍必须显示并提供修复入口。 |
| 77 | [inspection-readiness.test.tsx](../../tests/inspection-readiness.test.tsx) | `在线检测设备状态 > 真机生产配方未发布 → 发布条件计为阻塞，无法开工` | 真机配方未发布必须阻塞开工，不能沿用离线调试的豁免。 |

### 相机、标定、PLC与配置

| 序号 | 源码 | 实际执行用例 | 防止的回归 |
| ---: | --- | --- | --- |
| 78 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `相机参数未保存修改 > 改名后切换相机并选保存并应用 → 先把修改写入当前相机，再切到目标相机` | 未保存修改须写入原相机后才切换，避免参数串到目标设备。 |
| 79 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `相机参数未保存修改 > 有未保存修改时点链接离开并取消 → 停留在相机页且修改保留` | 取消离页时保留未保存相机参数，避免误丢配置。 |
| 80 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `相机参数校验 > 相机已释放给 MVS 时修改参数 → 不能保存并提示先重新连接` | 已交给MVS的相机禁止写入参数，避免控制权争用。 |
| 81 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `出图验证 > 软触发相机有未保存的参数修改 → 取一帧不可用并提示先保存` | 禁止用未下发的参数进行单帧验证，避免界面参数与真实采集参数不一致。 |
| 82 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `应用到全部相机 > 复制触发接线 → 其他已绑定相机写入同样的接线，已释放和未绑定的相机跳过并提示，不中断其余相机` | 批量接线应用不得写入已释放/未绑定设备，且保留各目标设备身份并继续可用相机。 |
| 83 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `高级参数（调试） > 临时下发曝光与增益 → 只写入当前相机设备，不写配方` | 临时调光只影响当前设备，不能污染配方成像参数。 |
| 84 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `触发方式 > 修改 Line0 帧率后保存 → 保留高电平并写入新帧率` | 固定帧率修改后仍必须保持Line0高电平门控契约。 |
| 85 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `触发方式 > Software 调试 → 隐藏硬件触发参数且允许单帧取图` | Software模式保留单帧调试能力，并隔离Line0硬件触发配置。 |
| 86 | [camera-page.test.tsx](../../tests/camera-page.test.tsx) | `旧 Software 边沿配置切回 Line0 → 保存为高电平` | Software切回Line0必须清除旧边沿配置，防止误用上升沿而无法连续采集。 |
| 87 | [flyshot-calibration.test.tsx](../../tests/flyshot-calibration.test.tsx) | `速度和拍照点间距未填 → 不给出满足结论并列出缺项` | 关键手填参数缺失时不能宣告满足飞拍要求。 |
| 88 | [flyshot-calibration.test.tsx](../../tests/flyshot-calibration.test.tsx) | `有图像尺寸和当量 → 视野取所选运动方向的像素数乘当量` | 运动方向切换后按正确图像轴和像素当量重算物理视野。 |
| 89 | [flyshot-calibration.test.tsx](../../tests/flyshot-calibration.test.tsx) | `手填像素当量后配方曝光更新 → 保留手填当量并重新核算` | 来源参数刷新不能覆盖用户手填当量，曝光更新仍须参与实时核算。 |
| 90 | [flyshot-calibration.test.tsx](../../tests/flyshot-calibration.test.tsx) | `单视角相机完成标定 → 按冻结图像更新像素当量与飞拍视野` | 当前单视角相机拍照与标定绑定同一冻结图像，标定结果更新飞拍模拟当量与物理视野。 |
| 91 | [image-viewer.test.tsx](../../tests/image-viewer.test.tsx) | `图像查看与加载 > 搜索带沿局部法向显示半宽并裁剪到原图，独立隐藏不影响测量结果` | 图像搜索带沿局部法向并裁剪到真实原图边界，切换可见性不隐藏测量结果。 |
| 92 | [image-viewer.test.tsx](../../tests/image-viewer.test.tsx) | `图像查看与加载 > 从候选图切到历史原图后，迟到的旧图不能覆盖当前来源` | 来源切换后丢弃迟到的旧图响应，避免历史记录显示另一原图。 |
| 93 | [image-viewer.test.tsx](../../tests/image-viewer.test.tsx) | `图像查看与加载 > 刷选固定按下时图像坐标 → 不修改中线，中键和取消后的移动不继续刷选` | 走廊刷选固定起笔坐标变换且裁剪到合法像素范围，平移/取消不得修改示教。 |
| 94 | [plc-handshake-points.test.tsx](../../tests/plc-handshake-points.test.tsx) | `编辑握手点位 → 地址等模板字段不可改，只有描述和记录变化写回草稿` | 编辑握手点只允许修改说明与记录选项，不能破坏固定协议地址/名称。 |
| 95 | [plc-operation-lock.test.tsx](../../tests/plc-operation-lock.test.tsx) | `PLC 操作锁 > 初次状态未知时锁定，空闲快照确认后才开放` | 生产状态未知时默认禁止PLC配置与手动写入，确认空闲后才开放。 |
| 96 | [plc-operation-lock.test.tsx](../../tests/plc-operation-lock.test.tsx) | `PLC 操作锁 > 最新事务事件优先于迟到的初始空闲响应，FAULT 未结事务仍锁定` | 迟到初始响应不得覆盖最新未结事务，FAULT仍需依据事务锁阻止危险操作。 |
| 97 | [plc-operation-lock.test.tsx](../../tests/plc-operation-lock.test.tsx) | `PLC 操作锁 > 订阅失败不能被随后到达的空闲读取解除锁定` | 无法订阅实时生产状态时保持PLC操作锁，不能靠过时快照解除。 |
| 98 | [plc-s7-template.test.tsx](../../tests/plc-s7-template.test.tsx) | `载入一期 S7 点表 > 两个区填同一个 DB → 提示必须分开且不请求后端生成` | PLC写区与PC写区不得使用同一DB，防止协议数据互相覆盖。 |
| 99 | [settings.test.tsx](../../tests/settings.test.tsx) | `离线原图保留设置 > 修改离线保留上限并保存 → 重新打开显示新值且保留其他分区设置` | 修改离线原图保留策略不能覆盖真机录图/历史分区设置，重开后必须持久生效。 |
| 100 | [site-mode.test.tsx](../../tests/site-mode.test.tsx) | `离线模式配方下拉 > 生产版本未发布但运行候选调试 → 发布阻塞只属于回放区域，候选入口与停止仍可用` | 候选无需生产发布即可调试，生产回放阻塞不能遮蔽候选入口或禁止紧急停止。 |
| 101 | [site-mode.test.tsx](../../tests/site-mode.test.tsx) | `离线模式配方下拉 > 所选配方暂不可运行 → 清空选择而不改选别的，恢复可运行后回到所选` | 所选配方暂时不可运行时不能悄悄换成其他产品，恢复后仍使用原选择。 |
| 102 | [site-mode.test.tsx](../../tests/site-mode.test.tsx) | `开工就绪报告刷新 > 就绪报告成功后刷新失败再恢复 → 旧报告仅供参考且成功前不再显示可开工` | 就绪刷新失败后旧报告不能继续宣告可以开工，重新成功才恢复就绪。 |
| 103 | [storage-preferences.test.tsx](../../tests/storage-preferences.test.tsx) | `参数延迟读回期间输入 → 保留新值并写入资料配置，失败可重试` | 迟到配置读取不能覆盖新输入，保存失败保留用户值且可重试落盘。 |

## 删除依据

不同业务规则未塞入单个循环来缩小数字；保留的参数化用例仍逐项展开计数。历史实测记录保留在[前端轻量回归](frontend-lightweight-tests.md#历史记录)，原有覆盖声明不代表当前集合。

### 配方、采集、示教与发布

| 文件范围 | 删除理由 |
| --- | --- |
| `tests/fixture-validation.test.tsx` | 旧拍照点特征定位两端建议的确认/命令映射，不属于当前每相机走廊流程 |
| `tests/geometry-page.test.tsx` | 仅校验摘要字段和静态段信息回显，没有新的用户行为回归 |
| `tests/library.test.tsx` | 删除旧格式文件清理、规模标签回显和删除重试细分；保留当前段配方创建失败恢复，真实创建主流程另有集成覆盖 |
| `tests/plan-sync.test.ts` | PLC提示列表的局部字符串/字段投影由当前段发布确认的代表性界面用例覆盖，Rust另保留计划身份规则 |
| `tests/progress-page.test.tsx` | 移除工作台文案、状态标签及剪贴板DTO回显等细分投影，保留真实创建流程及生产操作门槛 |
| `tests/recipe-calibration.test.tsx` | 删除只验证API参数映射的加入/移出/应用调用和重复局部输入重置；保留普通轮次门槛、标准件隔离与操作者容差确认 |
| `tests/recipe-capture.test.tsx` | 删除旧轮次夹具路径、按钮层级/重连文案、局部API转发和重复预览显示；保留采集准入、缺帧、失败轮及独立验证样本双向隔离 |
| `tests/recipe-editor.test.tsx` | 默认控件集合、差异徽章与局部onDraftChange字段映射未验证完整保存结果；保留真实Provider下创建与段编辑保存 |
| `tests/recipe-run-select.test.tsx` | 仅显示配方规模标签，包含已停用拍照点摘要，不形成独立业务结果保护 |
| `tests/sample-import.test.ts` | 旧拍照点逐视角样本导入选择规则，当前段采集不再沿用 |
| `tests/segment-edit.test.ts` | 删除与界面类型切换重叠的局部默认值及按文本检查的校验投影；保留结构编辑清空标定与连续段编号规则 |
| `tests/segments-page.test.tsx` | 删除待标定静态回显、重复类型切换和卸载提交断言；保留标定失效/保留及保存进行中离页不丢编辑，真实Provider离页另有集成覆盖 |
| `tests/teach-draft.test.ts` | 删除旧拍照点特征定位草稿和被真实走廊保存流程覆盖的局部复制/校验；保留容易导致反复自动保存的f32精度边界 |
| `tests/teaching-flow.test.tsx` | 删除搜索带/点距局部叠图与重复失败建议文案分支，保留手工自动保存、刷选提取、失败人工恢复及真实选帧完整流程 |
| `tests/teaching.test.tsx` | 删除相机列表/总览/采集信息静态展示及与整页流程重复的saveDoc转发；D-SB-45 起相机走廊与 teachCamera 已删，改为段走廊；保留未保存、缺当量和缺帧操作拦截 |
| `tests/trial-result.test.tsx` | 旧逐站试测结果表的状态/空值投影，不作为当前段配方走廊示教主流程验收 |
| `tests/unrolled-curve.test.tsx` | 旧逐站布局的段名截断和提示属于样式行为，当前段帧结果界面不以该旧布局验收 |
| `tests/validation.test.tsx` | 删除旧拍照点发布同步、离线提示文案与重复按钮禁用投影；保留当前段整件 / 逐段标定、发布前检查（D-SB-46 起不要求验证）和待生效版本生命周期门槛 |
| `tests/workspace-context.test.tsx` | 删除旧拍照点草稿与重复保存/通知行为；保留真实Provider下迟到响应隔离、相机草稿隔离与失败恢复 |

### 生产周期、就绪与历史

| 文件范围 | 删除理由 |
| --- | --- |
| `tests/app-log.test.tsx` | 保留迟到入库的cycle日志追溯回归；删除通用分页保留显示与已有链接刷新重复路径。 |
| `tests/burst-view.test.tsx` | 保留当前三相机缺帧格归属；删除纯展示文案、统计文字与结论标签的重复组件测试，整件告警语义由Inspect集成保护。 |
| `tests/cycle-isolation.test.tsx` | 删除旧拍照点Measured事件及回填路径，首项改为当前段采集快照的旧cycle隔离；保留初始读取与新事件竞态。 |
| `tests/history-detail.test.tsx` | 保留当前段历史原图、缺帧追溯及区域重判集成；删除旧拍照点/位姿/逐站比较和已停用的旧示教复测专属路径。 |
| `tests/history-list.test.tsx` | 保留跨真机/离线筛选失败的数据隔离；删除通用分页与旧拍照点兼容展示测试。 |
| `tests/history-replay.test.ts` | 删除旧拍照点逐站、视角和位姿重跑比较；保留当前段区域比较及计划身份契约，删除与容差内无变化等价的全字段相同输入。 |
| `tests/inspect.test.tsx` | 删除旧逐拍照点/定位徽标/旧曲线选择专属用例；删除纯文案和只断言mock请求的离线运行用例，保留操作锁、来源入口、当前帧归属及候选失败恢复。 |
| `tests/inspection-diagnostics.test.tsx` | 删除旧多视角拍照点诊断用例，保留当前三相机段帧组诊断。 |
| `tests/inspection-readiness.test.tsx` | 保留真机/离线发布豁免与PLC阻塞边界；删除DTO展示状态表、展开/折叠、提示文案和内部回调重复验证。 |
| `tests/rejudge-dialog.test.tsx` | 唯一测试只断言mock收到表单字段，未验证重判业务结果；实际区域重判结果由history-detail/history-replay保护。 |
| `tests/runtime-frame.test.tsx` | 删除停用旧拍照点实时图、位姿和测量状态专属测试及仅断言调用数的读图调度；当前段图像归属由Inspect与历史详情集成覆盖。 |
| `tests/verdict-meta.test.ts` | 删除逐枚举镜像静态语义色映射表；真实结论与告警的显示由当前段Inspect集成保护。 |
| `tests/visual-status.test.ts` | 删除停用旧拍照点逐站状态转换和归属测试，当前段使用帧组/相机状态。 |

### 相机、标定、PLC与配置

| 文件范围 | 删除理由 |
| --- | --- |
| `tests/camera-page.test.tsx` | 删除默认配置、重复的显示/几何细节、只核对调用或被保留流程覆盖的等价UI路径，集中保留采集隔离、安全锁与实际参数结果。 |
| `tests/context-help.test.tsx` | 删除帮助/提示文案、概览汇总及导航展示测试，核心操作安全与参数结果由保留流程测试保护。 |
| `tests/errors.test.ts` | 删除帮助/提示文案、概览汇总及导航展示测试，核心操作安全与参数结果由保留流程测试保护。 |
| `tests/flyshot-calibration.test.tsx` | 删除默认配置、重复的显示/几何细节、只核对调用或被保留流程覆盖的等价UI路径，集中保留采集隔离、安全锁与实际参数结果。 |
| `tests/image-viewer.test.tsx` | 删除默认配置、重复的显示/几何细节、只核对调用或被保留流程覆盖的等价UI路径，集中保留采集隔离、安全锁与实际参数结果。 |
| `tests/operation-guide.test.tsx` | 删除帮助/提示文案、概览汇总及导航展示测试，核心操作安全与参数结果由保留流程测试保护。 |
| `tests/plc-s7-template.test.tsx` | 删除默认配置、重复的显示/几何细节、只核对调用或被保留流程覆盖的等价UI路径，集中保留采集隔离、安全锁与实际参数结果。 |
| `tests/pose-overlay.test.ts` | 删除已停止维护的旧拍照点特征定位位姿专项。 |
| `tests/settings.test.tsx` | 删除默认配置、重复的显示/几何细节、只核对调用或被保留流程覆盖的等价UI路径，集中保留采集隔离、安全锁与实际参数结果。 |
| `tests/sidebar-nav.test.tsx` | 删除帮助/提示文案、概览汇总及导航展示测试，核心操作安全与参数结果由保留流程测试保护。 |
| `tests/site-mode.test.tsx` | 删除默认配置、重复的显示/几何细节、只核对调用或被保留流程覆盖的等价UI路径，集中保留采集隔离、安全锁与实际参数结果。 |
| `tests/storage-preferences.test.tsx` | 删除默认配置、重复的显示/几何细节、只核对调用或被保留流程覆盖的等价UI路径，集中保留采集隔离、安全锁与实际参数结果。 |

## 测试文件与夹具清理

删除以下 18 个测试文件；共享夹具的调用方已检查，未删除产品代码、桌面集成或现场操作脚本。

- `tests/fixture-validation.test.tsx`
- `tests/geometry-page.test.tsx`
- `tests/plan-sync.test.ts`
- `tests/progress-page.test.tsx`
- `tests/recipe-editor.test.tsx`
- `tests/recipe-run-select.test.tsx`
- `tests/sample-import.test.ts`
- `tests/trial-result.test.tsx`
- `tests/unrolled-curve.test.tsx`
- `tests/rejudge-dialog.test.tsx`
- `tests/runtime-frame.test.tsx`
- `tests/verdict-meta.test.ts`
- `tests/visual-status.test.ts`
- `tests/context-help.test.tsx`
- `tests/errors.test.ts`
- `tests/operation-guide.test.tsx`
- `tests/pose-overlay.test.ts`
- `tests/sidebar-nav.test.tsx`

通用工作区、生产和历史夹具改用当前段配方；`tests/fixtures.ts` 删除旧拍照点/三目工作区、逐拍历史等无调用夹具，`tests/cycle-visual-fixtures.ts` 删除旧逐拍帧/测量/结果夹具。保留的单视角标定用例保护冻结图身份、像素当量和模拟视野更新；同 SN 的新周期用例保护迟到旧快照隔离。
