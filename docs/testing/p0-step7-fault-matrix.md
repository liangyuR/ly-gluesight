# P0 步骤 7：准备中的软件故障矩阵

状态：`codex/p0-step7-validation` 的源码准备稿，尚未完成集成验收。2026-10-10 D: 存储故障后停止构建压测，先保存源码；下表说明代码覆盖位置，不代表本次执行通过。

| 场景 | 已准备的测试或路径 | 待完成 |
| --- | --- | --- |
| 跨设备乱序 | `shot_router::tests::interleaved_frames_bind_to_their_own_shots_in_any_order`、Cycle 四点交错回归 | 本版重跑；真实 SDK 与线路由 W0 验收 |
| 丢首、中、尾帧 | ShotRouter missing 测试、Cycle `first_missing_frame_keeps_later_shots_and_expires_err91`、camera/delivery 的真实满通道测试 | 回调 helper→PartCycle→最终 ERR 组合 |
| 多余触发 | ShotRouter extra 测试；camera/delivery 中即使额外帧被满通道拒绝，回调汇总仍拒绝该件 | S7 End `[3,1,1]` 和未使用槽计数专项、wire 故障保留测试 |
| 错误相机、重连、上一件迟到 | ShotRouter 会话/相机/计数核验；Cycle 同 SN 不同 cycleId 的迟到结果回归；S7 arm 前重连测试 | 本版重跑与真实设备拔线 |
| 帧队列满（64） | `camera/delivery.rs` 使用真实 bounded channel，核对回调计数、拒绝整组三视角、Closed、排空后恢复及缺帧位置 | 新模块尚未运行；最终周期判定组合 |
| 测量队列满（32） | `measure::tests::full_measure_queue_rejects_one_job_then_recovers_without_leaks_or_duplicate_results` | 新测试尚未运行；实际 CycleHost 压力 |
| 录制队列满（48） | Recorder `queue_full_rejects_all_views_and_finish_bypasses_the_frame_limit` | 本版重跑；不能替代帧队列测试 |
| DLL 卡住、预热排队超时 | blocking Gate、许可耗尽、迟到结果、预热绝对截止（包含许可及 blocking pool 等待） | 新 Gate 测试尚未运行；不声明注入了真实 DLL 永久卡死 |
| PLC 写拒绝、断线、ACK 错件 | S7 wire report/release 拒绝、done 后断线、匹配序号和持久 ACK 恢复 | 本版重跑；实际桌面重启到 SQLite 历史链路 |
| 录制磁盘错误 | Recorder 真实文件系统错误；Audit 注入 outcome；Pending 录制 SQLite reopen 与事务回滚 | 不声明真实磁盘满、断电或当前故障磁盘上完成了业务验收 |
| 单目及三目真实像素 | `production::regression::native_full_resolution_frozen_bundle_single_and_tricam_regression`：每模式至少 100 件、每件四次真实测量、1280×1024、冻结发布包；normal、gap、whole_empty | 尚未执行；输出版本、DLL/夹具哈希、逐帧 JSONL、分位数与进程内存 |

`measure` 的 `ms` 保留从提交到报告的总耗时；`queueMs` 是提交到 blocking executor 实际进入，包含通道、许可与执行器等待；`engineMs` 是执行器进入到 runner 正常返回的墙钟时间；`coreMs` 是成功图像 runner 原有的测量耗时。未知值序列化为显式 null，0 是有效值。超时或 panic 不补造引擎耗时，迟到结果不回填已报告结果。历史合成测量没有这些观测，填 null。

真实 DLL 基准是串行调用 Prepared，报告的 `productionPartEndToJudgeMs` 保持 null。仍需在实际 CycleHost 中记录 partEnd 到结果交付的尾延迟、逐帧排队/引擎耗时与内存趋势；Cycle 的 `drainMs` 包含 PLC 结果输出等待，不能标成纯判定时间。性能运行时不得并行执行 Cargo、前端覆盖率或其他高负载任务。

恢复存储后的顺序：步骤 5 最终四帧×两尺寸显示 → 步骤 6 全量回归及十二视角历史/原包桌面重现 → 步骤 7 补齐上述缺口、全量回归、单目/三目原生基准、实际 CycleHost 与五个演示故障场景。每次通过后记录真实结果并更新草稿 PR；现场 W0/W7 准确率及真实三目 SDK 交付形式继续独立验收。

演示依然使用开发 Modbus，不代表 S7 生产验收。`make-samples.py` 从桌面已冻结的 workspace 生成可核查夹具，不能从全新检出凭空生成已示教配方；使用参数和准备步骤见 `scripts/robot-plc-demo/README.md`。暂存目录、原始图像、大型二进制和本机客户数据不提交。
