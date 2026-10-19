# GlueSight 资料目录

Windows 的 ProgramData 由系统已知文件夹接口解析，不硬编码盘符。正式版使用 `%ProgramData%\ly\GlueSight`，开发版使用 `%ProgramData%\ly\GlueSight-dev`。两套资料独立，不自动同步。程序、运行库和源码不属于资料目录。

## 初始化与日常运行

正式安装包采用整机安装，一次性提权创建并授权资料目录；日常运行不需要管理员权限。升级、卸载保留资料。

本地开发首次在管理员 PowerShell 执行 `pnpm storage:init`，之后正常执行 `pnpm dev`。初始化命令只创建目录并授权，不移动或删除资料。无法写入时应修复权限，不会回退到 AppData。

| 位置 | 用途 |
| --- | --- |
| `config/` | 相机、PLC、检测配置、离线选择、飞拍估算参数和用户偏好 |
| `state/` | PLC 握手状态与审计。握手审计 `plc-handshake.audit.jsonl` 超过 4 MB 滚动到 `plc-handshake-audit/`，分段早于 30 天或总量超过 256 MB 时随检测记录清理删除，仍有未结 PLC 交付引用的分段保留；分段不随诊断包导出 |
| `recipes/`、`workspaces/` | 生产配方、候选配方及示教资料 |
| `calib/`、`vision/releases/` | 标定、发布资源 |
| `recipe-capture/` | 整圈采集原图 |
| `inspection.db` | 检测历史数据库 |
| `records/`、`records-offline/` | 真机、离线历史原图 |
| `samples/` | 外部样本导入后的受管副本 |
| `logs/`、`audit-spool/` | 程序、PLC 日志及审计 |
| `cache/` | 可重建缓存 |
| `webview/<SID>/` | 按 Windows 用户隔离的浏览器资料 |
| `exports/` | 默认导出位置 |
| `migration/` | 迁移报告和未完成迁移状态 |

同一资料目录使用文件锁独占。不同目录不表示可以同时占用相同的 PLC 或相机。业务资料在同一环境中由整机账号共享，最后打开的工作区等界面偏好按账号隔离。

## 迁移原则

关闭旧实例后，复制旧资料到暂存区，对 SQLite 做包含 WAL 的一致性快照，重定位明确的受管资源引用，再启用新目录。迁移保留源文件，不以内容哈希匹配资源，也不通过搬迁升级旧业务格式或更改验证结论。

既有目标资料不自动覆盖或合并。失败保留原目录并记录原因，未完成迁移不能进入检测服务。启用后不自动退回旧目录，防止新增资料分叉。旧目录的清理需要单独确认。

## 样本与清理

外部样本先复制、检查，再启用；原目录不修改。样本集身份保留 manifest 的 ID 和 part/shot/camera/view。重复 ID 不覆盖，用户可以选择已有副本或取消。

只有明确可重建的缓存属于普通缓存清理范围。示教、标定、样本、WebView 资料和迁移资料不按图片缓存删除。历史原图沿用现有保留策略，与数据库引用同步处理。

## 自动化测试

`Start-UiTestInstance.ps1` 显式设置隔离 `GLUESIGHT_DATA_DIR`，目录位于对应 `output/playwright/<profile>-regression/<test-identifier>`，并写入 `instance.json.dataRoot`。重置只允许清理这一已验证目录，不接触正式、开发或旧 AppData 资料。安装版不接受该覆盖变量。

首次启动通过软件内资料准备页确认来源和目标。目录不可用时，只为展示错误页使用临时 WebView 配置，不回退读取或写入 AppData 业务资料；正常启动的 WebView 配置仍位于 ProgramData。

候选离线调试历史通过历史中冻结的样本集 ID、工件 ID 和配方拍照点身份读取 samples/ 中的原图，界面标为“离线样本原图”。查看与原图复测不依赖当前站点模式或当前选择的样本集，也不把复用样本标成已录制。样本缺失、身份不匹配或同 ID 多副本会阻止原图复测；原始调试判定保留，对照结果另存。
