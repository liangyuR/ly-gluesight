# GlueSight 文档

## 架构与规划（[architecture/](architecture)）

- [软件架构、一期可行性与实施路线](architecture/gluesight-software-architecture.html)：目标架构、当前缺口、单负责人与多 Agent 的分工、W0–W7 工作包及 S7 实施进展。
- [三相机硬件连接架构](architecture/gluesight-hardware-architecture.html)：PLC 硬件触发、三台相机、交换机、工控机与 Robot 的连接关系，可按链路高亮。注意：2026-10-10 确认现场是一台三目设备（SDK 中一个 Device、一次触发出三幅图），图中三台独立相机与三条线路待现场确认后更新，见 [P0 计划 1.2 节](architecture/p0-plan.md)。
- [一期 P0 实施计划](architecture/p0-plan.md)：已定决策（含 D-11 三目相机）、步 0–7 与步 T 的顺序、范围与完成标准、现场并行事项、问题跟踪与剩余风险。
- [历史规划与实施记录](architecture/roadmap.md)：早期飞拍与随动规划，当前一期范围以软件架构图为准。

## 现场接入（[integration/](integration)）

- [PLC 通讯协议](integration/plc-communication-protocol.md)：供电气与视觉对接的通讯参数、完整点表、单件握手、结果码、时间参数、复位及验收清单。
- [一期 S7 握手契约与现场接入](integration/plc-s7-phase1.md)：DB 点表、事务与结果确认、故障恢复、计划导出和现场验证步骤。
- [PLC SCL 参考程序](../scripts/s7-handshake/phase1_plc.scl)与 [S7 测试 PLC](../scripts/s7-handshake/README.md)：可审查的 PLC 侧实现及回环报文测试服务。

## 测试与验收（[testing/](testing)）

- [测试记录与复现](testing/TESTING.md)：测试命令、覆盖范围、P0 验证汇总和未验证边界。
- [UI 操作验收](testing/UI_OPERATIONS.md)：基本操作、分层证据和边界。
- [操作逻辑修复记录](testing/UX_FIXES_2026-10-09.md)。

## 部署（[deployment/](deployment)）

- [Windows 打包与运行库部署](deployment/PACKAGING.md)。

## 使用教程

- [图文使用教程](user-guide/index.html)：`npm run guide:serve` 可本地预览。

## 其他

- `design/`：界面设计与实现契约的状态文件，以 `implementation-contract.json` 为准。

两张架构图均为自包含 HTML，无外部脚本、字体或联网请求。下载仓库后，用浏览器打开 `docs/architecture/` 下的文件即可离线查看；GitHub 文件页只展示源码。软件架构图中的完整源码路径相对于仓库根目录，Rust 短文件名位于 `src-tauri/src/`，`LyFlow/` 指外部引擎仓库。

2026-10-09：S7 软件实现与协议模拟联调已完成；真实 CPU、TIA 编译、三相机完整检测和现场验收仍待完成。架构图中的工期是带前提的规划估算，不代表现场交付承诺。
