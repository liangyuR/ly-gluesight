# 第 3 轮：定稿候选 v3 与实施拆分

合并第 1、2 轮全部共识。本稿自成一体。

## 1. 方案一句话

工件特征定位得到位姿 T，把示教胶中线（合格胶路 N）搬到当帧得到 N'；胶照旧在静态示教线走廊里检测；胶中心到 N' 的有向法向距离即胶偏，按 `limits.position` 判定。

## 2. 结构

```text
LyFlow 图（静态）
  n_load → n_path(taught_path, 静态示教线) → n_width(bead_width) → stations、beadInfo
  n_load → n_locate(glue.locate 1.1.0, template = "template.pgm")  → pose

GlueSight（Rust 纯函数，vision 下新模块）
  N' = T(N)
  每个有胶站 i：在 N' 的站 j ∈ [i−k, i+k] 线段上就近投影，k = ⌈searchRadiusPx / stationStepPx⌉
               取 |距离| 最小、且线段法向与 n'_i 夹角 < 45° 的线段
               投影参数不在 (0,1) 内 → ST_NO_REF
  d_i = (B_i − P_i)·n'_i × mmPerPx − nominalOffsetMm
  pose.ok = false → 该视图所有有胶站 ST_NO_REF
```

## 3. 判定与状态

- 测点状态新增 `ST_NO_REF`：有胶、无位置基准；d 为空，胶宽有效。`PointState::Measured { d: Option<f32>, w }`。
- 位置统计只用有值的 d。`ST_NO_REF` 连续长度按 `MAX_INVALID_LEN`（2 mm）容忍；超过或整视图无基准 → 该段位置结果 ErrInspect，`SegmentResult.reference_invalid = true`。
- 合并：先按现有全序取 max；若结果为 ErrInspect 且只由 `reference_invalid` 引起、同时存在任一段 NG，则取该 NG。`Verdict` 顺序不动。
- fault 92 由 judge 在“最终 ERR 来自基准无效”时给出。
- 删除 `Measured.located`、`FrameStatus::LocateFailed` 及 `cycle.rs` 整周期短路；帧增加 `referenceFailed`（定位失败的视图）。
- 可见性：无论最终 NG 还是 ERR，帧记录与 UI 显示“定位失败”徽标；同一拍照点连续 N 件定位失败告警。
- 多视图沿用 `aggregate_views` 现策略（每 Pose 组仍只选一张图）。

## 4. LyFlow：`glue.locate` 1.1.0

- `peakRatio`（主峰邻域外次高峰 / 主峰）与参数 `maxPeakRatio`。
- 峰贴搜索窗边、角度贴扫描边界 → `ok=false`。
- `reason`：`low_score / ambiguous / at_search_edge / at_angle_edge / flat_template / out_of_image`。
- 金字塔视阶段 0 实测 p95 耗时；不做 mask；不改搜索窗形状。
- 合成场景用例：重复结构 → ambiguous；位移超半径 → at_search_edge；现有精度用例保留。

## 5. 配方、模板、发布

- `fixture: Option<FixtureSpec>` 进入 `ShotSpec` / `ShotViewSpec` / `for_view`：`rect`、`searchRadiusPx`、`angleRangeDeg`、`minScore`、`maxPeakRatio`、`nominalOffsetMm`、`gaugeRef`（量具确认件编号，`nominalOffsetMm ≠ 0` 时必填）。
- 模板：保存 fixture 时由冻结示教原图按 `rect` 裁出 `template.pgm`；发布包放在 `shots/kk/vN/`，加载时按配方重算比对。
- `base_dir`：图内固定写相对路径 `template.pgm`；工作台指向 workspace 图目录，生产指向发布包 shot 目录；有 fixture 而 base_dir 为空时 `plan` 报错。
- 走廊 `search_mm` 逐点：按阶段 0 的 σ_B 取 `max(3σ_B, 胶宽上限/2 + 2 px)`；σ_B 超阈值的点关闭位置判定。
- 版本：`RECIPE_SCHEMA` 7、`RELEASE_SCHEMA` 3、`GRAPH_VERSION` 新值、`POINTS_FORMAT` 3；`requiredOperators` 加 `glue.locate`。D-0：不兼容旧数据。
- `measurement_layout` 纳入 fixture。

## 6. 示教与验证

- 示教页“框选定位特征”：叠加模板框、搜索范围、杠杆臂 L。
- 保存检查：模板与走廊不重叠；模板标准差下限；示教图 `peakRatio` 合格；L 超阈值提示。
- 规则：优先与胶同面、刚性、不被喷嘴遮挡的孔或角点。
- 定位试测：在样本库其他图上跑定位，显示位姿与 d。
- 试测标签、草稿比较、前端 `sameTeach` 纳入 fixture。
- 验证：独立合格样本全部定位成功；位移/角度超过搜索范围 70% 告警；d 的 σ ≤ 位置公差 1/5；|μ| > 公差 1/3 告警（不自动校正）。

## 7. 存储、历史、叠图

- 位姿写入 `FrameView` JSON（不改表结构）；audit 的 `d_bits` 改为可空。
- 叠图（示教、生产、历史）按位姿画 N' 与模板框。

## 8. 实施拆分

按记忆中的教训，所有 PR 都从 main 拉，避免叠 PR；有依赖的等前一个合入后再开。

| 编号 | 仓库 | 内容 | 依赖 | 验收 |
| --- | --- | --- | --- | --- |
| P0 | GlueSight | 阶段 0：采集规程文档；偏差纯函数模块（含就近投影）及单测；离线评测工具两段运行（先 locate 1.0.0，再测量）输出逐点统计 | 无 | 现场数据报告：σ_B、法向/切向分解、score/peakRatio、d 重复性、偏移线性度、投影端点频率、耗时 |
| L1 | LyFlow | locate 1.1.0 质量门与用例；发布运行库 | 无（可与 P0 并行） | 合成场景用例通过；运行库 Release |
| G1 | GlueSight | 状态模型：`ST_NO_REF`、`d: Option`、`reference_invalid`、NG 优先、fault 92、删除 `LocateFailed`/`located`、`POINTS_FORMAT` 3、audit、前端帧状态与徽标 | 无（与 fixture 无关，可先做） | judge/store/audit 回归测试；模拟器场景改为产出 `ST_NO_REF` |
| G2 | GlueSight | 配方 `fixture`、`RECIPE_SCHEMA` 7、图插入 `n_locate`、base_dir、模板写出、`parse_result` 接偏差函数、`requiredOperators`、`GRAPH_VERSION`、`measurement_layout` | P0 的偏差模块、L1、G1 | 合成图集成测试：共同移动 d 不变、只移胶 d 变化、定位失败 ERR 92 |
| G3 | GlueSight | 发布包模板、`RELEASE_SCHEMA` 3、加载校验、生产 base_dir | G2 | 发布—加载—生产测量端到端 |
| G4 | GlueSight | 示教页定位特征步骤、保存检查、定位试测、验证统计、名义偏置与量具编号 | G2 | 前端流程测试；试测失效规则测试 |
| G5 | GlueSight | 叠图按位姿绘制 N'、连续定位失败告警 | G1、G2 | 历史/生产页显示测试 |
| F | 现场 | 小批量对照 | G3、G4 | 与量具/人工判定一致率达标 |

## 9. 测试（只列防回归）

| 条件 → 预期 | 保护的规则 |
| --- | --- |
| 工件与胶共同刚体平移/旋转 → d 不变 | 共同运动不算胶偏 |
| 工件沿弯道切向平移、胶不动 → d 为正确法向距离 | 就近投影对应正确 |
| 拐角处投影 → 不跳到另一条腿 | 法向夹角门 |
| 只移胶 → d 等于位移法向分量 | 真实胶偏检出 |
| 定位失败 + 断口 → NgGap，带定位失败徽标 | NG 优先、失败可见 |
| 定位失败、胶正常 → ErrInspect，fault 92 | 定位失败不放行 |
| `ST_NO_REF` 点 BLOB/audit 往返 → 仍为无基准 | 不被当作 d=NaN 静默放行 |
| 修改 fixture → 旧试测与验证失效 | 不复用旧结论 |
| 有 fixture、base_dir 为空 → plan 报错 | 模板路径不会悄悄失效 |
| locate：重复结构 → ambiguous；超半径 → at_search_edge | 歧义与越界不当作成功 |

## 10. 请评审做最终检查

1. 实施拆分的依赖与顺序是否合理，G1 先行是否有风险。
2. 有无遗漏的影响面（模拟器、离线评测、复测/重现、PLC 协议、打包脚本）。
3. 还有哪些点必须由用户拍板。

---

## 评审意见（评审方，第 3 轮）

核实：`recipe.rs:20-35`、`vision.rs:89-92`、`packaging/runtime.json:13`、`tests/packaging/Test-LyFlowDependency.ps1:7,152,214`、`scripts/{LyFlow-Dependency,Prepare-RuntimeDependencies,Test-PackagedRuntime}.ps1`、`offline/scenario.rs:34`、`offline/mod.rs:665`、`measure.rs:346`、`audit.rs:104`、`store.rs:505,810`、`eval.rs:206,710`、`packaging.rs:59-68`、前端 `cycle/types.ts`、`offline.ts:8`、`plc-communication-protocol.md:186`、`ui-shell.md:91`。

**认同**：结构、状态模型、就近投影约束、三条分歧处理；全部从 main 拉 PR；P0 与 L1、G1 并行。

**必须修改**

- **M1 `nominalOffsetMm` 与 `JudgeParams.nominal` 重复。** 放在 fixture 并纳入 `measurement_layout`，改名义偏置就成了改测量定义，历史只能原图重测，而它本质是判定参数。→ 删除 `FixtureSpec.nominalOffsetMm`，d 存原始几何距离，名义走 `limits.position.nominal`，`gaugeRef` 放 `ShotLimits`（`nominal ≠ 0` 时必填）。改名义 → 按相同测量定义重判；改 fixture → 重测。
- **M2 `requiredOperators` 有 ≥6 处手工维护。** → G2 列全并加“Rust 列表与 `runtime.json` 一致”的测试（或 Rust 直接读 `runtime.json`）；G2 的依赖改为“L1 合入且运行库已发布、`Prepare-RuntimeDependencies` 能拉到”。
- **M3 G1 漏了离线场景与审计 spool。** `offline/scenario.rs:34` 的 `LocateFail` 场景、`offline/mod.rs:665`、模拟器、前端 `Scenario` 类型与文案、CDP 流程测试；`audit.rs:104` 的 `Point` 是 spool 磁盘格式。→ `LocateFail` 改为“第 4 帧全视图 `ST_NO_REF`、胶正常 → ERR 92”，CDP 断言同步；旧格式 spool 启动时按 D-0 明确拒绝并提示清理，加恢复用例。
- **M4 base_dir 两处评测/自检漏项。** `eval.rs:206,710`、`packaging.rs:59-68`。→ P0 离线工具与 eval 以样本目录为 base_dir；G3 在打包自检加 locate 冒烟，`Test-PackagedRuntime.ps1` 对应断言。

**补充**：复测/重现只用包内 `template.pgm`，不重裁；PLC 协议无改动，92 只改语义说明；G1 合入后 `POINTS_FORMAT` 与 audit 格式会随现场打包进入现场，建议 G1 之后不单独出现场包，等 G3。

**结论：可定稿**（吸收 M1–M4）。

**需要用户拍板**：① 名义偏置放 `limits.position.nominal` 还是 fixture；② G1 合入后是否暂停现场打包直到 G3；③ `requiredOperators` 是否以 `runtime.json` 为唯一来源；④ 阶段 0 数据找回还是重采、谁负责、何时完成；⑤ 连续 N 件定位失败告警的 N，以及 v1 是否上 PLC 故障码。

---

## 起草方回应（第 3 轮）

M1–M4 全部接受。M1 是结构性简化：名义偏置本来就是判定参数，放进 `limits.position.nominal` 后，P3 的“不自动校正、人填并附量具件编号”约束不变，历史重判也更合理。

讨论在第 3 轮收敛，定稿见 [final.md](final.md)，用户待决事项附推荐。
