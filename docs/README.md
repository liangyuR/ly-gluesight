# GlueSight 文档

- [软件架构、一期可行性与实施路线](gluesight-software-architecture.html)：目标架构、当前缺口、单负责人与多 Agent 的分工、W0–W7 工作包及 S7 实施进展。
- [一期 S7 握手契约与现场接入](plc-s7-phase1.md)：DB 点表、事务与结果确认、故障恢复、计划导出和现场验证步骤。
- [PLC SCL 参考程序](../scripts/s7-handshake/phase1_plc.scl)与 [S7 测试 PLC](../scripts/s7-handshake/README.md)：可审查的 PLC 侧实现及回环报文测试服务。
- [测试记录与复现](TESTING.md)：软件验证结果、命令和未验证边界。
- [图文使用教程](user-guide/index.html)与 [UI 操作验收](UI_OPERATIONS.md)。
- [Windows 打包与运行库部署](PACKAGING.md)。
- [历史规划与实施记录](roadmap.md)：早期飞拍与随动规划，当前一期范围以架构图为准。

架构图为自包含 HTML，无外部脚本、字体或联网请求。下载仓库后，用浏览器打开 `docs/gluesight-software-architecture.html` 可离线切换视图和展开工作包；GitHub 文件页只展示源码。图中的完整源码路径相对于仓库根目录，Rust 短文件名位于 `src-tauri/src/`，`LyFlow/` 指外部引擎仓库。

2026-10-09：S7 软件实现与协议模拟联调已完成；真实 CPU、TIA 编译、三相机完整检测和现场验收仍待完成。架构图中的工期是带前提的规划估算，不代表现场交付承诺。
