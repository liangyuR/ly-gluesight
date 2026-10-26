# 决策索引

本目录只记现行决策：规则取舍、理由与回退方式。过期条目直接删除，历史以 Git 记录为准。现行生产模型只有段配方（分段连拍飞拍），基准是 [segment-burst.md](segment-burst.md)；PLC 握手以 [生产通讯协议 7](../integration/plc-communication-protocol.md) 为准，整圈示教握手见 [示教采集接口 V1](../integration/teaching-capture-sequence.md)。

| 文件 | 主题 | 仍有效的要点 |
| --- | --- | --- |
| [segment-burst.md](segment-burst.md) | 段配方（现行基准） | Line0 高电平门控、固定帧率（默认 10 fps）、三台独立单视角相机；SDK 帧号配组；未分段运行、整件 ΣN_exp 标定即可发布；缺帧与帧数越界只标记不改结论；IPC 契约、口径、队列与吞吐门槛 |
| [maintenance-mode.md](maintenance-mode.md) | 维护模式与采集版配方 | 模式按件锁存、维护件结果码 0、维护期不拒收、采集件只录图、记录库原地加 `mode` 列 |
| [core-flow.md](core-flow.md) | 核心业务流程 | 拒收顺序 95 → 100；故障只改写等确认的交付；空配方库；链路门与配置门 |
| [r2-plc.md](r2-plc.md) | PLC 会话 | 布防预算 500 ms（100–5000）、写后立即读回；离线事务记来源；主动断开 3.5 s 后重连一次；复位失败带原因；心跳按轮询缓存判断；plc.db 带 requestSeq |
| [r2-records.md](r2-records.md) | 记录、追溯与告警 | 录制失败告警；录制结果日志；中断件补 ERR 98；握手审计 4 MB 滚动、30 天 / 256 MB 清理；运行日志容量估算 |
| [log-replay.md](log-replay.md) | 运行日志、诊断包与回放 | JSON Lines、按日 16 MB 分段、30 天 / 1 GB；关联字段自动补齐；诊断包 ZIP；回放比对容差 0.01 mm |
| [soak.md](soak.md) | 长稳与故障注入 | 模拟当量 0.112；`offline_pace` 与注入钩子；资料盘低于 2 GB 暂停接件；持久追溯故障需重启；判定口径与退出码 |
| [r2-test-infra.md](r2-test-infra.md) | 隔离实例与桌面测试设施 | 子进程只继承指定句柄；CDP 地址取清单；WebView2 资料目录长度校验；隐藏窗口启动失败退出码 1；生产落盘用 Playwright Test |
| [ui-shell.md](ui-shell.md) | 外壳与生产页 | 动效只用 transform / opacity；局部超差警告色、故障紫；判定字 80 px；设备状态一行；判定卡固定高度 |
| [ui-recipe.md](ui-recipe.md) | 工作台与设备页 | 画布标记按屏幕像素；最大 400%；字号下限 12 px；发布前检查折叠；标定页布局；相机页 9 个问题的取舍 |
| [r2-ui.md](r2-ui.md) | 全局提示、SN 与笔刷 | 提示在主区顶部居中、同时一条；50 字符 SN 显示；默认笔刷 120 px 与失败提示 |
| [r2-perf.md](r2-perf.md) | 前端性能与内存 | 实时帧画同一块 canvas；读图节流；列表新查询返回前保留旧结果 |
| [qing-steel-080.md](qing-steel-080.md) | qing-steel 组件迁移 | 覆写组件样式提高权重；测试定位 Radix 组件的方法；保留本地实现的组件与 qing-steel 缺口 |
| [algorithm.md](algorithm.md) | 检测算法 | 误报修在 lyFlow；高光并集候选；默认检测参数不变；lyFlow ≥ 1.1.4 |
| [r2-algorithm.md](r2-algorithm.md) | 中线提取与胶宽限值 | 框选“顶帽找胶 → 笔刷对中”；笔刷提取容错；胶宽 4 ± 1.5 mm、允许超差 5 mm 不放宽 |

## 一、上线前必须处理

1. **PLC 方提供**（协议 7 第十节）：
   - 机器人 IO（工件到位、MotionPermit、MotionActive）及 PC 不在线时机器人跳过等待的实现方式；
   - “记未检 / 记 NG / 记检测错误”的去向（HMI、输出位或 MES）；
   - Line0 输出点与三台相机的接线；
   - DB4、DB2 在现场工程中的块号；
   - CPU 型号与固件，确认已允许 PUT/GET 访问。

   另外，PLC 等布防不得小于 PC 布防预算 + 2 s，建议保持 10 s（[r2-plc.md](r2-plc.md) R2P-1）。
2. **真机验证**：以下都还没有验证，各项验证只基于进程内虚拟 S7、合成图三相机与样本集回放。
   - 协议 7 与示教接口 V1 的现场 PLC 联调，Line0 门控与三台相机接线，现场节拍；
   - 布防各阶段的实际耗时（布防预算默认 500 ms 需在专机上复测，[r2-plc.md](r2-plc.md) R2P-1）；
   - **3 × 25 Hz 吞吐门槛**：没有达标，也没有被直接验证。门槛口径不改，在现场工控机用真实相机来源跑 `pnpm test:soak:throughput` 并重跑 `throughput_probe` 探针后，再决定降帧率还是加测量并发（[segment-burst.md](segment-burst.md) 第七节、第九节第 17 条）；
   - **分段算法**：只有 `Segmenter` 接口与 `Unsegmented` 实现，要等现场按量产曝光与速度拍的含弯道三相机图像序列再开发验证；验证前整条胶路按未分段处理，只跑每帧检测、不判胶偏（segment-burst 第一节、D-SB-21）；
   - 弯道帧走出走廊的判据（默认做法 4）没有真实弯道图验证。
3. **真机标定**：样本集没有标定，模拟当量只用于打通业务链路。段配方读设备标定 `calib/device/plane_calib_{device}.json`，只存过 `{device}-v1` 的单视角相机要在标定页重新标定一次（D-SB-30）。胶宽限值在真机标定后按正常件统计复核（[r2-algorithm.md](r2-algorithm.md) RA-5）。

## 二、需要用户决定

| 事项 | 现状 | 文档 |
| --- | --- | --- |
| 吞吐门槛复测后的取舍 | 现场工控机真实相机复测后，在 ① 降相机帧率（各段 N_exp 随之变，须重新标定）与 ② 加测量并发（开发机 4 物理核，4 → 6 线程增益有限，需改代码重测）之间选。离线实测：突发排空约 49 帧/s，按降级公式可持续约 11 Hz | segment-burst 第七节、第九节第 17 条 |
| 段配方的待确认取舍 | 胶偏 `ToEdge` 测量量未实现（只做 `AbsoluteX`）；提前结束只加 `short_part` 标记、不判 ERR，可能出现 PLC 记未检而 PC 记 OK；气泡、拉丝不检 | segment-burst 第三节、D-SB-15、D-SB-34、D-SB-37 |
| 运行日志容量 | 1 GB 上限在每天数千件时只能保留几天。建议提高到 4–8 GB，或超限时先删旧分段的 debug 行；`plc.db` 没有容量上限 | r2-records R-6 |
| 胶宽上限与允许连续超差 | 保持默认（上限 5.5 mm、允许 5 mm）。实拍正常件在起点、胶堆、拐角处常有 5.3–6 mm 的段落，余量小，需现场按产品复核；直线 / 弯道可分别调 | r2-algorithm RA-5、segment-burst D-SB-16 |
| 维护模式一期未做事项 | 找不到配方的件不录图；`plc.earlyEnd` 未写；采集占用空间未统计 | maintenance-mode “一期未做” |

## 三、已按建议实现的取舍

| 事项 | 采用 | 文档 |
| --- | --- | --- |
| 计划模型 | 段列表 + 三台独立单视角相机；没有段的配方文件读入时报“旧拍照点配方已停用”，代码已删除 | segment-burst D-SB-1、D-SB-11 |
| 多段配方的发布与运行 | 整件 ΣN_exp 由多轮标定得出即可发布，各段 N_exp 可为 null（全部标定轮次确认手动分段时逐段标定）；运行时整条胶路一个未分段区域，只核对整件帧组数 ∈ ΣN_exp ± Σtol | segment-burst 第二节、D-SB-21 |
| 缺帧、帧数越界、丢帧 | 某台相机缺帧只把该帧组标检测不完整；帧组数越界记“帧数异常”、帧组数 < ΣN_exp − Σtol 加 `short_part`；丢帧只告警；都不改 OK/NG。只有三台相机整件一帧都没测成才判 ERR 91 | segment-burst D-SB-3、D-SB-13、D-SB-34 |
| 拒收顺序 | 先核型号（95），再看配方能否编出计划（100） | core-flow D-CF-4 |
| 布防预算 | 写后立即轮询确认，握手日志落盘移出预算，默认 500 ms，范围 100–5000 ms | r2-plc R2P-1 |
| 离线未结事务 | 握手日志记事务来源；离线事务可进入离线由虚拟 PLC 复位，真机或来源不明的事务仍需真机复位 | r2-plc R2P-2 |
| PC 主动断开 | 3.5 s 后自动重连一次，重连后仍须明确复位 | r2-plc R2P-3 |
| 维护模式 | 按件锁存、结果码 0、不拒收、采集件只录图 | maintenance-mode MM-1–MM-4 |
| 进程中断的在途件 | 重启时补一条 ERR 98 历史记录，按开工时的来源记录 | r2-records R-4 |
| 握手审计保留 | 4 MB 滚动，30 天 / 256 MB 清理，保留未结交付需要的分段 | r2-records R-5 |
| 首启配方库 | 不写入样例配方；新建配方的模板是段配方 | core-flow D-CF-8、segment-burst D-SB-24 |
| 检测默认参数 | 不变（搜索半宽 15 mm、胶宽 1–10 mm），现场按产品收紧 | algorithm A-8 |
| 示教默认笔刷 | 固定 120 px；提取失败时引导加大笔刷或改用笔划生成 | r2-ui U-9、U-10 |
| 矩形框选提取 | 「顶帽找胶 → 笔刷提取对中」 | r2-algorithm RA-1 |
| 全局提示 | 主区顶部居中，同时只显示一条；只放瞬时的成功提示，错误留在页面内 | r2-ui U-1–U-3 |
| 局部超差的颜色 | 统一用警告色；未测成和设备故障用故障紫，与 NG 红分开 | ui-shell §4–5 |
| 生产页判定字号 | 80 px，右栏 420 px | ui-shell §6 |
| 相机页设计稿的 9 个问题 | 2 项已实现，7 项维持现状 | ui-recipe 相机页一节 |
| 运行日志保留 | 按日滚动，单个分段 16 MB，保留 30 天，总量上限 1 GB | log-replay §3 |
| 资料盘下限 | 剩余空间低于 2 GB 时暂停接件，恢复后自动就绪 | soak S-7 |
| 持久追溯故障 | 维持「排查后重启恢复」；布防预算超时只拒绝本件，不锁死追溯；审计 spool 残留未完成文件时保留证据并拒绝就绪 | soak S-8、S-10 |

## 四、依赖版本

| 依赖 | 版本 |
| --- | --- |
| lyFlow 运行库与 `lyflow-client` | v1.1.5（1.1 系列，见 [lyFlow 依赖](../architecture/lyflow-dependency.md)） |
| ly-plc | v0.4.0（fe6c210） |
| qing-steel | v0.9.0 |
