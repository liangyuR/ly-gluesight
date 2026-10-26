# 长稳与故障注入

范围：离线模式下进程内虚拟 S7 产线 + 实拍样本集 + lyFlow 运行库，按段配方连续生产与故障注入。决策见 [soak 决策](../decisions/soak.md)。吞吐门槛阶段的判定口径与当前状态见[测试说明](TESTING.md#3--25-hz-吞吐门槛)。

## 运行

```powershell
$env:GLUESIGHT_SAMPLE_SET = '<样本集目录>'
$env:GLUESIGHT_LYFLOW_RUNTIME = '<lyflow 运行库目录>'
pwsh -NoProfile -File scripts/Test-Soak.ps1 -Parts 1000                              # 默认阶段：soak、faults、engine、audit、restart
pwsh -NoProfile -File scripts/Test-Soak.ps1 -SkipBuild -Stages soak,faults,engine,disk  # 指定阶段
pwsh -NoProfile -File scripts/Test-Soak.ps1 -SkipBuild -Stages faults                 # 单独跑一个阶段（每次都重建隔离数据、重新建配方）
node scripts/soak/soak.mjs run --parts 300 --page /inspect                           # 在线检测页打开时的界面内存
```

隔离实例为 `soak`（标识 `com.xyzrobotics.tujiaovision.soak-tests`）。建站（`setup`）只支持段配方：样本集上走廊示教 → 10 轮标定 → 独立样本验证 → 发布，配方 `SOAK-1`。样本集是停稳拍数据、一条走廊对不上其他点位，检测项开着样本全判 NG_GAP，所以建站时各段检测项全关：长稳验证稳定性、资源与故障恢复，不验证判胶质量。模拟标定当量 0.112 mm/px 只写入输出目录的样本集副本（S-1）。

阶段：`soak` 连续生产；`faults` PLC / 相机 / lyFlow / 目录 / 复位类注入（含 `pulseJitter` 脉冲首尾 ±1）；`engine` lyFlow 运行库缺失；`disk` 资料盘不足（用 `GLUESIGHT_DISK_MIN_FREE_MB` 调高下限模拟，仅开发构建生效）；`audit` 追溯目录不可写后重启；`restart` 事务中强制结束进程；`throughput` 3 × 25 Hz 吞吐门槛（`pnpm test:soak:throughput`）。`audit`、`restart` 遇到剩余风险 1 时会停在“重启前留下的离线未结事务阻止回到离线”，脚本如实报错。

阶段按上面顺序执行，某一阶段失败即中止后续阶段；要拿到后面阶段的数据，用 `-Stages` 分开跑。

判定口径（S-14）：

| 阶段 | 失败（退出码 1，中止后续阶段） | 已知问题（退出码 2，继续后续阶段） |
| --- | --- | --- |
| `soak` | 生产不足；正常样本判 NG；cycleId 重复；判定与 PLC 交付不一致（合格件未确认，或任何件交付未落定）；ERR 比例超过 `-MaxErrRatio`（默认 20%） | — |
| `faults` | 场景出错或超时；该进故障没进，或拒写这类不该进故障的进了；在途件没有记录；复位输入保持高电平就离开故障、先低后高不回就绪；PLC 停机未到期就重连；错误的 OK；复位后 3 件内不合格；重启后投产不合格 | PC 主动断开 PLC 后不自动重连、经接口补连接（剩余风险 2） |
| `engine`、`disk`、`restart` | 结果文件中 `passed` 为 false | — |

段配方下缺帧、坏帧只把所在帧组标检测不完整、不影响 OK/NG，所以 `cameraLostFrame`、`replayMissing*`、`cameraCorruptImage` 判 OK 不算错误的 OK，改为核对整件标了检测不完整（`EXPECT_INCOMPLETE`）；检测项全关时空白图、噪声图判 OK 也是正确结果。`pulseJitter` 件帧组数低于 ΣN_exp − Σtol 时应标疑似提前终止，在容差内却标了算失败，结论变成 NG 也算失败。

`Test-Soak.ps1` 退出码：0 全部通过；1 有阶段失败；2 只有已知问题，未全部通过；3 输入缺失，未运行。

结果写到 `output/soak/`（`soak-<件数>.json`、`faults-*.json`、`restart.json`、`engine-missing.json`、`disk-*.json`、`throughput-*.json`），不入库。`soak-<件数>.json` 的 `failures` 与 `faults-*.json` 的 `failures`、`knownIssues` 列出未通过的原因。

## 最近一次段配方运行与证据边界

2026-10-23，i5-1135G7 调试构建，`pnpm test:soak -Parts 30`，默认阶段 soak、faults、engine、audit、restart，退出码 0：连续生产 30 件全部 OK 且 PLC 交付已确认、cycleId 不重复、没有产线中止；故障注入没有错误的 OK、该进故障的都进了故障、复位后都恢复合格，缺帧 / 坏帧 / 缺图都标了检测不完整；lyFlow 运行库缺失、追溯目录不可写后重启、事务中强制结束进程后重启都按预期恢复，没有出现已知问题。

边界：

- 该次运行早于生产通讯协议 7 与维护模式合入，之后没有重跑。
- 段配方下没有跑过 1000 件长稳，也没有跑 `disk` 阶段；段配方的内存、句柄与磁盘长时间增长未验证。
- 耗时来自调试构建且与其他工作线争用 CPU，只作相对比较，不代表发布版现场节拍。
- 资料盘不足用调高下限模拟，追溯目录不可写用拒写 ACL 模拟；真实磁盘写满（ERROR_DISK_FULL）未实测。

## 剩余风险

1. **离线未结事务导致重启后无法回到离线**：离线事务进行中崩溃，或在途件进入故障后重启，重启总回到真机模式，握手日志仍保留离线虚拟 PLC 的未结事务；真机 PLC 不在线时无法复位，又因“检测仍有在途事务”进不了离线，只能人工处理 `state/plc-handshake.json`。脚本在 `restart.json` 的 `blockedByOfflinePending` 记下这种情况。
2. **PC 主动断开后需人工连接**：置故障输出时恰逢断线、无法确认撤销布防，PC 断开以停止心跳，之后自动重连一次；自动重连后仍无法确认撤销时停止重连，需人工连接并明确复位。离线期间 PLC 页只读，只能经接口或重启恢复；脚本记为已知问题（恢复记录的 `manualReconnect`）。
3. **记录库只按天清理**：检测记录按 `history_days`（默认 180 天）清理，没有件数或容量上限，高节拍现场需按节拍核算。
