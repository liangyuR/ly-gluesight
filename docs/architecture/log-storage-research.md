# 运行日志与视觉存储调研

调研日期：2026-10-27。代码基线：`0827951`。本文记录现有存储关系，以及将日志集中到 `~/gs_logs` 的建议和待决策项；集中存储与棋盘格原图持久化尚未实施。当前生效的目录契约见[资料目录](storage-layout.md)。

## 目标与边界

将运行日志和生产视觉证据集中存放，便于现场查找、导出和控制容量；示教图、标定图及发布所需资源继续随 project 保存、复制和导出。

本文的“vision 日志”指生产与离线历史原图及其录制元数据。现有项目目录中的 `vision/releases/` 是不可变发布资源，仍属于项目资产。

## 当前路径

资料根目录由 Windows ProgramData 已知文件夹接口解析：正式版为 `%ProgramData%\ly\GlueSight`，开发版为 `%ProgramData%\ly\GlueSight-dev`。开发及测试可显式设置绝对路径 `GLUESIGHT_DATA_DIR`，安装版拒绝该覆盖变量。当前布局按整机共享，正式、开发和测试资料隔离。

下表中的路径均相对资料根目录，`<project>` 为项目编号。

| 内容 | 当前路径 | 归属及用途 |
| --- | --- | --- |
| 程序运行日志 | `logs/gluesight-YYYY-MM-DD[.N].jsonl` | 本机日志，多个项目共用目录；`app.config` 快照记录当前项目 |
| PLC 通讯日志 | `logs/plc.db` | 本机 SQLite 日志库；运行时可能有 `plc.db-wal`、`plc.db-shm` |
| PLC 握手状态与审计 | `state/` | 含恢复状态、握手审计及全局计划版本计数，不能整体视为可清理日志 |
| 真机历史原图 | `projects/<project>/records/` | 当前项目的生产、维护采集及维护试判原图 |
| 离线历史原图 | `projects/<project>/records-offline/` | 当前项目的离线录制原图，与真机分开清理 |
| 检测历史数据库 | `projects/<project>/inspection.db` | 项目历史、逐帧指标及录制引用 |
| 采集轮次原图 | `projects/<project>/recipe-capture/<轮次>/<相机>/<seq>.pgm` | 示教、多轮标定及独立验证使用的项目资产；轮次另有 `round.json`、`frames.jsonl` |
| 冻结示教图 | `projects/<project>/workspaces/<配方>/images/` | 候选引用的示教帧 |
| 多轮标定特征 | `projects/<project>/workspaces/<配方>/calibration/<轮次>/` | 特征与分段结果；原图来自项目采集轮次 |
| 工位标定结果 | `projects/<project>/calib/` | 棋盘格标定结果 JSON，含设备标定及显式标定引用 |
| 发布资源 | `projects/<project>/vision/releases/<配方>/<发布ID>/` | 配方、图、冻结示教图、走廊及标定引用 |
| 待入库证据 | `projects/<project>/audit-spool/` | 生产追溯恢复队列，不属于普通日志清理范围 |
| 受管离线样本 | `samples/` | 外部样本的受管副本；候选离线调试可直接引用，不重复录制 |

历史录制目录按日期分层，每件目录为 `*_cycle_<cycleId>/`，包含 `{相机}_s{seq:06}.pgm` 和 `part.json`。元数据记录相机、帧号、PLC 请求、检测模式、缺帧及原图落盘情况。

### 棋盘格标定原图的缺口

工位标定页面拍照或导入后的冻结样本，目前只保存在 `WorkspaceHost.station` 内存中。`vision_calibrate` 将标定结果、`sampleId`、相机编号和视图写入 JSON，但没有将这张棋盘格原图保存为项目文件。

因此，“标定图继续跟 project 走”包含两种情况：多轮标定原图已经在 `recipe-capture/`；棋盘格标定原图还需增加持久化，不能只调整现有目录。

### 当前保留规则

- 运行日志按本地日期与大小滚动：保留 30 天、总量上限 1 GiB、单段上限 16 MiB；只清理 `gluesight-` 前缀文件，不清理 PLC 日志库。
- PLC 日志保留天数来自 PLC 配置 `log_retention_days`。
- 历史数据库默认保留 180 天；真机原图默认最多保留 100 件、20 GB，离线原图默认 300 件、10 GB，取当前项目的设置。原图清理还保护最新一件、在用及待入库目录，并同步数据库引用。
- 带 `maintenance-collect` 标记的维护采集原图跳过自动滚动清理，不能把上述容量上限理解为全部原图的硬上限。
- 示教、标定、采集轮次、样本和发布资源不按历史原图策略清理。

## 建议布局（未实施）

`~` 表示运行程序的 Windows 用户主目录，应用需解析为绝对路径，不将字面量 `~` 直接拼入文件路径。

```text
~/gs_logs/
├─ runtime/
│  ├─ production/
│  │  ├─ gluesight-YYYY-MM-DD[.N].jsonl
│  │  └─ plc.db
│  └─ development/
└─ vision/
   ├─ production/<project>/
   │  ├─ records/
   │  └─ records-offline/
   └─ development/<project>/
```

运行日志仍按环境汇集；视觉证据按环境、项目、真机或离线分开。自动化测试继续使用隔离的测试输出目录，不能写入日常 `gs_logs`。如果允许同一环境使用多个资料根目录，外部日志命名空间还需包含资料实例身份，避免不同实例的同名项目共用录制目录或日志库。

目前运行日志没有统一的 `projectId` 事件字段，项目身份主要通过 `app.config` 快照关联。集中存储若要支持直接按项目检索，应补齐事件的项目和运行会话标识，不能仅靠目录名推断。

项目根目录继续留在当前资料根下，保留 `config/`、`recipes/`、`workspaces/`、`calib/`、`recipe-capture/`、`vision/releases/`、`inspection.db` 和 `audit-spool/`。本机 `state/` 的恢复语义维持原契约；是否单独外置握手审计日志另行确定。

棋盘格原图可新增保存到 `projects/<project>/calib/images/<样本ID>.pgm`，标定结果以项目内相对引用关联，并随项目导出和复制。具体的保存时机、替换与保留规则需与标定结果一致，避免只留下引用或孤立原图。

## 实施影响

### 历史查询与旧数据重定位

`parts.recording_directory` 保存录制目录路径，逐帧 `raw_file` 相对对应录制根目录解析。历史读图会核对路径边界、`cycleId`、相机、帧组和元数据身份。新布局应保持原图内部层次及身份校验，并统一提供按来源解析的录制根目录。

迁移需同步重定位数据库与审计 spool 中明确的录制引用。只移动目录会导致历史原图不可读、待入库保护失败或清理范围不一致。

### 历史件转示教和标定

“从历史选用”通过硬链接把录制原图物化到项目的 `recipe-capture/`。这让历史录制清理后，项目轮次仍可独立使用，但当前实现明确要求录制目录与采集目录在同一卷。

外置目录可能位于另一卷，需增加跨卷复制路径：同卷可继续硬链接，跨卷时复制并核验文件；完成后，采集轮次只引用项目内文件，不能依赖日志目录永久保留。导入中任一步失败应撤回未完成轮次，不改变原始历史件。

### 项目管理与诊断导出

当前项目导出和复制的文件白名单从项目根目录收集 `records/`、`records-offline/`。外置后需调整以下行为：

- “包含检测记录与历史原图”仍能收集该项目的外部原图，导入或复制后重定位到目标项目的外部录制目录。
- 不包含历史记录时，仍完整带走示教、标定、采集轮次和发布资源。
- 项目占用空间统计区分项目资产与外部视觉证据；删除项目时明确外部证据的处理规则，避免遗漏或误删其他项目的数据。
- 诊断包、日志查询、系统设置中的路径展示及“打开目录”入口使用同一套路径解析，不继续假设所有文件都在资料根下。

### 磁盘空间、保留策略与账号

空间守护当前检查项目所在磁盘。视觉原图外置后，需检查实际录制盘和项目盘；各自的空间不足原因应能定位到具体目录。原图清理继续按项目和来源执行，保留待入库保护、维护采集保护及数据库同步，不能跨项目清理。

ProgramData 当前提供整机共享资料，而用户主目录按 Windows 账号隔离。使用默认 `~/gs_logs` 后，换账号可能无法访问原日志。需明确现场是否固定运行账号，以及是否允许设置统一的绝对日志根目录。

运行日志初始化早于部分业务状态，新的日志根配置应能在启动阶段读取。现有资料根目录的独占锁也不能自动保护另一个实例共用的外部日志根，需要保持日志库及录制目录的独占边界。

## 建议迁移步骤与验收

1. 确定日志根目录、账号范围、环境和项目命名空间；先检查目标可写、磁盘空间和目标冲突。
2. 停止接件和采集，写完录制与审计，关闭相关文件句柄；对 SQLite 做包含 WAL 的一致性快照。
3. 复制到暂存目录并核验，重定位受管引用，保留旧文件和迁移报告；不覆盖或合并既有目标资料。
4. 全部校验成功后启用新根目录；失败保持原存储有效，启用后不自动回退到旧目录产生数据分叉。旧目录清理单独处理。

验收至少覆盖：

- 正式、开发、测试及不同项目之间不混用日志或原图。
- 迁移前后的历史图像按相机和帧组读回一致，重判、原图复测及诊断导出仍可用。
- 同卷和跨卷都能把历史件导入项目，清理历史原图后示教、标定轮次仍完整。
- 项目导出、导入、复制和删除正确处理外部原图，未选择历史记录时仍保留全部项目资产。
- 棋盘格标定原图重启后可读，随项目迁移，引用与标定结果一致。
- 清理、磁盘不足、目录不可写及迁移中断时，保护未结事务和待入库证据。

## 待确定事项

1. `~/gs_logs` 是固定默认值，还是允许现场配置其他绝对路径；多账号是否共用同一日志根。
2. 本次是否只外置运行/PLC 日志和历史原图；握手审计继续留在 `state/`，还是单独迁移其日志文件。
3. 删除项目时是否同时删除外部视觉证据；维护采集原图是否增加独立的归档或清理入口。
4. 棋盘格原图保存全部成功标定样本，还是只保留当前有效结果所引用的样本。

## 代码核对入口

| 范围 | 代码与关键入口 |
| --- | --- |
| 资料与项目根 | [`storage.rs`](../../src-tauri/src/storage.rs)：`selected_root`、`root`、`project`；[`project.rs`](../../src-tauri/src/project.rs) |
| 运行与 PLC 日志 | [`applog.rs`](../../src-tauri/src/applog.rs)：`LIMITS`、`dir`；[`plc.rs`](../../src-tauri/src/plc.rs)：`PlcHost::init` |
| 录制根、容量与空间 | [`cycle.rs`](../../src-tauri/src/cycle.rs)：`CycleHost::init`；[`recorder.rs`](../../src-tauri/src/recorder.rs)：`retention`、`prune`；[`disk_guard.rs`](../../src-tauri/src/disk_guard.rs) |
| 历史引用与校验 | [`store.rs`](../../src-tauri/src/store.rs)；[`workspace/recorded.rs`](../../src-tauri/src/workspace/recorded.rs)：`load_frame_verified` |
| 历史件物化 | [`recipe_capture/burst/history.rs`](../../src-tauri/src/recipe_capture/burst/history.rs)：`materialize` |
| 棋盘格标定样本 | [`workspace.rs`](../../src-tauri/src/workspace.rs)：`WorkspaceHost.station`、`workspace_station_capture`、`workspace_station_import`；[`vision.rs`](../../src-tauri/src/vision.rs)：`vision_calibrate` |
| 打包与重定位 | [`project/transfer.rs`](../../src-tauri/src/project/transfer.rs)：`CONTENT`、`RECORDS`；[`storage/relocate.rs`](../../src-tauri/src/storage/relocate.rs)；[`diagnostics.rs`](../../src-tauri/src/diagnostics.rs) |
