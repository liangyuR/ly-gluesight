# 宽胶测量与质量判定分离

本文描述彩色测量的实现契约。固定走廊与逐帧定向采用同一站级证据；灰度测量仍使用原策略。现场配方的标称、公差、跳帧和测量区间不由升级自动修改。

## 职责与空间边界

- lyFlow 输出胶色候选、跟踪几何及边界证据，不包含产品公差或 OK/NG 规则。
- GlueSight 关联目标胶路，解释测站，再按原胶宽、溢胶、断胶和胶偏规则判定。
- `detect.searchMm` 是有限搜索半宽，限制 ROI、卡尺和计算量；不新增必须调节的“最大可识别胶宽”。
- 彩色路径的 `detect.widthRange` 下限继续过滤噪声，上限只作参考。完整宽胶不因超过上限被删除，搜索和反光合并范围也不再由该上限截断。产品规格仍由 `checks` 决定。

主入口：[彩色定向图](../../src-tauri/graphs/heading_color.lyflow.json)、[彩色走廊图](../../src-tauri/graphs/corridor_color.lyflow.json)、[定向量测](../../src-tauri/src/vision/heading.rs)、[走廊量测](../../src-tauri/src/vision/taught.rs)、[彩色站判定](../../src-tauri/src/vision/judge/taught.rs)、[质量汇总](../../src-tauri/src/burst_judge.rs)。

## 算子与关联

新彩色图显式设置 `widthPolicy: "extent"`；lyFlow 的默认 `range` 行为保持兼容。粗跟踪在有限 `searchHalf` 内保留宽胶；不完整边、歧义站不伪造胶中心参与拟合。`incompleteS`、`ambiguousS` 提供定位失败的诊断。横向卡尺通过 `loComplete/hiComplete`、`loCause/hiCause` 区分真实胶边、高光/暗区与 `caliper/roi/image` 边界。

真实背景仍阻断合并；满足既有反光或低信息条件的间隙才可合并，合并宽度上界取有限观察窗口。相邻同色胶归属不明时返回未知，不因面积更大或更宽就改选邻胶。

最终站判定不再用整帧胶宽中位数乘在位比例筛除正常窄处。参考宽只约束关联，宽鼓包不会提高正常胶必须达到的宽度。固定走廊保留沿线偏移轨迹，定向路径使用拟合胶路；两者共享完整性分类。

## 站级数据契约

`FrameMetrics.measurementEvidence` 为可选字段；有值时 `version = 1`。没有该字段的旧记录继续按原语义读取，不推断丢失的宽度。

| `stations[].state` | 含义 | `widths` 对应值 | 在位 / 断胶 |
| --- | --- | --- | --- |
| `exact` | 已关联目标胶，两边完整 | 精确 mm 值 | 有胶 |
| `clipped` | 连续目标胶延伸到搜索、ROI 或图像边界 | `null`，可另有 `widthLowerBoundMm` | 有胶，量宽不完整 |
| `absent` | 有效观察区内确认没有目标胶 | `null` | 计入断胶 |
| `unknown` | 遮挡、歧义或路径不可信 | `null` | 不计在位分母，不连接断口，记不完整 |

每站可带 `reason`、`loEdge/hiEdge`（真实可见边的原图像素坐标）、`loClipped/hiClipped`、`normal`（从 lo 指向 hi 的图像单位法向）与 `position`（搜索站位置，**不是胶中心**）。只有完整站提供精确宽度和胶中心。高光、暗区和歧义不直接产生可靠下界。

帧摘要保存 `exactStations/clippedStations/absentStations/unknownStations`、`maxWidthLowerBoundMm` 和 `presentRatio`。在位率为 `(exact + clipped) / (exact + clipped + absent)`，分母为零时是 `null`。旧 `FrameMetrics.presentRatio` 保留兼容数值，新消费者优先读证据的可空值。

- 宽度 min/max/mean、均匀性和精确曲线只用 `exact`。下界不填入精确宽度列。
- 断胶长度只累计连续 `absent`；未知站打断累计但不证明胶桥存在。
- 胶偏仅使用完整几何站，截断/未知仍给帧和区域标记检测不完整。
- 定向失败只有在有效候选跟踪均证明没有胶、也没有未知站时才记全区 `absent`，其余记 `unknown:path_untrusted`。
- 生产跳过是计划适用性，优先于以上测量状态；诊断可观察跳过帧，生产聚合不纳入。

## 质量与不可用结果

精确宽度沿用既有判定。可靠下界**严格超过**胶宽上限时累计超宽长度，仍遵守 `maxExcursionMm` 与 `wideWarnOnly`；超过溢胶上限时沿用独立溢胶规则。下界等于上限不算超限，下界较小不能证明偏窄或宽度合格。

部分未知或截断不会覆盖已确认的 NG，但区域列出 `incompleteGroups`、整件标 `flags.incomplete`，不称“全部合格”。单帧无可用证据的 `frame_verdict` 是 `ERR_INSPECT`；所有参与检测帧都无可用证据时，整件使用现有检测错误路径（PLC 90，fault 91），不新增 PLC 协议码。其他缺帧行为保持原规则。

发布示教帧试测与预热也要求可用证据，全未知不能算测成；部分不完整给出提醒。起涂/收尾建议不会把未知或尚不能证明合格的截断帧当作已确认未成形。

## 界面、MCP 与存储

走廊试测、总试测逐帧页、历史详情/概览和在线帧共享证据语义：截断显示“超出测量范围”，有下界时显示“≥ x mm”；未知显示原因和测量不完整；只有确认无胶使用无胶/断胶含义。叠图显示可见边及截断方向；生产跳过仍优先。

历史 `detail_overview` 同时裁剪 `widths` 和 `measurementEvidence.stations`，保留证据摘要；逐帧接口返回完整证据。原始和对照结果使用各自指标，不覆盖原始记录。

MCP `teach_trial` 的 `measurementEvidence` 列逐帧摘要；`limits.measurementIncomplete`、`widthLowerBounds` 与 `overflowFrames` 保留不完整及可证明的过宽。`presentBelow` 与 `worst` 不把无可用在位率的未知帧当作 0% 有胶。`measurement` 同时给出测量帧数、完整帧数、不完整测量帧和无可用证据帧，不能仅凭 `state=measured` 宣称测量完整。

## 兼容、发布和回退

- 发布图版本为 `flow-3`，彩色图参数冻结 `widthPolicy=extent`，结果证据版本为 1。
- `Prepared::load` 明确拒绝图版本或引擎版本不同的旧发布包，提示重新验证并发布，不静默套新算法。已有胶色模型可在新候选中沿用，但应重测完整段与独立样本。
- 缺证据字段的旧记录可读；旧程序使用严格字段解析，不能读取新证据记录。回退需使用匹配的程序、运行库与旧发布包，并保留新记录副本，不把新记录反向覆盖成旧格式。
- 按历史指标重判不能恢复已经丢弃的宽胶；修复验证必须从原图复测，另存对照结果。没有原图时不补造数据。
- lyFlow 与 GlueSight 独立提交、CI、发布。新彩色策略需要 lyFlow 1.3.2；引擎加载时检查两个彩色算子明确支持 `widthPolicy=extent`，旧运行库或缺失能力的缓存被拒绝；GlueSight 更新客户端依赖，运行库沿既有系列恢复流程获取正式 Release，并校验实际 DLL。现场 HTTP 服务不能代替所用 DLL 的验证。

## 验证边界

可提交回归使用合成数据：宽鼓包与正常胶并存、反光合并但不跨真实背景、平行胶歧义、各观察边界截断、真无胶与遮挡区分、下界阈值/允许超差/只预警、全未知错误、历史摘要裁剪及界面生产跳过优先级。

现场原图及参数只在本机忽略目录保存，用于异常帧与完整段的诊断对比，不提交真实配置和原图。没有独立正常件/缺陷件真值与测量基准时，只报告诊断改善，不宣称生产精度、缺陷检出率或节拍验收通过。上线前按匹配版本重新验证生产配方；不通过增加跳帧、缩短测量区间或放宽公差掩盖异常。

验证入口见 [测试说明](../testing/TESTING.md)，相关契约见 [胶色决策](../decisions/color.md)、[段模型决策](../decisions/segment-burst.md)、[Agent 接口](../integration/agent-teaching.md)、[lyFlow 依赖](lyflow-dependency.md)。
