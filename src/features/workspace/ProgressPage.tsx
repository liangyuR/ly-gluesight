import { Link, useLocation } from "react-router-dom";
import { useWorkspace } from "./context";
import { Badge, KV, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import { calibrationText, stepStatusText, stepStatusTone, targetPath } from "./progress";
import { teachingGroups } from "./teachingGroups";
import { returnState } from "./returnNavigation";

export default function ProgressPage() {
  const { data, doc } = useWorkspace();
  const location = useLocation();
  if (!data || !doc) return <WorkspaceEmpty/>;
  const { progress, workspace: w } = data;
  const groups = teachingGroups(doc.shots, progress);
  const back = returnState(location, "进度总览");
  return <div className="wp-page"><WorkspaceBar/><Steps/><Panel title="配方创建进度" detail="草稿、示教、标定验证、整套验证与生产生效分别记录"
    actions={progress.next ? <Link className="btn primary" to={targetPath(progress.next.target)} state={back}>{progress.next.label}</Link> : <Badge tone="ok">已全部完成</Badge>}>
    <div className="wp-checklist">{progress.steps.map(step => <div className={"wp-check-row" + (step.status === "done" ? " passed" : "")} key={step.id}>
      <div><strong>{step.label}{step.optional && "（可选）"}</strong> <Badge tone={stepStatusTone[step.status]}>{stepStatusText[step.status]}</Badge><p>{step.detail}</p>
        {step.blockers.length > 0 && <ul className="wp-blockers">{step.blockers.map((b, i) => <li key={i}><Link to={targetPath(b.target)} state={back}>{b.message}</Link></li>)}</ul>}</div>
    </div>)}</div>
    <KV label="上次编辑">{w.lastPosition ? `${doc.shots[w.lastPosition.k]?.poseId ?? ""} · ${doc.shots[w.lastPosition.k]?.camera ?? ""} · 图 ${w.lastPosition.view}` : "尚未开始示教"}</KV><Badge tone="neutral">自动保存仅保留编辑进度</Badge>
  </Panel>
  {groups.length > 0 && <Panel title="逐拍照组状态" detail="每组只示教一张检测图，其余相机图仍采集">
    <div className="table-wrap"><table className="table"><thead><tr><th>拍照组</th><th>检测图</th><th>示教</th><th>像素当量与标定验证</th><th /></tr></thead><tbody>
      {groups.map(group => { const choice = group.choice; const view = choice && progress.shots[choice.k]?.views.find(v => v.view === choice.view); return <tr key={group.poseId}><td>{group.poseId}</td><td>{choice ? `${doc.shots[choice.k].camera} · 图 ${choice.view}` : "未选定"}</td><td>{group.status}</td>
        <td>{view ? calibrationText(view.calibration) : "选择检测图后验证"}</td>
        <td><Link className="btn small" to={`/recipe/teach?frame=${choice?.k ?? group.members[0]}`}>查看</Link></td></tr>; })}
    </tbody></table></div>
  </Panel>}</div>;
}
