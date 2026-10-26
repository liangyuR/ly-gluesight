import { Button, StatusBadge } from "qing-steel";
import { verdictLabel, verdictTone } from "../../history/meta";
import type { BurstJudgement, LossAlarm, PathSegment, RegionResult } from "../../plan/types";
import { countStatusLabel, expectedText, regionBadges, regionKindLabel, regionSegments, uncheckedText, uninspectedText } from "../../plan/vis";
import "./burst.css";

/** 丢帧告警的文字：哪台相机、收到几帧、与谁相差超过 1。camera 是计划相机下标。 */
export function lossAlarmText(alarm: LossAlarm, cameras: string[] = []) {
  const name = (i: number) => cameras[i] ?? `相机 ${i + 1}`;
  return alarm.reason.type === "triggers"
    ? `${name(alarm.camera)} 疑似丢帧：收到 ${alarm.count} 帧，与 PLC 触发数 ${alarm.reason.deviceTriggers} 相差超过 1`
    : `${name(alarm.camera)} 疑似丢帧：收到 ${alarm.count} 帧，与 ${name(alarm.reason.other)} 的 ${alarm.reason.otherCount} 帧相差超过 1`;
}

const mm = (v: number) => v.toFixed(2);

/** 区域覆盖的段：用来写检测区间与全长（段长未知时不写）。 */
function coveredSegments(region: RegionResult, segments: PathSegment[]) {
  const [first, last] = regionSegments(region.region.kind);
  return segments.filter(s => s.index >= first && s.index <= last);
}

function RegionItem({ result, segments, cameras, onSelectGroup }: { result: RegionResult; segments: PathSegment[]; cameras: string[]; onSelectGroup?: (seq: number) => void }) {
  const badges = regionBadges(result);
  const covered = coveredSegments(result, segments);
  const lengths = covered.map(s => s.lengthMm);
  const length = covered.length && lengths.every(v => v != null) ? lengths.reduce<number>((sum, v) => sum + v!, 0) : null;
  const status = result.countStatus === "normal" ? " · 帧数正常" : result.countStatus === null ? ` · ${countStatusLabel(null)}` : "";
  const offset = result.offset
    ? `均值偏移 ${mm(result.offset.meanShift)} · 最大偏差 ${mm(result.offset.maxDev)} mm`
    : result.region.kind.type === "corner" ? "弯道不判" : result.region.kind.type === "skipped" ? "跳过不检" : "—";
  return <li className="region-item" data-tone={verdictTone(result.verdict)} data-kind={result.region.kind.type}>
    <div className="region-head">
      <strong>{regionKindLabel(result.region.kind)}</strong>
      <StatusBadge tone={verdictTone(result.verdict)}>{verdictLabel[result.verdict]}</StatusBadge>
    </div>
    <div className="mono">{result.frameCount} 帧 · 预期 {expectedText(result.expected, result.tolerance)}{status}{length != null && ` · 全长 ${length} mm`}</div>
    {!!badges.length && <div className="region-badges">{badges.map(b => <StatusBadge key={b.key} tone={b.tone}>{b.label}</StatusBadge>)}</div>}
    <dl className="region-kv">
      <dt>胶宽 min / max / mean</dt>
      <dd>{result.width ? `${mm(result.width.min)} / ${mm(result.width.max)} / ${mm(result.width.mean)} mm` : "—"}</dd>
      <dt>胶偏</dt>
      <dd>{offset}</dd>
      <dt>各相机帧数</dt>
      <dd>{result.cameraFrames.map((n, c) => cameras[c] ? `${cameras[c]} ${n}` : String(n)).join(" · ") || "—"}</dd>
    </dl>
    {!!result.incompleteGroups.length && <div className="region-groups">
      <span className="muted">不完整的帧组</span>
      {result.incompleteGroups.map(seq => <Button key={seq} size="sm" variant="tonal" aria-label={`查看帧组 ${seq}`} disabled={!onSelectGroup} onClick={() => onSelectGroup?.(seq)}>{seq}</Button>)}
    </div>}
    {result.reason && <p className="region-reason">{result.reason}</p>}
  </li>;
}

/**
 * 段模型的检测区域结果：每个区域（直线 / 弯道 / 未分段）一条——结论、帧数是否在容差内、胶宽 min/max/mean、
 * 胶偏或未判原因、不完整帧组；下面是未检区域、未检项和丢帧告警。只展示后端给出的结果，不做判定。
 */
export default function RegionResultPanel({ judgement, segments = [], cameras = [], expectedTotal, toleranceTotal = 0, onSelectGroup }: {
  judgement: BurstJudgement | null;
  /** 本件配方的段：写区域的检测区间与全长 */
  segments?: PathSegment[];
  /** 计划相机：写各相机帧数与丢帧告警 */
  cameras?: string[];
  /** 给了才写整件预期帧数（ΣN_exp ± Σtol）；null 为待标定 */
  expectedTotal?: number | null;
  toleranceTotal?: number;
  onSelectGroup?: (seq: number) => void;
}) {
  if (!judgement) return <p className="muted">暂无区域结果</p>;
  return <div className="region-panel" aria-label="检测区域结果">
    <div className="region-sum">
      <span className="mono">帧组 {judgement.groupCount}</span>
      {expectedTotal !== undefined && <span className="mono muted">预期 {expectedText(expectedTotal, toleranceTotal)}</span>}
      <StatusBadge tone={judgement.countStatus === "normal" ? "ok" : judgement.countStatus ? "warn" : "off"}>{countStatusLabel(judgement.countStatus)}</StatusBadge>
    </div>
    <ul className="region-list" aria-label="区域">
      {judgement.regions.map((result, i) => <RegionItem key={i} result={result} segments={segments} cameras={cameras} onSelectGroup={onSelectGroup} />)}
    </ul>
    {!!judgement.uninspected.length && <div className="region-extra">
      <h4>未检区域</h4>
      <ul aria-label="未检区域">{judgement.uninspected.map((u, i) => <li key={i} className="mono">{uninspectedText(u)}</li>)}</ul>
    </div>}
    {!!judgement.unchecked.length && <p className="region-reason">未检项（本轮没有算法）：{judgement.unchecked.map(item => uncheckedText[item]).join("、")}</p>}
    {!!judgement.alarms.length && <div className="region-extra">
      <h4>丢帧告警</h4>
      <ul aria-label="丢帧告警">{judgement.alarms.map((alarm, i) => <li key={i}>{lossAlarmText(alarm, cameras)}</li>)}</ul>
    </div>}
  </div>;
}
