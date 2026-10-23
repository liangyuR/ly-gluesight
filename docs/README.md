# GlueSight 文档

## 架构与规划（[architecture/](architecture)）

- [工件边缘中心距检测方案](architecture/workpiece-edge-position.md)：以工件边为基准测胶条中心距的方案草案，配套 [方法与资料调研](research/glue-position-methods.md)。
- [软件架构、一期可行性与实施路线](architecture/gluesight-software-architecture.html)：历史目标架构与实施估算、Agent 分工及 S7 实施进展；当前测试范围以测试说明为准。
- [三相机硬件连接架构](architecture/gluesight-hardware-architecture.html)：PLC 硬件触发、三台相机、交换机、工控机与 Robot 的连接关系，可按链路高亮。图中是三台独立相机，与分段连拍的段配方一致；只有旧拍照点配方才按一台三目设备建模，见 [P0 计划 1.2 节](architecture/p0-plan.md)。
- [一期 P0 实施计划](architecture/p0-plan.md)：已定决策（含 D-11 三目相机；段配方下 D-7、D-10、D-11 被分段连拍取代，旧拍照点配方仍按原决策）、步 0–7 与步 T 的顺序、范围与完成标准、历史问题跟踪与软件验证边界。
- [分段连拍飞拍决策记录](decisions/segment-burst.md)：段列表取代拍照点列表、三台独立相机共用一个触发、协议 6 的冲突与决策，IPC 契约、帧数口径、内存与队列，以及旧拍照点模型并存的规则。
- [配方创建与多图示教](architecture/recipe-creation-workflow.md)与[当前进度](architecture/recipe-creation-progress.md)：段配方（多轮标定、相机走廊示教）与旧拍照点配方的流程；[资料目录与存储布局](architecture/storage-layout.md)。
- [历史规划与实施记录](architecture/roadmap.md)：早期飞拍与随动规划，当前一期范围以软件架构图为准。

## 现场接入（[integration/](integration)）

- [PLC 通讯协议 HTML 与时序图](integration/plc-communication-protocol.html)：可离线阅读，协议版本 6，含完整点表搜索及打印样式，三张时序图已随协议 6 更新；另附 [Markdown 原文](integration/plc-communication-protocol.md)。
- [一期 S7 握手契约与现场接入](integration/plc-s7-phase1.md)：DB 点表、事务与结果确认、故障恢复、计划导出与参考程序限制。
- [PLC 交互表适配决策](decisions/plc-table.md)：协议 4（交互表 V1.0，S7-200 SMART、DB1）、协议 5（交互表 V1.1，S7-1200、DB4/DB2）与协议 6（分段连拍，布局同协议 5）的适配取舍、回退方式及待 PLC 工程师确认的问题清单。
- [PLC SCL 参考程序](../scripts/s7-handshake/phase1_plc.scl)与 [S7 测试 PLC](../scripts/s7-handshake/README.md)：可审查的 PLC 侧实现及回环报文测试服务。

## 测试（[testing/](testing)）

- [测试记录与复现](testing/TESTING.md)：测试命令、覆盖范围、历史验证记录和当前边界。
- [UI 覆盖边界](testing/UI_OPERATIONS.md)：当前主流程覆盖及未覆盖范围。
- [操作逻辑修复记录](testing/UX_FIXES_2026-10-09.md)。
- [算法评测报告](testing/algorithm-evaluation.md)：实拍 36 张的稳定性、合成缺陷检出、耗时与剩余风险；取舍见 [算法决策记录](decisions/algorithm.md)。

## 部署（[deployment/](deployment)）

- [Windows 打包与运行库部署](deployment/PACKAGING.md)。
- [lyFlow 运行库依赖](architecture/lyflow-dependency.md)：Release 来源、系列锁定、校验、缓存、开发启动参数与升级命令。

## 其他

- `design/`：界面设计与实现契约的状态文件，以 `implementation-contract.json` 为准。

两张架构图均为自包含 HTML，无外部脚本、字体或联网请求。下载仓库后，用浏览器打开 `docs/architecture/` 下的文件即可离线查看；GitHub 文件页只展示源码。软件架构图中的完整源码路径相对于仓库根目录，Rust 短文件名位于 `src-tauri/src/`，`LyFlow/` 指外部引擎仓库。

PLC 协议 HTML 的正文、时序图、样式、交互及 Mermaid 渲染脚本也已全部嵌入，可直接离线打开。时序图以 Mermaid 写在协议 Markdown 第 9 节，GitHub 可直接渲染。修改 Markdown 或时序图后，先在仓库根目录运行 `pnpm install`（提供锁定版本的 `mermaid`），再安装 `scripts/docs/requirements.txt` 中的依赖，运行 `python scripts/docs/build-plc-protocol.py` 重新生成；添加 `--check` 可核对生成物是否同步。

2026-10-09：S7 软件实现与协议模拟联调已完成；真实 CPU、TIA 编译、三相机完整检测和现场验收仍待完成。架构图中的工期是带前提的规划估算，不代表现场交付承诺。
