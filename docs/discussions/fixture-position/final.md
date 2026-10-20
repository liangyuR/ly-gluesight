# 定稿：特征定位 + 示教胶路判胶偏

讨论 3 轮收敛（[第 1 轮](round-1.md) · [第 2 轮](round-2.md) · [第 3 轮](round-3.md)）。起草方与评审方对下文无分歧；第 8 节的待定事项已由用户于 2026-10-20 拍板，第 9 节说明阶段 0 数据能证明什么、不能证明什么。

## 1. 方案

工件特征定位得到位姿 T，把示教胶中线（合格胶路 N）搬到当帧得到 N'；胶照旧在静态示教线走廊里检测；胶中心到 N' 的有向法向距离即胶偏，按 `limits.position` 判定。

胶检测走廊不随位姿，位置基准随工件。这一拆分不依赖任何关于“胶在画面里跟不跟工件走”的现场结论（MX11 数据已判定不可信，不作依据，见 §8）：

- d = B − T(N) 的定义就是“胶相对工件上合格位置偏了多少”。胶随工件动时，B 与 T(N) 一起动，d 不变；胶不随工件动时，d 如实给出胶相对工件的偏移。两种情形下 d 都正确。
- 走廊保持静态，沿用已验证的胶检测与解析契约（`glue.taught_path` / `glue.bead_width`、`parse_result` 的站数、弧长、法向校验），不引入“走廊随位姿重建”的新失效面。前提是胶落在静态走廊内，走廊宽度见 §5、§9。

## 2. 结构

```text
LyFlow 图（静态，发布包“图一致”校验不变）
  n_load → n_path(taught_path, 静态示教线) → n_width(bead_width) → stations、beadInfo
  n_load → n_locate(glue.locate 1.1.0, template = "template.pgm") → pose

GlueSight（Rust 纯函数）
  N' = T(N)
  每个有胶站 i：在 N' 的站 j ∈ [i−k, i+k] 线段上就近投影，k = ⌈searchRadiusPx / stationStepPx⌉
               取 |距离| 最小、且线段法向与 n'_i 夹角 < 45° 的线段（挡拐角）
               投影参数不在 [0,1] 内 → 该线段不算；弯道外侧落在相邻两段之间楔形区、两段都合格 → 按共同顶点距离
               越出 N' 两端或窗口两端、找不到合格线段 → ST_NO_REF
  d_i = B_i 到所选线段的有向垂直距离 × mmPerPx，正向取 n'_i 一侧（原始几何距离，名义值在判定里扣）
        即 (B_i − P_i)·m_j，m_j 为线段法向；与 (B_i − P_i)·n'_i 只差两法向夹角的余弦（门限内 ≥ 0.71）
  pose.ok = false → 该视图所有有胶站 ST_NO_REF
```

## 3. 判定与状态

- 新测点状态 `ST_NO_REF`：有胶、无位置基准，胶宽有效。`PointState::Measured { d: Option<f32>, w }`。
- 位置统计只用有值的 d；`ST_NO_REF` 连续长度按 `MAX_INVALID_LEN`（2 mm）容忍；超过或整视图无基准 → 该段位置 ErrInspect，`SegmentResult.reference_invalid = true`。
- 合并：按现有全序取 max；若 ERR 只由基准无效引起、且存在任一段 NG，则取该 NG。`Verdict` 顺序不动。
- fault 92 由 judge 给出；删除 `Measured.located`、`FrameStatus::LocateFailed` 和 `cycle.rs` 整周期短路；帧增加 `referenceFailed`。
- 定位失败始终可见：帧记录与 UI 徽标；同一拍照点连续 N 件定位失败告警。
- 名义值：`limits.position.nominal`（默认 0）。验证阶段统计合格样本 d 的 μ/σ 只做诊断，不自动校正；`nominal ≠ 0` 时必须填写量具确认件编号 `gaugeRef`。改名义按相同测量定义重判历史；改 fixture 需原图重测。
- PLC 协议无改动，fault 92 只更新语义说明。

## 4. LyFlow：`glue.locate` 1.1.0

- 输出 `peakRatio`，参数 `maxPeakRatio`；峰贴搜索窗边、角度贴扫描边界 → `ok=false`。
- `reason`：`low_score / ambiguous / at_search_edge / at_angle_edge / flat_template / out_of_image`。
- 金字塔视阶段 0 实测 p95 耗时再定；不做 mask；不改搜索窗形状。

## 5. GlueSight

- **配方**：`fixture: Option<FixtureSpec>`（`rect`、`searchRadiusPx`、`angleRangeDeg`、`minScore`、`maxPeakRatio`）进入 `ShotSpec` / `ShotViewSpec` / `for_view` 及其全部同步点；`gaugeRef` 进 `ShotLimits`。`RECIPE_SCHEMA` 7；`measurement_layout` 纳入 fixture。
- **模板**：保存 fixture 时由冻结示教原图按 `rect` 裁出 `template.pgm`；发布包放 `shots/kk/vN/`，加载时按配方重算比对；复测/重现只用包内模板。
- **base_dir**：图内固定写 `template.pgm`；工作台指向 workspace 图目录，生产指向发布包 shot 目录，评测/离线工具指向样本目录；有 fixture 且 base_dir 为空时 `plan` 报错。
- **走廊**：`search_mm` 逐点按阶段 0 的 σ_B 取 `max(3σ_B, 胶宽上限/2 + 2 px)`；σ_B 超阈值的点关闭位置判定。
- **示教**：框选定位特征（叠加模板框、搜索范围、杠杆臂 L）；保存检查模板与走廊不重叠、纹理、`peakRatio`、L；优先与胶同面、刚性、不被喷嘴遮挡的孔或角点；在样本库其他图上做定位试测；试测标签与草稿比较纳入 fixture。
- **验证**：独立合格样本全部定位成功；位姿超出搜索范围 70% 告警；d 的 σ ≤ 位置公差 1/5；|μ − nominal| > 公差 1/3 告警。
- **存储/叠图**：位姿写入 `FrameView` JSON；`POINTS_FORMAT` 3；audit `d_bits` 可空，旧格式 spool 按 D-0 拒绝并提示清理；示教、生产、历史叠图按位姿画 N' 与模板框。
- **版本与依赖**：`RELEASE_SCHEMA` 3、`GRAPH_VERSION` 新值；`requiredOperators` 加 `glue.locate`，并加“Rust 列表与 `runtime.json` 一致”的测试；打包自检加 locate 冒烟。

## 6. 实施拆分

所有 PR 从 main 拉，依赖合入后再开。

| 编号 | 仓库 | 内容 | 依赖 | 验收 |
| --- | --- | --- | --- | --- |
| P0 | GlueSight | 偏差纯函数模块（含就近投影）与单测；离线评测在 2026-10-17 样本集上两段运行（先 locate，再测量；base_dir 为评测输出下的模板目录）输出逐点统计 | 无 | 样本集评测报告：模板候选与 score/peakRatio、定位精度、共同运动下 d 不变性、合成只移胶的检出、无基准站比例、耗时、复拍对比；σ_B、实物偏移线性度、视差留待后续采集（§9） |
| L1 | LyFlow | locate 1.1.0 质量门与合成场景用例；发布运行库 | 无 | 用例通过；Release 可被 `Prepare-RuntimeDependencies` 拉取 |
| G1 | GlueSight | 状态模型（`ST_NO_REF`、`d: Option`、`reference_invalid`、NG 优先、fault 92、删除 `LocateFailed`/`located`）、`POINTS_FORMAT` 3、audit spool、离线 `LocateFail` 场景与 CDP 断言、前端帧状态与徽标 | 无 | judge/store/audit/恢复回归测试；离线场景“全视图 `ST_NO_REF`、胶正常 → ERR 92” |
| G2 | GlueSight | 配方 fixture、图插入 `n_locate`、base_dir、模板写出、`parse_result` 接偏差模块、`requiredOperators` 全部维护点与一致性测试、`GRAPH_VERSION`、`measurement_layout` | P0 偏差模块、L1 运行库已发布、G1 | 合成图集成测试：共同移动 d 不变、只移胶 d 变化、定位失败 ERR 92 |
| G3 | GlueSight | 发布包模板、`RELEASE_SCHEMA` 3、加载校验、生产 base_dir、打包自检 locate 冒烟 | G2 | 发布—加载—生产测量端到端；`Test-PackagedRuntime.ps1` 断言 |
| G4 | GlueSight | 示教页定位特征步骤、保存检查、定位试测、验证统计、`gaugeRef` | G2 | 前端流程测试；试测失效测试 |
| G5 | GlueSight | 叠图按位姿绘制、连续定位失败告警 | G1、G2 | 生产/历史页显示测试 |
| F | 现场 | 小批量对照 | G3、G4 | 与量具/人工判定一致率达标 |

P0、L1、G1 可并行。

## 7. 测试（只列防回归）

| 条件 → 预期 | 保护的规则 |
| --- | --- |
| 工件与胶共同刚体平移/旋转 → d 不变 | 共同运动不算胶偏 |
| 工件沿弯道切向平移、胶不动 → d 为正确法向距离 | 就近投影对应正确 |
| 拐角处投影 → 不跳到另一条腿 | 法向夹角门 |
| 只移胶 → d 等于位移法向分量 | 真实胶偏检出 |
| 定位失败 + 断口 → NgGap，带定位失败徽标 | NG 优先、失败可见 |
| 定位失败、胶正常 → ErrInspect，fault 92 | 定位失败不放行 |
| `ST_NO_REF` 点经 BLOB/audit 往返 → 仍为无基准 | 不被当作 NaN 静默放行 |
| 修改 fixture → 旧试测与验证失效；只改名义 → 历史可重判 | 测量定义与判定参数分开 |
| 有 fixture、base_dir 为空 → plan 报错 | 模板路径不会悄悄失效 |
| Rust `requiredOperators` 与 `runtime.json` 不一致 → 测试失败 | 运行库清单单一口径 |
| locate：重复结构 → ambiguous；超半径 → at_search_edge | 歧义与越界不当作成功 |

## 8. 用户决定（2026-10-20）

用户意见：第 4 项改用已有样本集，MX11 数据不可信、忽略；其余按推荐执行。

| # | 事项 | 决定 |
| --- | --- | --- |
| 1 | 名义偏置放 `limits.position.nominal` 还是 fixture | **放 `limits.position.nominal`**（按推荐）。它是判定参数，改名义可直接重判历史 |
| 2 | G1 合入后是否暂停现场打包直到 G3 | **暂停**，现场修复走 G1 之前的分支（按推荐） |
| 3 | `requiredOperators` 是否以 `runtime.json` 为唯一来源 | **是**，Rust 编译期读取 `runtime.json`（按推荐） |
| 4 | 阶段 0 数据 | **不重采，先用已有样本集** `gs-20261017-normal`（`C:/Users/XYZ/Documents/gs/2026-10-17-normal-sampleset`，只读；1 件、12 点位 × 3 相机，S034–S036 为 S001–S003 的复拍；`mmPerPx = null`，沿用评测的模拟当量，决策 A-3）。**MX11 数据不可信，忽略**，不作为任何结论的依据。样本集测不到的项见 §9，留待后续采集，负责人与时间届时再定 |
| 5 | 连续定位失败告警的 N；v1 是否上 PLC 故障码 | **N = 3；v1 只进 UI/日志**（按推荐） |
| 6 | 是否立即开工 P0 / L1 / G1 | **先开 P0 与 L1**，G1 随后（按推荐） |

## 9. 数据与局限

阶段 0 只有 2026-10-17 样本集（1 件、单圈、未标定）。它能测的：

| 能测 | 方法 |
| --- | --- |
| 模板候选能否找到：纹理、与走廊不重叠、杠杆臂 L、自匹配分数 | 每张图按规则自动提议候选框，在同图上跑 `glue.locate` |
| 定位精度 | 整图刚体扰动（平移、旋转、缩放）单独及叠加成像扰动（噪声、亮度、模糊），真值由扰动定义给出 |
| 共同运动下 d 不变 | 同一刚体扰动下静态走廊测胶，再用位姿算 d，对比原图 |
| 只移胶的检出 | 评测已有的合成偏移缺陷（1/3/6 mm）：模板区不动，位姿应为恒等 |
| 定位耗时 | 单线程逐张计时 |
| 复拍对比（3 对） | S001/S034、S002/S035、S003/S036：用原图模板定位复拍图，比较静态 d 与定位后 d 的差 |

它测不了的：

| 测不了 | 原因与后续 |
| --- | --- |
| 件间胶位抖动 σ_B | 只有 1 件；3 对复拍不构成分布。§5 逐点 `search_mm` 暂不按 σ_B 设定，沿用默认值 |
| 已知物理偏移的线性度 | 没有 0.5/1/2 mm 实物偏移样本，只能用合成偏移代替 |
| 视差与高度：特征与胶不同面、工件倾斜 | 合成扰动都是整图平面变换，测不到 Z 向变化 |
| 真实 mm 刻度 | 未标定，mm 值来自模拟当量（A-3） |
| 模板框里是否有喷嘴或胶枪 | 随枪相机下喷嘴跟相机走，框进去会让位姿恒为恒等；工具判断不了，每个选中的框都要人工复核 |

后续采集至少需要：多件、同一件重复上下件 ≥ 10 次、已知偏移样本（0.5/1/2 mm）、标定后的当量。

阶段 0 评测结果见 [算法评测报告 §9](../../testing/algorithm-evaluation.md#9-特征定位评测fixture-阶段p0)。其中三条影响后续实施：

- **L1**：glue.locate 1.0.0 在有噪声的图上估出 0.3–0.7° 的假角度（φ = 0 不经插值、其他角度经双线性插值），三对复拍图的 d' 因此比静态 d 还抖；只找平移时 d' 重复性优于静态。locate 1.1.0 需让各角度走同一插值路径，并加对应用例。整图过曝时有错锁仍报成功，`at_angle_edge` 与 `peakRatio` 门要覆盖。
- **G1**：共同运动带切向分量时，胶路两端约一个切向位移长度的站投影越出 N'（无基准），按 §3 的 2 mm 容忍会报 ERR（平移 2.9 mm 时 21/36 张）。需在 G1 前决定这些站按“超出检测范围”处理还是示教时两端留余量。
- **G4**：自动提议的框会落在离焦前景件、镜面高光或喷嘴边上；示教保存检查需拒绝高光占比高的框，并要求人工确认不含喷嘴。
