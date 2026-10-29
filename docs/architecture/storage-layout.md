# GlueSight 资料目录

Windows 的 ProgramData 由系统已知文件夹接口解析，不硬编码盘符。正式版使用 `%ProgramData%\ly\GlueSight`，开发版使用 `%ProgramData%\ly\GlueSight-dev`。两套资料独立，不自动同步。程序、运行库和源码不属于资料目录。

运行日志与历史原图集中在 `~/gs_logs`，按环境、资料实例和项目隔离；示教、标定和发布资源继续随项目保存。根绑定和清理规则见[运行日志与视觉证据存储](log-storage-research.md)。

## 初始化与日常运行

正式安装包采用整机安装，一次性提权创建并授权资料目录；日常运行不需要管理员权限。升级、卸载保留资料。

本地开发首次在管理员 PowerShell 执行 `pnpm storage:init`，之后正常执行 `pnpm dev`。初始化命令只创建目录并授权，不移动或删除资料。无法写入时应修复权限，不会回退到 AppData。

资料分为本机资料和项目资料。项目是一个工位的完整检测方案（相机组、PLC、标定、配方及其检测历史），设计见 [项目（Project）设计](project.md)。任一时刻只打开一个当前项目，切换项目会重启软件。

**本机资料**（资料目录根下）：

| 位置 | 用途 |
| --- | --- |
| `config/` | 本机配置：`current-project.json`（当前项目编号）、`site-offline.json`（离线选择）、`preferences-<SID>.json`（按账号的界面偏好，最后打开的工作区按项目分别记）、`log-storage.json`（日志根、环境和资料实例绑定） |
| `projects/<项目编号>/` | 各项目的资料，见下表；`projects/.trash/` 是删除中的项目，启动时清理 |
| `state/` | PLC 握手状态与审计，以及所有项目共用的计划版本计数 `plan-version`（换项目不回退）。握手审计 `plc-handshake.audit.jsonl` 超过 4 MB 滚动到 `plc-handshake-audit/`，分段早于 30 天或总量超过 256 MB 时随检测记录清理删除，仍有未结 PLC 交付引用的分段保留；诊断包带上当前文件和与导出时间段有交集的分段，其余分段在清单 notes 中计数 |
| `samples/` | 外部样本导入后的受管副本 |
| 日志根外置 | `~/gs_logs/runtime/<环境>/<实例>/` 保存运行与 PLC 日志，`vision/<环境>/<实例>/<项目>/` 保存真机、离线历史原图 |
| `cache/` | 可重建缓存 |
| `webview/<SID>/` | 按 Windows 用户隔离的浏览器资料 |
| `exports/` | 默认导出位置；项目导出包的保存对话框默认打开这里 |
| `migration/bootstrap/` | 资料准备窗口的 WebView 配置 |

**项目资料**（`projects/<项目编号>/` 下）：

| 位置 | 用途 |
| --- | --- |
| `project.json` | 项目编号、名称、描述、创建时间与来源 |
| `config/` | 相机组 `cameras.json`（含按相机编号保存的曝光与增益 `imaging`）、PLC `plc.json`、节拍与检测设置 `cycle.json`（含历史保留天数；`maintenance` 为维护模式的开启时间与原因，未开启时为空）、飞拍估算参数 `feasibility.json` |
| `recipes/`、`workspaces/` | 生产配方、候选配方及示教资料。`recipes/<配方>.json` 的 `stage` 为 `"collect"` 时是采集版（只供维护模式采图，没有发布包）；正式版不写这一项，没有该项的文件按正式版读。候选目录 `workspaces/<配方>/` 另有 `calibration/<轮次>/`：每轮标定的 `features.jsonl`（一行一帧 `{camera, seq, metrics}`，1..=帧组数内每台相机每个 seq 一行，缺帧也有）与 `boundaries.json`（分段结果，本轮恒为 `{"kind":"unsegmented"}`，供以后的分段算法和重新标定使用）；各相机的示教帧冻结为 `images/` 下的 PGM |
| `calib/`、`vision/releases/` | 标定、发布资源。棋盘格标定成功时原图写入 `calib/images/<样本ID>.pgm`，结果 JSON 的 `samplePath` 保存项目相对路径；临时冻结样本仍在内存。发布包在 `vision/releases/<配方>/<发布 ID>/`（schema 5）：`manifest.json`、`recipe.json`、`graph.json`，每台计划相机一个 `cameras/NN/`（NN 为计划相机下标，从 00 起），内有 `calibration.json`（像素当量与标定引用），它检测的每一段一个 `sMM/`（MM 为段号）子目录，内有 `image.<扩展名>`（示教帧）与 `corridor.json`（走廊）。工位标定缺省为设备标定 `calib/device/plane_calib_{device}.json`（发布包 `calibration.json` 的 `reference` 记 `device/{device}`）；显式标定引用 `{引用}` 为 `calib/plane_calib_{引用}.json` |
| `workspaces/<配方>/navigation/`、`recipe-support/<配方>/<发布 ID>/` | RB-9 可选导航辅助资料：候选的 `images/<SHA-256>.png` 和 `maps/<SHA-256>.json` 由 `workspace.json.navigationId` 引用；正式辅助快照含 `image.png`、`snapshot.json`、`manifest.json`，独立记录发布身份、空间分段及示教件原始帧范围。发布准备在 `.pending-<发布 ID>`，生效后转为正式目录；临时与未生效目录不导出。地图不加入检测发布包，不进计划身份；缺失或损坏只关闭导航。项目复制和导出导入携带资料，资料不按日志或图像缓存清理 |
| `recipe-capture/` | 采集轮次原图。每个轮次：`<轮次>/round.json`（格式 3，格式 2 照常读取）、`frames.jsonl`（一帧一行）和 `<相机>/<seq:06>.pgm`（无压缩 PGM，一帧一个文件，seq 为相机帧号减该相机基线）。历史件导入的轮次（[在线投运](online-commissioning.md) 5.2）的 PGM 同卷时为外部录制原图的硬链接，跨卷时为校验后的独立副本，`round.json` 的 `origin` 记 `{kind: "history", cycleId, recordId, standard}`，录制被清理后轮次原图仍在。轮次只在选示教件、标定件时导入，界面不显示；换示教件、弃用标定件或删除候选时删除对应轮次（[示教件与标定件决策](../decisions/recipe-build-flow.md)）。`capture_fixture` 生成的仿历史录制件在真机录制根下（`part.json` 带 `fixture: {label}`），同样经导入成为 `history` 轮次。磁盘上已有的 `{kind: "prepared", label}` 轮次与示教实拍轮次（格式 2 带 `teachingRoundId`，或格式 3）照常读取 |
| `inspection.db` | 检测历史数据库（版本 9）。`parts.mode` 记件的检测模式（`production` / `maintenanceCollect` / `maintenanceShadow`），旧库启动时原地加列、旧记录按 `production`。每台相机每一帧写 `part_frames` 表，`parts` 另存整份区域判定与整件标记。库版本不符（含更高版本）时先备份为 `.v<版本>.*.bak` 再新建空库 |
| 外部 `records/`、`records-offline/` | 位于日志根的 `vision/<环境>/<实例>/<项目>/`，不再写入项目目录。真机、离线历史原图。每件目录 `*_cycle_<cycleId>` 内的原图为 `{相机}_s{seq:06}.pgm`，`part.json` 另列 `missingFrames`（相机没出图，只追溯）与 `rawDropped`（录制队列满没录，证据标不完整），以及 `partMode`、`devices`（每台计划相机的 `cameraId`、`cameraConfig`、本次打开实际写入的 `imaging`（没有则取相机保存值）、`deviceSession`）与 `plc`（`requestSeq`、`productCode`）；维护采集件目录另有空文件 `maintenance-collect`，自动清理跳过这些目录 |
| `audit-spool/` | 待入库的件证据与审计。审计 spool 格式为 4（帧引用 `files[].frame`）；目录里残留其他格式的文件时就绪检查失败、拒绝接件（保护证据）；切换项目前必须写完 |

项目化之前放在资料目录根下的 `config/cameras.json`、`plc.json`、`cycle.json`、`feasibility.json`、`calib/`、`recipes/`、`workspaces/`、`vision/`、`recipe-capture/`、`inspection.db`、`records/`、`records-offline/`、`audit-spool/` 不迁移、不读取，也不自动删除；启动时检测到会写一条提示日志，确认不再需要后人工删除。

同一资料目录使用文件锁独占，外部日志命名空间另持独占锁。不同目录不表示可以同时占用相同的 PLC 或相机。业务资料在同一环境中由整机账号共享，最后打开的工作区等界面偏好按账号隔离。

## 采集轮次整理记录

`workspaces/<配方>/workspace.json` 的 `captureReview` 保存候选的轮次分段与屏蔽记录，旧候选缺省为空。`rounds` 以轮次 ID 为键，保存原始 `groupCount`、边界 `cuts`、`draft/confirmed/stale` 状态、带原因的 `excludedGroups`、段结构与采集配置签名（不含曝光增益）以及来源模板版本；`template` 保存已确认参考轮次的边界快照和版本。

标注随候选修订原子保存，不改写 `recipe-capture/` 下的 `round.json`、`frames.jsonl` 或 PGM。边界使用原始帧组序号，屏蔽与恢复不重排、不扣减原始采集计数。这里的手动边界与 `calibration/<轮次>/boundaries.json` 的算法输出是不同数据：本阶段后者及生产分段仍为未分段。屏蔽只作用于候选标定取样，不写入发布包或生产检测记录。

## 项目导出包

项目管理页的“导出 / 导入项目”把项目资产和所选外部历史原图打成 ZIP（`src-tauri/src/project/transfer.rs`，格式版本 1），规则见[项目设计](project.md#导出与导入)。导出默认保存在 `exports/`；导入暂存在 `cache/project-import-*`，校验通过、路径与计划版本改写完成后才原子移入 `projects/<新编号>/`。“复制”不经 ZIP，把同样的内容拷进 `cache/project-copy-*` 后走相同的落地流程。

## 旧资料

首次启动按空白资料初始化，不从其它资料目录或旧 AppData 复制资料；资料目录没有初始化标记却已有其它内容时拒绝启动，需人工检查。沿用其它机器的配置用项目导出导入。

早于外置日志根的版本留在资料根 `logs/` 和项目内 `records/`、`records-offline/` 的日志与原图不导入日志根，软件不读取这些旧原图，也不自动删除。

## 样本与清理

外部样本先复制、检查，再启用；原目录不修改。样本集身份保留 manifest 的 ID 和 part/shot/camera/view。重复 ID 不覆盖，用户可以选择已有副本或取消。

只有明确可重建的缓存属于普通缓存清理范围。示教、标定、样本和 WebView 资料不按图片缓存删除。历史原图沿用现有保留策略，与数据库引用同步处理。

## 自动化测试

`Start-UiTestInstance.ps1` 显式设置隔离 `GLUESIGHT_DATA_DIR`，目录默认位于对应 `output/playwright/<profile>-regression/<test-identifier>`，可用 `-DataRoot` 或 `GLUESIGHT_TEST_DATA_ROOT` 改为 `<DataRoot>/<test-identifier>`，并写入 `instance.json.dataRoot`。重置只允许清理这一已验证目录，不接触正式、开发或旧 AppData 资料。安装版不接受该覆盖变量。

启动时校验主窗口的 WebView2 资料目录（`webview/<SID>`）不超过 229 个字符，否则 WebView2 建不出 `EBWebView\Default` 目录；资料目录尚未初始化、需要先建资料准备窗口时，还校验准备窗口的资料目录（`migration/bootstrap/webview/<SID>`）不超过 237 个字符，否则建不出 `EBWebView` 目录。两种情况下窗口都不出现且进程挂起，所以超限按启动失败处理。

首次启动在资料准备页按空白资料初始化，不询问来源。目录不可用时，只为展示错误页使用临时 WebView 配置，不回退读取或写入 AppData 业务资料；正常启动的 WebView 配置仍位于 ProgramData。窗口全部配置为隐藏的测试实例没有人能看到错误页，启动失败时把原因写到标准错误并以退出码 1 结束。

候选离线调试历史通过历史中冻结的样本集 ID、工件 ID 读取 samples/ 中的原图，界面标为“离线样本原图”；调试记录按（计划相机下标，帧组序号）读，每台计划相机在样本清单里按顺序的拍照点即它的帧序列（D-SB-28）。查看与原图复测不依赖当前站点模式或当前选择的样本集，也不把复用样本标成已录制。样本缺失、身份不匹配或同 ID 多副本会阻止原图复测；原始调试判定保留，对照结果另存。
