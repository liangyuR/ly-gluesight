# lyFlow 只提供算子，流程与判定归 GlueSight（草案）

状态：P1–P3 的 GlueSight 侧已实施；P3 依赖的 lyFlow 1.2 还没发版，`packaging/runtime.json` 的系列与 `Cargo.toml` 的 `lyflow-client` 仍锁 1.1，只能联合开发本地构建运行（见“分期”）。不迁移旧配方。采纳后修订 [algorithm.md](algorithm.md) A-6（“误报修在 lyFlow，不在 gluesight 调参数”），并把 [segment-burst.md](segment-burst.md) D-SB-47 里的定向判定、回退改由 GlueSight 实现。

## 问题

现在 GlueSight 用 Rust 代码拼测量图（`vision/taught.rs` 的 `bead_graph`、`vision/heading.rs` 的 `plan_heading`），只用了 `glue.taught_path`、`glue.bead_path`、`glue.bead_width`、`glue.edge_distance` 几个大算子。业务判定被分成两半，两边都改不动：

- **判定埋在 lyFlow 里。** `bead_path` 自己决定扇形搜索取几个峰、覆盖率与陡度够不够、算不算找到胶；`bead_width` 自己决定暗段怎么拆分合并、参考宽怎么取、哪一站有胶。GlueSight 只拿到结论。
- **门限写死在 GlueSight 里。** `bead_width` 的 `contrastMin`、`mergeGap`、`presentRatio`、`centerRatio`、`contrastRatio`，`taught_path` 的 `sharpMin` 都是代码常量；`bead_path` 的 `minCoverage`、`sharpMin`、`maxGap` 不传，用 lyFlow 默认值。界面只开放检测参数（搜索半宽、极性、胶宽范围）、站距、像素当量、检测窗口、出胶点、方向容差和找边参数。

实际后果（2026-10-29，seg-new14 cam2 段 2，lyFlow 1.1.6）：

- 示教帧定向回退，原因只在 `bead_path` 的 `info.reason` 里（“暗带边缘太缓，不像胶”）。现在 GlueSight 已记下这条原因并在试测里显示，但覆盖率、陡度和各候选方向的得分仍然看不到。
- 连续的胶被逐站判成无胶（31 站丢 10 站）。原因是拟合胶路调用 `lateralRuns` 时不按半高拆分，胶条和旁边的灰暗区连成一段、超宽被丢。只改 `contrastMin` 就能从 21/31 恢复到 31/31，但这个参数改不了，算子内部的拆分方式也改不了，只能等 lyFlow 发版。

lyFlow 本身的设计是节点图平台：GraphDoc 是统一的图文档，支持顶层图参数和参数配方，宿主按次通过 `params_json` 传参，算子参数界面可由 `OperatorManifest` 生成。GlueSight 没有用上这些能力。

## LF-1 分工

- **lyFlow**：提供确定性的图像算子和图执行。算子只做测量，所有门限都是参数，输出完整的中间结果，不替业务下结论。
- **GlueSight**：负责流程编排（用哪些算子、怎么连）、业务判定（有无胶、像不像胶、方向回退、取哪一段）、参数的来源与分层，以及发布冻结。

## LF-2 算子粒度

- 按“一步图像处理、一次处理整幅图或整条胶路”切分。逐站、逐像素的循环留在算子里，不放进图里，也不在 FFI 两侧来回传大数组。
- 算子（lyFlow 1.2，契约见 lyFlow 仓库 `docs/glue-measure-ops.md`）：
  1. `glue.response` 响应图：黑顶帽 / 白顶帽，参数为结构元大小，可只算一块；
  2. `glue.fan_scan` 扇形扫描：给出锚点、扇区、径向区间，输出每个方向的得分与前几个峰；
  3. `glue.track_path` 粗跟踪与拟合：沿给定方向找胶点并拟合胶路，输出胶点、覆盖率、边缘陡度，不判 ok；
  4. `glue.lateral_runs` 横向卡尺：沿胶路布站，输出每站的全部暗段（边、宽、峰值、陡度），拆分方式与分段门限是参数，不做挑选；
  5. `glue.part_edges` 找零件边：每站两侧的全部合格跳变，不选侧、不剔野；
  6. `glue.polyline` 折线胶路：把示教走廊重采样成胶路，接横向卡尺与找边。
- 判定类逻辑不进算子：参考宽、参考峰值、中心偏离门限、像胶判定、回退条件，都由 GlueSight 根据算子输出计算。

## LF-3 判定归 GlueSight

- 有无胶、是否像胶、定向是否回退、每站取哪一段，写成 GlueSight 的领域逻辑，与段配方的其他规则放在一起，按项目测试规则做单元测试。
- 判定规则的变化随 GlueSight 发版，并写进 `docs/`；不再依赖 lyFlow 发版。
- 现有 lyFlow 里的规则（D15 陡度、D16 有无胶、高光合并）先按原样迁移，再按现场数据调整。迁移时用同一批录制帧对照新旧结果。

## LF-4 图作为数据

- 每种测量方式一张 GraphDoc 模板（示教走廊、逐帧定向；找边是两张模板里的节点，不找边时不运行），随 GlueSight 版本管理，不在 Rust 里逐张拼 JSON。
- 模板里会随配方或每帧变化的参数（出胶点、扇区、检测区、走廊点、胶宽范围等）提升为顶层图参数。GlueSight 每次运行把有效值合成后通过 `params_json` 传入，不再为每个（相机, 段）生成一张图。
- 发布时冻结模板、全部有效参数与判定门限，延续 `graph.json` 随发布包冻结的做法；测量图版本 `flow-2`。
- 不迁移旧配方，也不兼容旧发布包：`corridor-2` / `corridor-3` 的测量图和加载路径直接删除，新版本起候选与配方都要重新示教、标定并发布。

## LF-5 参数分层

| 层 | 内容 | 入口 |
|---|---|---|
| 业务参数 | 现有的检测参数、窗口、出胶点、方向容差、找边参数，按 mm 或度填 | 现有界面 |
| 专家参数 | 分段门限、合并缝、覆盖率与陡度门限、有无胶比例等 | 高级面板，按相机覆盖配方默认值；表单可由 `OperatorManifest` 生成 |
| 固定参数 | 与现场无关的实现细节 | 不开放 |

- 专家参数分两类，规格（标签、范围、默认值、说明）都写在测量图模板里，配方默认与相机覆盖逐层叠加，两张模板里同名的规格必须一致：
  - 绑定算子参数的放顶层图参数（`advanced: true`）：横向卡尺的 `segmentMin`、`peakMin`、`mergeGap`，粗跟踪的 `pathSegmentMin`、`pathPeakMin`、`pathMaxGap`、`pathSharpMin`（`pathSharpMin` 同时是选方向的陡度门限）；
  - GlueSight 判定用、不绑定算子的放模板扩展位 `x.gluesight.judge`：`trackSharpMin`、`minCoverage`、`presentRatio`、`centerRatio`、`contrastRatio`。
- 专家参数改动后，发布前检查要求重新试测或跑独立样本验证，并在发布记录里列出改了哪些。
- 默认值与现在写死的值一致，不改变已有配方的结果。

## LF-6 流程调试页

- 候选工作区增加“流程调试”页：选示教件或标定件里的一帧，在本段、本相机的测量图上运行，显示每个节点的输出，并叠加 GlueSight 的判定结果。
- 页面嵌入 lyFlow 的 `@lyflow/editor`。拓扑锁定，只调参数：图模板随 GlueSight 版本发布，页面里的改动存为候选的专家参数，发布前要重新试测或跑样本验证。改拓扑只在开发时进行，现场不开放。
- 编辑器通过 GlueSight 自己实现的 Transport 接到现有 `lyflow-client` 运行库，不引入 lyFlow 的 Rust 宿主（它要从源码编译核心，会把 OpenCV 与 vcpkg 带进 GlueSight 的构建）。
- 锁定拓扑需要编辑器提供只读拓扑的开关，由 lyFlow 侧补上。
- 一个页面只能挂一个编辑器，切换帧、段或相机时卸载后重挂。

## LF-7 验证

- lyFlow 只负责算子自身的单元测试与性能。
- GlueSight 建离线回放评估：用录制帧对照判定结果，`examples/heading_eval.rs` 扩展为通用入口。判定规则和专家参数默认值的修改都要过这一步。
- 每帧耗时仍按现有门槛核对：目前逐帧定向含回退 p50 / p95 约 16 / 31 ms，示教走廊约 11 / 18 ms。拆分算子后单帧总耗时不应明显增加。

## 分期

1. **P1，不改 lyFlow**（已实施）：原先写死的算子门限改为专家参数，配方默认加相机覆盖（`RecipeDoc.expert`、`CameraInspect.expert`），默认值与原来相同；相机视图有“专家参数”卡片。`bead_path`、`bead_width` 其余的诊断输出（覆盖率、陡度、候选方向与每站暗段）还没带到试测界面。眼前的假断胶可以先调“响应下限”缓解：seg-new14 cam2 段 2 帧 4 从 16 调到 32，有胶站从 21/31 到 31/31。
2. **P2**（已实施）：测量图改为 `src-tauri/graphs/` 下的 GraphDoc 模板（`corridor`、`heading`）加顶层图参数，GlueSight 按次通过 params_json 传入全部有效参数；不需要找边时不运行找边节点。
3. **流程调试页**（LF-6，已实施，依赖 `@lyflow/editor` 1.2，发版前按本机打包安装）：选帧、逐节点运行与查看、保存专家参数；判定结果叠加还没做。
4. **P3，配合 lyFlow 1.2 系列**（GlueSight 侧已实施；lyFlow 1.2 未发版，系列锁与 `lyflow-client` 的 tag 发版后再升）：两张模板换成 LF-2 的算子，判定迁到 `vision/judge/`（示教走廊 `taught.rs`、拟合胶路 `fit.rs`、找边 `edge.rs`），规则与 1.1 相同，只有拟合胶路选方向与 θ 的口径有意不同（见 segment-burst D-SB-47）；逐帧定向一张图含搜索与回退两支，一次跑完、先判搜索支；1.1 的专家参数名不迁移；`requiredOperators` 换成实际用到的算子；发布包测量图版本 `flow-2`。
   - 对照（2026-10-29，seg-new14 的 NEW-1、NEW-14 候选在采集轮次 `capture-1793260666636-0` 上的 48 次测量，以同一 1.2 引擎跑 1.1 旧图为基准）：状态、最长断胶、有胶比例、逐站有无胶、是否回退与回退原因全部相同；胶宽与 x 只差 1.1 输出的舍入（≤ 6 × 10⁻⁵ mm），胶边 ≤ 0.001 px；拟合胶路逐点相同，回退后仍没找到胶的 5 帧直线各有 1–2 点差 0.001 px（回退扇区在 1.2 是 f32 参数，起跑方向差约 10⁻⁵°）。找边（三段直线临时设 ToEdge，两种极性，各 22 帧）胶边距有无全部相同，都有值的 73 帧差中位 0.0005 mm、最大 0.28 mm（1.2 剖面采样相位不同，贴着门限的跳变与剔野结果会变）。
   - 耗时（同一批帧，release，冷缓存，p50 / p95）：示教走廊 1.1 图 6.3 / 7.3 ms，新图 6.7 / 8.5 ms；逐帧定向 1.1 图（含回退重跑）10.2 / 14.2 ms，新图搜索与回退两支一次跑完 11.6–12.1 / 17.0–17.4 ms（分阶段只跑需要的分支更慢，没采用）。新图每帧都算两支，1.1 只在搜索失败时重跑回退。lyFlow 的 `glue.response` 设了 roi 时只输出块内，三张图与块位置打包为一个 `maps` 输出，下游只接它；执行器的图像统计也已提速。

## 代价与风险

- 判定逻辑从 lyFlow 搬到 GlueSight，GlueSight 的代码量和验证责任都会增加；lyFlow 现有的 glue 评估工具不能直接复用。
- 专家参数开放后，现场可能调出偏差大的配方；靠发布前的样本验证和改动记录约束。
- 拆分后单帧耗时约为 1.1 的两倍（见“分期”P3）；lyFlow 改掉响应图输出的开销之前，现场吞吐要按新耗时复核。
- P3 依赖 lyFlow 发新系列，两边要约定算子契约和发版节奏。

## 回退

- 不保留旧测量图，回退只能整体回到改动前的 GlueSight 与 lyFlow 1.1 版本，并用当时的发布包。

## 待确认

- 专家参数是否只允许按相机覆盖，还是也允许按段覆盖。
