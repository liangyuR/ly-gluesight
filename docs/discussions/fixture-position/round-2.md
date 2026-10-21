# 第 2 轮：修订稿 v2

在 v1 基础上吸收第 1 轮评审。本稿自成一体，可单独阅读。

## 0. 起点事实（修订）

- MX11 对照（LyFlow `glue-integration.md:76-80`）：约 30 个直线/缓弯点件间胶位差 ≤5 px；8–13 个点胶位移 20–107 px，成因（工件移动、触发抖动、机器人路径、起止/拐角）未分解。当时 locate 为 1.0.0，没有次峰和边界检查，“按零件模板配准更差”不能当作位姿真值的结论。
- MX11 原图与真值表两仓均未找到（`p0-plan.md` P0-02），阶段 0 先找回或重采。
- `ShotLimits.position` 是 `Option`，拍照点已可单独关闭位置判定。

## 1. 结构

```text
胶检测（图像/喷嘴坐标，现有图不动）
  n_load → n_path(taught_path, 静态示教线) → n_width(bead_width) → 每站胶边 lo/hi、胶中心 B_i、胶宽 w_i、有无胶

位置基准（工件坐标）
  n_load → n_locate(glue.locate 1.1.0) → 位姿 T（ok、score、peakRatio、angle、center、teachCenter、reason）

GlueSight parse_result（Rust 纯函数）
  N' = T(N)，N 为示教中线（合格胶路）
  对每个有胶站：B_i 在 N' 上就近投影（在投影弧长 s_i ± 窗口内搜索折线段），得到投影点 P_i 与法向 n'_i
  d_i = (B_i − P_i)·n'_i × mmPerPx − nominalOffsetMm
  位置基准无效（T.ok=false）→ 该视图所有站“位置无基准”，胶宽/断胶照常
```

- 就近投影替代按站号对应：工件沿切向平移时，弯道上按站号对应会产生 t²/2R 量级的误差。
- 图只多一个 `n_locate` 节点和一个 `pose` 输出，仍是静态图，发布包“图一致”校验不变。
- LyFlow 不新增算子，`requiredOperators` 只加 `glue.locate`。

## 2. 判定语义

- `PointState::Measured { d: Option<f32>, w: f32 }`：`d = None` 表示位置无基准（与胶宽有效并存）。
- `judge`：
  - 位置统计只用 `Some(d)`；某段位置任务存在 `None` 且连续长度超过 `MAX_REFERENCE_GAP`（第一版取 0，即该视图定位失败整段无基准）→ 该段位置结果 ErrInspect，原因“定位失败”。
  - 胶宽、断胶照常判定。
  - 合并：已确定的 NG（胶宽、断胶、位置）优先于“基准无效”导致的 ErrInspect；其他来源的 ERR（缺帧、超时、点数不符）优先级不变。
- `cycle.rs`：真实链路下不再用 `located=false` 清空整帧；`FrameStatus::LocateFailed` 改为“定位失败但胶已测”，`fault 92` 只在最终结果因基准无效而为 ERR 时给出。

## 3. 名义偏置

- 示教中线代表工艺名义位置。示教时要求操作员在已确认合格的件上画线。
- 验证阶段统计独立合格样本 d 的均值 μ 与 σ：
  - σ ≤ 位置公差的 1/5 才通过。
  - |μ| 超过公差的 1/3 给告警“示教件可能偏离名义位置”，不自动校正。
- `nominalOffsetMm`（默认 0）只能由人填写，界面要求附“量具确认件编号”，并随发布冻结。

## 4. LyFlow：`glue.locate` 1.1.0

- 新增 `peakRatio`：主峰邻域（模板尺寸 1/4）以外的次高峰 / 主峰；参数 `maxPeakRatio`，超出 → `ok=false`。
- 峰贴搜索窗边界、角度贴扫描边界 → `ok=false`。
- 输出 `reason` 原因码：`low_score / ambiguous / at_search_edge / at_angle_edge / flat_template / out_of_image`。
- 搜索窗由方形改为圆形（与参数语义一致）。
- 金字塔：阶段 0 实测 p95 耗时超预算再做。mask 不做。
- 测试（合成刚体场景）：重复结构（两个相同孔）→ ambiguous；位移超过半径 → at_search_edge；已有的平移/旋转精度用例保留。

## 5. 模板与配方

- `ShotSpec` / `ShotViewSpec` / `for_view` 增加 `fixture: Option<FixtureSpec>`：`rect`、`searchRadiusPx`、`angleRangeDeg`、`minScore`、`maxPeakRatio`、`nominalOffsetMm`。
- 模板由冻结示教原图按 `rect` 裁出，保存为 `template.pgm`（工作台在 workspace 图目录；发布包在 `shots/kk/vN/`），发布加载时与 centerline/points 一样“按配方重算比对”。
- `base_dir`：工作台指向 workspace 图目录，生产指向发布包 shot 目录（目前 `base_dir` 恒为空，需要打通）。
- 走廊（`search_mm`）逐点设置：阶段 0 测出件间胶位抖动 σ_B 后按 `max(3σ_B, 胶宽上限/2 + 2 px)` 设定，示教页显示依据。σ_B 超阈值的点关闭位置判定（`position = None`），只检胶宽/断胶。
- `RECIPE_SCHEMA` 7、`RELEASE_SCHEMA` 3、`GRAPH_VERSION` 更新；按 D-0 不兼容旧数据。

## 6. 示教与验证

- 示教页增加“框选定位特征”：叠加模板框、搜索范围、杠杆臂 L（模板中心到最远测点）。
- 保存时检查：模板框与走廊不重叠；模板标准差下限；在示教图上 `peakRatio` 合格；L 超阈值提示。
- 示教规则：优先选与胶同一表面、刚性、不被喷嘴/胶枪遮挡的孔或角点。
- “定位试测”：在样本库其他图上跑定位并显示位姿与 d。
- 验证：独立合格样本全部定位成功；位移/角度超过搜索范围 70% 告警；d 的 μ/σ 检查见第 3 节。

## 7. 存储、历史、叠图

- `measurement_layout` 纳入 `fixture`。
- 位姿写入 `FrameView` JSON；points BLOB 的 d 需要表示 None（用 NaN 编码 + 状态位区分），若改格式则升 `POINTS_FORMAT`。
- 叠图：示教页、生产页、历史页按位姿画 N' 与模板框；`shotOverlay` 增加可选位姿。

## 8. 阶段

| 阶段 | 内容 | 退出条件 |
| --- | --- | --- |
| 0 数据 | 找回/重采 MX11；重复上下件 ≥10 次；已知偏移样本（0.5/1/2 mm）；离线工具跑 locate 1.0.0 + Rust 偏差函数 | 逐点给出：胶位移法向/切向分解、σ_B、模板 score/peakRatio、d 重复性、偏移线性度、耗时 |
| 1 LyFlow | locate 1.1.0 | 合成场景用例通过 |
| 2 GlueSight | 第 2、5、6、7 节 | 集成测试通过，样例配方端到端可跑 |
| 3 现场 | 小批量对照 | 与量具/人工判定一致率达标 |

## 9. 测试（只列防回归）

| 条件 → 预期 | 保护的规则 |
| --- | --- |
| 工件与胶共同刚体平移/旋转 → d 不变 | 共同运动不算胶偏 |
| 工件沿弯道切向平移、胶不动 → d 等于正确法向距离（就近投影） | 弯道上的对应关系正确 |
| 只移胶 → d 等于位移的法向分量 | 真实胶偏检出 |
| 定位失败 + 断口 → 结果为 NgGap，记录含定位失败原因 | NG 优先于基准无效 |
| 定位失败、胶正常 → ErrInspect，fault 92 | 定位失败不放行 |
| 修改 fixture → 旧试测与验证失效 | 不复用旧结论 |

## 10. 本轮待评审

1. `PointState` 改为 `d: Option<f32>` 的影响面是否可控，还是另设 `PointState::MeasuredNoReference`？
2. “NG 优先于基准无效 ERR”的合并规则是否会给现场带来误解？
3. 就近投影的搜索窗口如何取，是否会在拐角处跳到另一段？
4. 第 3 节名义偏置的处理是否足够。

---

## 评审意见（评审方，第 2 轮）

核实：`measure.rs:44-100,343-361`、`cycle.rs:654-700,728-745`、`judge.rs:8-40,84-91,193-230`、`store.rs:158-190,775,810`、`audit.rs:108-116`、`production.rs:102-121`、`workspace.rs:1047,2178`、`eval.rs:206`、`packaging.rs:59-68`、前端 `cycle/types.ts`、`vis.ts:75`、`ShotStrip.tsx`、`PartTrace.tsx`、`RuntimeFrame.tsx:72`。

**认同**：§0 改写、Rust 计算偏差、派生 PGM、locate 只做质量门、走廊逐点定；就近投影的补充成立；P3 接受起草方立场（均值只诊断，名义偏置人填并附量具件编号）。

**必须修改**

- **M1 “基准无效”要有自己的状态码，不能只靠 Option + NaN。** `cycle.rs:664-669`、`measure.rs:80`、`store.rs:166-172`、`audit.rs:108-116` 都按状态字节和 d 的位模式工作，`store.rs:810` 逐字节比对。→ `measure.rs` 增 `ST_NO_REF`（有胶、无基准，d=NaN、w 有效），映射 `Measured { d: None, w }`；store 状态字节新增值（与现有 `ST_PENDING=3` 错开），`POINTS_FORMAT` 升 3；audit `d_bits: Option<u32>`。回归测试：BLOB 往返后仍是 `d: None`。
- **M2 删除 `FrameStatus::LocateFailed` 与 `Measured.located`，不改语义。** 真实链路 `located` 恒为 true，只有模拟器置 false；前端三处把它当 fault 画。→ 帧加 `reference_failed`（哪些视图定位失败）；模拟器改为产出 `ST_NO_REF`；fault 92 由 judge 产出；删除 `cycle.rs:738` 整周期短路。
- **M3 `MAX_REFERENCE_GAP = 0` 与就近投影端点情形冲突。** 端点附近站投影落到折线端点，没有合法法向。→ None 的连续长度复用 `MAX_INVALID_LEN`（2 mm）容忍；整个视图全 None 才 ErrInspect；不新增常量。
- **M4 “NG 优先”在 judge 内用独立标记实现，不动 `Verdict` 全序。** → `SegmentResult.reference_invalid`；合并后若 ERR 仅由基准无效引起且有任一段 NG，则取该 NG。另加：无论 NG/ERR，帧记录与 UI 显示“定位失败”徽标；连续 N 件定位失败告警（防系统性模板损坏被 NG 掩盖）。多视图沿用 `aggregate_views` 现策略并写明。
- **M5 base_dir 打通要规定路径写法与写出时机。** → 图内 `template` 固定为相对路径 `template.pgm`，只换 base_dir；工作台保存 fixture 时写出 PGM；有 fixture 时 `plan` 拒绝空 base_dir；无 fixture 配方保持 `""`。

**简化**：删掉“搜索窗方形改圆形”。就近投影只在站 j∈[i−k, i+k] 的线段上做，k = ⌈searchRadiusPx / stationStepPx⌉；取 |距离| 最小且法向与 n'_i 夹角 < 45° 的线段（挡拐角）；投影参数不在 (0,1) 内 → `ST_NO_REF`。

**立场**：① `d: Option` 可控（非测试匹配点约 16 处），前提是 M1 先定；② 规则本身不误导，掩盖风险由徽标 + 连续失败告警解决；③ 见上；④ 足够，后续可加生产期 d 均值漂移统计。

**数据依赖**：同第 1 轮，补“投影落到端点/拐角的频率”。

**剩余分歧**：LocateFailed 删除 vs 改语义；`MAX_REFERENCE_GAP` vs 复用 `MAX_INVALID_LEN`；搜索窗改圆形删 vs 留。

---

## 起草方回应（第 2 轮）

三条剩余分歧全部接受评审意见：

- 删除 `LocateFailed` 与 `located`：改语义会留下名不副实的枚举值，三处前端和模拟器无论如何都要改。
- 复用 `MAX_INVALID_LEN`：端点投影情形是真实存在的，0 容忍会制造无谓的 ERR。
- 不改圆形搜索窗：没有功能收益。

M1、M4、M5 与就近投影的约束全部吸收。第 3 轮把方案合并成定稿 v3，并补上实施拆分（PR 顺序、依赖、每个 PR 的验收），请评审做最终检查。
