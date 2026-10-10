# P0 步骤 7：验收中的软件故障矩阵

> 当前版本已按 AGENTS.md 移除业务内容哈希匹配。本文既有摘要、字节篡改门禁和旧版本验收仅为历史记录；新实现以明确 ID/版本、实际结构、路径/尺寸与设备计数为准。旧性能数据不能代表本次去哈希版本。

状态：迁移到 C: 后恢复验收，最终审查集成版 Rust 默认 279 项通过、30 项默认忽略；S7 回环专项 24 项通过。新回调队列、测量队列和预热截止测试均已运行。前端首轮在并行原生构建时有两项 5 秒超时及五项级联失败；相同命令、相同默认超时在安静窗口重跑 947 项全部通过（114.57 秒）；合入 Codex 标定/延后刷新修复后 954 项通过（114.56 秒），再补检测设置保存通知后最终 960 项及覆盖率、类型检查、构建通过（175.62 秒）。真实 DLL 另五项专项通过。默认金属纹理性能夹具首件预期 OK、实际 2 的失败保留；独立洁净像素控制已完成单目/三目各 100 件、400 次串行测量及报告核验，实际 CycleHost 四组共 400 件基线通过，最新审计程序四组合单件复验和桌面演示继续执行。真实结果及日志哈希见 [C 盘回归记录](evidence/p0-step7-regression-c.json)。

| 场景 | 已准备的测试或路径 | 待完成 |
| --- | --- | --- |
| 跨设备乱序 | `shot_router::tests::interleaved_frames_bind_to_their_own_shots_in_any_order`、Cycle 四点交错回归 | 默认回归通过；真实 SDK 与线路由 W0 验收 |
| 丢首、中、尾帧 | ShotRouter missing 测试、Cycle `first_missing_frame_keeps_later_shots_and_expires_err91`、camera/delivery 的真实满通道测试 | 真实满通道回调→ShotRouter→Part 最终 ERR 91 / PLC 90 组合通过，恢复后下一件四点 OK；不声称覆盖 Machine 或实际 PLC 写入 |
| 多余触发 | ShotRouter extra 测试；camera/delivery 中即使额外帧被满通道拒绝，回调汇总仍拒绝该件 | S7 End `[3,1,1]` 和 `[4,0,0]` 未使用槽触发专项通过：不提交 OK/done，保留事务、只在中性输入及显式复位后恢复；多余回调即使丢弃仍使完整件 ERR 96 / PLC 90 |
| 错误相机、重连、上一件迟到 | ShotRouter 会话/相机/计数核验；Cycle 同 SN 不同 cycleId 的迟到结果回归；S7 arm 前重连测试 | 默认及 S7 回归通过；真实设备拔线仍属 W0 |
| 帧队列满（64） | `camera/delivery.rs` 使用真实 bounded channel，核对回调计数、拒绝整组三视角、Closed、排空后恢复及缺帧位置 | 八项通道测试通过，含 Part 最终错误判定和下一件恢复 OK；Machine 桌面压力另测 |
| 测量队列满（32） | `measure::tests::full_measure_queue_rejects_one_job_then_recovers_without_leaks_or_duplicate_results` | 默认回归通过；实际 CycleHost 压力另测 |
| 录制队列满（48） | Recorder `queue_full_rejects_all_views_and_finish_bypasses_the_frame_limit` | 默认回归通过；不能替代帧队列测试 |
| DLL 卡住、预热排队超时 | blocking Gate、许可耗尽、迟到结果、预热排队与执行独立截止（排队包含许可及 blocking pool 等待） | 八项预热 Gate 测试和测量超时许可回归通过；不声明注入了真实 DLL 永久卡死 |
| PLC 写拒绝、断线、ACK 错件 | S7 wire report/release 拒绝、done 后断线、匹配序号和持久 ACK 恢复 | 24 项 S7 回环通过；实际桌面重启到 SQLite 历史链路随步 6 验收 |
| 录制磁盘错误 | Recorder 真实文件系统错误；Audit 注入 outcome；Pending 录制 SQLite reopen 与事务回滚 | Windows 真实旧文件禁止 DELETE 共享使保留清理失败，当前完整录制仍可用且可哈希回放、Audit 告警独立且去重；全量 260 项通过。不声明真实磁盘满、断电或故障 D: 业务验收 |
| 单目及三目真实像素 | `production::regression::native_full_resolution_frozen_bundle_single_and_tricam_regression`：每模式至少 100 件、每件四次真实测量、1280×1024、冻结发布包；normal、gap、whole_empty | 默认金属纹理夹具首件严格 OK 失败；独立 clean_step_edge 夹具各 100 件通过，版本、DLL/夹具哈希、逐帧 JSONL、分位数与进程内存见 p0-step7-prepared-c.json；不声明修复 P0-09 |

`measure` 的 `ms` 保留从提交到报告的总耗时；`queueMs` 是提交到 blocking executor 实际进入，包含通道、许可与执行器等待；`engineMs` 是执行器进入到 runner 正常返回的墙钟时间；`coreMs` 是成功图像 runner 原有的测量耗时。未知值序列化为显式 null，0 是有效值。超时或 panic 不补造引擎耗时，迟到结果不回填已报告结果。历史合成测量没有这些观测，填 null。

较早真实 DLL 基准是串行调用 Prepared，报告的 `productionPartEndToJudgeMs` 保持 null。后续实际 CycleHost 400 件已经记录 partEnd 到 PLC 提交的尾延迟、逐帧排队/引擎耗时与内存趋势；Cycle 的 `drainMs` 包含 PLC 结果输出等待，不能标成纯判定时间。性能运行时不得并行执行 Cargo、前端覆盖率或其他高负载任务。

恢复存储后的顺序：步骤 5 最终四帧×两尺寸显示 → 步骤 6 全量回归及十二视角历史/原包桌面重现 → 步骤 7 补齐上述缺口、全量回归、单目/三目原生基准、实际 CycleHost 与五个演示故障场景。每次通过后记录真实结果并更新草稿 PR；现场 W0/W7 准确率及真实三目 SDK 交付形式继续独立验收。

演示依然使用开发 Modbus，不代表 S7 生产验收。`make-samples.py` 从桌面已冻结的 workspace 生成可核查夹具，不能从全新检出凭空生成已示教配方；使用参数和准备步骤见 `scripts/robot-plc-demo/README.md`。暂存目录、原始图像、大型二进制和本机客户数据不提交。

独立洁净性能控制通过 `GLUESIGHT_P0_FIXTURE_STYLE=clean_step_edge` 显式选择：1280×1024、灰度 210 背景、灰度 38 的 32 px 直胶带，0.125 mm/px，独立 `-CLEAN` 配方身份。默认 `simulated_metal` 保留纹理、噪声及首次失败。单目 / 三目各 100 件、400 次测量，最终 Prepared P95 11.603 / 11.855 ms；只读报告工具重新核验全部原图与冻结资源 SHA256，详见 [性能控制证据](evidence/p0-step7-prepared-c.json)。最终串行四次测量 P95 44.195 / 44.247 ms；此值不能当作实际 PLC 收尾延迟。

最终 Prepared 报告以 `f738e05` 为软件基线，完整源文件与测试程序字节封存在新报告目录的 `software/` 中，后续构建不会覆盖引用的证据。报告保留先前控制运行，未以较小耗时替换最终结果。PR #12 的旧录制保留清理错误与当前录制完整性已完成分离，最终 Rust 260 项通过，不改变该串行测量控制的算法/包/像素。

实际 CycleHost 首轮三目洁净回放在第三件布防核验超过原 200 ms，前两件严格 OK、12 幅原图逐幅匹配；第三件 ERR 99、零帧零测量、完成 ACK 后 IDLE，批次只完成 2 件。P0-14 修复保留首次完整语义加载及逐件全量清单/资源/DLL 校验，私有快照逐字节一致性、完整目录和 Windows 重解析点拒绝仍在；只在单次调用中复用祖先 metadata，不使用时间戳缓存。相同发布包 20 次只读核验从 76–119 ms 降到 19–54 ms，DLL 全读哈希 7–13 ms；额外内存为每个已加载发布包的一份共享不可变资源字节，后续周期不累积。15 项发布安全回归、实际 DLL 同大小改动拒绝专项、全量 Rust 263 项通过、30 项默认忽略。此段为修复后的历史剖析记录；后续原生 100 件恢复控制及 400 件基线已经完成，不能将本段只读剖析值当作 CycleHost 布防结果。见 [P0-14 失败与修复](evidence/p0-arm-verify-c.json)。

审查修复最终集成 `a3d4ad4`：步骤5预热纯排队超时冷却可重试，实际执行超时仍隔离；步骤6孤立pending分批回收和AppData启动独占锁已集成，完整Rust275项、真实DLL冻结/篡改专项、CycleHost证据工具51项通过。P0-14 d03fade原生恢复控制100/100件严格OK、400测量、1200原图逐幅匹配，独立validator通过；arm P50/P95/最大34/42/56ms，frame总耗时14/16/19ms、queue全0，partEnd到PLC提交15/31/31ms。主进程private增加16.21MB（多数发生在首10件），全部原始采样与斜率保留，不能据此声明无限时长无增长。此段之后四组400件基线及原版重复启动已通过，最新审计程序单件复验与五演示仍单独记录。

预热最终双预算修正 `6a110a6` 全量 Rust 276 项通过；排队最多 30 秒，实际 worker 开始后另有完整 30 秒，避免临近排队截止才开始的有效任务被永久误判。真实单线程 blocking pool 临界开始回归通过；实际超时仍持有许可并隔离迟到成功。前端960项、Prepared、发布逐件核验未变。


最终更新：审计重试集成 `e5af201` 完整 Rust 279 项通过、30 项默认忽略，前端 960 项保持适用。实际 CycleHost 四组合各 100 件全部通过（运行时 `6a110a6` / 构建源 `d41d19e`）：1600 测量、3200 原图、四组独立验证器通过；arm P95 为 66/75/76/75 ms、最大 112 ms，原 200 ms 限制未变。见 [400 件基线](evidence/p0-step7-cyclehost-c.json)。新审计程序单件复验及五演示待执行，旧失败证据保留；正常 400 件不证明实际队列溢出场景。审计持续退避仍受 128 待写/512 缓存/30 分钟未收到新事件的容量与 TTL 边界约束，无持久 spool。


## 2026-10-10 · 当前最终记录耐久接受补充

P0-15 已通过当前源码 `8ffc091` 的18项新增默认回归和实际原生恢复专项。完整结果进入有界spool后才提交PLC DONE；长期SQLite失败、坏日志、容量及持久写错误阻止后续布防。原图提升目录、Pending引用和历史删除同时受保护。提交耗时仍在实际PLC写完成后单独保存，不把提前采样当作总延迟。

实际S7请求前锁定SQLite，DONE前已有完整Insert且无主行；90/98安全拒绝被真实ACK后终止进程，重启恢复原判定、四shot、Failed录制和精确ACK。中断时ACK只在S7日志，因此实际证明先恢复主记录再恢复ACK；ACK已在spool、snapshot后追加ACK、提交后未清receipt的路径由真实文件/SQLite回归证明。原始报告、首次失败与边界见[审计证据](evidence/p0-audit-durability-c.json)和[验收说明](TESTING.md)。当前Rust335、S7回环24、真实DLL5通过；下文及前文旧版本279/964/400等分别保留来源，不标为新审计程序的400件或五工况结果。

该保证从最终Record或事件被耐久接受开始，不覆盖Acquire中尚未生成的记录，不声明断电零丢失。下一轮继续当前程序的单目/三目长时性能、实际Machine队列压力及五个Robot工况；W0/W7硬件和准确率门槛独立保留。
