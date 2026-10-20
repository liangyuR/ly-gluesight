# 第 1 轮：特征定位 + 示教胶路判胶偏（初稿 v1）

前提：不急于上线；追求稳定、实用；LyFlow 与 GlueSight 结构允许大改；遵循 D-0（不兼容旧数据）。

## 0. 起点事实

- `glue.locate`（LyFlow 5cefe3c）已有：全分辨率 NCC，角度粗扫 + 细扫 + 抛物线插值；无金字塔、无次峰/歧义检查、无 mask；`ok=false` 作为值输出。
- `glue.taught_path` 无输入端口；只有 `glue.station_calipers` 消费 pose。
- 现场教训（LyFlow `docs/glue-integration.md:79`、GlueSight `p0-plan.md` 步 L）：MX11 上“按零件模板配准更差——胶粘在喷嘴后面，不随零件刚体移动”。相机装在胶枪上，刚出的胶在画面里跟着喷嘴走，工件在画面里移动。
- GlueSight 已有 `LOCATE_FAILED = 92` 与 `FrameStatus::LocateFailed` 通路，但真实 LyFlow 链路从未置 `located=false`。

## 1. 核心设计：胶检测与位置基准解耦

上面的现场教训说明：**把胶的搜索走廊随工件位姿移动是错的**，胶在画面里的位置由喷嘴决定。但**位置判定的基准必须随工件**。因此拆成两条独立链路：

```text
                 ┌─ 胶检测（图像/喷嘴坐标）：静态示教线 → bead_width（含偏移轨迹）→ 胶两边、胶中心 B_i、胶宽
当前帧图像 ──────┤
                 └─ 位置基准（工件坐标）：glue.locate（工件特征模板）→ 位姿 T → 合格胶路 N' = T(N)

胶偏 d_i = 胶中心 B_i 到 N' 的有向法向距离；按 limits.position（名义 0 ± 公差）判定
胶宽、断胶：照旧来自胶检测链路，与位姿无关
```

- 合格胶路 N：示教图上画的胶中线（示教图上 T = 恒等，示教图自身 d≈0）。
- 工件与胶一起移动：B 与 N' 一起动，d 不变；胶相对工件偏移：d 改变。
- 位姿失败时：位置任务 ERR（沿用 fault 92 LOCATE_FAILED），胶宽/断胶证据照常保留。

## 2. LyFlow 改动

1. **`glue.locate` 1.1.0 稳健化**
   - 输出质量：次峰比（主峰邻域外的次高峰 / 主峰）、峰是否贴搜索窗边、角度是否贴扫描边界；任一不满足 → `ok=false` 并给出原因码。
   - 金字塔粗定位 + 全分辨率精定位，控制耗时。
   - 模板来源支持“整图 + 矩形”：`templateImage`（Path）+ `templateRect`（x, y, w, h），模板直接从冻结示教图裁，不再需要单独的模板文件。
   - 可选 mask（排除模板矩形内的胶走廊/喷嘴区域），视 OpenCV 版本支持情况定。
2. **新算子 `glue.path_deviation`**
   - 输入：`bead`（Bundle<glue.Bead>，来自 bead_width）、`nominal`（Bundle<glue.Path>，来自 taught_path）、`pose`（可选 Record glue.Pose2D）。
   - 计算：N' = T(nominal)；对每站胶中心 B_i，在 N' 上取局部投影，输出有向距离（px，可选 mm）、投影点、法向。
   - 状态：`ok / no_bead / pose_fail / projection_out_of_range`。
   - 输出 overlay：N'、投影线、模板框。
3. **示例图与测试**：用 `test_flyshot.cpp` 的刚体场景（翻边、安装孔、胶条）扩展：工件 + 胶共同刚体变换 → 偏差不变；只移胶 → 偏差等于位移；只移工件（胶留在喷嘴处）→ 偏差等于工件位移的反向。

算法图（静态，可通过发布包“图一致”校验）：

```text
n_load → n_locate ─────────────────────────┐ pose
n_load → n_path(taught_path) → n_width ─────┤ bead
              └───────────────────────────── ┤ nominal
                                       n_dev(path_deviation)
输出：stations、beadInfo、pose、deviation
```

## 3. GlueSight 改动

1. **配方**：`ShotSpec` / `ShotViewSpec` / `for_view` 增加可选 `fixture`：`{ rect: [x,y,w,h], searchRadiusPx, angleRangeDeg, minScore, maxPeakRatio }`。模板像素来自已冻结的示教图，不新增资源类型。`RECIPE_SCHEMA` 7。
2. **图构建与解析（vision/taught.rs）**
   - 有 fixture 时插入 `n_locate`、`n_dev`；`base_dir` 指向示教图所在目录（工作台为 workspace 图目录，生产为发布包 shot 目录）。
   - `parse_result`：读 pose 与 deviation；d 取 deviation 的有向距离；`ShotMeasurement` 增加 `pose`（ok、score、peakRatio、angle、offset）。
   - pose 失败 → `located=false` → 现有 cycle 通路给 ERR 90 / fault 92。
3. **示教页**
   - 新步骤“框选定位特征”：叠加模板框、搜索范围。
   - 保存时自动检查：模板框与“示教线 ± 搜索半宽”走廊不重叠；纹理充足（标准差下限）；示教图自身次峰比合格。
   - 因示教图上位姿恒为恒等，增加“定位试测”：在已采集的其他图（样本库）上跑定位，显示分数、次峰比、位移、角度和 d。
4. **验证**：独立正常样本必须全部定位成功；记录位姿分布，位移/角度超出 `searchRadius`/`angleRange` 的 70% 时给告警。
5. **发布**：`RELEASE_SCHEMA` 3；fixture 参数随配方冻结，模板由冻结原图 + 矩形推导，与 centerline/points 一样做“按配方重算比对”；`requiredOperators` 增加 `glue.locate`、`glue.path_deviation`；`GRAPH_VERSION` 更新。
6. **存储/历史**：`measurement_layout` 纳入 fixture；位姿写入 frames JSON（`FrameView`），表结构不变则不升 DB_VERSION；历史叠图按存储的位姿画 N'。
7. **试测标签**：`shot_tags` 试测标签加入 fixture；草稿比较、`workspace_save_draft`、前端 `sameTeach` 同步。

## 4. 实施阶段

| 阶段 | 内容 | 退出条件 |
| --- | --- | --- |
| 0 数据 | 采集重复上下件、正常补偿、人为胶偏的现场图；离线评测工具上跑 locate + 偏差计算（GlueSight 侧临时两段运行即可） | 每个拍照点给出：特征可用与否、分数/次峰比分布、位姿重复性、d 的稳定性 |
| 1 LyFlow | locate 1.1.0、path_deviation、合成场景测试 | 合成场景位姿误差 ≤0.3 px / 0.1°，偏差误差 ≤0.3 px |
| 2 GlueSight | 配方、示教、图、解析、cycle、发布、存储、叠图 | 集成测试通过；样例配方端到端可跑 |
| 3 现场 | 小批量对照 | 与人工/量具判定一致率达标 |

## 5. 测试（只列防回归）

| 条件 → 预期 | 保护的规则 |
| --- | --- |
| 工件与胶共同刚体移动 → d 不变 | 共同运动不算胶偏 |
| 工件移动、胶留在喷嘴处 → d 等于工件位移的法向分量（反号） | 偏差相对工件 |
| 定位分数低 / 次峰比超限 → 拍照点 ERR 92，胶宽证据保留 | 定位失败可见、不放行 |
| 模板框与胶走廊重叠 → 拒绝保存 | 基准独立于胶 |
| 改 fixture 参数 → 旧试测与验证失效 | 不复用旧结论 |

## 6. 留给评审的问题

1. 偏差计算放 LyFlow（新算子）还是 GlueSight Rust？
2. 模板从冻结示教图 + 矩形推导，还是独立模板资源？
3. 胶检测走廊完全不随位姿，是否会在工件大位移时让走廊里出现工件暗边（MX11 折边暗影问题）？是否需要“走廊随胶自身配准”？
4. NCC 是否足够，还是需要形状/边缘匹配？
5. 每个拍照点一个模板是否足够？

---

## 评审意见（评审方，第 1 轮）

核实范围：LyFlow `glue-integration.md:70-80`、`locate.cpp`、`flyshot.cpp:46-140`、`station_calipers.cpp:65-146`、`bead.cpp:373-395`；GlueSight `p0-plan.md:137-141`、`taught.rs`、`cycle.rs:654-700,738`、`judge.rs:106-215`、`recipe.rs`、`store.rs:379`、`release.rs:196-222`。

**认同**：两条链路拆开的结构对“胶随不随工件”都稳健——d = B − T(N) 在“胶静止、工件动”和“胶与工件一起动”两种情形下都正确；走廊不随位姿可保留 `parse_result` 的站点契约；模板与走廊不重叠、定位失败可见、fixture 改动使试测失效都对。

**必须修改（按严重度）**

- **P1 现场结论被过度推断。** 原文只能推出“胶的位移 ≠ 工件的位移”，推不出“胶静止”：同批数据约 30 点件间胶位差 ≤5 px，另有 8–13 点胶位移 20–107 px（2–12 mm），成因未分解；且当时 locate 1.0.0 无次峰/边界检查，“配准更差”也可能是定位本身错了。→ §0 改写，走廊宽度改为阶段 0 的数据产出。
- **P2 “位姿失败时胶宽/断胶照常保留”与现有通路矛盾。** `cycle.rs:672,690,695-697` 在 `!located` 时把点全置 Invalid 并清空；`:738` 任一 LocateFailed 整周期 ERR 92。若改用 NaN 绕过，`judge.rs:209-212` 只取有限值，位置会“无数据但判 OK”。→ PointState 增加独立的基准状态；基准失败段的位置任务 ErrInspect，胶宽/断胶照判，NG 与 ERR 并存时 NG 优先；改掉 `cycle.rs:738` 的整周期短路。回归测试：定位失败 + 断口 → 含 NgGap。
- **P3 名义 0 是示教件的隐含偏置。** 示教件胶若本身偏 e₀，之后所有 d 都带 −e₀。→ 验证阶段统计合格样本 d 均值作为 `nominalOffsetMm` 写进配方，或要求示教件经量具确认；退出条件加“合格样本 d 的 σ ≤ 位置公差 1/5”。
- **P4 搜索半宽未逐点确定。** 默认 `search_mm = 15`（`recipe.rs:635`），`tolerance = half − hi/2`（`taught.rs:163`），tol 越大折边暗影越易进入（MX11 误报来源）。→ 按阶段 0 测得的胶件间抖动 σ_B 逐点设 `max(3σ_B, 胶宽上限/2 + 2 px)`；超阈值点走“只检存在”，v1 不做“走廊随胶配准”。
- **P5 缺高度/视差前提。** 特征与胶不同面时，Z 向/倾斜变化会被当成胶偏。→ 示教规则优先选与胶同面的特征；阶段 0 加“同一件重复上下 ≥10 次的 d 重复性”。
- **P6 locate 稳健化范围过大。** → v1 只做次峰比、贴窗口/贴角度边界 → `ok=false` 带原因码、输出 `peakRatio`；金字塔视实测 p95 耗时再定；mask 不做。

**替代方案：偏差在 GlueSight 侧算，不新增 `glue.path_deviation`。**

```text
图：n_load → n_locate → outputs.pose；n_load → n_path → n_width → stations / beadInfo（现有图不动）
parse_result：B_i = N_i + d_i·n_i；N'_i = R(θ)(N_i − teachCenter) + center；n'_i = R(θ)n_i
             d'_i = (B_i − N'_i)·n'_i × scale − nominalOffsetMm；pose.ok=false → 该视图站点基准无效
```

理由：不新增算子与 Bundle 消费者；纯函数便于 Rust 单测；叠图本由 GlueSight 画；`requiredOperators` 只加 `glue.locate`。模板：发布/保存时从冻结原图裁出 `template.pgm`，与 centerline/points 同样“按配方重算比对”，locate 接口不动。

**对 §6 的立场**：① GlueSight Rust；② 冻结原图 + 矩形，落地为派生 PGM；③ v1 走廊静态，宽度逐点由数据定；④ NCC 够用（补次峰比与边界门）；⑤ 一个模板够，示教页显示杠杆臂 L（模板中心到最远测点），L 大时提示。

**必须靠现场数据决定**：胶在图像里的位移分解（法向/切向，对比 ΔW）；逐点 search_mm；各点模板 score/peakRatio 分布与 p95 耗时；同件重复上下 d 的 σ 与特征是否同面；≥5 件量具确认件的 d 均值（名义偏置）；已知偏移量样本（0.5/1/2 mm）的线性度。另：`p0-plan.md:227` P0-02 记录 MX11 原图与真值表两仓均未找到，阶段 0 第一件事是找回或重采。

---

## 起草方回应（第 1 轮）

- **接受**：P1、P2、P4、P5、P6；偏差改在 GlueSight Rust 计算；模板落地为派生 PGM。
- **部分接受 P2**：“NG 优先于基准无效 ERR”只适用于基准无效这一种 ERR，不改变其他 ERR（缺帧、超时）的优先级。
- **不同意 P3 的自动校正**：用合格样本 d 均值作为名义偏置，会把机器人系统性偏移当成“名义”吸收掉，正好掩盖要检的缺陷。改为：均值只作诊断告警；名义偏置必须由量具确认件给出并由人确认后写入。
- **补充评审未覆盖的问题**：评审给出的 d' 公式按站号对应（N'_i 对 B_i），工件沿胶路切向平移时，弯道上会产生一阶误差（切向位移 t、曲率半径 R 时约 t²/2R，t=50 px、R=200 px 约 6 px）。需改为在 N' 折线上就近投影（在 i 附近窗口内搜索）。

见第 2 轮修订稿。
