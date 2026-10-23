# 决策索引（2026-10-19 起，2026-10-23 更新）

上线准备分两轮推进：第一轮 7 条工作线，第二轮 7 条工作线处理第一轮报告的跨线问题；之后界面迁移到 qing-steel 0.8.0 另记一批（第三批）；再之后是分段连拍飞拍（第四批，改变拍照计划模型与握手协议，旧拍照点模型已退出）。每条工作线遇到难以决策的问题时，先按建议实现，并把问题、可选项、采用的做法和回退方式写进各自的决策文档。本页汇总各决策文档的现状和需要用户确认的事项，按影响排序。实测步骤见 [上线前实测指南](../testing/hands-on-2026-10-19.md)。

**现行设计的基准是第四批的 [segment-burst.md](segment-burst.md)。** 旧拍照点配方从未部署，现已退出，不再要求兼容或迁移。段配方为唯一生产流程；相机 Line0 高电平固定帧率，Software 保留单帧标定与调试，PLC 协议版本保持 6。下文第一至三批和第四批早期的旧模型说明均属于历史设计，不再约束新实现。

**标注约定**：决策记录是历史，正文不改写。被取代的条目在文件开头或条目处加引用块标注：

- “段配方下已被取代（日期，见 segment-burst.md D-SB-n）”：段配方改按新做法，旧拍照点配方（保留、停用）仍按原条目；
- “已被取代（日期，见 …）”：对两种配方都不再成立（例如协议版本、请求层计数校验）。

## 工作线、PR 与现状

| 批次 | 工作线 | 决策文档 | 已合入的 PR | 现状 |
| --- | --- | --- | --- | --- |
| 一 | PLC 交互表适配 | [plc-table.md](plc-table.md) | #89、#108，ly-plc#3（v0.3.0） | 部分被取代：协议 4 的第 1–12 条中 S7-200 SMART、DB1、WORD 与 200 SMART 字符串的取舍，第 3 条（版本 4）、第 11 条（SCL 协议 4）与 Q1–Q13 清单已被协议 5、6 取代；协议 5 的 P2、V1 被协议 6（P7、W1）取代。仍有效：第 7.2 条（S7 STRING 部分）、第 8、12 条，P1、P3–P6、V2–V6，协议 6 的 P7–P13、W1–W8 |
| 一 | 核心业务流程 | [core-flow.md](core-flow.md)、[核对清单](../testing/launch-core-flow-checklist.md) | #97–#101、#105 | 有效；D-CF-4 最后一项（`deviceShots≠shotCount` 属请求 payload 不合法）已被 D-SB-5 取代（现为接件 94）；D-CF-2、D-CF-3、D-CF-7 段配方下被取代 |
| 一 | 外壳与生产页 UI | [ui-shell.md](ui-shell.md) | #90–#92，qing-steel#2（v0.3.0） | 有效；§5 的 2026-10-21 补记（特征定位“定位失败”）段配方下被取代（D-SB-15、D-SB-13） |
| 一 | 配方工作流 UI | [ui-recipe.md](ui-recipe.md) | #93–#96、#113 | 走查对象是旧拍照点配方的页面，段配方另有页面（segment-burst 5.3）；D6 与单帧示教相关条目只对旧配方有效，共用组件与设备页各条有效 |
| 一 | 算法 | [algorithm.md](algorithm.md)、[评测报告](../testing/algorithm-evaluation.md) | #106、#107，LyFlow#29 | 部分段配方下被取代（D-SB-15）：评测工具（A-1）与 A-5、A-10 的逐站位置判定只针对旧拍照点配方；A-2、A-7 的中线提取被段配方的走廊提取复用；其余有效 |
| 一 | 日志与回放 | [log-replay.md](log-replay.md) | #102–#104 | 有效；§15、§17 段配方下被取代（第九节第 17 条） |
| 一 | 长稳与故障注入 | [soak.md](soak.md)、[长稳报告](../testing/soak-report.md) | #109–#112、#114 | 有效；S-2、S-3、S-14 段配方下被取代；S-12、S-13 已由 r2-plc R2P-1–R2P-3 处理 |
| 二 | 测试设施 | [r2-test-infra.md](r2-test-infra.md) | #122 | 有效；第 7 条段配方下被取代（第九节第 17 条） |
| 二 | 遗留 UI | [r2-ui.md](r2-ui.md) | #116–#120，qing-steel#3（v0.4.0） | 有效（U-11 的胶路示教比例只对旧拍照点配方有意义，段配方的 `coverage` 为 null） |
| 二 | 中线提取与判定 | [r2-algorithm.md](r2-algorithm.md) | #125、#127，LyFlow#31（已合入，未发版） | RA-1、RA-2（中线提取，段配方走廊提取复用）、RA-3、RA-5、RA-6 有效；RA-4（`judge.rs` 横移兜底）段配方下被取代（D-SB-15） |
| 二 | lyFlow 1.1.4 升级 | [r2-lyflow-upgrade.md](r2-lyflow-upgrade.md) | #131，LyFlow#30（v1.1.4 已发布） | 有效，依赖其后已升到 1.1.5（`src-tauri/Cargo.toml` tag v1.1.5）；L-2 段配方下被取代 |
| 二 | PLC 会话、布防与重连 | [r2-plc.md](r2-plc.md) | #132–#136，ly-plc#4、#5（v0.4.0） | 有效 |
| 二 | 记录、追溯与告警 | [r2-records.md](r2-records.md) | #121、#124、#126、#128–#130、#137 | 有效 |
| 二 | 前端性能与内存 | [r2-perf.md](r2-perf.md) | #138、#139、#123 | 有效（段配方的在线读图同样走 P-2 的 canvas 绘制） |
| 三 | qing-steel 0.8.0 界面迁移 | [qing-steel-080.md](qing-steel-080.md) | #141–#146，收尾 PR 见该文档 | 有效，依赖其后已升到 qing-steel 0.9.0 |
| 四 | 分段连拍飞拍 | [segment-burst.md](segment-burst.md)、[plc-table.md 协议 6 适配](plc-table.md#协议-6-适配分段连拍)、[协议文档](../integration/plc-communication-protocol.md) | 分支 `feat/segment-burst-flyshot`，尚未合入 main（各工作包提交见 segment-burst.md） | 现行基准 |

## 一、上线前必须处理

1. **PLC 工程师确认问题清单**：[plc-table.md](plc-table.md) 的[协议 6 待确认](plc-table.md#协议-6-待-plc-工程师确认)（W1–W8）与[协议 5 待确认](plc-table.md#协议-5-待-plc-工程师确认)（V2–V6）；协议 4 的 Q1–Q13 已被取代。会直接影响通讯的几项：
   - `protocolVersion` 写 6，协议 5 的 PLC 程序会被拒绝（W1）；
   - `shotCount`、`deviceShots` 都填 PC 导出计划的 `expectedTotal`（ΣN_exp），`planVersion` 填导出版本，重新标定后须同步（W2）；
   - 相机使能为高期间按固定频率连续发脉冲串，不以“已发数达到 `shotCount`”停发；频率、脉宽、三台相机共用触发的接线（W3）；
   - `deviceTriggers` 的计数源与 `partEnd` 置位时机（W4、W5）；“本件未检”的处置（W6）；
   - 现场草稿 `gluesight_site.scl` 按 `scripts/s7-handshake/site/README.md` 的“协议 6 需要的改动点”修改（W7）；
   - DB4 / DB2 取消优化的块访问、允许 PUT/GET（V2）；`partSn` 声明与 SN 来源（V3）；CPU 型号与协商 PDU（V5）；IP 汇总（V6）。

   另外，PLC 的 ArmTimeout 不得小于 `arm_ms` + 2 s，建议保持 10 s（[r2-plc.md](r2-plc.md) R2P-1）。
2. **真机验证**：以下都还没有验证。各轮验证全部基于虚拟 S7、实拍样本集回放（模拟标定，停稳拍数据）、合成图三相机离线环境和本机 lyFlow。
   - PLC、三台相机共用的硬触发、现场节拍；
   - 真机模式下的发布（`validationSite=real`）；
   - 布防各阶段的实际耗时（`arm_ms` 默认 500 ms 需在现场专机上复测）；
   - **3 × 25 Hz 吞吐门槛**：没有达标、也没有被直接验证。已定（2026-10-23）：门槛口径不改（按相机使能窗口计持续帧率），在现场工控机用真实相机来源跑 `pnpm test:soak:throughput`（并重跑 `throughput_probe` 探针）后，再决定降 PLC 脉冲频率（约 11 Hz）还是加测量并发；合并前须完成该复测（segment-burst 7.1、第九节第 17 条）；
   - **分段算法**：本轮只有 `Segmenter` 接口与 `Unsegmented` 实现，算法要等现场按量产曝光与速度拍的含弯道三相机图像序列再开发验证；验证前整条胶路按未分段处理，只跑每帧检测、不判胶偏（segment-burst 第一节、D-SB-21）。
3. **真机标定**：实拍样本集没有标定，各工作线按名义胶宽推算的模拟当量在 0.09–0.117 mm/px 之间，只用于打通业务链路。段配方读设备标定 `calib/device/plane_calib_{device}.json`，只存过 `{device}-v1` 的单视角相机要在标定页重新标定一次（D-SB-30）。
4. **升级到协议 6 前清空审计 spool**：审计 spool 格式由 3 升到 4，残留的旧格式文件会让就绪检查失败（现有安全行为）。升级前先确认 `audit-spool` 已清空；记录库由 8 升到 9，旧库自动备份后重建（D-SB-29、[存储布局](../architecture/storage-layout.md)）。

## 二、需要用户决定

| 事项 | 现状 | 文档 |
| --- | --- | --- |
| 3 × 25 Hz 吞吐门槛之后的取舍 | 现场工控机真实相机复测后，在 ① 降 PLC 脉冲频率到约 11 Hz（各段 N_exp 随之变，须重新标定并换计划版本）与 ② 加测量并发（本机 4 物理核，4 → 6 线程增益有限，需改代码重测）之间选。离线实测：合成图 Unmeasured 12 / 缺帧 46（渲染上限，lyFlow 未参与）；样本集按旧口径平均 25.9 帧/s、突发排空 49 帧/s，持续 75 帧/s 约 40 s 装满测量队列；门槛已改为按相机使能窗口计持续帧率（2d30515），样本集尚未按新口径重跑 | segment-burst 7.1、第九节第 17 条 |
| 分段连拍的其余待确认事项 | 胶偏的 `ToEdge` 测量量未实现（只做 `AbsoluteX`）；“每台相机一条按直线段几何的走廊”没有真实弯道图验证；提前 partEnd 只加 `short_part` 标记、不判 ERR，可能出现机器人标了未检而 PC 记录为 OK；气泡、拉丝不检；各段 N_exp 要等分段算法，多段配方现在只按整件帧组数发布与运行；现场 PLC 草稿 `gluesight_site.scl` 未改，只列改动点；段配方唯一走 lyFlow 全链路的测试 `vision::taught::engine_tests::engine_burst_publish_load_measure_frame_round_trip` 默认忽略，需要设 `LYFLOW_CORE_DLL` 手动运行（测试清单记 lyFlow 1.1.5 实测通过） | segment-burst 第二、三节、D-SB-15/17/34/37、[测试清单](../testing/segment-burst-test-inventory.md) |
| lyFlow 1.1.5 已发布（2026-10-21）并已作为依赖 | `lyflow-client` 与运行库系列升到 v1.1.5，含 #32 的带外圈模板 `glue.locate`。LyFlow#31（胶整段横移时跟过去，不再当成断口）的发布归属未在本仓记录；GlueSight 判定兜底（#125：以位置超差站为主的区间报位置超差，原因中注明可能整体横移）仍保留，是否去掉待用户对照 LyFlow 发布说明决定。两者都只作用于旧拍照点配方的逐站位置判定；段配方的胶偏按段级判（D-SB-15），不受影响 | r2-algorithm RA-4、[lyFlow 依赖](../architecture/lyflow-dependency.md) |
| 运行日志容量 | 1 GB 上限在约 1 万件/天时只能保留约 4–5 天。建议提高到 4–8 GB，或超限时先删旧分段的 debug 行；`plc.db` 没有容量上限 | r2-records R-6 |
| 胶宽上限与允许连续超差 | 保持默认（5.5 mm、5 mm）。实拍正常件在起点、胶堆、拐角处常有 5.3–6 mm 的段落，余量小，需现场按产品复核。段配方的直线 / 弯道默认限值沿用同一组值（胶宽 4 ± 1.5 mm、帧内允许超差 5 mm，`plan::KindDefaults`），按段类型分别可调 | r2-algorithm RA-5、segment-burst D-SB-16 |
| 审计 spool 残留未完成文件 | 维持现状：保留证据并拒绝就绪，需人工排查后处理 | soak S-8 |

## 三、已按建议实现、需要确认的取舍

| 事项 | 采用 | 文档 |
| --- | --- | --- |
| 拍照计划模型 | 段列表 + 三台独立相机共用一个触发；`segments` 为空即旧拍照点配方，两套并存，旧模型（拍照点、停稳拍、特征定位、三目拼接、组选图、发布包 schema 3）只停用不删除 | segment-burst D-SB-1、第八节 |
| 多段配方的发布与运行 | 整件 ΣN_exp（`expected_total`）由多轮标定得出即可发布，各段 N_exp 可为 null；运行时整条胶路一个未分段区域，只跑每帧检测、不判胶偏，只核对整件帧组数 ∈ ΣN_exp ± Σtol | segment-burst 第二节、D-SB-21 |
| 缺帧、帧数越界、丢帧（段配方） | 某台相机缺帧只把该帧组标检测不完整；帧组数越界记“帧数异常”、帧组数 < ΣN_exp − Σtol 加 `short_part`；丢帧只告警；都不改 OK/NG。只有三台相机整件一帧都没测成才判 ERR 91 | segment-burst D-SB-3、D-SB-13、D-SB-34 |
| 共享区 DB 号 | 两个标准 DB：PLC 区默认 DB4、PC 区默认 DB2，可改号但必须不同 | plc-table 协议 5 节 |
| 握手协议版本 | 先升为 5（拒绝 4 及更早版本），后升为 6（分段连拍，DB4/DB2 布局与点表同协议 5，拒绝 5 及更早版本） | plc-table 协议 5、协议 6 适配节 |
| `shotCount` / `deviceShots` | 预期总数 ΣN_exp：生产请求精确核对（不符 90/94），段配方的示教采集不核对、不限 1–64；`deviceTriggers` 只作参考，帧归属取相机 SDK 帧号。旧拍照点配方的示教采集仍按协议 5 规则（1–64、计数相等） | segment-burst D-SB-2/4/5/8、plc-table P8–P10 |
| 参考 FB 的 `DeviceShots <= 0` 判 7004 | 不改 FB，在协议文档注明示教估算值须大于 0 | plc-table P10、协议文档 4.1 |
| SN 字符串格式 | 只用 S7 STRING[50]，1–50 字符；不再支持 200 SMART 字符串 | plc-table 协议 5 节 |
| 序号类型 | DInt，只接受 0–2147483647，负数按越界故障处理；16 位字段为 Int，只接受 0–32767 | plc-table 协议 5 节 |
| 布防预算 | 写后立即轮询确认，握手日志落盘移出预算，默认 500 ms，范围 100–5000 ms | r2-plc R2P-1 |
| 离线未结事务 | 握手日志记事务来源；离线事务可进入离线由虚拟 PLC 复位，真机或来源不明的事务仍需真机复位 | r2-plc R2P-2 |
| PC 主动断开 | 3.5 s 后自动重连一次，重连后仍须明确复位 | r2-plc R2P-3 |
| 进程中断的在途件 | 重启时补一条 ERR 历史记录，按开工时的来源记录 | r2-records R-4 |
| 握手审计保留 | 4 MB 滚动，30 天 / 256 MB 清理，保留未结交付需要的分段 | r2-records R-5 |
| 整组不检 | 不支持，每个拍照点组必须恰好选一张检测图（旧拍照点配方；段配方每台相机每一帧都测，不选图） | core-flow D-CF-7、segment-burst D-SB-12 |
| 首启配方库 | 新安装与空库重启不再写入 MTR-HSG-A/B；已有配方保留，旧配方回归测试显式准备夹具。新建配方的模板是段配方（D-SB-24） | core-flow D-CF-8 |
| 检测默认参数 | 不变（搜索半宽 15 mm、胶宽 1–10 mm），现场按产品收紧 | algorithm A-8 |
| 示教默认笔刷 | 固定 120 px；提取失败时引导加大笔刷或改用笔划生成 | r2-ui U-9、U-10 |
| 矩形框选提取 | 改为「顶帽找胶 → 笔刷提取对中」 | r2-algorithm RA-1 |
| 全局提示 | 主区顶部居中，同时只显示一条；只放瞬时的成功提示，错误留在页面内 | r2-ui U-1–U-3 |
| 局部超差的颜色 | 统一用警告色；未测成和设备故障用故障紫，与 NG 红分开 | ui-shell §4–5 |
| 生产页判定字号 | 80 px，右栏 420 px | ui-shell §6 |
| 相机页设计稿的 9 个问题 | 2 项已实现，7 项维持现状 | ui-recipe 相机页一节 |
| 运行日志保留 | 按日滚动，单个分段 16 MB，保留 30 天，总量上限 1 GB | log-replay §3 |
| 资料盘下限 | 剩余空间低于 2 GB 时暂停接件，恢复后自动就绪 | soak S-7 |
| 持久追溯故障 | 维持「排查后重启恢复」；布防预算超时只拒绝本件，不再锁死追溯 | soak S-8、S-10 |

## 四、依赖版本

| 依赖 | 版本 |
| --- | --- |
| lyFlow 运行库与 `lyflow-client` | v1.1.5 |
| ly-plc | v0.4.0（fe6c210） |
| qing-steel | v0.9.0（界面迁移记录见 [qing-steel-080.md](qing-steel-080.md)） |
