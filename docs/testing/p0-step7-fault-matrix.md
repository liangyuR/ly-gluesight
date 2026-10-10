# P0 步骤 7：验收中的软件故障矩阵

> 当前版本已按 AGENTS.md 移除业务内容哈希匹配。本文既有摘要、字节篡改门禁和旧版本验收仅为历史记录；新实现以明确 ID/版本、实际结构、路径/尺寸与设备计数为准。旧性能数据不能代表本次去哈希版本。

2026-10-10 PR #14 第三轮审查已修复外部手改配方未升版、旧候选基线猜测和模拟器缺省版本不一致。源码提交 `29bfdd95550b2eea6862e6d07fd5925a5f2ef9f2`：默认 Rust 331 项通过、0 失败、30 项忽略（11.36 秒，构建 53.28 秒）；Python 模拟器 36 项通过（13.381 秒）；提交后真实 DLL 六项通过（1.47 秒，构建 1.06 秒）。Rust 测试执行于提交前的同一最终代码树，五项新增 Rust 回归均实际通过。 详见[第三轮审查证据](evidence/p0-no-hash-review3-c.json)。

第三轮最终补修提交 `e83216c7e7de90563f4802f2c8773fd79b96cff1` 修复新编号首次保存后来源落盘失败导致同进程无法重试的问题；默认 Rust 334 项通过、0 失败、30 项忽略（11.06 秒，构建 52.71 秒），三项新增重试回归均实际通过。仅回滚本次新建且实际结构仍一致的普通配方文件；已有文件、外部改动及读取/删除失败均保留并明确报告。此前 `29bfdd9` 的 331 项 Rust、36 项 Python、六项真实 DLL 结果保留为前一阶段证据；最终 `e83216c` 未重跑 Python 或 DLL，不将前一提交结果标记为最终源码的新验收。

配方来源以持久保存的完整实际字段进行结构比较，不计算内容摘要；外部修改、版本回退及删除重建会获得新的明确版本。首次升级若没有来源记录、但历史下限已占用当前版本，会保守升版，新生产版本需要重新示教、验证及发布。旧 `baseHash` 没有明确有效的生产基线时拒绝猜测或自动迁移，普通候选、待生效版本和归档均适用；原文件保留，未加载的候选不得被打开操作覆盖，须先备份旧目录后基于当前生产版本明确重建、重新示教验证。模拟器缺省版本与校验规则统一为 0。

本轮未重跑原生桌面、S7 回环、400 件或五工况联调，不将旧版本结果计入本次修复；前端未变，覆盖率和构建也未重跑。P0-09 及现场硬件/新版长时性能边界保持。

2026-10-10 PR #14 第二轮审查修复：默认 Rust 326 项通过、0 失败、30 项忽略（3.74 秒）；Node 工具回归 57 项、Demo 启动守卫 3 项通过，后者没有启动或停止真实进程。真实 DLL 六项通过（1.70 秒），实际源码提交 `3632f0fe2ad10cf58873fff7d724905541d95e42`；本轮未重跑 S7、前端覆盖率、原生桌面或长时性能。首次 Rust 323 项通过、3 项失败的原日志保留：两项 SQLite 测试使用了非法空 judgement，另一项错误文案未保留既有测试要求的“身份”字样；修正夹具及文案后全量通过，既有断言和生产拒绝规则未放宽。详见[第二轮审查证据](evidence/p0-no-hash-review2-c.json)。

本轮六项修复覆盖：旧配方仅按真实精确 ID/版本关联、历史原图完整身份、录制原图实际解码与尺寸、新发布包显式 bundleId、Demo 拒绝复用仍存活的旧进程、按当前 C 盘 APPDATA 核对独立性能 profile 的 recordsRoot。历史原图必须有 part.json，逐字段核对 cycleId、k、shotId、camera、selectedView、session、ordinal、frameCounter、triggerCounter 及 view/file/尺寸；缺失或不符的条目保留，但不可复测。录制收尾拒绝同长度损坏或尺寸变化的原图，其余视角仍保留为部分证据，原判定不变。

旧 schema 的 NULL digest 记录若已经由早期迁移填入修订号，无法仅凭旧列分辨合成关联与当时新写入的记录。本次纠正保守解除没有真实精确快照或来源证明的复测关联，包含这个无法区分的过渡窗口；原 JSON、记录、原图引用和 ACK 均保留，也不回退当前配方。后续 Store.insert 在同一事务内写 explicit_recipe_records 来源表与快照、主记录、拍照点；写入失败全部回滚，历史清理级联删除来源行。此边界不声明 PR #15 持久审计已合入或运行，也不改变 P0-09 默认带噪良品预期严格 OK 的未解决状态。

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

2026-10-10 PR #15审查补充：`fc1e38a` 修复spool失败发布与ready/历史清理检查的并发空窗。append/probe/read/remove在同一spool同步边界锁存故障，超时发布独立于磁盘锁；3项新增回归及完整Rust338项通过。原生恢复报告仍归属 `8ffc091`，见[并发修复证据](evidence/p0-audit-review-atomic-c.json)。


2026-10-10 PR #15第二项P1补充：`04c3839` 覆盖ACK后录制终态回调仍未发生的窗口。Recorder开始前登记，Recording事件真正耐久接受后同spool锁解除；下一件布防及历史清理同时检查未决录制。真实Recorder延迟回调/SQLite ACK与空spool、终态容量拒写及Off同步回调3项回归、完整Rust341项通过。慢录制时暂态不就绪，原判定和ACK保持，收尾入库后恢复。见[录制屏障证据](evidence/p0-audit-recording-barrier-c.json)；原生压力与性能待包含全部最新审查修复的源码执行。

## 2026-10-10 · P0-18 当前源码压力准备与40图导入

此前队列压力准备尚未进入实际故障场景：attempt2/3 被 UI 精确错误“正在处理 PLC 事务，请稍后重试取图”阻断，属于生产互斥保护；先离线示教，再连接 PLC。工具仅在按钮重新可用、无在途工件且 PLC 未锁、IDLE 或相机停止的明确安全 FAULT 下有界重试，保留每次拒绝，不吞其他错误。

attempt4 的实际源码为 `9c90f66d72c8c1e55b36e22dd2d3988c93634f7e`：四点采图/试测4项全部通过且准备成功；40点采图/真实DLL试测40项全部通过，记录17次显式取图重试。随后样本导入的页面/上下文关闭。后端已持久保存normal样本 `1791632654217-163-46`，候选修订164，40张1280×1024 PGM共52,429,480字节；响应中断不能等同于保存失败。三次尝试的实际压力场景均为0，不声明压力、Prepared或400件通过。

样本输入改为传输原图编码字节的base64字符串，后端仍逐图实际解码、保存原像素；空/非法base64与超过对应15MB的编码长度提前拒绝，解码后单图15MB、整组80MB、完整/唯一拍照点、实际尺寸及事务失败清理均保留，不新增内容哈希匹配。页面关闭与大数字数组传输的内存开销可能相关，但没有证据确认OOM。后续工具新增page crash/close、browser disconnected和受控子进程exit观测，该修复当时仍待原生40图导入及压力重测；attempt5的后续结果见下文。

当前compact修复源码 `a9a69b597c31dbcb1c9a2fffd31d8bb73fd7c29c` 的默认Rust353项通过、0失败、31忽略，6.12秒；完整前端覆盖率42个文件/968项通过，119.13秒；statements/branches/functions/lines为91.34/89.80/88.58/93.96%，全部全局及专项门槛通过，默认maxWorkers=2未改。应用及测试两套TypeScript检查均exit0（合计2.73秒）。此前352项feature集成检查归属旧源码，不计作compact修复后通过。原始报告、已落盘边界和后续待测项见[压力准备证据](evidence/p0-pressure-preparation-c.json)。默认带噪良品P0-09及现场W0/W7保持独立缺口，后续实际压力、Prepared及400件结果另建证据。

attempt5 使用源码 `3b736f144d53de9971afcb9d9e8aab00e8f1f640` 实际重测：40点采图及真实DLL试测全部通过，7次显式取图重试；normal/gap各40张原图通过UI导入，严格验证分别为OK/NG_GAP，发布生产版本1。候选验证修订165，发布后的候选版本2不能当作已发布版本。生命周期记录在清理前没有页面崩溃或上下文关闭，P0-18的40图准备重测已通过；此前attempt2/3/4失败报告保持原样。压力验收仍因下述P0-19中断。

## 2026-10-10 · P0-19 压力工具误判保留的空闲快照

attempt5 首个 `callback-missing-91` 场景实际将回调队列填满64项，再触发4次回调并观察掉帧4次、工件收到0帧；真实PLC DONE为90/91，历史精确cycleId/SN保存原ERR_INSPECT和acknowledged。ACK释放后机器已IDLE、plcLocked=false，回调/测量队列及运行worker均为0，归档audit-spool仅有零字节`.health`。Snapshot有意保留最后工件及结果：filled/total为404/404，四帧均missing。工具却要求`!cycle.part`，因此20秒后错误超时。

这次报告整体passed=false，cases数组为空是因为工具在收尾和正常恢复完成后才登记case，不代表没有执行首个故障注入。正常恢复未执行，额外回调及测量队列压力两个场景未执行，不能记为三项压力验收通过。工具修复提交 `eacb039cb6353648193334ff7a554f3e449e2931` 按严格空闲判定核对原cycleId/SN、完整filled/result、零队列/worker、ACK及spool；25项工具回归通过、0失败，121.8407ms（`p0-quiescent-tools-c.log`）。生产实现和默认200/30000/1000/3000/5000ms超时不变，原生压力仍需重跑。原生PID43704及Modbus PID37112由工具核实路径/启动时间后仅停止自身进程，生命周期close/disconnect/exit均发生在cleanup阶段。见[空闲快照判定失败证据](evidence/p0-pressure-idle-snapshot-c.json)。洁净normal样本不能替代默认带噪良品P0-09；Prepared串行结果、CycleHost长时性能及W0/W7保持独立验收。

## 2026-10-10 · P0-20 切换40点配方后的预热等待错误

attempt6 源码 `eacb039cb6353648193334ff7a554f3e449e2931` 已原生复验P0-19工具修复：callback-missing-91及callback-extra-96分别实际返回90/91和90/96，原历史ACK与严格空闲收尾通过；各自后续四拍正常件返回1/0、历史ACK、完整录制及四项实际测量时序均通过。故障与恢复分别保持相机会话4、5，没有靠重启相机恢复。报告cases为4条，即两项故障加两项正常恢复，不能计成四项压力场景。

第三项measure-full-99切换到40点配方后，在begin布防前实际被90/95拒绝，原因明确为“发布资源与图像引擎正在预热，完成前不能布防”；此时全局visionReady=true仅表示其他配方可就绪，不能证明所选40点配方已完成预热。尚未触发该项40次回调或实际测量队列溢出，不能把90/95当作预期90/99通过。整体报告passed=false。P0-20工具修复 `1e1162da5466424b626248ff550a9bc1fa4860f5` 新增只读`waitForPublishedRecipeReady`：需明确bundleId对应的预热成功日志、同ID/version/revision布局、无目标告警、引擎ready/measuring能力均为true及严格IDLE；其他目标错误立即失败，不发送试探工件。最多90秒仅用于工件前准备等待，原默认timeouts与生产拒绝/ACK规则不变。共享工具53项回归通过、0失败，148.2214ms（`p0-published-warmup-tools-c.log`）；400件runner还要求构建manifest.features必须为[]。当时包含第三项的attempt7原生重测待执行；其后正式通过结果见下节。原报告及实际两故障/两恢复身份、时序见[预热等待失败证据](evidence/p0-pressure-warmup-wait-c.json)，attempt5/P0-19原始证据不改。默认带噪P0-09、CycleHost400及现场W0/W7不因本次部分通过而关闭。

## 2026-10-10 · 当前源码原生队列压力正式通过

attempt7 源码 `1e1162da5466424b626248ff550a9bc1fa4860f5` 在独占运行窗口实际通过三项压力及三次同会话正常恢复。构建开启专用`p0-pressure-test`，实际经过普通Replay相机回调、64项回调队列/32项测量队列、真实LyFlow DLL1.1.0及外部回环Modbus TCP写回/ACK；不是默认features构建的400件性能验收。

| 场景 | 实际故障结果 | 同会话下一件 | 关键实际观察 |
| --- | --- | --- | --- |
| callback-missing-91 | ERR_INSPECT，90/91 | OK，1/0，会话4 | 回调队列预填64，再4次触发掉4帧，received=0；四shot明确missing，录制failed无原图 |
| callback-extra-96 | ERR_INSPECT，90/96 | OK，1/0，会话5 | 4项计划帧已收到，额外第5次回调被拒1次；超过本件计划截止计数，不吞掉异常 |
| measure-full-99 | ERR_INSPECT，90/99 | OK，1/0，会话6 | 40次回调：消费者持有1项、队列32项、Full拒绝7项；33项done、7项明确error，40张原图完整保存 |

六件原结果、精确cycleId/SN及历史acknowledged均核对通过，ACK后严格IDLE、plcLocked=false、运行worker/两队列为0、spool无待入库事件。恢复件不重启进程/相机、不发故障复位，分别4/4/40帧全部完成、完整录制及实际非负有限测量时序通过。测量队列持有上界86ms小于原proc3000ms，7项真实Full错误没有用超时替代。40点工件前只读等待4858ms/26次轮询，明确目标bundle预热成功日志耗时12407ms，随后按同ID/version/revision及就绪门槛布防；原默认200/30000/1000/3000/5000ms未改。

本轮只读复核六份逐件结果、五份实际part.json及92张1280×1024 PGM的大小/头部（120,587,804字节），不计算内容哈希。首项没有收到帧，故没有part.json/原图，保留明确录制失败。其余五件的cycle/bundle/revision、shot/camera/view/session/ordinal/帧及触发计数与历史对应。最终统计6件=3OK+3ERR、机器IDLE，原生及Modbus子进程由自身身份守卫停止。正式证据见[当前队列压力验收](evidence/p0-queue-pressure-current-c.json)。P0-18准备与P0-19/P0-20工具修复均已在本轮原生复验，attempt2–6失败报告不改。

后续400件工具预防修复 `09dbddb517a8644cc40f45e8864f3d1a50ab01f1`：每组先离线且严格空闲，通过正常Settings/检测节拍UI保存原值，完整settings读回相等后配置Replay；连接后要求目标bundle预热成功日志时间不早于本组saveStartedAt，避免300条日志窗口挤出旧成功日志。68项工具回归通过、0失败，155.3036ms（`p0-current-prewarm-tools-c.log`）；400件manifest.features必须为[]，本轮400原生实测仍待执行。该预防工具提交不能替换本次压力实测源1e1162d。洁净对照不关闭默认带噪良品P0-09，也不替代Robot五工况、现场W0/W7或硬件验收。

P0-19同类工具补充：Robot准备脚本修复提交 `f7785b2d616f82eb2ca9669969f3aa69ec239972` 复用严格空闲判定并要求sim.running===false，接受身份一致且filled/total完整的IDLE/FAULT保留last part/result；锁定或未知锁、ACQUIRE、排队/未填满、SN/cycle/recipe不一致及模拟器运行均拒绝。4项工具自检通过、0失败，10.8577ms（`p0-robot-quiescent-tools-c.log`），其中site守卫组覆盖上述边界。未运行真实Robot准备/发布/五工况，不计Robot原生通过或400件通过；已完成的压力实测仍归属1e1162d，原证据不改。

## 2026-10-10 · PR #15 第三轮审查：P0-21 / P0-22

源码 `bdfd76b35951d389da5d7fca7e27d17571ef067b` 的完整默认 Rust **358通过/0失败/33忽略**（12.63秒），完整真实 S7 回环 **25通过/0失败**（54.17秒）。新增8项默认与2项S7测试，证据和日志见[第三轮审查记录](evidence/p0-audit-review3-c.json)。

| 故障 / 等待 | 修复及实际验证 | 边界 |
| --- | --- | --- |
| P0-21：Off/提前失败回调先于 Insert，Acquire 中断留下旧 Recording-only spool | 新同步终态先暂存；Insert与暂存终态同锁顺序耐久接受，终态拒写保留已接受的Insert与屏障。真实Recorder回调中断及SQLite重启回归通过 | Acquire尚未生成的最终记录不伪造；旧孤立事件只在数据库无主行且全部Recording一致合法时归档，混合Submission/Delivery、冲突、查询错误仍拒绝 |
| P0-21：健康探针写入后中断 | 仅精确单字节 `[0]` 可截空并sync，原审计事件保持；未知字节拒绝且原文保留，重启回归通过 | 不把未知或损坏文件当作探针清除 |
| P0-22：S7 ACK后等待录制/入库 | 等待时Ready低且ACK/pending保留，完成后自动释放，同SN下一序号完整件通过；实际设备故障仍进入Fault | 真实S7本机回环，未重跑原生长时性能或现场PLC |
| P0-22：同步begin回调审计失败后仍准备启动相机 | camera.begin_part前检查故障、writer及实际磁盘探针；真实Recorder同步回调和Windows写拒绝回归通过 | 本件正常录制屏障不被误判健康失败；故障仍锁存且不发布DONE |

旧中断归档 `.interrupted-preinsert` 保留原始事件字节、cycleId和路径，归档引用及原图目录参与清理保护，日志说明该件未生成最终记录。活跃spool与归档分别受4096事件/64MiB约束，不是合计64MiB；归档原图不自动删除，满、坏文件、未知文件或路径重解析错误仍保留并拒绝恢复。无内容摘要计算或匹配。

本轮没有新增当前源码400件、Robot五工况、原生进程中断或W0/W7现场硬件证据；较早原生报告保持原来源。P0-09带噪normal严格OK要求不变，不能用clean夹具或实际2替代通过。
