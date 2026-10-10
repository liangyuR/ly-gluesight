# P0 步骤 7：软件故障矩阵

> 本文列步 7 的故障场景、对应的测试和执行边界。数字结果汇总见 [测试记录](TESTING.md#p0-验证汇总)，命令见 [真实 DLL 回归](TESTING.md#真实-dll-回归) 与 [S7 一期握手](TESTING.md#s7-一期握手)；逐轮执行记录在 git 历史里。原 `evidence/` 下的运行记录已移出目录树，取回方法见测试记录的汇总一节。

| 场景 | 测试或路径 | 结果与边界 |
| --- | --- | --- |
| 跨设备乱序 | `shot_router::tests::interleaved_frames_bind_to_their_own_shots_in_any_order`、Cycle 四点交错回归 | 默认回归覆盖；真实 SDK 与线路由 W0 验收 |
| 丢首、中、尾帧 | ShotRouter missing 测试、Cycle `first_missing_frame_keeps_later_shots_and_expires_err91`、camera/delivery 的真实满通道测试 | 满通道回调 → ShotRouter → Part 最终 ERR 91 / PLC 90，恢复后下一件四点 OK；不覆盖 Machine 或实际 PLC 写入 |
| 多余触发 | ShotRouter extra 测试；camera/delivery 中即使额外帧被满通道拒绝，回调汇总仍拒绝该件 | S7 End `[3,1,1]` 和 `[4,0,0]` 未使用槽触发：不提交 OK/done，保留事务，只在中性输入及显式复位后恢复；多余回调即使丢弃仍使完整件 ERR 96 / PLC 90 |
| 错误相机、重连、上一件迟到 | ShotRouter 会话 / 相机 / 计数核验；Cycle 同 SN 不同 cycleId 的迟到结果回归；S7 arm 前重连测试 | 默认及 S7 回归覆盖；真实设备拔线属 W0 |
| 帧队列满（64） | `camera/delivery.rs` 使用真实 bounded channel，核对回调计数、拒绝整组三视角、Closed、排空后恢复及缺帧位置 | 含 Part 最终错误判定和下一件恢复 OK；Machine 桌面压力未测 |
| 测量队列满（32） | `measure::tests::full_measure_queue_rejects_one_job_then_recovers_without_leaks_or_duplicate_results` | 默认回归覆盖；实际 CycleHost 下的队列溢出未测（正常 400 件不证明溢出场景） |
| 录制队列满（48） | Recorder `queue_full_rejects_all_views_and_finish_bypasses_the_frame_limit` | 默认回归覆盖；不能替代帧队列测试 |
| DLL 卡住、预热排队超时 | blocking Gate、许可耗尽、迟到结果、预热排队与执行各自截止（排队含许可及 blocking pool 等待，最多 30 秒；worker 开始后另有完整 30 秒） | 预热 Gate 与测量超时许可回归覆盖：排队超时可冷却重试，执行超时永久拒绝且许可持有到线程返回；不声明注入了真实 DLL 永久卡死 |
| PLC 写拒绝、断线、ACK 错件 | S7 wire report/release 拒绝、done 后断线、匹配序号和持久 ACK 恢复 | S7 回环覆盖；桌面进程中断后按握手主日志恢复 SQLite ACK 已在步 6 验收 |
| 录制磁盘错误 | Recorder 真实文件系统错误；Audit 注入 outcome；Pending 录制 SQLite reopen 与事务回滚 | Windows 真实文件占用使旧件保留清理失败时，本件录制仍 Complete、可回放，Audit 告警独立且去重。不声明真实磁盘满、断电或故障盘业务验收 |
| 单目及三目真实像素 | `production::regression::native_full_resolution_frozen_bundle_single_and_tricam_regression`：每模式至少 100 件、每件四次真实测量、1280×1024、冻结发布包；场景 normal、gap、whole_empty | whole_empty（整帧无胶）现在期望 NG_GAP、0 次测量错误，原为 ERR；改后没有用真实 DLL 重跑。报告记录软件与 DLL 版本、夹具参数、逐帧 JSONL、分位数与进程内存 |

## 耗时字段

`measure` 的 `ms` 是从提交到报告的总耗时；`queueMs` 是提交到 blocking executor 实际进入，包含通道、许可与执行器等待；`engineMs` 是执行器进入到 runner 正常返回的墙钟时间；`coreMs` 是成功图像 runner 原有的测量耗时。未知值序列化为显式 null，0 是有效值。超时或 panic 不补造引擎耗时，迟到结果不回填已报告结果。历史合成测量没有这些观测，填 null。

Prepared 基准是串行调用，`productionPartEndToJudgeMs` 为 null。实际 CycleHost 运行记录 partEnd 到 PLC 提交的尾延迟、逐帧排队 / 引擎耗时与内存趋势；Cycle 的 `drainMs` 包含 PLC 结果输出等待，不能标成纯判定时间。内存只采原生主进程，不含 WebView2，100 件的趋势不能证明长期无泄漏。

## 夹具

- 默认 `simulated_metal`：带金属纹理与噪声。首件预期严格 OK、实际 `OK_WITH_EXCURSION` 的失败保留，对应 P0-09，没有放宽限值。
- `clean_step_edge`（`GLUESIGHT_P0_FIXTURE_STYLE=clean_step_edge` 显式选择）：1280×1024、灰度 210 背景、灰度 38 的 32 px 直胶带，0.125 mm/px，独立 `-CLEAN` 配方身份，用作性能控制。
- 实际 CycleHost 用桌面已冻结的 workspace 由 `scripts/robot-plc-demo/make-samples.py` 生成可核查夹具，不能从全新检出凭空生成已示教配方；参数和准备步骤见 `scripts/robot-plc-demo/README.md`。暂存目录、原始图像、大型二进制和本机客户数据不提交。

## 执行边界

- 演示使用开发 Modbus，不代表 S7 生产验收。
- 性能运行时不得并行执行 Cargo、前端覆盖率或其他高负载任务。
- 四组 CycleHost 各 100 件是去哈希前的程序；去哈希与集成后的程序没有重跑 400 件，同 profile 重复启动、四组合单件复验与五个演示场景待执行。
- 审计瞬态失败按 1/2/4/8/16/30 秒退避后每 30 秒持续重试，但待写仍是有界内存（128 待写 / 512 缓存 / 该 cycle 30 分钟无新事件即淘汰），无持久 spool（P0-15）。
- 不声明完成现场 W0 / W7，也不声明真实三目 SDK 交付形式已验证。
