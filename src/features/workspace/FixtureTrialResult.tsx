import type { FixtureTrial, FixtureTrialRow } from "./types";
import { Badge } from "./components";

const fmt = (v: number | null | undefined, digits = 2) => v != null && Number.isFinite(v) ? v.toFixed(digits) : "—";

function deviationSummary(row: FixtureTrialRow) {
  const values = row.stations.map(s => s.d).filter((d): d is number => d != null && Number.isFinite(d));
  if (!values.length) return "—";
  const mean = values.reduce((a, b) => a + b, 0) / values.length;
  return `${fmt(Math.min(...values))} / ${fmt(mean)} / ${fmt(Math.max(...values))}`;
}

const statusText = { ok: "有基准", no_ref: "无基准", out_of_range: "超出检测范围", no_bead: "无胶" } as const;

/** 定位试测结果：每帧一行（位姿是否采用、原因、分数、次峰比、偏移、角度、两端搬移、d′ 的最小 / 平均 / 最大），可展开看逐站 d′。只作诊断。 */
export function FixtureTrialResult({ trial }: { trial: FixtureTrial }) {
  const teach = trial.rows.find(row => row.source === "teach");
  const invalid = !teach?.pose?.accepted;
  return <section className="panel wp-fixture-trial" aria-label="定位试测结果">
    <h3>定位试测 <small>示教图与样本库同一拍照点、同一幅图；只作诊断，不保存</small></h3>
    {invalid && <p className="c-warn" role="alert">示教图上定位没被采用，这次定位试测无效：{teach?.reason ?? teach?.error ?? "没有定位结果"}</p>}
    {trial.rows.length === 1 && <p className="muted">样本库里还没有这个拍照点、这幅图的其他原图；验证页实拍或导入独立样本后可在这里试定位。</p>}
    <div className="table-wrap"><table className="table" aria-label="定位试测逐帧结果">
      <thead><tr><th>帧</th><th>位姿</th><th>分数</th><th>次峰比</th><th>偏移 px</th><th>角度 °</th><th>两端搬移 mm</th><th>d′ 最小 / 平均 / 最大 mm</th></tr></thead>
      <tbody>{trial.rows.map((row, i) => <tr key={i}>
        <td><details><summary>{row.label}</summary>{row.stations.length ? <ol className="wp-fixture-stations">{row.stations.map(s => <li key={s.id}>站 {s.id}：{statusText[s.status]}{s.d != null ? ` · ${fmt(s.d, 3)} mm` : ""}</li>)}</ol> : <p className="muted">没有逐站结果</p>}</details></td>
        <td>{row.error ? <span className="c-warn">{row.error}</span> : row.pose?.accepted ? <Badge tone="ok">采用</Badge> : <><Badge tone="warn">不采用</Badge> <span>{row.reason}</span></>}</td>
        <td>{fmt(row.pose?.score, 3)}</td>
        <td>{fmt(row.pose?.peakRatio)}</td>
        <td>{row.pose?.offset ? `${fmt(row.pose.offset[0], 1)}, ${fmt(row.pose.offset[1], 1)}` : "—"}</td>
        <td>{fmt(row.pose?.angle)}</td>
        <td>{row.pose?.endShiftMm ? `${fmt(row.pose.endShiftMm[0])}, ${fmt(row.pose.endShiftMm[1])}` : "—"}</td>
        <td>{deviationSummary(row)}</td>
      </tr>)}</tbody>
    </table></div>
  </section>;
}
