# GlueSight 资料目录

Windows 的 ProgramData 由系统已知文件夹接口解析，不硬编码盘符。正式版使用 `%ProgramData%\ly\GlueSight`，开发版使用 `%ProgramData%\ly\GlueSight-dev`。两套资料独立，不自动同步。程序、运行库和源码不属于资料目录。

## 初始化与日常运行

正式安装包采用整机安装，一次性提权创建并授权资料目录；日常运行不需要管理员权限。升级、卸载保留资料。

本地开发首次在管理员 PowerShell 执行 `pnpm storage:init`，之后正常执行 `pnpm dev`。初始化命令只创建目录并授权，不移动或删除资料。无法写入时应修复权限，不会回退到 AppData。

资料分为本机资料和项目资料。项目是一个工位的完整检测方案（相机组、PLC、标定、配方及其检测历史），设计见 [项目（Project）设计](project.md)。任一时刻只打开一个当前项目，切换项目会重启软件。

**本机资料**（资料目录根下）：

| 位置 | 用途 |
| --- | --- |
| `config/` | 本机配置：`current-project.json`（当前项目编号）、`site-offline.json`（离线选择）、`preferences-<SID>.json`（按账号的界面偏好，最后打开的工作区按项目分别记） |
| `projects/<项目编号>/` | 各项目的资料，见下表；`projects/.trash/` 是删除中的项目，启动时清理 |
| `state/` | PLC 握手状态与审计，以及所有项目共用的计划号计数 `plan-version`（PLC 是同一台设备，换项目不回退）。握手审计 `plc-handshake.audit.jsonl` 超过 4 MB 滚动到 `plc-handshake-audit/`，分段早于 30 天或总量超过 256 MB 时随检测记录清理删除，仍有未结 PLC 交付引用的分段保留；诊断包带上当前文件和与导出时间段有交集的分段，其余分段在清单 notes 中计数 |
| `samples/` | 外部样本导入后的受管副本 |
| `logs/` | 程序、PLC 日志 |
| `cache/` | 可重建缓存 |
| `webview/<SID>/` | 按 Windows 用户隔离的浏览器资料 |
| `exports/` | 默认导出位置；配方迁移包的保存与选择对话框默认打开这里 |
| `migration/` | 迁移报告和未完成迁移状态 |

**项目资料**（`projects/<项目编号>/` 下）：

| 位置 | 用途 |
| --- | --- |
| `project.json` | 项目编号、名称、描述、创建时间与来源 |
| `config/` | 相机组 `cameras.json`、PLC `plc.json`、节拍与检测设置 `cycle.json`（含历史保留天数）、飞拍估算参数 `feasibility.json` |
| `recipes/`、`workspaces/` | 生产配方、候选配方及示教资料。段配方的候选目录 `workspaces/<配方>/` 另有 `calibration/<轮次>/`：每轮标定的 `features.jsonl`（一行一帧 `{camera, seq, metrics}`，1..=帧组数内每台相机每个 seq 一行，缺帧也有）与 `boundaries.json`（分段结果，本轮恒为 `{"kind":"unsegmented"}`，供以后的分段算法和重新标定使用）；各相机的示教帧冻结为 `images/` 下的 PGM |
| `calib/`、`vision/releases/` | 标定、发布资源。发布包在 `vision/releases/<配方>/<发布 ID>/`，按配方类型分两种格式，加载时格式必须与配方类型一致。**旧拍照点配方（schema 3）：** `manifest.json`、`recipe.json`、`graph.json`，每个（拍照点 kk，图 N）一个 `shots/kk/vN/`，内有 `image.<扩展名>`（示教原图）、`calibration.json`、`centerline.json`、`points.json`，用特征定位的视图另有 `template.pgm`（生产 glue.locate 的 base_dir 就是这个目录）。**段配方（schema 4）：** 同样有 `manifest.json`、`recipe.json`、`graph.json`，每台计划相机一个 `cameras/NN/`（NN 为计划相机下标，从 00 起），内有 `image.<扩展名>`（示教帧）、`calibration.json`（像素当量与标定引用）、`corridor.json`（走廊）。工位标定文件：旧拍照点配方为 `calib/plane_calib_{device}-v{view}.json`（显式标定引用 `{引用}` 为 `calib/plane_calib_{引用}.json`，段配方的显式引用同此）；段配方缺省为设备标定 `calib/device/plane_calib_{device}.json`（发布包 `calibration.json` 的 `reference` 记 `device/{device}`），两者并存、互不读取。设备标定单独放子目录：相机编号允许 `-`，与旧配方共用扁平文件名时相机 `cam1-v1` 的设备标定就是相机 `cam1` 图 1 的旧配方标定文件，一方覆盖另一方 |
| `recipe-capture/` | 整圈采集原图。旧拍照点配方的轮次：`<轮次>/shot-NNNN/<相机>/`。段配方的轮次：`<轮次>/round.json`（只在状态变化时原子重写）、`frames.jsonl`（每写完一帧追加一行，重启后据此恢复统计）和 `<相机>/<seq:06>.pgm`（无压缩 PGM，一帧一个文件，seq 为相机帧号减该相机基线） |
| `inspection.db` | 检测历史数据库（版本 9）。旧拍照点配方的 `part_shots`、`part_points` 保留；段配方按每台相机每一帧写 `part_frames` 表，`parts` 另存整份区域判定与整件标记。库版本不符（含更高版本）时先备份为 `.v<版本>.*.bak` 再新建空库 |
| `records/`、`records-offline/` | 真机、离线历史原图。段配方每件目录 `*_cycle_<cycleId>` 内的原图为 `{相机}_s{seq:06}.pgm`，`part.json` 另列 `missingFrames`（相机没出图，只追溯）与 `rawDropped`（录制队列满没录，证据标不完整）；旧拍照点配方的命名与结构不变 |
| `audit-spool/` | 待入库的件证据与审计。审计 spool 格式为 4（段配方的帧引用 `files[].frame`）；升级前须确认 `audit-spool/` 已清空，见下节；切换项目前必须写完 |

项目化之前放在资料目录根下的 `config/cameras.json`、`plc.json`、`cycle.json`、`feasibility.json`、`calib/`、`recipes/`、`workspaces/`、`vision/`、`recipe-capture/`、`inspection.db`、`records/`、`records-offline/`、`audit-spool/` 不迁移、不读取，也不自动删除；启动时检测到会写一条提示日志，确认不再需要后人工删除。

同一资料目录使用文件锁独占。不同目录不表示可以同时占用相同的 PLC 或相机。业务资料在同一环境中由整机账号共享，最后打开的工作区等界面偏好按账号隔离。

## 采集轮次整理记录

`workspaces/<配方>/workspace.json` 的 `captureReview` 保存候选的轮次分段与屏蔽记录，旧候选缺省为空。`rounds` 以轮次 ID 为键，保存原始 `groupCount`、边界 `cuts`、`draft/confirmed/stale` 状态、带原因的 `excludedGroups`、段结构与采集配置签名以及来源模板版本；`template` 保存已确认参考轮次的边界快照和版本。

标注随候选修订原子保存，不改写 `recipe-capture/` 下的 `round.json`、`frames.jsonl` 或 PGM。边界使用原始帧组序号，屏蔽与恢复不重排、不扣减原始采集计数。这里的手动边界与 `calibration/<轮次>/boundaries.json` 的算法输出是不同数据：本阶段后者及生产分段仍为未分段。屏蔽只作用于候选标定取样，不写入发布包或生产检测记录。

## 配方迁移包

配方库卡片的“导出”把段配方候选打成一个 ZIP（`src-tauri/src/workspace/transfer.rs`，格式版本 1）：`workspace/workspace.json`（候选记录）、`workspace/images/` 中走廊示教帧、`workspace/calibration/<轮次>/` 标定特征，以及候选引用的每个采集轮次 `rounds/<轮次>/` 下的 `round.json`、`frames.jsonl` 和 `<相机>/<seq:06>.pgm`。引用的轮次取走廊示教、多轮标定、验证样本库与采集复核记录；本机不完整或缺失的轮次不导出，并在清单 `notes` 中说明。配方只有已发布版本、没有候选时，只导出 `recipe.json`（生产配方文档），不含示教帧、标定与原图。包内 `manifest.json` 记格式版本、配方编号/名称/产品代码/版本、导出时间、来源软件版本、轮次列表，以及每个文件的路径、字节数与 SHA-256。

“导入配方”先把整包解到 `cache/transfer-import-*` 暂存目录：格式版本不认识、路径不在上述结构内（含 `../`、绝对路径）、清单外条目、重复条目、字节数或校验和不符都拒绝整包；暂存目录用后删除。操作员确认的编号与产品代码沿用新建候选的规则，不能与本机配方冲突。轮次移入 `recipe-capture/` 并按采集记录核对，归属改为新编号；本机已有同名轮次时导入轮次改用新编号，候选中的引用一并改名。导入的候选从草稿开始，不带待生效版本、验证结论、基准版本和来源机历史记录样本；示教帧与标定轮次所记的相机配置照常与本机设备核对，不一致时需重新采集、示教与标定后再验证发布。

## 升级到段模型（协议 6）前

升级只保证旧数据被安全拒绝或备份，不做迁移（D-0，项目尚未上线）。升级前逐项核对：

- **先清空 `audit-spool/`。** 审计 spool 格式由 3 升到 4，目录里残留的旧格式文件会让就绪检查失败、拒绝接件（保护证据的现有安全行为）。升级前停止接件，等上一件的录制与入库完成、`audit-spool/` 为空后再退出旧版本。
- **记录库：** 版本 8 升到 9，启动时把旧库备份为 `.v8.*.bak` 后新建空库；历史记录不迁移，需要保留就保留备份文件。
- **配方：** 配方文件格式 7 升到 8，旧文件加载时列为不可用，需按新格式重建（重建的可以是旧拍照点配方，也可以是段配方）；`.plan-version` 保留，计划号继续单调递增，PLC 不会收到用过的版本号。
- **发布包：** 旧拍照点配方的 schema 3 包照常加载；段配方用 schema 4，需要在新模型下重新验证并发布。
- **标定：** 段配方缺省读设备标定 `calib/device/plane_calib_{device}.json`，旧 `{device}-v{view}` 文件不读，段配方的三台相机要重新做飞拍标定。标定页对单视角相机（图 1）的标定同时写 `calib/plane_calib_{device}-v1.json` 和设备标定，三目拼接相机的各图只写 `{device}-v{view}`。此前写在 `calib/plane_calib_{device}.json` 的段配方设备标定不再读取，也不回退读取（回退会重新引入与旧配方 `{device}-v{view}` 同名的冲突），要在标定页重新标定一次。
- **PLC：** 协议版本由 5 升到 6，PLC 写 `protocolVersion=6`，并把 `shotCount`/`deviceShots` 改填导出计划的 ΣN_exp；`state/plc-handshake.json` 里协议版本不是 6 的未结事务改名为 `plc-handshake.v5.bak`，以空事务启动并要求人工复位，两边上线后做一次 faultReset。
- **相机组：** 段配方要求三台互不相同的单视角相机；三目拼接的相机配置仍可用于旧拍照点配方。

## 迁移原则

关闭旧实例后，复制旧资料到暂存区，对 SQLite 做包含 WAL 的一致性快照，重定位明确的受管资源引用，再启用新目录。迁移保留源文件，不以内容哈希匹配资源，也不通过搬迁升级旧业务格式或更改验证结论。

既有目标资料不自动覆盖或合并。失败保留原目录并记录原因，未完成迁移不能进入检测服务。启用后不自动退回旧目录，防止新增资料分叉。旧目录的清理需要单独确认。

## 样本与清理

外部样本先复制、检查，再启用；原目录不修改。样本集身份保留 manifest 的 ID 和 part/shot/camera/view。重复 ID 不覆盖，用户可以选择已有副本或取消。

只有明确可重建的缓存属于普通缓存清理范围。示教、标定、样本、WebView 资料和迁移资料不按图片缓存删除。历史原图沿用现有保留策略，与数据库引用同步处理。

## 自动化测试

`Start-UiTestInstance.ps1` 显式设置隔离 `GLUESIGHT_DATA_DIR`，目录默认位于对应 `output/playwright/<profile>-regression/<test-identifier>`，可用 `-DataRoot` 或 `GLUESIGHT_TEST_DATA_ROOT` 改为 `<DataRoot>/<test-identifier>`，并写入 `instance.json.dataRoot`。重置只允许清理这一已验证目录，不接触正式、开发或旧 AppData 资料。安装版不接受该覆盖变量。

启动时校验主窗口的 WebView2 资料目录（`webview/<SID>`）不超过 229 个字符，否则 WebView2 建不出 `EBWebView\Default` 目录；资料目录尚未初始化、需要先建资料准备窗口时，还校验准备窗口的资料目录（`migration/bootstrap/webview/<SID>`）不超过 237 个字符，否则建不出 `EBWebView` 目录。两种情况下窗口都不出现且进程挂起，所以超限按启动失败处理。

首次启动通过软件内资料准备页确认来源和目标。目录不可用时，只为展示错误页使用临时 WebView 配置，不回退读取或写入 AppData 业务资料；正常启动的 WebView 配置仍位于 ProgramData。窗口全部配置为隐藏的测试实例没有人能看到错误页，启动失败时把原因写到标准错误并以退出码 1 结束。

候选离线调试历史通过历史中冻结的样本集 ID、工件 ID 和配方拍照点身份读取 samples/ 中的原图，界面标为“离线样本原图”；段配方的调试记录按（计划相机下标，帧组序号）读，每台计划相机在样本清单里按顺序的拍照点即它的帧序列（D-SB-28）。查看与原图复测不依赖当前站点模式或当前选择的样本集，也不把复用样本标成已录制。样本缺失、身份不匹配或同 ID 多副本会阻止原图复测；原始调试判定保留，对照结果另存。
