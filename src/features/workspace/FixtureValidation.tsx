import { useState } from "react";
import { CircleCheck, TriangleAlert } from "lucide-react";
import { Button, Dialog } from "qing-steel";
import type { FixtureCheckItem, FixtureViewReport } from "./types";
import { Badge, Panel } from "./components";

const mm = (v: number | null | undefined, digits = 3) => v != null && Number.isFinite(v) ? `${v.toFixed(digits)} mm` : "—";

function Item({ item }: { item: FixtureCheckItem }) {
  return <li className={"wp-fixture-check" + (item.passed ? "" : item.level === "block" ? " block" : " warn")}>
    {item.passed ? <CircleCheck size={14} aria-hidden="true"/> : <TriangleAlert size={14} aria-hidden="true"/>}<span>{item.detail}</span>
  </li>;
}

/**
 * 验证页“特征定位”：每个特征定位视图在独立正常样本上的统计（全部定位成功、σ、μ、搜索范围占用、亮度 ±40）、
 * 两端参数建议（确认后由后端从同一份统计写进配方）与默认折叠的转角证据。只读统计，不改框、不校正名义值。
 */
export function FixtureValidation({ reports, current, disabled, onApply }: { reports: FixtureViewReport[]; current: boolean; disabled: boolean; onApply: (k: number, view: number) => Promise<boolean> }) {
  const [confirm, setConfirm] = useState<FixtureViewReport | null>(null);
  const [applying, setApplying] = useState(false);
  const apply = async () => {
    if (!confirm || applying) return;
    setApplying(true);
    try { if (await onApply(confirm.k, confirm.view)) setConfirm(null); } finally { setApplying(false); }
  };
  return <Panel title="特征定位" detail="独立正常样本上的定位统计；两端参数建议要确认后才写进配方，名义值不会自动校正" actions={<Badge tone={reports.every(r => r.passed) ? "ok" : "warn"}>{reports.every(r => r.passed) ? "全部通过" : `${reports.filter(r => !r.passed).length} 幅未通过`}</Badge>}>
    {!current && <p className="c-warn" role="note">这是之前修订的统计，候选改过后要重新验证。</p>}
    {reports.map(report => {
      const failed = report.checks.filter(c => !c.passed), passed = report.checks.filter(c => c.passed);
      const s = report.suggestion;
      return <section key={`${report.k}-${report.view}`} className="wp-fixture-report" aria-label={`拍照点 ${report.shotId} 图 ${report.view} 特征定位统计`}>
        <h3>拍照点 {report.shotId} · 图 {report.view} <Badge tone={report.passed ? "ok" : "warn"}>{report.passed ? "通过" : "未通过"}</Badge></h3>
        <p className="muted">{report.samples} 件 · σ {mm(report.sigmaMm)}（≤ {mm(report.toleranceMm / 5)}） · μ {mm(report.meanMm)}（名义 {mm(report.nominalMm, 2)}） · 搜索范围最多用到 {report.searchUse != null ? `${Math.round(report.searchUse * 100)}%` : "—"}</p>
        {failed.length > 0 && <ul className="wp-fixture-checks" aria-label="未满足的检查">{failed.map(item => <Item key={item.id} item={item}/>)}</ul>}
        {passed.length > 0 && <details><summary>已满足 {passed.length} 项</summary><ul className="wp-fixture-checks">{passed.map(item => <Item key={item.id} item={item}/>)}</ul></details>}
        {s && <div className="wp-fixture-suggestion" role="group" aria-label="两端参数建议">
          <p>建议：两端搬移上限 <b>{s.maxEndShiftMm} mm</b>、示教余量 m <b>{s.endMarginMm} mm</b>（两端 |Δ| 的 P99 = {s.p99Mm.toFixed(2)} mm）</p>
          <p className={s.applicable ? "muted" : "c-warn"}>{s.note}</p>
          <Button size="sm" variant="tonal" disabled={disabled || !current || !s.applicable} title={!current ? "统计不是当前修订的，先重新验证" : undefined} onClick={() => setConfirm(report)}>采用建议…</Button>
        </div>}
        <details className="wp-fixture-rotation"><summary>转角证据（开转角的条件）</summary>
          <ul>
            <li>±3° 离线定位的 L·|θ| P95：{mm(report.rotation.leverThetaP95Mm)}，门限 公差/5 = {mm(report.rotation.thresholdMm)}（L = {report.rotation.leverMm.toFixed(1)} mm）{report.rotation.exceeds ? "，超过" : "，没超过"}</li>
            <li>±3° 下看得出角度的帧（含亮度 ±40）：{report.rotation.observableFrames} / {report.rotation.frames}</li>
            <li>{report.rotation.enabled ? "这幅图已开转角。" : report.rotation.autoConditions ? "前两个条件都满足；还要有独立证据，才能到示教页高级设置里开转角。" : "不满足开转角的条件，保持只找平移。"}</li>
          </ul>
        </details>
      </section>;
    })}
    {confirm?.suggestion && <Dialog open title="采用两端参数建议" closeDisabled={applying} onOpenChange={open => { if (!open) setConfirm(null); }} footer={<>
      <Button disabled={applying} onClick={() => setConfirm(null)}>取消</Button>
      <Button variant="filled" disabled={applying} onClick={() => void apply()}>确认写入配方</Button>
    </>}>
      <p>将把拍照点 {confirm.shotId} 图 {confirm.view} 的两端搬移上限设为 <b>{confirm.suggestion.maxEndShiftMm} mm</b>、示教余量 m 设为 <b>{confirm.suggestion.endMarginMm} mm</b>。</p>
      <p>以后这幅图胶路首末各长 m 的一段（起胶、收胶）永远不判。改的是测量定义：这幅图要重新试测，整套要重新验证。</p>
    </Dialog>}
  </Panel>;
}
