import { Link, useLocation } from "react-router-dom";
import { AlertTriangle, ArrowRight, ClipboardCheck, Crosshair, Database, FileCog, Scan, Upload, type LucideIcon } from "lucide-react";
import { useWorkspace } from "./context";
import { Badge, Panel, Steps, WorkspaceBar, WorkspaceEmpty } from "./components";
import { stepStatusText, stepStatusTone, targetPath } from "./progress";
import { teachingTasks } from "./teachingTasks";
import { returnState } from "./returnNavigation";
import { teachingAction } from "./TeachingProgress";
import type { StepId } from "./types";

const stages: { id: StepId; title: string; icon: LucideIcon; href: string; action: string }[] = [
  { id: "capture", title: "整圈采集", icon: Database, href: "/recipe/capture", action: "查看采集" },
  { id: "defaults", title: "配方默认参数", icon: FileCog, href: "/recipe/geometry", action: "配置参数" },
  { id: "teach", title: "单帧示教", icon: Scan, href: "/recipe/teach", action: "查看示教" },
  { id: "calibration", title: "像素当量与标定", icon: Crosshair, href: "/recipe/teach", action: "去标定" },
  { id: "validation", title: "独立样本验证", icon: ClipboardCheck, href: "/recipe/validation", action: "查看验证条件" },
  { id: "publish", title: "发布与生效", icon: Upload, href: "/recipe/validation", action: "查看发布条件" },
];

export default function ProgressPage() {
  const { data, doc } = useWorkspace();
  const location = useLocation();
  if (!data || !doc) return <WorkspaceEmpty/>;
  const { progress, workspace: w } = data;
  const tasks = teachingTasks(doc.shots, progress);
  const capture = progress.steps.find(step => step.id === "capture");
  const teach = progress.steps.find(step => step.id === "teach");
  const action = teachingAction(tasks, capture, teach);
  const next = action?.capture && progress.next?.step !== "capture" ? action : progress.next?.step === "teach" ? action : progress.next ? { href: targetPath(progress.next.target), label: progress.next.label } : action;
  const nextStep = progress.steps.find(step => step.id === (action?.capture ? "capture" : progress.next?.step ?? "teach"));
  const warning = nextStep?.status === "blocked" || action?.capture && tasks.some(task => task.phase === "recapture");
  const completed = tasks.filter(task => task.phase === "done").length;
  const currentTask = tasks.find(task => task.phase !== "done");
  const back = returnState(location, "进度总览");
  return <div className="wp-page"><WorkspaceBar/><Steps/><Panel title="配方进度" className="wp-progress-board">
    {next && <div className={"wp-progress-next " + (warning ? "warn" : "info")}>
      {warning ? <AlertTriangle size={22} aria-hidden="true"/> : <ArrowRight size={22} aria-hidden="true"/>}
      <strong>{nextStep?.detail || "继续完成当前候选配置"}</strong>
      <Link className="btn primary" to={next.href} state={back}>{next.label}<ArrowRight size={15} aria-hidden="true"/></Link>
    </div>}
    <div className="wp-progress-grid">{stages.map(({ id, title, icon: Icon, href, action: label }, index) => {
      const step = progress.steps.find(item => item.id === id);
      const status = id === "teach" && step?.status === "done" && currentTask ? currentTask.exception ? "blocked" : "inProgress" : step?.status;
      const target = id === "teach" && action ? action.href : id === "calibration" && step?.blockers[0] ? targetPath(step.blockers[0].target) : href;
      return <section key={id} className="wp-progress-stage" aria-label={id === "teach" ? "单帧示教进度" : title}>
        <span className="wp-progress-number" aria-hidden="true">{String(index + 1).padStart(2, "0")}</span>
        <div className="wp-progress-stage-body">
          <h3>{title}{step?.optional && "（可选）"}</h3>
          <Badge tone={status ? stepStatusTone[status] : "neutral"}>{status ? stepStatusText[status] : "状态待确认"}</Badge>
          {id === "teach" ? <output aria-label="有效示教组数">{completed} / {tasks.length} 组有效</output> : <p>{step?.detail || "尚未取得当前阶段进度"}</p>}
          <Link className="wp-progress-link" to={target} state={back}><ArrowRight size={14} aria-hidden="true"/>{id === "teach" && action ? action.label : label}</Link>
        </div>
        <Icon className="wp-progress-stage-icon" size={42} strokeWidth={1.25} aria-hidden="true"/>
      </section>;
    })}</div>
    {w.lastPosition && <div className="wp-progress-last"><span>上次编辑</span><span>{doc.shots[w.lastPosition.k]?.poseId ?? ""} · {doc.shots[w.lastPosition.k]?.camera ?? ""} · 图 {w.lastPosition.view}</span></div>}
  </Panel></div>;
}
