# P0 步骤 7：验收中的软件故障矩阵

状态：迁移到 C: 后恢复验收，最终审查集成版 Rust 默认 275 项通过、30 项默认忽略；S7 回环专项 24 项通过。新回调队列、测量队列和预热截止测试均已运行。前端首轮在并行原生构建时有两项 5 秒超时及五项级联失败；相同命令、相同默认超时在安静窗口重跑 947 项全部通过（114.57 秒）；合入 Codex 标定/延后刷新修复后 954 项通过（114.56 秒），再补检测设置保存通知后最终 960 项及覆盖率、类型检查、构建通过（175.62 秒）。真实 DLL 另五项专项通过。默认金属纹理性能夹具首件预期 OK、实际 2 的失败保留；独立洁净像素控制已完成单目/三目各 100 件、400 次串行测量及报告核验，实际 CycleHost 和桌面演示继续执行。真实结果及日志哈希见 [C 盘回归记录](evidence/p0-step7-regression-c.json)。

| 场景 | 已准备的测试或路径 | 待完成 |
| --- | --- | --- |
| 跨设备乱序 | `shot_router::tests::interleaved_frames_bind_to_their_own_shots_in_any_order`、Cycle 四点交错回归 | 默认回归通过；真实 SDK 与线路由 W0 验收 |
| 丢首、中、尾帧 | ShotRouter missing 测试、Cycle `first_missing_frame_keeps_later_shots_and_expires_err91`、camera/delivery 的真实满通道测试 | 真实满通道回调→ShotRouter→Part 最终 ERR 91 / PLC 90 组合通过，恢复后下一件四点 OK；不声称覆盖 Machine 或实际 PLC 写入 |
| 多余触发 | ShotRouter extra 测试；camera/delivery 中即使额外帧被满通道拒绝，回调汇总仍拒绝该件 | S7 End `[3,1,1]` 和 `[4,0,0]` 未使用槽触发专项通过：不提交 OK/done，保留事务、只在中性输入及显式复位后恢复；多余回调即使丢弃仍使完整件 ERR 96 / PLC 90 |
| 错误相机、重连、上一件迟到 | ShotRouter 会话/相机/计数核验；Cycle 同 SN 不同 cycleId 的迟到结果回归；S7 arm 前重连测试 | 默认及 S7 回归通过；真实设备拔线仍属 W0 |
| 帧队列满（64） | `camera/delivery.rs` 使用真实 bounded channel，核对回调计数、拒绝整组三视角、Closed、排空后恢复及缺帧位置 | 八项通道测试通过，含 Part 最终错误判定和下一件恢复 OK；Machine 桌面压力另测 |
| 测量队列满（32） | `measure::tests::full_measure_queue_rejects_one_job_then_recovers_without_leaks_or_duplicate_results` | 默认回归通过；实际 CycleHost 压力另测 |
| 录制队列满（48） | Recorder `queue_full_rejects_all_views_and_finish_bypasses_the_frame_limit` | 默认回归通过；不能替代帧队列测试 |
| DLL 卡住、预热排队超时 | blocking Gate、许可耗尽、迟到结果、预热绝对截止（包含许可及 blocking pool 等待） | 七项预热 Gate 测试和测量超时许可回归通过；不声明注入了真实 DLL 永久卡死 |
| PLC 写拒绝、断线、ACK 错件 | S7 wire report/release 拒绝、done 后断线、匹配序号和持久 ACK 恢复 | 24 项 S7 回环通过；实际桌面重启到 SQLite 历史链路随步 6 验收 |
| 录制磁盘错误 | Recorder 真实文件系统错误；Audit 注入 outcome；Pending 录制 SQLite reopen 与事务回滚 | Windows 真实旧文件禁止 DELETE 共享使保留清理失败，当前完整录制仍可用且可哈希回放、Audit 告警独立且去重；全量 260 项通过。不声明真实磁盘满、断电或故障 D: 业务验收 |
| 单目及三目真实像素 | `production::regression::native_full_resolution_frozen_bundle_single_and_tricam_regression`：每模式至少 100 件、每件四次真实测量、1280×1024、冻结发布包；normal、gap、whole_empty | 默认金属纹理夹具首件严格 OK 失败；独立 clean_step_edge 夹具各 100 件通过，版本、DLL/夹具哈希、逐帧 JSONL、分位数与进程内存见 p0-step7-prepared-c.json；不声明修复 P0-09 |

`measure` 的 `ms` 保留从提交到报告的总耗时；`queueMs` 是提交到 blocking executor 实际进入，包含通道、许可与执行器等待；`engineMs` 是执行器进入到 runner 正常返回的墙钟时间；`coreMs` 是成功图像 runner 原有的测量耗时。未知值序列化为显式 null，0 是有效值。超时或 panic 不补造引擎耗时，迟到结果不回填已报告结果。历史合成测量没有这些观测，填 null。

真实 DLL 基准是串行调用 Prepared，报告的 `productionPartEndToJudgeMs` 保持 null。仍需在实际 CycleHost 中记录 partEnd 到结果交付的尾延迟、逐帧排队/引擎耗时与内存趋势；Cycle 的 `drainMs` 包含 PLC 结果输出等待，不能标成纯判定时间。性能运行时不得并行执行 Cargo、前端覆盖率或其他高负载任务。

恢复存储后的顺序：步骤 5 最终四帧×两尺寸显示 → 步骤 6 全量回归及十二视角历史/原包桌面重现 → 步骤 7 补齐上述缺口、全量回归、单目/三目原生基准、实际 CycleHost 与五个演示故障场景。每次通过后记录真实结果并更新草稿 PR；现场 W0/W7 准确率及真实三目 SDK 交付形式继续独立验收。

演示依然使用开发 Modbus，不代表 S7 生产验收。`make-samples.py` 从桌面已冻结的 workspace 生成可核查夹具，不能从全新检出凭空生成已示教配方；使用参数和准备步骤见 `scripts/robot-plc-demo/README.md`。暂存目录、原始图像、大型二进制和本机客户数据不提交。

独立洁净性能控制通过 `GLUESIGHT_P0_FIXTURE_STYLE=clean_step_edge` 显式选择：1280×1024、灰度 210 背景、灰度 38 的 32 px 直胶带，0.125 mm/px，独立 `-CLEAN` 配方身份。默认 `simulated_metal` 保留纹理、噪声及首次失败。单目 / 三目各 100 件、400 次测量，最终 Prepared P95 11.603 / 11.855 ms；只读报告工具重新核验全部原图与冻结资源 SHA256，详见 [性能控制证据](evidence/p0-step7-prepared-c.json)。最终串行四次测量 P95 44.195 / 44.247 ms；此值不能当作实际 PLC 收尾延迟。

最终 Prepared 报告以 `f738e05` 为软件基线，完整源文件与测试程序字节封存在新报告目录的 `software/` 中，后续构建不会覆盖引用的证据。报告保留先前控制运行，未以较小耗时替换最终结果。PR #12 的旧录制保留清理错误与当前录制完整性已完成分离，最终 Rust 260 项通过，不改变该串行测量控制的算法/包/像素。

实际 CycleHost 首轮三目洁净回放在第三件布防核验超过原 200 ms，前两件严格 OK、12 幅原图逐幅匹配；第三件 ERR 99、零帧零测量、完成 ACK 后 IDLE，批次只完成 2 件。P0-14 修复保留首次完整语义加载及逐件全量清单/资源/DLL 校验，私有快照逐字节一致性、完整目录和 Windows 重解析点拒绝仍在；只在单次调用中复用祖先 metadata，不使用时间戳缓存。相同发布包 20 次只读核验从 76–119 ms 降到 19–54 ms，DLL 全读哈希 7–13 ms；额外内存为每个已加载发布包的一份共享不可变资源字节，后续周期不累积。15 项发布安全回归、实际 DLL 同大小改动拒绝专项、全量 Rust 263 项通过、30 项默认忽略。实际原生重建与新批次复验待执行，不能将剖析值当作 CycleHost 布防结果。见 [P0-14 失败与修复](evidence/p0-arm-verify-c.json)。

审查修复最终集成 `a3d4ad4`：步骤5预热纯排队超时冷却可重试，实际执行超时仍隔离；步骤6孤立pending分批回收和AppData启动独占锁已集成，完整Rust275项、真实DLL冻结/篡改专项、CycleHost证据工具51项通过。P0-14 d03fade原生恢复控制100/100件严格OK、400测量、1200原图逐幅匹配，独立validator通过；arm P50/P95/最大34/42/56ms，frame总耗时14/16/19ms、queue全0，partEnd到PLC提交15/31/31ms。主进程private增加16.21MB（多数发生在首10件），全部原始采样与斜率保留，不能据此声明无限时长无增长。最终版本四组400件与重复启动、五演示验收继续执行。
