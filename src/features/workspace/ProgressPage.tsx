import { Link, useLocation } from "react-router-dom";
import { useWorkspace } from "./context";
import { Badge, KV, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import { stepStatusText, stepStatusTone, targetPath } from "./progress";
import { teachingTasks } from "./teachingTasks";
import { returnState } from "./returnNavigation";
import { TeachingGroupTable, TeachingProgressCard, teachingAction } from "./TeachingProgress";
import type { ProgressStep } from "./types";

export default function ProgressPage() {
  const { data, doc } = useWorkspace();
  const location = useLocation();
  if (!data || !doc) return <WorkspaceEmpty/>;
  const { progress, workspace: w } = data;
  const tasks = teachingTasks(doc.shots, progress);
  const capture = progress.steps.find(step => step.id === "capture");
  const teach = progress.steps.find(step => step.id === "teach");
  const action = teachingAction(tasks, capture, teach);
  const next = progress.next?.step === "teach" ? action : progress.next ? { href: targetPath(progress.next.target), label: progress.next.label } : null;
  const complete = progress.steps.length > 0 && progress.steps.every(step => step.optional || step.status === "done") && tasks.every(task => task.phase === "done");
  const back = returnState(location, "进度总览");
  const blockers = (step: ProgressStep) => <ul className="wp-blockers">{step.blockers.map((b, i) => <li key={i}><Link to={targetPath(b.target)} state={back}>{b.message}</Link></li>)}</ul>;
  return <div className="wp-page"><WorkspaceBar/><Steps/><Panel title="配方创建进度" detail="草稿、示教、标定验证、整套验证与生产生效分别记录"
    actions={next ? <Link className="btn primary" to={next.href} state={back}>{next.label}</Link> : <Badge tone={complete ? "ok" : "neutral"}>{complete ? "已全部完成" : "状态待确认"}</Badge>}>
    <div className="wp-checklist">{progress.steps.map(step => step.id === "teach" ? <TeachingProgressCard key={step.id} tasks={tasks} step={step} capture={capture}/> : <section className={"wp-check-row" + (step.status === "done" ? " passed" : "")} key={step.id} aria-label={step.label}>
      <div><strong>{step.label}{step.optional && "（可选）"}</strong> <Badge tone={stepStatusTone[step.status]}>{stepStatusText[step.status]}</Badge><p>{step.detail}</p>
        {step.blockers.length > 1 ? <details className="wp-progress-diagnostics"><summary>查看 {step.blockers.length} 项阻塞原因</summary>{blockers(step)}</details> : step.blockers.length > 0 && blockers(step)}</div>
    </section>)}</div>
    <KV label="上次编辑">{w.lastPosition ? `${doc.shots[w.lastPosition.k]?.poseId ?? ""} · ${doc.shots[w.lastPosition.k]?.camera ?? ""} · 图 ${w.lastPosition.view}` : "尚未开始示教"}</KV><Badge tone="neutral">自动保存仅保留编辑进度</Badge>
  </Panel>
  {tasks.length > 0 && <TeachingGroupTable key={doc.id} tasks={tasks} shots={doc.shots}/>}</div>;
}
