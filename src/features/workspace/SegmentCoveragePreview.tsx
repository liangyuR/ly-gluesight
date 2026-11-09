import { Table, type TableColumn } from "qing-steel";
import type { CoverageRange, InspectionCoverage, OffsetCoverageReason, SegmentCoverage } from "../plan/types";
import { Notice } from "./components";

const offsetReason: Record<OffsetCoverageReason, string> = {
  disabled: "未启用胶偏",
  skipped: "不检测",
  corner: "弯道不判胶偏",
  unsegmented: "尚未形成可判胶偏的直线区域",
  lowConfidence: "分段位置不确定",
  noReference: "缺少胶偏参考",
  measureNotImplemented: "缺少零件边查找参数",
  noInteriorFrames: "去掉区域首尾后没有可判帧",
  excludedGroups: "可判帧均已在示教件中屏蔽",
};

function rangesText(ranges: CoverageRange[]) {
  return ranges.map(range => range.firstSeq === range.lastSeq ? `${range.firstSeq}` : `${range.firstSeq}–${range.lastSeq}`).join("、") || "无";
}

export default function SegmentCoveragePreview({ coverage }: { coverage: InspectionCoverage }) {
  const columns: TableColumn<SegmentCoverage>[] = [
    { key: "segment", header: "段", render: row => `第 ${row.segment} 段 · ${row.skipped ? "不检测" : row.kind === "straight" ? "直线" : "弯道"}` },
    { key: "frames", header: "检测范围", render: row => row.skipped ? "不检测" : row.expectedCameraFrames == null ? "尚无法确认" : <div>
      <strong className={row.expectedCameraFrames === 0 ? "c-warn" : undefined}>应测 {row.expectedCameraFrames} 帧</strong>
      {row.availableCameraFrames !== row.expectedCameraFrames && <p>示教件可用 {row.availableCameraFrames} 帧</p>}
      {row.cameras.map(camera => <p key={camera.camera}>{camera.camera}：帧组 {rangesText(camera.ranges)}</p>)}
    </div> },
    { key: "offset", header: "胶偏", render: row => <div>
      {row.offset.expectedFrames > 0 && <p>{row.offset.camera}：应判 {row.offset.expectedFrames} 帧{row.offset.availableFrames !== row.offset.expectedFrames && `，示教件可用 ${row.offset.availableFrames} 帧`}（帧组 {rangesText(row.offset.ranges)}）</p>}
      {row.offset.unjudgedReasons.length > 0 && <p>{row.offset.unjudgedReasons.map(reason => offsetReason[reason]).join("；")}</p>}
    </div> },
  ];
  return <section className="wp-stack" aria-label="检测覆盖预览">
    <p>边界保护 {coverage.boundaryGuard} 个帧组{coverage.expectedCameraFrames != null && ` · 合计应测 ${coverage.expectedCameraFrames} 帧（各检测相机分别计数）`}。</p>
    {coverage.zeroCoverageSegments.length > 0 && <Notice title="有检测段没有有效帧" tone="warn">
      第 {coverage.zeroCoverageSegments.join("、")} 段在边界保护或跳过范围展开后，应测帧数为 0。请复核分段边界和跳过设置，再决定是否保存。
    </Notice>}
    {!coverage.boundariesTrusted && <Notice title="检测范围尚无法确认" tone="warn">
      当前帧数或分段配置不能确定每段位置，不能据此认定各段已经覆盖；逐帧检测结果仍需试测确认。
    </Notice>}
    {coverage.excludedGroups.length > 0 && <p>示教件已屏蔽帧组 {coverage.excludedGroups.join("、")}，只扣除本次示教可用帧，不改变生产检测范围。</p>}
    <Table caption="逐段检测覆盖" columns={columns} rows={coverage.segments} rowKey={row => String(row.segment)} />
    <p className="muted">以上按检测规则计算应测、应判帧数；图像缺失或测量失败仍会减少实测帧数，请在试测结果中核对。</p>
  </section>;
}
