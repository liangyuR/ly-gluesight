# 上线前决策索引（2026-10-19）

上线准备分两轮推进：第一轮 7 条工作线，第二轮 7 条工作线处理第一轮报告的跨线问题；之后界面迁移到 qing-steel 0.8.0 另记一批（第三批）；再之后是分段连拍飞拍（第四批，改变拍照计划模型与握手协议，旧拍照点模型只停用不删除）。每条工作线遇到难以决策的问题时，先按建议实现，并把问题、可选项、采用的做法和回退方式写进各自的决策文档。本页汇总需要用户确认的事项，按影响排序。实测步骤见 [上线前实测指南](../testing/hands-on-2026-10-19.md)。

## 工作线与 PR

| 轮次 | 工作线 | 决策文档 | 已合入的 PR |
| --- | --- | --- | --- |
| 一 | PLC 交互表适配 | [plc-table.md](plc-table.md) | #89、#108，ly-plc#3（v0.3.0） |
| 一 | 核心业务流程 | [core-flow.md](core-flow.md)、[核对清单](../testing/launch-core-flow-checklist.md) | #97–#101、#105 |
| 一 | 外壳与生产页 UI | [ui-shell.md](ui-shell.md) | #90–#92，qing-steel#2（v0.3.0） |
| 一 | 配方工作流 UI | [ui-recipe.md](ui-recipe.md) | #93–#96、#113 |
| 一 | 算法 | [algorithm.md](algorithm.md)、[评测报告](../testing/algorithm-evaluation.md) | #106、#107，LyFlow#29 |
| 一 | 日志与回放 | [log-replay.md](log-replay.md) | #102–#104 |
| 一 | 长稳与故障注入 | [soak.md](soak.md)、[长稳报告](../testing/soak-report.md) | #109–#112、#114 |
| 二 | 测试设施 | [r2-test-infra.md](r2-test-infra.md) | #122 |
| 二 | 遗留 UI | [r2-ui.md](r2-ui.md) | #116–#120，qing-steel#3（v0.4.0） |
| 二 | 中线提取与判定 | [r2-algorithm.md](r2-algorithm.md) | #125、#127，LyFlow#31（已合入，未发版） |
| 二 | lyFlow 1.1.4 升级 | [r2-lyflow-upgrade.md](r2-lyflow-upgrade.md) | #131，LyFlow#30（v1.1.4 已发布） |
| 二 | PLC 会话、布防与重连 | [r2-plc.md](r2-plc.md) | #132–#136，ly-plc#4、#5（v0.4.0） |
| 二 | 记录、追溯与告警 | [r2-records.md](r2-records.md) | #121、#124、#126、#128–#130、#137 |
| 二 | 前端性能与内存 | [r2-perf.md](r2-perf.md) | #138、#139、#123 |
| 三 | qing-steel 0.8.0 界面迁移 | [qing-steel-080.md](qing-steel-080.md) | #141–#146，收尾 PR 见该文档 |
| 四 | 分段连拍飞拍 | [segment-burst.md](segment-burst.md)、[plc-table.md 协议 6 适配](plc-table.md#协议-6-适配分段连拍)、[协议文档](../integration/plc-communication-protocol.md) | 分支 `feat/segment-burst-flyshot`，尚未合入 main（各工作包提交见 segment-burst.md） |

## 一、上线前必须处理

1. **PLC 工程师确认问题清单**（[plc-table.md](plc-table.md) 末尾，Q1–Q13）。会直接影响通讯的几项：
   - 协议版本写 4（Q1）；
   - `resultCode`、`faultCode` 是 VW76、VW78（Q2）；
   - SN 是字符串还是数值、最长多少、用哪种字符串格式（Q3–Q5）；
   - TSAP 0x0102 / 0x0201 能否接入（Q9）；
   - 各设备的 IP 汇总（Q10）。

   另外，PLC 的 ArmTimeout 不得小于 `arm_ms` + 2 s，建议保持 10 s（[r2-plc.md](r2-plc.md) R2P-1）。
2. **真机验证**：以下都还没有验证。两轮验证全部基于虚拟 S7、实拍样本集回放（模拟标定）和本机 lyFlow。
   - PLC、相机、硬触发、现场节拍；
   - 真机模式下的发布（`validationSite=real`）；
   - 布防各阶段的实际耗时（`arm_ms` 默认 500 ms 需在现场专机上复测）。
3. **真机标定**：实拍样本集没有标定，各工作线按名义胶宽推算的模拟当量在 0.09–0.117 mm/px 之间，只用于打通业务链路。

## 二、需要用户决定

| 事项 | 现状 | 文档 |
| --- | --- | --- |
| lyFlow 1.1.5 已发布（2026-10-21）并已作为依赖 | `lyflow-client` 与运行库系列升到 v1.1.5，含 #32 的带外圈模板 `glue.locate`。LyFlow#31（胶整段横移时跟过去，不再当成断口）的发布归属未在本仓记录；GlueSight 判定兜底（#125：以位置超差站为主的区间报位置超差，原因中注明可能整体横移）仍保留，是否去掉待用户对照 LyFlow 发布说明决定 | r2-algorithm RA-4、[lyFlow 依赖](../architecture/lyflow-dependency.md) |
| 运行日志容量 | 1 GB 上限在约 1 万件/天时只能保留约 4–5 天。建议提高到 4–8 GB，或超限时先删旧分段的 debug 行；`plc.db` 没有容量上限 | r2-records R-6 |
| 胶宽上限与允许连续超差 | 保持默认（5.5 mm、5 mm）。实拍正常件在起点、胶堆、拐角处常有 5.3–6 mm 的段落，余量小，需现场按产品复核 | r2-algorithm RA-5 |
| 审计 spool 残留未完成文件 | 维持现状：保留证据并拒绝就绪，需人工排查后处理 | soak S-8 |
| 分段连拍的待确认事项 | 胶偏的 `ToEdge` 测量量未实现（只做 `AbsoluteX`）；“每台相机一条按直线段几何的走廊”没有真实弯道图验证；提前 partEnd 只加 `short_part` 标记、不判 ERR，可能出现机器人标了未检而 PC 记录为 OK；气泡、拉丝不检；各段 N_exp 要等分段算法，多段配方现在只按整件帧组数发布与运行；现场 PLC 草稿 `gluesight_site.scl` 未改，只列改动点；**E2 的 3 × 25 Hz 吞吐合并门槛没有达标、也没有被直接验证**（合成图 Unmeasured 12 / 缺帧 46；样本集按旧口径平均 25.9 帧/s、突发排空 49 帧/s，持续 75 帧/s 约 40 s 装满测量队列；门槛已改为按相机使能窗口计持续帧率（2d30515），样本集尚未按新口径重跑），已定（2026-10-23）：门槛不改，等现场工控机用真实相机来源跑 `pnpm test:soak:throughput` 后再决定降频（约 11 Hz）还是加测量并发，合并前须完成该复测 | segment-burst 第二、三节、D-SB-15/17/34/37、第九节第 17 条 |
| 升级到协议 6 前清空审计 spool | 审计 spool 格式由 3 升到 4，残留的旧格式文件会让就绪检查失败（现有安全行为）。升级前先确认 `audit-spool` 已清空；记录库由 8 升到 9，旧库自动备份后重建 | segment-burst D-SB-29、[存储布局](../architecture/storage-layout.md) |

## 三、已按建议实现、需要确认的取舍

| 事项 | 采用 | 文档 |
| --- | --- | --- |
| 共享区 DB 号 | 两个标准 DB：PLC 区默认 DB4、PC 区默认 DB2，可改号但必须不同 | plc-table 协议 5 节 |
| 握手协议版本 | 先升为 5（拒绝 4 及更早版本），后升为 6（分段连拍，DB4/DB2 布局与点表同协议 5，拒绝 5 及更早版本） | plc-table 协议 5、协议 6 适配节 |
| SN 字符串格式 | 只用 S7 STRING[50]，1–50 字符；不再支持 200 SMART 字符串 | plc-table 协议 5 节 |
| 序号类型 | DInt，只接受 0–2147483647，负数按越界故障处理；16 位字段为 Int，只接受 0–32767 | plc-table 协议 5 节 |
| 布防预算 | 写后立即轮询确认，握手日志落盘移出预算，默认 500 ms，范围 100–5000 ms | r2-plc R2P-1 |
| 离线未结事务 | 握手日志记事务来源；离线事务可进入离线由虚拟 PLC 复位，真机或来源不明的事务仍需真机复位 | r2-plc R2P-2 |
| PC 主动断开 | 3.5 s 后自动重连一次，重连后仍须明确复位 | r2-plc R2P-3 |
| 进程中断的在途件 | 重启时补一条 ERR 历史记录，按开工时的来源记录 | r2-records R-4 |
| 握手审计保留 | 4 MB 滚动，30 天 / 256 MB 清理，保留未结交付需要的分段 | r2-records R-5 |
| 整组不检 | 不支持，每个拍照点组必须恰好选一张检测图（旧拍照点配方；段配方每台相机每一帧都测，不选图） | core-flow D-CF-7、segment-burst D-SB-12 |
| 首启样例配方 MTR-HSG-A/B | 保留，列为现场部署后的清理项 | core-flow D-CF-8 |
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
| 拍照计划模型 | 段列表 + 三台独立相机共用一个触发；`segments` 为空即旧拍照点配方，两套并存，旧模型（拍照点、停稳拍、特征定位、三目拼接、发布包 schema 3）只停用不删除 | segment-burst D-SB-1、第八节 |
| `shotCount` / `deviceShots` | 预期总数 ΣN_exp：生产请求精确核对（不符 90/94），段配方的示教采集不核对、不限 1–64；`deviceTriggers` 只作参考，帧归属取相机 SDK 帧号 | segment-burst D-SB-2/4/5、plc-table P8–P10 |
| 参考 FB 的 `DeviceShots <= 0` 判 7004 | 不改 FB，在协议文档注明示教估算值须大于 0 | plc-table P10、协议文档 4.1 |

## 四、依赖版本

| 依赖 | 版本 |
| --- | --- |
| lyFlow 运行库与 `lyflow-client` | v1.1.5 |
| ly-plc | v0.4.0（fe6c210） |
| qing-steel | v0.9.0（界面迁移记录见 [qing-steel-080.md](qing-steel-080.md)） |
