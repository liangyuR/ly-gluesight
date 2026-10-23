# 胶条位置基准：方法及资料调研

整理日期：2026-10-20。用途：支撑 [工件边缘中心距方案](../architecture/workpiece-edge-position.md)。以下资料均查阅了公开正文；每条注明它支撑的结论和适用范围。

> 现状（2026-10-23）：现场布置已定为固定胶枪、机器人抓工件从胶嘴下走过、三台相机固定在胶嘴周围（分段连拍飞拍，见 [分段连拍决策记录](../decisions/segment-burst.md)），下文“相机随枪移动”等表述是调研时的一般情形，不是本现场。“工件边缘基准”在段配方里对应直线段胶偏的 `ToEdge` 测量量，模型中保留、尚未实现（D-SB-15）；现行只有 `AbsoluteX`（帧内胶中心相对走廊的法向偏移均值与标准件 x_ref 比）。

## 1. 三种基准

| 问题 | 基准 | 适用场景 |
| --- | --- | --- |
| 胶相对示教图偏了多少 | 固定图像中线 | 拍照位姿和工件摆放重复稳定 |
| 刚出胶的位置在哪里 | 枪嘴/出胶点与相机的关系 | 搜索区域、自动示教、随枪检测 |
| 胶是否打在工件规定位置 | 工件指定边缘、特征或已定位的设计胶路 | 工件坐标下的位置要求 |

本项目的现场需求属于第三种。机器人 TCP 标定服务于运动控制，视觉枪嘴标定建立相机与出胶点的关系，像素/平面标定服务于尺寸换算；三者协同工作，各管一件事。

相机随枪移动时，机器人补偿工件位姿后画面可能与示教图一致，也可能因转弯、姿态和触发时刻而变化，以实际图像为准。

## 2. 综述：整体技术路线

**Data technologies in robotic sealing: a systematic review on inspection and quality control**，Giovana Sarti 等，The International Journal of Advanced Manufacturing Technology，144，6461–6479，2026-05-05。[全文](https://link.springer.com/article/10.1007/s00170-026-18219-x)，[PDF](https://link.springer.com/content/pdf/10.1007/s00170-026-18219-x.pdf)，DOI `10.1007/s00170-026-18219-x`。

- 内容：系统整理 65 项研究，覆盖二维视觉、三维测量、机器人轨迹与质量控制；第 2.2 节涉及工件位姿、配准与机器人集成，第 4.3 节涉及复杂表面与三维自适应。
- 用途：了解整体路线、追溯原始论文。文中的速度和准确率来自其他场景，仅作参考。

## 3. 先定位工件再测胶

**RELIABOT PC Adhesive Inspection**，Robotic Industries Association / A3，2004-07-29。[案例原文](https://www.automate.org/robotics/case-studies/reliabot-pc-adhesive-inspection)

- 内容：先训练工件稳定特征，用 PatMax 求工件平移和转角，再示教胶路，用 FindLine/Caliper 测边，按位置公差判定；明确定位的目的是得到胶相对工件的位置。
- 支撑结论：“工件定位 → 测胶 → 相对工件判定”的流程，与本方案的基准思想一致。
- 适用范围：涂胶后检测的早期案例，用于参考流程，性能与随枪系统需另行验证。

## 4. 枪嘴作为搜索起点

**一种胶条检测自动示教方法**，易思维，CN110189316A，2019-08-30。[专利全文](https://patents.google.com/patent/CN110189316A/zh)

- 内容：实施例 S1 以胶枪头的图像坐标为预设初始点，设置扇环形检测区域，按位置、宽度选择胶边并迭代更新区域，存储示教轮廓。
- 支撑结论：枪嘴坐标用于自动示教和搜索区域。文中的位置误差项用于筛选候选边，属于搜索环节，与成品位置验收是两件事。

## 5. 机器人轨迹映射到图像

**一种基于机器人示教点信息的涂胶在线检测方法**，易思维（天津），CN108537808A，2018-09-14。[专利全文](https://patents.google.com/patent/CN108537808A/zh)

- 内容：通过标定把机器人示教点的三维坐标映射到图像，再做示教点校正、插值和胶条轮廓追踪。
- 支撑结论：机器人轨迹经标定后可作为图像搜索先验。校正后的搜索线贴合实际胶条，便于提取胶边；判断胶偏仍需要独立的工艺基准，这正是本方案引入工件边的原因。

## 6. ISV 产品公开资料

- [ISV 官方：涂胶质量检测](https://www.isv-tech.com/Automobile/601.html)：在线检测胶宽、位置和连续性，支持人工/自动示教。
- [ISV EHR：Glue bead inspection](https://www.ehr.de/en/products/glue-bead-inspection/)：传感器装在枪嘴附近，三相机提供环绕枪嘴的 360° 视野。

这两份资料确认了随枪、多相机的结构。位置检测具体采用哪种基准、枪嘴标定输出哪些参数，需要现场型号的标定界面或手册确认。

## 7. 可复用的 LyFlow 源码

链接固定在调研时的提交；是否已随当前安装包交付、现场效果如何，在阶段 A 验证。

| 源码 | 能力 |
| --- | --- |
| [station_calipers.cpp](https://github.com/liangyuR/lyFlow/blob/5cefe3c75b227d138f1435ae7d6e7486b397926b/packs/glue/ops/station_calipers.cpp) | 宿主给法向，分别搜索工件边和胶边，输出中心距、胶宽及缺边/缺胶状态；毫米距离为无符号，像素距离有向 |
| [locate.cpp](https://github.com/liangyuR/lyFlow/blob/5cefe3c75b227d138f1435ae7d6e7486b397926b/packs/glue/ops/locate.cpp) | 模板估计二维刚体位姿，可变换测点和法向 |
| [flyshot.cpp](https://github.com/liangyuR/lyFlow/blob/5cefe3c75b227d138f1435ae7d6e7486b397926b/packs/glue/algo/flyshot.cpp) | 卡尺式工件边搜索和胶边测量的底层实现 |
| [edge.cpp](https://github.com/liangyuR/lyFlow/blob/5cefe3c75b227d138f1435ae7d6e7486b397926b/packs/glue/algo/edge.cpp) | 从胶向两侧找边、自动选侧、剔除异常距离；适合辅助对比 |

GlueSight 当前的图构建与偏移计算见 [vision/taught.rs](../../src-tauri/src/vision/taught.rs)，使用示教路径和胶宽算子。

## 8. 调研结论

主线方案：指定工件边 → 独立识别 → 沿法向测胶中心距 → 按 N～M 判定；需要沿边定位时加独立工件特征。以下几点是从资料推出的设计原则，在本项目中通过阶段 A 验证：

- 工件基准的识别独立于胶，真实胶偏得以保留在测量结果中。
- 胶搜索带宽于验收范围，超差的胶按超差判定。
- 单条直边只确定法向位置，沿边定位依靠额外特征。
- 同帧相对测距抵消共同平移和平面内旋转；透视、高度视差、错边和运动模糊需要单独评估。

待现场补充：指定工艺边、各视图可见性、距离要求与公差规则、位姿变化范围、绝对尺寸依据，以及 ISV 现场型号的标定说明。
