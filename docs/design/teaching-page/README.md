# 单帧示教页重设计 · 设计稿

> **旧拍照点模型（保留、停用）**（2026-10-23 标注，依据 [segment-burst.md](../../decisions/segment-burst.md) 第八节、D-SB-12、D-SB-17 与第九节第 12 条）：本设计稿是旧拍照点配方的“单帧示教”页——按拍照组（Pose）选一张检测图、示教中线、逐点试测、保存本组。它已按 v3 实现，代码现为 `src/features/workspace/LegacyTeachingPage.tsx`（下文 3.6“需要改的文件”里写的 `TeachingPage.tsx` 指当时的文件，现在的同名文件是分流入口），只在 `WorkspaceView.plan` 为空（配方 `segments` 为空）时挂载，旧配方行为不变。
>
> 新建配方都是段配方：`/recipe/teach` 在 `plan` 非空时挂载“相机走廊示教”（`src/features/workspace/TeachingPage.tsx`）——每台计划相机从一轮完整采集里选一帧冻结为示教帧，在这一帧上提取一条按直线段几何的走廊，所有帧共用；不逐拍照点示教中线、不做逐点比对，也没有单帧试测命令。走廊示教页沿用了本设计的布局约定（底栏 `.wp-teach-dock`、选帧条 `.wp-picker`、画布模式 `SegmentedControl`、禁用原因用 Tooltip 外包可聚焦 span），但没有单独的设计稿；它的业务规则见 segment-burst.md 第九节第 12 条“走廊示教”。

状态：v3（最终版）已审查并修订，已定稿进入实现。浏览入口 `index.html`；重截图 `pwsh ./shoot.ps1 v3`；v2/v3 页面由各自目录下 `build.py` 生成（改脚本 → `python v3/build.py` → 重截）。

**已定（2026-10-20）**：按 v3 实现；待拍板 5 条全部采用 3.7 中审查者的推荐——1 底栏贯穿内容区三栏、不含导航栏；2 中栏内滚 + 底栏固定；3 最后一组“保存并进入下一组”禁用 + Tooltip；4 标定卡三段文案留原位；5 1280 下分段控件常显两字“编辑 / 刷选 / 框选”。

“整页宽”口径（用户已确认）：底栏贯穿内容区三栏（左侧拍照组、中间示教区、右栏设置），不含应用左侧导航栏。

## 0 本页职责与边界

| 页面 | 使用者 / 时机 | 职责 | 不做什么 |
| --- | --- | --- | --- |
| 单帧示教 `/recipe/teach` | 工艺工程师，整圈采集采用图像之后、验证发布之前 | 每个拍照组（Pose）选一张检测图 → 刷选 / 框选胶路 → 提取或按笔划生成中线 → 手工修点 → 运行试测 → 保存本组；顺带按图覆盖像素当量、检测参数、判定限值并记录标定验证 | 不采集原图（只给“重新采集”链接回整圈采集）、不发布、不做整圈多样本验证 |
| 整圈采集 `/recipe/capture` | 同上，更早 | 按拍照计划采集整圈原图并“采用本轮图像”，生成拍照点 | 不示教 |
| 默认参数 `/recipe/geometry`（2026-10-26 已拆分移除，见配方创建流程） | 同上，可选 | 配方级检测与判定默认值 | 不按图覆盖 |
| 验证与发布 `/recipe/validation` | 同上，更晚 | 独立样本验证通过后发布生效 | 不改中线 |

本页只改布局、层级、组件与文案；业务流程（选图 → 中线 → 试测会先保存参数 → 保存本组 / 并进入下一组）、数据与 API 不变。

## 1 v1

### 1.1 改了什么

1. **底部操作栏贯穿全宽并吸底**（`.wp-teach-dock`，替代 `.wp-teach-actions`）。从中间示教卡片内部移到 `.wp-teach` 三栏网格之后、`.wp-page` 末尾，`margin: 0 -20px -20px` 吃掉页面内边距，横跨左侧拍照组列表、中间示教区、右侧设置栏；`position: sticky; bottom: -20px`，始终可见；`flex-wrap: nowrap`，提示文案 `flex:1` 截断，四个按钮永远一行（这是用户的硬要求）。
2. **底栏内容分三段**：左 = 当前状态徽章（待试测 / 试测通过 · 未保存）+ 位置（S005 · cam1 · 图 1）；中 = 一句话提示（现状 `hint` 文案，放在这里而不是夹在按钮之间）；右 = 运行试测 / 保存本组示教 / 保存并进入下一组。主按钮随流程变化：有中线未试测 → “运行试测” filled；试测通过 → “保存并进入下一组 S006” filled（带下一组编号），“运行试测”降级为 tonal “重新试测”。
3. **禁用原因就近显示**：保存按钮禁用时在按钮左侧用锁图标 + 短句“试测通过后可保存”（现状靠 `title` 悬停）。
4. **画布工具栏瘦身**：缩放 −/100%/+/适应 与“中线叠加”“编辑点”两个 Chip 从顶部工具栏下沉到画布底部信息行右侧；顶部工具栏只保留示教动作（刷选、选区方式、提取、笔划生成、撤销/删点/清空）。
5. **组件对齐 qing-steel**：按钮用 `.qs-btn filled / tonal / sm`（胶囊、36 / 30 高）、`IconButton`、`SegmentedControl`（inset outline 胶囊）、`Chip.selected`、`StatusBadge`（带点徽章，`neutral` 为空心点）、`Progress` 4 px 条、折叠卡 `summary` 右侧统一 ChevronDown；颜色只用 `--ok / --warn / --ng / --primary` 四个语义色，灰度画布上的叠加同样只用这四色。
6. **试测通过画面**：画布叠加各站胶宽横线（绿 = 有效胶宽，红 = 候选未通过，每 4 站标一个数值），试测结果条（合格 · 量成比例 · 耗时 · 中线点数/长度/站数 · 图例），下方逐点表沿用 `TrialResult`。

### 1.2 为什么

- 底栏横跨三栏后，“当前组做到哪一步、下一步按什么”与左栏“整体 4 / 12 组”在同一条视线上，右栏参数改动（当量、限值）也直接受这条栏的“运行试测会先保存参数”约束，放在中间卡片内部会让右栏的改动看起来与保存无关。
- 主按钮随流程切换、禁用原因就近写出，是本软件其它页（相机页保存栏、PLC 页状态条）已经采用的做法，这里对齐。
- 缩放与叠加开关是“看图”工具，不是“示教”动作，和示教按钮同排会让一排按钮过长（现状在 1440 下就已换行）。

### 1.3 自评（v1 问题清单，供审查与 v2 参考）

1. **画布高度未随视口与底栏联动**：a 在 1440×900 首屏画布只露出约 240 px，c 在 1280×800 不滚动几乎看不见画布（c 截图是滚过的）。顶部“工作台条 + 步骤条 + 单帧示教摘要 + 检测图选择条”共占约 300 px。v2 应考虑：摘要行并入步骤条或底栏、检测图选择条压成一行 48 px、画布高度改为 `calc(100vh - 顶部固定区 - 底栏)`。
2. **状态徽章重复三处**：卡片标题右侧（选图›中线›试测›保存 + 徽章）、试测结果条、底栏左侧都在说“待试测”。v2 应只留一处（建议底栏），卡片标题只留四步小流程或干脆去掉。
3. **顶部工具栏在 1440 仍换行**：撤销/删点/清空三个图标按钮被挤到第二行；1280 更差（笔划生成也掉下去）。v2 考虑：把“笔划直接生成中线”收进提取按钮旁的次级菜单，或把“应用到当前中线”和点间距移出参数行；参数行（笔刷宽度/点间距）只在刷选模式下显示。
4. **底栏提示被截断后信息丢失**：1280 下“运行试测会先保存中线与参数到候选，再对…”只剩半句。应改为两级：常驻短句（≤ 20 字）+ 悬停/点击展开全文，或把“会先保存参数”改成按钮下方的小字。
5. **右栏标定卡默认展开占满高度**，判定限值与采集信息被底栏遮住；现状逻辑是 stale/failed 才展开。v1 为展示内容强行展开，v2 恢复按状态展开并给右栏 `max-height` + 内部滚动，避免右栏比画布还长。
6. **底栏右侧三个按钮宽度 ≈ 500 px**，是导致窄屏拥挤的主因。候选方案：试测通过后把“保存本组示教”降为 outlined 文本按钮，或与“保存并进入下一组”合并为分裂按钮（qing-steel 没有 SplitButton，需用 Button + Menu 组合）。
7. **图像采集信息**折叠卡在右栏底部，与底栏争位置；可以移到检测图选择条的“当前检测图”悬停/信息图标里。
8. 未覆盖的状态：提取失败（含加大笔刷 / 改用笔划的建议）、需重新采集、试测异常、生产中只读。v2 至少补“提取失败”一张，因为它决定主按钮切到“笔划直接生成中线”的表现。

### 1.4 v1 审查意见（独立审查，要点）

**必须修**
1. 丢了画布“搜索带”Chip（GrayViewer `searchHalfPx`）。
2. 底栏顺序应与代码一致：`[运行试测] [状态+提示 flex:1 可截断] [保存本组示教] [保存并进入下一组]`；删掉底栏里的位置信息。
3. 底栏遮挡内容（判定限值卡被切、逐点表压在底栏下），滚动区要留与底栏等高的 padding/scroll-padding。
4. 首屏画布太小（P0）：摘要行并进步骤条、选图条压到约 48px、画布高度 `calc(100vh - 顶部固定区 - 底栏)`。
5. 状态覆盖不全，至少补：提取失败（两次后主按钮切“笔划直接生成中线”）、需重新采集、最后一组、生产中；另需说明未选检测图 / 已保存 / pending / 试测异常时底栏表现。
6. 改了中线后左侧拍照组与选图条整体禁用，界面无说明；底栏提示改为“有未试测的修改，先试测或清空后再切组”，组项加 Tooltip。
7. 标定卡丢了“复用”的适用条件说明、“当量已修改，先运行试测保存，再重新验证”、样本集声明分支。

**应该改**
1. 选区模式三处表达同一件事，合并为一个 SegmentedControl `编辑点 | 沿胶路刷选 | 矩形框选`；画布 Chip“编辑点”只管显示全部点柄。
2. Undo2 图标实为“删除末点”，换图标并配 Tooltip。
3. 笔刷宽度只在刷选时显示，框选只留点间距；借此让 1440 下工具栏不换行。
4. 提示文案三处重复：画布下只留操作类提示，底栏只留“下一步做什么”；删掉卡片标题的四步流程和徽章。
5. 1280 下左栏组状态被截成“待选检…”：圆点 + 两字短文案或左栏加宽到约 168px；底栏按钮 1280 下用 sm，提示放 Tooltip。
6. 图像采集信息卡缺 ChevronDown。
7. b 图判定“合格”但画布有红色候选：图例需说明候选未通过不影响整体判定。
8. 禁用按钮用 qing-steel Tooltip 给原因（现用 `title`）。

**可以考虑**：采集信息改为选图条上的 Popover；最后一组改“保存并前往工件总览”；右栏 max-height 内部滚动。

**对自评 8 条**：1 同意升 P0；2 同意只留底栏徽章；3 同意按“模式合并 + 参数行按工具显示”；4 同意先删位置信息；5 同意恢复按状态展开；6 部分同意——不要用 Menu 拼 SplitButton，先缩 sm、“保存本组”降为文本按钮；7 放可以考虑；8 同意，范围扩到全部状态。

## 2 v2

### 2.1 改了什么

**布局与首屏**
1. 删掉独立的“单帧示教 · 已完成 4／12 组 · 进度条”摘要行；进度改由步骤条第 3 步承载：右侧 `4 / 12 组` + 步骤格底部 3px 细进度条（`.wp-step-progress`）。
2. 选图条压成 48px 单行（`.wp-picker`）：`S005 检测图` + 三个胶囊缩略图（40×30，文字“cam1 · 图 1 当前检测图 / 选用此图”并排）+ ⓘ 采集信息 + “重新采集”。
3. 去掉中间“cam1 · 中线示教”卡片外壳与标题（标题、四步流程、徽章全部删除），画布 `.wp-viewport` 直接作为中栏顶层面板，组号/相机/图号写进画布底部信息行左侧加粗。
4. 画布高度 `clamp(260px, calc(100vh - 522px), 640px)`：1440×900 下 378px（v1 首屏 ≈ 240px），1280×800 下 278px 且首屏可见；522 = 顶部固定区（页头 60 + 工作台条 52 + 步骤条 66 + 选图条 64 + 工具行 34 + 参数行 42）+ 画布下方（信息行 34 + 试测条 46 + 底栏 60 + 边距）。
5. 左栏 172px（1280 下 168px），组状态改两字短文案：已示教 / 待试测 / 未配置 / 待选图 / 需重采 / 试测通过；右栏 `position: sticky; max-height: calc(100vh - 262px); overflow: auto` 自己滚，不会被底栏盖住。
6. `main-body` 加 `scroll-padding-bottom: 64px`；底栏是文档流末尾的 sticky 元素，滚到底时自然位于内容之后，不再压住最后一张卡。

**底栏**（`.wp-teach-dock`，贯穿内容区三栏）
7. 顺序按代码：`[运行试测] [状态徽章 + 一句提示（flex:1，可截断）] [保存本组示教] [保存并进入下一组]`；删掉位置信息。
8. 主按钮随流程：待试测 → “运行试测”filled；试测通过且有下一组 → “保存并进入下一组 S006”filled，“保存本组示教”降为 text 按钮，“运行试测”改“重新试测”tonal；最后一组 → “保存本组示教”filled，右侧“保存并前往工件总览”tonal（见待拍板）。1280 下底栏按钮用 sm。
9. 禁用原因改用 Tooltip（a：悬停“保存本组示教”→“试测通过后可保存”；d：悬停“运行试测”→“请先提取或绘制至少两个中线点”），不再在栏内常驻文字。

**画布工具**
10. 一个 SegmentedControl 承载画布模式：`编辑点 | 沿胶路刷选 | 矩形框选`，替代“刷选胶路区域”开关 + 刷选/框选分段两处；画布底部 Chip 为 `中线叠加 / 搜索带 / 编辑点`（恢复搜索带；“编辑点”Chip 只管显示全部点柄）。
11. “删除末点”换 lucide `Delete`（退格形）图标，`Trash2` 删选中点、`Eraser` 清空，各带 Tooltip。
12. 参数行按模式显示：刷选时 `笔刷宽度 + 滑块 + 中线点间距 + 应用到当前中线`，编辑点/框选时只有 `中线点间距 + 应用`；1440 下工具行单行（c 的 1280 刷选模式仍会换行，见自评）。
13. 画布参数行下方只放操作类提示（提取后的“请检查后运行试测”、刷选引导、提取失败原因）；提取失败用黄色 ⚠ 行 + 粗体原因 + 建议。

**右栏**
14. 标定卡恢复“复用到其他适用拍照点”下的“适用条件”折叠说明、“当量已修改，先运行试测保存，再重新验证”提示（g）；样本集声明分支文案不变（`样本集 {id} 声明：{mmPerPx} mm/px。此来源不等同于人工验证。`），位置同现状，未单独出图。标定卡默认按状态展开（verified 收起，stale/failed 展开），a 为展示强制展开。
15. “图像采集信息”卡移除，改为选图条 ⓘ 的 Popover（b 展示打开态；曝光/增益与配方不一致的黄字提示也在其中）。
16. 右栏三张卡摘要缩短为一行：`暗胶 · 2.0–5.0 mm · 搜索 ±3.0 mm`、`位置 · 胶宽 · 缺胶`。

**新增状态**
17. d 提取失败两次：主按钮切到“笔划直接生成中线”（filled），自动提取降为 tonal，画布只显示刷选半透明带 + 失败点红叉；底栏“待示教”+“自动提取两次失败：按笔划生成中线后手工修正，再运行试测”。
18. e 需重新采集：选图条“重新采集”升为 tonal 按钮；中栏顶部 Notice（warn）+“整圈重新采集”按钮；画布淡显并盖“图像或设备已变化 · 中线可查看，不能编辑或试测”；提取/笔划禁用；试测条“试测已过期”；底栏三按钮禁用，提示黄字“图像或设备已变化，请重新采集后再试测”。
19. f 最后一组 S012：步骤条 `11 / 12 组`；底栏提示“这是最后一组；保存后 12 组全部完成，可前往工件总览核对位置”。
20. g 工件检测中 + 中线已修改：左栏其余组与选图条禁用（opacity .55），悬停组项 Tooltip“有未试测的修改，先试测或清空后再切组”；选图条右侧 🔒“工件检测中，结束后可重采”替代“重新采集”；试测条“试测已过期 · 上次：合格 97.5%（中线已改，不再用于保存）”；底栏“中线已修改 · 需试测”+“有未试测的修改：先试测或清空后再切组、换图；现有试测不用于保存”。

### 2.2 未出图状态的底栏表现（文字说明）

| 状态 | 判定 | 运行试测 | 徽章 / 提示 | 保存本组 | 保存并进入下一组 |
| --- | --- | --- | --- | --- | --- |
| 未选检测图 | `!enabled`（组无 choice 或多选） | 禁用，Tooltip“请先选择本组检测图” | neutral“待选图” / “点上方一张图作为本组检测图，每组只需完成这一张”；多选时 warn“本组原先选择了多张图，请重新选择一张” | 禁用 | 禁用 |
| 已保存（taught，未改） | `viewState=taught && !editing` | tonal“重新试测” | ok“已保存” / “本组已完成；可直接进入下一组，或修改中线后重新试测” | 禁用，Tooltip“本组已保存，修改后需重新试测” | filled“进入下一组 S006”（只跳组，不写入；文案去掉“保存并”） |
| 候选待生效（pending） | `workspace.pending` | 禁用，Tooltip“候选版本正在等待生效” | warn“候选待生效” / “等待当前工件结束后切换，期间不能修改示教” | 禁用 | 禁用（切组仍可用） |
| 试测异常 | `viewState=trialFailed && !editing` | filled“重新试测” | ng“试测异常” / 取 `trial.reason` 首句；中栏保留现有 Notice（warn，全文） | 禁用，Tooltip“试测通过后可保存” | 禁用 |
| 处理中 | `working` | 按钮文字“计算中…”/“提取中…”，禁用 | 保持上一状态徽章 / “正在处理或保存，请稍候” | 禁用 | 禁用 |

### 2.3 为什么

- 画布是本页唯一的工作面，其它所有东西都是围绕它的输入与输出；首屏放不下画布就谈不上示教体验，所以把能并的行都并掉（摘要进步骤条、标题进信息行、采集信息进 Popover），把高度留给画布。
- 底栏顺序照代码（运行试测在最左）是因为用户已经认可那条栏，只要求它变宽；状态徽章与提示放中间，左右两端都是动作，视线从左到右正好是“先试测 → 看结果 → 再保存”。
- 模式合并成一个 SegmentedControl，是因为“刷选开关 + 刷选/框选分段 + 编辑点 Chip”在逻辑上互斥，三处表达让操作员分不清当前点击画布会发生什么；合并后画布底部信息行的文字（“按住左键沿胶路刷过” / “点击图像加中线点”）与分段选中项一一对应。
- 禁用原因改 Tooltip，是对齐 qing-steel 的做法并给底栏腾宽度；但“为什么整条栏都不能用”（需重采 / pending / 工件检测中）这种页面级原因仍常驻在底栏提示里，不藏进悬停。

### 2.4 对审查意见的逐条回应

| 意见 | 处理 | 说明 |
| --- | --- | --- |
| 必须修 1 搜索带 | 采纳 | 画布底部 Chip `中线叠加 / 搜索带 / 编辑点`，b/f 展示“搜索带”开启 |
| 必须修 2 底栏顺序 / 删位置 | 采纳 | 见 2.1-7 |
| 必须修 3 底栏遮挡 | 采纳（方式不同） | 底栏是文档流末尾的 sticky 元素，滚到底时不遮挡；加 `scroll-padding-bottom: 64px`；右栏改为自滚，不再进入页面滚动 |
| 必须修 4 首屏画布 | 采纳 | 见 2.1-1~4，1440×900 画布 378px |
| 必须修 5 补状态 | 采纳 | 新增 d/e/f/g 四张；未出图状态见 2.2 |
| 必须修 6 切组禁用说明 | 采纳 | g：Tooltip + 底栏提示 + 选图条禁用 |
| 必须修 7 标定卡文案 | 采纳 | 适用条件折叠、当量已修改提示已出图；样本集分支按现状文案，未单独出图 |
| 应该改 1 模式合并 | 采纳 | 见 2.1-10 |
| 应该改 2 删末点图标 | 采纳 | lucide `Delete` |
| 应该改 3 笔刷只在刷选显示 | 采纳 | 1440 单行；1280 刷选模式仍换行（自评 1） |
| 应该改 4 文案去重 | 采纳 | 卡片标题与流程删除；提示分“画布 = 操作类 / 底栏 = 下一步” |
| 应该改 5 1280 左栏 / sm 按钮 | 采纳 | 168px + 两字状态；底栏 sm；提示保留常驻短句而非 Tooltip（页面级原因需常显，见 2.3） |
| 应该改 6 采集信息卡 Chevron | 采纳（以移除代替） | 卡片改 Popover，不再有折叠卡 |
| 应该改 7 候选红线图例 | 采纳 | 图例文字“候选胶宽未通过（仅供参考，不影响判定）” |
| 应该改 8 Tooltip 给原因 | 采纳 | a/d/g 三处展示；实现用 qing-steel `Tooltip` 包 Button |
| 可考虑 Popover | 采纳 | b 展示 |
| 可考虑 最后一组文案 | 采纳为方案，待拍板 | f 用“保存并前往工件总览”；工件总览是可选步骤，是否改为“保存并前往验证与发布”需用户定 |
| 可考虑 右栏自滚 | 采纳 | sticky + max-height |
| 自评 6 SplitButton | 采纳审查意见 | 不拼 Menu；“保存本组”在试测通过时降为 text 按钮，1280 用 sm |

### 2.5 v2 自评（供审查）

1. **1280 刷选模式工具行仍两行**（c）：分段控件 + 两个按钮 + 三个图标在 545px 中栏放不下，图标组掉到第二行；参数行“应用到当前中线”也换行。候选：1280 以下分段控件只显示图标（Tooltip 给文字），或把三个图标按钮移到画布底部信息行右侧。
2. **画布 calc 常量（522px）是手算的**，实现时若工作台条出现 Notice（候选未保存、操作未完成等）会把画布再压一截；应改成容器查询或 `flex: 1` + 外层 `height: 100%` 的真正自适应，而不是 vh 减常量。
3. **步骤格内的 3px 进度条**是新增视觉元素，qing-steel 没有对应组件，用的是 `Progress` 的 token 复刻；审查者判断是否值得为此加一个样式。
4. **g 把“生产中”和“已修改未试测”叠在一张图**，两个原因共用一个底栏提示位，现在只显示“已修改”的；若两者同时成立该显示哪条、还是合并成一句，需要规则（建议：阻塞当前动作的优先）。
5. **b 的 Popover 压在工具行上**，真实实现应向下弹并避开工具行，或改为点击缩略图上的 ⓘ；静态稿只表达内容与归属。
6. **f “保存并前往工件总览”改变了按钮语义**（从“保存并切组”变“保存并跳页”），虽然不改数据，但是否算流程变化需用户拍板；保守方案是最后一组右侧按钮隐藏，只留“保存本组示教”filled。
7. 未覆盖：未选检测图、taught、pending、trialFailed、working 五个状态只有 2.2 的文字表，没有出图；多选图告警（`group.selected.length > 1`）在选图条下方的红字也未出图。
8. 右栏自滚后，若右栏内容短于视口（常见），`sticky` 让它跟随，但 1280×800 下标定卡展开时右栏需要滚动，滚动条贴着页面滚动条，视觉上两条滚动条并排；可考虑右栏滚动条 `scrollbar-gutter` 或改为只在 >900 高时 sticky。

### 2.6 v2 审查意见（独立审查，要点）

**v1 必须修复核**：①搜索带已修，但刷选模式下仍显示“编辑点”Chip（代码 regionMode 时 onEdit 为 undefined）；②底栏顺序与贯穿满足；③遮挡部分修：b/f/g 试测条/逐点表上沿被底栏盖住，g 右栏判定限值被切（右栏 max-height 没扣底栏）；④1280 未达标：画布信息行与 Chip 行在底栏下面，工具行/参数行各折两行，选图条 cam2 被截、cam3 不可见；⑤2.2 表两处与代码不符，未选图/多选告警没出图；⑥已修；⑦g 在 editing 时仍显示“填写参考与实测尺寸后可记录”且复用勾选框可点，代码此时隐藏提示、禁用勾选框。

**新必须修**：1 试测结果首屏不可见——试测条并进画布信息行，TrialResult 默认折叠；2 e 改变了业务规则——`unlocked` 不含 recapture，中线仍可编辑/提取，只禁试测与保存，画布不淡显；3 f 不要“保存并前往工件总览”——`!nextGroup` 时第二个按钮禁用 + Tooltip“已是最后一组”，工件总览只作保存后提示里的文字链接；4 2.2 表：pending 时组列表也禁用；试测异常徽章代码是 warn；5 qing-steel Tooltip 是 Radix Trigger asChild，disabled 按钮收不到 pointer 事件，需外包 `<span tabIndex=0>`；6 画布高度不能用 vh 减常量——`.wp-gray-image` 有 aspectRatio 内联样式；改为中栏 flex 列，画布 flex:1 min-height，右栏高度 = 中栏高度。

**应该改**：“编辑点”Chip 改名“全部点柄”且只在编辑点模式显示；1280 分段控件只显示图标、“应用到当前中线”改 sm text、缩略图溢出要有提示；b 画布标注压胶路/被裁，加深色底并避边；Popover 向下弹（side=bottom align=end）；说明 @media(max-width:1250px) 区间右栏表现；底栏 sticky 写法对齐 `.wp-defaults-savebar`，负边距要注明原因。

**可以考虑**：e 两处“重新采集”去一处；g 保留“重新采集”链接只加锁提示；步骤条进度用 qing-steel Progress 改高度（Steps 是共用组件，确认其它页也显示 n/12 组）。

**对 v2 自评**：1 同意 P1；2 升 P0（= 必须修 6）；3 可保留、用 Progress；4 同意“阻塞当前动作的优先”，生产中不阻塞编辑，显示“已修改”那条对；5 同意 side=bottom；6 同意保守方案（禁用 + Tooltip 不隐藏）；7 至少补“未选检测图”与多选告警；8 根因是 262 没扣底栏，改为右栏高度 = 中栏高度。

## 3 v3（最终版）

### 3.1 改了什么

**高度与滚动（必须修 6 / 复核 ③④）**
1. 不再用 `vh − 常量`。`.main-body` 改为 flex 列，`.wp-page` `flex:1; min-height:0`，三栏网格 `.wp-teach` `flex:1; align-items:stretch`，底栏是网格之后的普通流内元素（不 sticky，永远不会盖住任何内容）。三栏等高；左栏与右栏各自 `overflow:auto`；中栏 `.wp-mid` 是 flex 列：选图条 → 画布容器（`flex:1; min-height:400px`）→ 折叠的逐点表；中栏内容超出时中栏自己滚（b2），画布保持 ≥ 400px 容器高（图像区 ≈ 290px）。
2. 画布 `.wp-gray-image` 去掉固定高度与 aspectRatio，`flex:1`，用 GrayViewer 已有的 clientWidth/clientHeight fit 逻辑 letterbox。1440×900 图像区 ≈ 380px，1280×800 ≈ 300px（刷选模式 ≈ 250px）。
3. 1280×800（c、c2）：分段控件改常显两字“编辑 / 刷选 / 框选”（审查推荐，不做纯图标，触摸屏无悬停），“应用到当前中线”改 sm text 按钮紧跟点间距；未选中的缩略图只显示相机名，缩略图条右侧渐隐提示可滚；底栏按钮 sm；画布容器最小高 340px；信息行隐藏“· 212 ms”，图例从逐点表摘要行移到展开后的正文——这样折叠的逐点表摘要行与底栏都在首屏。

**试测结果（必须修 1）**
4. 画布底部信息行左侧 = 判定徽章 + `量成 97.5% · 212 ms · 15 点 · 182.4 mm · 15 站`（未试测时 = `未试测 · 15 点 · 182.4 mm`；过期 = `试测已过期`）；右侧 = 缩放 + `中线 / 搜索带 / 点柄` Chip。原 `.wp-trial-bar` 删除。
5. 逐点试测结果（TrialResult）默认折叠成一行 `逐点试测结果 · 测得 14 / 15 · 未测 1` + 图例，位于画布下方；展开后中栏滚动（b2）。图像尺寸 `2448 × 2048 px` 移入 ⓘ Popover。

**按代码语义修正（必须修 2/3/4，复核 ①⑦）**
6. e 需重新采集：画布不淡显，中线可编辑、可提取；只禁用“运行试测”（Tooltip“图像或设备已变化，请重新采集”）与两个保存；Notice 文案“中线仍可修改，重新采集后才能试测”；“重新采集”只保留 Notice 里一处（选图条只剩 ⓘ）。
7. f 最后一组：“保存本组示教”filled，“保存并进入下一组”禁用 + Tooltip“已是最后一组”（按钮位置稳定）；底栏提示“已是最后一组；保存后 12 组全部完成，可到 [工件总览] 核对位置”（文字链接）。
8. “编辑点”Chip 改名“点柄”，只在编辑点模式显示（刷选/框选模式下不渲染，与 `onEdit` 为 undefined 一致）。
9. g 的标定卡：editing 时隐藏“填写参考与实测尺寸后可记录”，复用勾选框禁用；“重新采集”链接保留，灰显 + 锁图标 + title“工件正在检测，重采需等待工件结束”。
10. 2.2 表修正：pending 时组列表、选图条一并禁用；试测异常徽章 warn（见 3.2）。

**a/c 按代码画成“刚提取后”状态**
10b. 自动提取成功后 `setFrameDraft` 使 `editing=true`（TeachingPage 第 184、123 行）：徽章 warn“中线已修改 · 需试测”，组列表与选图条禁用（第 266、271 行），底栏提示“有未试测的修改：先试测或清空后再切组、换图；试测会先保存中线与参数到候选”；画布参数行保留“中线已提取，请检查后运行试测”。原 info“待试测”徽章只出现在“草稿已保存到候选但未试测”（`pendingTrial` 且 `!editing`）这一较少见状态。

**新增状态**
11. h 未选检测图（S007）：中栏只有选图条（三张都“选用此图”）+ Notice“选择本组检测图”；右栏不渲染（代码如此），用虚线占位保持底栏与三栏位置稳定；底栏三按钮禁用，“运行试测”Tooltip“请先选择本组检测图”，徽章“待选图”。
12. i 多选告警（S003）：被多选的两张缩略图用 warn 容器色 + “已选 · 需重选”，缩略图行保持完整，红字告警换行放在选图条正下方（沿用代码 `role=alert` 文案）；中栏 Notice（warn）解释来源与做法；底栏徽章“需重选图”。
13. b2 逐点表展开后的滚动态。

**其它应该改**
14. 画布标注：文字加深色描边底（`paint-order: stroke`），靠近左右边缘的站不出标签。
15. Popover 向下弹、右对齐（`side="bottom" align="end"`），锚在选图条 ⓘ；静态稿把它挂在 `.wp-teach` 右上（覆盖右栏顶部，不盖工具行），实现中 qing-steel Popover 由 Radix portal 到 body，不受中栏 `overflow:auto` 裁剪。
16. 步骤条第 3 步显示 `4 / 12 组` + 细进度条：这是对共用 `Steps` 组件的改动（现有 `stageStatusText` 只有“进行中 / 完成 / 阻塞…”），组数要用 `teachingTasks(doc.shots, data.progress)` 按 poseId 归并计算（与 `teachingGroups` 一致），进度条复用 `Progress`（无 className prop，外包 `<span class="wp-step-progress">` 定位/限高），只在第 3 步 active 且 `pathname === "/recipe/teach"` 时渲染。
17. 工具行在 1440 单行：分段控件内边距 9px、按钮 12px、间距 5px。

### 3.2 未出图状态的底栏表现（修正版）

| 状态 | 判定 | 运行试测 | 徽章 / 提示 | 保存本组 | 保存并进入下一组 | 其它 |
| --- | --- | --- | --- | --- | --- | --- |
| 已保存（taught，未改） | `viewState=taught && !editing` | tonal“重新试测” | ok“已保存” / “本组已完成；可直接进入下一组，或修改中线后重新试测” | 禁用，Tooltip“本组已保存，修改后需重新试测” | filled“进入下一组 S006”（只跳组，不写入） | — |
| 候选待生效（pending） | `workspace.pending` | 禁用，Tooltip“候选版本正在等待生效” | warn“候选待生效” / “等待当前工件结束后切换，期间不能修改示教” | 禁用 | 禁用 | 组列表、选图条、画布编辑、右栏全部禁用（`unlocked` 含 `!pending`） |
| 试测异常 | `viewState=trialFailed && !editing` | filled“重新试测” | warn“试测异常” / 取 `trial.reason` 首句；中栏保留现有 Notice（warn，全文） | 禁用，Tooltip“试测通过后可保存” | 禁用 | — |
| 处理中 | `working` | “计算中…”/“提取中…”禁用 | 保持上一状态徽章 / “正在处理或保存，请稍候” | 禁用 | 禁用 | 组列表、选图条禁用 |
| 样本集声明当量 | `calibration.state=sampleSet` | 不变 | 不变 | 不变 | 不变 | 标定卡徽章 info“样本集声明”，正文一句话替代参考/实测表单（文案同代码） |

### 3.3 为什么

- 用 flex 列而不是 vh 常量，是因为画布高度本来就应该是“剩下多少给多少”，任何顶部横幅（候选未保存、操作未完成）出现时自动让位，不需要再算一遍；底栏放回文档流后，也不再需要 sticky、负边距和 scroll-padding 这些补丁。
- 试测结果进画布信息行，是因为“合格 / 量成比例 / 耗时”是看完画布叠加后紧接着要看的数字，和缩放控件同一行正好；逐点表是审查用的明细，折叠不占首屏。
- e/f 按代码改回，是因为设计稿不改业务规则（任务约束）；“最后一组”禁用而不是隐藏，保证四个按钮位置稳定，操作员形成的肌肉记忆不会落空。

### 3.4 对 v2 审查意见的逐条回应

| 意见 | 处理 | 说明 |
| --- | --- | --- |
| 复核 ① 刷选模式的“编辑点”Chip | 采纳 | 改名“点柄”，只在编辑点模式渲染 |
| 复核 ③ 遮挡 | 采纳 | 底栏入文档流，右栏/中栏各自滚，任何状态下无遮挡（b2 验证） |
| 复核 ④ 1280 | 采纳 | c/c2 全部首屏可见，工具行单行，缩略图只显示相机名 + 渐隐提示 |
| 复核 ⑤ 2.2 表 / 未选图 / 多选 | 采纳 | 表修正见 3.2；新增 h、i |
| 复核 ⑦ g 标定卡 | 采纳 | 隐藏提示、禁用勾选 |
| 新 1 试测结果首屏 | 采纳 | 并入信息行 + 默认折叠 |
| 新 2 e 业务规则 | 采纳 | 只禁试测/保存，画布可编辑 |
| 新 3 f 按钮 | 采纳 | 禁用 + Tooltip；工件总览改文字链接 |
| 新 4 pending / 试测异常 | 采纳 | 见 3.2 |
| 新 5 Tooltip 外包 span | 采纳 | 静态稿已按 `<span tabindex="0">` 包裹；实现要点见 3.6 |
| 新 6 高度方案 | 采纳 | 见 3.1-1/2 与 3.6 |
| 应该改 Chip 改名 | 采纳（“点柄”而非“全部点柄”） | 信息行宽度有限，Tooltip 给全称“显示全部点柄” |
| 应该改 1280 三项 | 采纳 | 图标分段、sm text、渐隐提示 |
| 应该改 画布标注 | 采纳 | 深色描边 + 避边 |
| 应该改 Popover 方向 | 采纳 | bottom / end |
| 应该改 ≤1250 区间说明 | 采纳 | 见 3.6“窄屏” |
| 应该改 sticky 对齐 savebar | 采纳（以去 sticky 代替） | 底栏不再 sticky，也无负边距；默认参数页 `.wp-defaults-savebar` 不受影响 |
| 可考虑 e 去一处重新采集 | 采纳 | 只留 Notice 内按钮 |
| 可考虑 g 保留链接 | 采纳 | 灰显 + 锁 |
| 可考虑 Progress | 采纳 | 复用 `.qs-progress`；Steps 为共用组件，n/12 组文案本就来自现有 `stageStatus`，进度条只在第 3 步 active 时渲染 |

### 3.5 v3 自评

1. 中栏滚动（而非整页滚动）在逐点表展开时，底栏固定、画布缩到 400px 容器；如果用户习惯整页滚动，改回“底栏 sticky bottom:0（同 `.wp-defaults-savebar`）+ 中栏不滚”也成立，但那样 b2 又会出现遮挡问题，v3 选了前者。
2. 画布容器 `min-height:400px` 是经验值：1280×800 刷选模式下图像区约 250px；再矮的窗口只能靠中栏滚动。
3. h/i 右栏用虚线占位是设计稿表达，代码里 `!enabled` 时右栏不渲染；实现可直接留空列（grid 列仍在），不必画虚线框。
4. 画布信息行在 1280 下允许换行（两行），1440 单行；信息行里“15 站”在未保存中线时不显示（代码 `own && !editing`），静态稿未区分。
5. 1280 下分段控件改为常显两字“编辑 / 刷选 / 框选”，不再依赖 Tooltip；是否需要同时保留图标待实现时按宽度定。

### 3.6 组件映射 · 需改文件 · 实现要点 · 风险 · 待拍板

**组件映射（均为 qing-steel 或仓库已有）**

| 区域 / 元素 | 组件 |
| --- | --- |
| 步骤条进度 | 现有 `Steps` + qing-steel `Progress`（加 `.wp-step-progress` 高度/定位类） |
| 选图条 | 现有 `.wp-image-picker` 重排为 `.wp-picker`；缩略图 `FrozenViewOption`（胶囊化）；ⓘ `IconButton` + qing-steel `Popover`（`side="bottom" align="end"`） |
| 画布模式 | qing-steel `SegmentedControl`（`编辑点 / 沿胶路刷选 / 矩形框选`，图标 lucide `MousePointer2 / Brush / SquareDashed`） |
| 提取 / 笔划 | `Button size="sm" variant={primary===… ? "filled" : "tonal"}` |
| 删末点 / 删选中 / 清空 | `IconButton` + `Tooltip`（图标 `Delete / Trash2 / Eraser`） |
| 参数行 | `NumberField`、`Slider`（仅刷选）、`Button size="sm" variant="text"` |
| 画布 | 现有 `GrayViewer`（去 aspectRatio、`flex:1`）；Chip `中线 / 搜索带 / 点柄` 用 qing-steel `Chip` |
| 信息行试测摘要 | 现有 `Badge`（StatusBadge）+ 文本 |
| 逐点表 | 现有 `TrialResult`（`<details>` 默认收起 + `Table`） |
| Notice（需重采 / 未选图 / 多选 / 试测异常） | 现有 `Notice`（Alert） |
| 右栏三卡 | 现有 `TeachingSettings`（`<details class="panel wp-settings-card">`）不变 |
| 底栏 | 新 `.wp-teach-dock`：`Button`×3 + `Badge` + 文本；禁用原因 qing-steel `Tooltip` |
| 组项禁用说明 | `Tooltip` 包 `<span tabIndex={0}>` 包 `button.wp-group-item` |

**需要改的文件**

| 文件 | 改动 |
| --- | --- |
| `src/features/workspace/TeachingPage.tsx` | 删 `.wp-teach-summary`；选图条重排；去掉 `Panel` 外壳与 `wp-teach-flow`；`tools` 改为 SegmentedControl 三态（`regionMode`+`selectionTool` 合成一个 `mode`）；`params` 按 mode 渲染笔刷；试测摘要移到 `GrayViewer` 的 footer；`wp-trial-bar` 删除；底栏移到 `.wp-teach` 之后并重排顺序；最后一组禁用 + Tooltip；recapture 状态文案；未选图时底栏仍渲染 |
| `src/features/workspace/components.tsx`（`GrayViewer`） | 新增 prop（建议 `controlsAt="footer"` + `summary` 节点）仅示教页启用：缩放/Chip 下沉到 footer、footer 左侧放试测摘要、Chip 文案“中线 / 搜索带 / 点柄”、操作提示移出 caption；默认行为不变，其他使用方（`CapturePage`、`OverviewPage`、`RuntimeFrame`、`StationCapture`、相机页 `camera.css`）不受影响。不去 aspectRatio，高度由示教页作用域 CSS 接管（见实现要点） |
| `src/features/workspace/TrialResult.tsx` | `<details>` 默认收起，summary 加图例 |
| `src/features/workspace/TeachingSettings.tsx` | 不改逻辑；仅 class 调整（摘要缩短可选） |
| `src/features/workspace/components.tsx`（`Steps`） | 第 3 步 active 时渲染 `Progress` |
| `src/features/workspace/workspace.css` | 新增 `.wp-mid / .wp-picker / .wp-teach-dock / .wp-step-progress`；`.wp-teach` 改 `align-items:stretch` + 高度链；删 `.wp-teach-actions / .wp-trial-bar / .wp-teach-summary / .wp-teach-flow / .wp-image-picker*`；`.wp-group-panel` 去 sticky 改 `overflow:auto`；更新 `@media(max-width:1250px)` |
| `src/styles.css` | `.main-body:has(.wp-teach-page) { display:flex; flex-direction:column }`（不要 `.wp-page{height:100%}`） |
| `tests/image-viewer.test.tsx` | 依赖“中线叠加 / 编辑点”文案 → 改为“中线 / 点柄”或改用 aria-label |
| `tests/teaching-flow.test.tsx` | 依赖“刷选胶路区域 / 结束选区 / 保存并进入下一组” → 改为分段控件“沿胶路刷选 / 编辑点”与固定 aria-label |
| `tests/teaching.test.tsx` | 依赖“图像采集信息”折叠卡 → 改为 ⓘ 按钮（aria-label="图像采集信息"）+ Popover |
| 可变文案按钮 | “保存并进入下一组 S006”“进入下一组 S006”等带组号的按钮加固定 `aria-label="保存并进入下一组"`，测试与读屏都按固定名找 |

**实现要点（高度方案，含审查者补充）**

```
.main-body:has(.wp-teach-page) { display:flex; flex-direction:column }     ← 只在示教页改 main-body
.wp-page.wp-teach-page { flex:1 1 auto; min-height:0 }                     ← 不用 height:100%
   ├ WorkspaceBar、Steps（沿用 workspace.css 第 2 行 .wp-page > * { flex-shrink:0 }；Notice 出现时自然占位，画布让位）
   ├ .wp-teach-page > .wp-teach { flex:1 1 0; min-height:360px; display:grid; grid-template-columns:172px 1fr 254px; align-items:stretch }
   │                                ↑ 覆盖 .wp-page > * 的 flex-shrink:0；360px 保证 WorkspaceBar 最多 5 条 Notice 时画布仍可用
   │  ├ .wp-group-panel { overflow:auto }                       ← 左栏自滚（去 sticky）
   │  ├ .wp-mid { display:flex; flex-direction:column; min-height:0; overflow:auto }
   │  │   ├ .wp-picker（48px）
   │  │   ├ Notice（可选）
   │  │   ├ .wp-viewport { flex:1 1 auto; min-height:400px（≤1300px 时 340px）; display:flex; flex-direction:column }
   │  │   │   ├ tools / params（flex-shrink:0）
   │  │   │   ├ .wp-teach-page .wp-gray-image { flex:1; min-height:0; height:0 }   ← 照 cycle.css 36–37 行 .wp-runtime-frame 的写法，示教页作用域；不去 aspectRatio
   │  │   │   └ footer（flex-shrink:0）
   │  │   └ TrialResult <details>（默认收起；展开后 .wp-mid 滚动）
   │  └ .wp-teach-settings { overflow:auto }                    ← 右栏自滚，高度 = 网格行高 = 中栏高
   └ .wp-teach-dock（flex-shrink:0；普通流，不 sticky）
@media(max-width:820px)：撤销整条高度链（main-body 恢复块级滚动、.wp-teach 单列、画布回到 clamp 高度），底栏按钮允许换行。
```

- `GrayViewer` 的 fit 逻辑已按 clientWidth/clientHeight 计算，容器变成 `flex:1; height:0` 后自动 letterbox，不需要改组件高度逻辑；只加 `controlsAt="footer"`/`summary` prop。
- 画布模式：UI 是一个三态 `SegmentedControl`（编辑点 / 沿胶路刷选 / 矩形框选），但状态上不能用它替代 `selectionTool`——保留 `lastSelectionTool`：`extract` 在编辑点模式也要用上次 ROI；参数行“笔刷宽度”按 `lastSelectionTool === "brush"` 显示而不是按当前模式。切到编辑点 = `regionMode=false`；切到刷选/框选 = `regionMode=true` + 设 `selectionTool`。
- 禁用按钮的 Tooltip：qing-steel `Tooltip` 是 Radix `Trigger asChild`，disabled 元素不发 pointer 事件，统一写法 `<Tooltip content=…><span tabIndex={0} className="wp-tip-anchor"><Button disabled …/></span></Tooltip>`；有原因才包，无原因直接渲染 Button。`SegmentedControl` 单项不能包 Tooltip，分段项用 `title` 或常显文字。
- 左栏两三字状态（已示教 / 待试测 / 未配置 / 待选图 / 需重采 / 需重选图 / 试测通过）需要在 `teachingGroups` 之外新增一个短文案映射，不改 `viewStateText`（其它页仍用长文案）。
- 底栏主按钮判定沿用现有 `primary = strokeAdvised ? "stroke" : !hasPath ? "extract" : !trialCurrent ? "trial" : "save"`；底栏只关心 `trial / save`，`extract / stroke` 作用在工具行按钮。
- 画布“候选”标签沿法向往胶路外侧偏移，不压在胶路上；文字加深色描边（`paint-order: stroke`）。
- g 的“重新采集”链接在生产中：用 `aria-disabled="true"` + 阻止跳转 + title 原因，或保持可点只加锁提示——二选一，不要 `disabled` 掉 `<a>`。
- Steps 进度：`Progress` 无 className，外包 `<span className="wp-step-progress">` 做绝对定位与 3px 高。
- 窄屏：保留 `@media(max-width:1250px)`，但右栏不再掉到下方；改为三栏列宽 `148px 1fr 220px`，右栏仍在网格内自滚。1280 本身 >1250，用 `@media(max-width:1300px)` 承载“分段控件两字文案 / 缩略图只显示相机名 / 底栏 sm / 画布容器 340px / 信息行隐藏耗时”。

**风险**

1. `GrayViewer` 的 footer/Chip 改动通过 `controlsAt="footer"` prop 隔离，默认路径不变；但 `.wp-gray-image` 的示教页作用域高度规则与 cycle.css 的 `.wp-runtime-frame` 规则同名同属性，注意选择器特异性顺序，并回归 CapturePage / OverviewPage / RuntimeFrame / StationCapture / 相机页。
2. 中栏自滚 + 右栏自滚两条滚动条并存，鼠标滚轮落点决定滚哪一列；若现场反馈不适，退回“整页滚 + 底栏 sticky”方案（见 3.5-1）。
3. 工具行在 1440 下单行是按当前文案宽度算的，文案一改就可能换行；实现时给 `.wp-canvas-tools` 加 `container-type` 并在容器 <700px 时切图标模式，而不是只靠 1300 媒体查询。
4. Steps 为多页共用，第 3 步加进度条要确认其它页（总览、验证）看着不突兀；可只在 `pathname === "/recipe/teach"` 时渲染。

**待用户拍板（≤5）**

1. “整页宽”口径：底栏贯穿内容区三栏、不含应用左侧导航栏（审查者口径），是否确认？
2. 逐点表展开后是“中栏内部滚动、底栏固定”（v3）还是“整页滚动、底栏 sticky”？
3. 最后一组：“保存并进入下一组”禁用 + Tooltip（v3），还是改为“保存并前往验证与发布”？（工件总览是可选步骤）
4. 标定“当量已修改 / 复用适用条件 / 样本集声明”三段文案保持现状位置（右栏卡内），还是把“适用条件”改成 ⓘ Tooltip 以缩短右栏？
5. 1280×800 是否为真实目标分辨率（现场触摸屏？）——v3 已按审查推荐把 1280 下分段控件改为常显两字“编辑 / 刷选 / 框选”；若有触摸屏，所有 Tooltip 原因还需要有点击可见的替代（如底栏提示常驻）。

### 3.7 v3 审查意见（独立审查，要点）

结论：修完 4 条即可进入实现，不再出新版。

1. i 多选告警画在缩略图同一行，cam2/cam3 看不见、cam1 被截——告警换行放到选图条下方，缩略图行保持完整。
2. a/c 与代码不符：“中线已提取，请检查后运行试测”只在刚提取后出现，此时 `setFrameDraft` 使 `editing=true`（第 184、123 行）→ 徽章应为 warn“中线已修改 · 需试测”，组列表与选图条禁用（第 266、271 行）；a/c 是最高频状态，务必按代码画。
3. README 三处失实：Steps 的“4 / 12 组”不是现有文案（`stageStatusText` 只有“进行中”），需写明是共用 Steps 的改动并用 `teachingTasks` 算组数；b 的 Popover 实际盖住工具行且弹层文字溢出截断；c2 并非“全部首屏可见”——1280 下逐点表摘要行被切、图例丢失。
4. 需改文件补全：GrayViewer 其他使用方（CapturePage、OverviewPage、RuntimeFrame、StationCapture、camera.css）→ 加 prop 只在示教页启用；测试 `tests/image-viewer.test.tsx`、`tests/teaching-flow.test.tsx`、`tests/teaching.test.tsx` 受文案影响；可变文案按钮加固定 aria-label。

实现注意事项（已并入 3.6）：不去 aspectRatio，照 cycle.css `.wp-runtime-frame .wp-gray-image{flex:1;min-height:0;height:0}` 限定示教页作用域；用 `.main-body:has(.wp-teach-page)` 而不是 `.wp-page{height:100%}`；覆盖 `.wp-page > *{flex-shrink:0}`；`.wp-teach` 最低 360px；≤820px 撤销高度链；三态 mode 不替代 `selectionTool`，保留 `lastSelectionTool`；Progress 包 span；SegmentedControl 单项不包 Tooltip；左栏短状态新增映射；“候选”标签沿法向外偏；g 的链接用 aria-disabled 或保持可点。

审查者对待拍板 5 条的推荐：1 确认“内容区三栏、不含导航栏”；2 中栏内滚 + 底栏固定（v3）；3 禁用 + Tooltip（v3），不改“前往验证与发布”；4 三段文案留原位，不改 Tooltip（触摸屏 / 可读性）；5 1280 下分段控件常显两字“编辑 / 刷选 / 框选”，不做纯图标。

### 3.8 修订记录（v3 审查后，直接改 v3）

| 项 | 改动 | 受影响文件 |
| --- | --- | --- |
| 审 1 | 多选红字告警移出选图条、单独一行放在其下，缩略图行完整 | `v3/i-multi-selected.*`、`teaching-design.css`（`.wp-picker-warn`） |
| 审 2 | a / c 改为“刚提取后”状态：warn 徽章“中线已修改 · 需试测”、组列表与选图条禁用、底栏提示改写；参数行保留“中线已提取，请检查后运行试测” | `v3/a-teaching-draft.*`、`v3/c-narrow-1280.*` |
| 审 3-Steps | README 3.1-16 改写：说明是共用 Steps 改动，组数用 `teachingTasks`；Progress 包 span | README |
| 审 3-Popover | Popover 移出中栏（挂 `.wp-teach` 右上），不盖工具行；修正 `.c-warn` nowrap 误伤导致的文字截断 | `v3/b-trial-passed.*`、`teaching-design.css` |
| 审 3-c2 | 1280 下画布容器 340px，信息行隐藏“· 212 ms”，图例移入逐点表正文；摘要行与底栏均在首屏 | `v3/c-narrow-1280.*`、`v3/c2-narrow-1280-passed.*`、`teaching-design.css` |
| 审 4 | 需改文件表补 GrayViewer 其他使用方、三份测试、固定 aria-label；实现要点并入审查者注意事项 | README 3.6 |
| 拍板 5 | 1280 分段控件改常显两字“编辑 / 刷选 / 框选” | `v3/c*.html`、`teaching-design.css` |
