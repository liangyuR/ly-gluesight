import { Link, useLocation } from "react-router-dom";
import { AlertTriangle, ArrowRight, History, Lock } from "lucide-react";
import { Progress } from "qing-steel";
import { useWorkspace } from "./context";
import { Badge, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { stepStatusText, stepStatusTone, targetPath } from "./progress";
import { stageOf, stages, stageStatus, stageStatusText, stageStatusTone } from "./stages";
import { teachingTasks } from "./teachingTasks";
import { returnState } from "./returnNavigation";
import { teachingAction } from "./TeachingProgress";
import { PlcParamsPanel } from "./PlcParams";
import { useSiteStatus } from "../cycle/api";
import type { StepId } from "./types";

const actions: Record<string, string> = { "/recipe/capture": "查看采集", "/recipe/geometry": "配置参数", "/recipe/teach": "查看示教", "/recipe/overview": "查看总览", "/recipe/validation": "查看验证与发布" };
const subLabels: Partial<Record<StepId, string>> = { calibration: "标定", publish: "发布" };

export default function ProgressPage() {
  const { data, doc } = useWorkspace();
  const location = useLocation();
  const site = useSiteStatus();
  if (!data || !doc) return <WorkspaceEmpty/>;
  const { progress, workspace: w } = data;
  const tasks = teachingTasks(doc.shots, progress);
  const capture = progress.steps.find(step => step.id === "capture");
  const teach = progress.steps.find(step => step.id === "teach");
  const action = teachingAction(tasks, capture, teach);
  const next = action?.capture && progress.next?.step !== "capture" ? action : progress.next?.step === "teach" ? action : progress.next ? { href: targetPath(progress.next.target), label: progress.next.label } : action;
  const nextStep = progress.steps.find(step => step.id === (action?.capture ? "capture" : progress.next?.step ?? "teach"));
  const warning = nextStep?.status === "blocked" || action?.capture && tasks.some(task => task.phase === "recapture");
  const reasons = [...new Set(nextStep?.blockers.map(blocker => blocker.message) ?? [])].filter(message => message !== nextStep?.detail).slice(0, 3);
  const completed = tasks.filter(task => task.phase === "done").length;
  const back = returnState(location, "进度总览");
  const last = w.lastPosition && doc.shots[w.lastPosition.k];
  return <div className="wp-page"><WorkspaceBar /><Panel title="配方进度" className="wp-progress-board">
    {next && <section className={"wp-progress-next " + (warning ? "warn" : "info")} aria-label="下一步">
      {warning ? <AlertTriangle size={22} aria-hidden="true"/> : <ArrowRight size={22} aria-hidden="true"/>}
      <div className="wp-progress-next-body">
        <small>下一步{nextStep && " · " + (stageOf(nextStep.id)?.label ?? nextStep.label)}</small>
        <strong>{nextStep?.detail || "继续完成当前候选配置"}</strong>
        {reasons.length > 0 && <ul>{reasons.map(reason => <li key={reason}>{reason}</li>)}</ul>}
      </div>
      <Link className="qs-btn" to={next.href} state={back}>{next.label}<ArrowRight size={15} aria-hidden="true"/></Link>
    </section>}
    <div className="wp-progress-list">{stages.map((stage, index) => {
      const status = stageStatus(progress, stage, tasks);
      const waiting = status === "waiting";
      const main = progress.steps.find(step => step.id === stage.ids[0]);
      const subs = [...stage.ids.slice(1), ...stage.sub].flatMap(id => progress.steps.filter(step => step.id === id));
      const isTeach = stage.ids.includes("teach");
      const link = isTeach && action && !action.capture ? action : { href: stage.path, label: actions[stage.path] };
      return <section key={stage.path} className={"wp-progress-row" + (waiting ? " waiting" : "")} aria-label={stage.label + "进度"}>
        <span className={"wp-progress-number" + (stage.optional ? " optional" : "")} aria-hidden="true">{index + 1}</span>
        <div className="wp-progress-row-body">
          <h3>{stage.label}{stage.optional && <small>可选</small>}</h3>
          <p>{waiting ? "整圈采集完成后可继续" : main?.detail && main.status !== "optional" ? main.detail : stage.hint}</p>
          {!waiting && subs.map(step => <p key={step.id} className="wp-progress-sub"><span>{subLabels[step.id] ?? step.label}</span><Badge tone={stepStatusTone[step.status]}>{stepStatusText[step.status]}</Badge>{step.detail}</p>)}
        </div>
        <div className="wp-progress-meter">{isTeach && <>
          <Progress label="示教进度" value={completed} max={Math.max(1, tasks.length)} />
          <output aria-label="有效示教组数">{completed} / {tasks.length} 组有效</output>
        </>}</div>
        <output className="wp-progress-state" aria-label={stage.label + "状态"}>{waiting ? <Badge tone="neutral"><Lock size={12} aria-hidden="true"/>{stageStatusText.waiting}</Badge> : status !== "optional" && <Badge tone={status ? stageStatusTone[status] : "neutral"}>{status ? stageStatusText[status] : "状态待确认"}</Badge>}</output>
        <Link className="wp-progress-link" to={link.href} state={back}>{link.label}<ArrowRight size={14} aria-hidden="true"/></Link>
      </section>;
    })}</div>
    {w.lastPosition && last && <div className="wp-progress-last">
      <History size={14} aria-hidden="true"/><span>上次编辑 · {last.poseId} · {last.camera} · 图 {w.lastPosition.view}</span>
      <Link className="wp-progress-link" to={targetPath({ page: "teach", k: w.lastPosition.k })} state={back}>回到上次位置<ArrowRight size={14} aria-hidden="true"/></Link>
    </div>}
  </Panel>
  <PlcParamsPanel production={data.productionPlc} version={data.productionVersion} planChanged={data.planChanged} offline={site?.mode === "offline"} /></div>;
}
