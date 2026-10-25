# PLC 通讯协议

现行为**生产通讯协议 7**，规格只在 artifact 维护：

- [生产通讯协议 7](https://claude.ai/artifact/An1Zp8eKdikunc1ixxtWjX)：点表、状态与原因码、时序、迁移表、规则、结果码、时间参数。

仓库内与协议对应的实现：

- 点表模板与标签：`src-tauri/src/inspection.rs`；PC 侧握手与会话：`src-tauri/src/handshake.rs`、`src-tauri/src/plc_session.rs`。
- PLC 参考程序与 S7 测试 PLC：[`scripts/s7-handshake/`](../../scripts/s7-handshake/README.md)。
- 整圈示教不在本协议内，见 [示教采集接口 V1](teaching-capture-sequence.md)。

协议 6 及更早版本的文档已移除，需要时查 git 历史（最后版本在提交 `a0cb88f`）。
