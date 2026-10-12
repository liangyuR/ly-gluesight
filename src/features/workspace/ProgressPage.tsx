import { Link, useLocation } from "react-router-dom";
import { useWorkspace } from "./context";
import { Badge, KV, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import { calibrationText, shotStateText, stepStatusText, stepStatusTone, targetPath } from "./progress";
import { returnState } from "./returnNavigation";

export default function ProgressPage() {
  const { data, doc } = useWorkspace();
  const location = useLocation();
  if (!data || !doc) return <WorkspaceEmpty/>;
  const { progress, workspace: w } = data;
  const back = returnState(location, "进度总览");
  return <div className="wp-page"><WorkspaceBar/><Steps/><Panel title="配方创建进度" detail="草稿、示教、标定验证、整套验证与生产生效分别记录"
    actions={progress.next ? <Link className="btn primary" to={targetPath(progress.next.target)} state={back}>{progress.next.label}</Link> : <Badge tone="ok">已全部完成</Badge>}>
    <div className="wp-checklist">{progress.steps.map(step => <div className={"wp-check-row" + (step.status === "done" ? " passed" : "")} key={step.id}>
      <div><strong>{step.label}{step.optional && "（可选）"}</strong> <Badge tone={stepStatusTone[step.status]}>{stepStatusText[step.status]}</Badge><p>{step.detail}</p>
        {step.blockers.length > 0 && <ul className="wp-blockers">{step.blockers.map((b, i) => <li key={i}><Link to={targetPath(b.target)} state={back}>{b.message}</Link></li>)}</ul>}</div>
    </div>)}</div>
    <KV label="上次编辑">{w.lastPosition ? `拍照点 ${w.lastPosition.k+1} · 图 ${w.lastPosition.view}` : "尚未开始示教"}</KV><Badge tone="neutral">自动保存仅保留编辑进度</Badge>
  </Panel>
  {progress.shots.length > 0 && <Panel title="逐拍照点状态" detail="示教与像素当量标定验证按选中的检测图分别记录">
    <div className="table-wrap"><table className="table"><thead><tr><th>拍照点</th><th>示教</th><th>像素当量与标定验证</th><th /></tr></thead><tbody>
      {progress.shots.map(s => { const views = s.views.filter(v => v.enabled); return <tr key={s.k}><td>k{s.k + 1} · {s.shotId}</td><td>{shotStateText[s.state]}{s.total > 1 ? ` · ${s.completed}/${s.total}` : ""}</td>
        <td>{s.state === "skipped" ? "—" : views.length ? views.map(v => `图 ${v.view}：${calibrationText(v.calibration)}`).join("；") : "未选检测图"}</td>
        <td><Link className="btn small" to={`/recipe/teach?frame=${s.k}`}>查看</Link></td></tr>; })}
    </tbody></table></div>
  </Panel>}</div>;
}
