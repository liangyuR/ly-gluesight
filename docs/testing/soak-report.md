# 长稳与故障注入运行说明

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

阶段：`soak` 连续生产；`faults` PLC / 相机 / lyFlow / 目录 / 复位类注入（含 `pulseJitter` 脉冲首尾 ±1）；`engine` lyFlow 运行库缺失；`disk` 资料盘不足（用 `GLUESIGHT_DISK_MIN_FREE_MB` 调高下限模拟，仅开发构建生效）；`audit` 追溯目录不可写后重启；`restart` 事务中强制结束进程；`throughput` 3 × 25 Hz 吞吐门槛（`pnpm test:soak:throughput`）。恢复失败时脚本保留原始错误与结果文件，不把恢复失败视为通过。

所选阶段固定按 `soak → throughput → faults → engine → disk → audit → restart` 执行，与 `-Stages` 的书写顺序无关。某一阶段失败即中止后续阶段；要单独收集其他阶段的数据，用 `-Stages` 分开跑。

判定口径（S-14）：

| 阶段 | 失败（退出码 1，中止后续阶段） | 已知问题（退出码 2，继续后续阶段） |
| --- | --- | --- |
| `soak` | 生产不足；正常样本判 NG；cycleId 重复；判定与 PLC 交付不一致（合格件未确认，或任何件交付未落定）；ERR 比例超过 `-MaxErrRatio`（默认 20%） | — |
| `faults` | 场景出错或超时；该进故障没进，或拒写这类不该进故障的进了；在途件没有记录；复位输入保持高电平就离开故障、先低后高不回就绪；PLC 停机未到期就重连；错误的 OK；复位后 3 件内不合格；重启后投产不合格 | PC 主动断开 PLC 后不自动重连、经接口补连接（剩余风险 2） |
| `engine`、`disk`、`audit`、`restart` | 结果文件中 `passed` 为 false | — |

段配方下缺帧、坏帧只把所在帧组标检测不完整、不影响 OK/NG，所以 `cameraLostFrame`、`replayMissing*`、`cameraCorruptImage` 判 OK 不算错误的 OK，改为核对整件标了检测不完整（`EXPECT_INCOMPLETE`）；检测项全关时空白图、噪声图判 OK 也是正确结果。`pulseJitter` 件帧组数低于 ΣN_exp − Σtol 时应标疑似提前终止，在容差内却标了算失败，结论变成 NG 也算失败。

`Test-Soak.ps1` 退出码：0 全部通过；1 有阶段失败；2 只有已知问题，未全部通过；3 输入缺失，未运行。

结果写到 `output/soak/`（`soak-<件数>.json`、`faults-*.json`、`restart.json`、`engine-missing.json`、`disk-*.json`、`throughput-*.json`），不入库。`soak-<件数>.json` 的 `failures` 与 `faults-*.json` 的 `failures`、`knownIssues` 列出未通过的原因。

## 验证边界

- 段配方的长时间内存、句柄与磁盘增长仍须在当前版本上验证，旧协议下的短跑不能作为当前版本通过依据。
- 调试构建和开发机资源争用下的耗时只作相对比较，不代表发布版现场节拍；吞吐门槛需在现场工控机上复测。
- 资料盘不足通过调高守护下限模拟，追溯目录不可写通过拒写 ACL 模拟；这些方式不等价于真实磁盘写满（ERROR_DISK_FULL）。
- 输出保留在已忽略的 `output/soak/`，验收需记录源码版本、运行库版本、设备、输入与实际结果。

## 剩余风险

1. **未结事务的来源限制**：离线来源事务允许重新进入离线，由虚拟 PLC 明确复位；真机或来源不明的事务、握手日志不可用时仍阻止切换，必须连接真机 PLC 排查与复位。恢复规则见 [PLC 会话决策](../decisions/r2-plc.md#r2p-2-离线事务崩溃死锁事务记来源离线事务可进入离线复位)。脚本遇到仍被阻止的事务会在 `restart.json` 记录 `blockedByOfflinePending`，不能按成功处理。
2. **自动重连后的持续拒写**：PC 主动断开后等待 3.5 s 自动重连一次；重连后仍无法确认撤下就绪或置故障时停止重连，需排查后人工连接并明确复位。离线期间 PLC 页只读，脚本经接口补连接时记录 `manualReconnect`，按已知问题处理。
3. **记录库只按天清理**：检测记录按 `history_days`（默认 180 天）清理，没有件数或容量上限，高节拍现场需按节拍核算。
