import { useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { AlertTriangle, ArrowRight } from "lucide-react";
import type { ShotSpec } from "../cycle/types";
import type { ProgressStep } from "./types";
import type { TeachingTask } from "./teachingTasks";
import { Badge, Panel } from "./components";
import { calibrationText, stepStatusText, stepStatusTone, targetPath } from "./progress";
import { returnState } from "./returnNavigation";

export function teachingAction(tasks: TeachingTask[], capture: ProgressStep | undefined, step: ProgressStep | undefined) {
  const recapture = tasks.some(task => task.phase === "recapture") || capture?.status === "blocked";
  if (capture?.status !== "done" || recapture || step?.blockers.some(b => b.target.page === "capture")) return { label: recapture ? "前往重新采集" : "前往整圈采集", href: "/recipe/capture", capture: true };
  const next = tasks.find(task => task.phase !== "done");
  return next ? { label: "继续示教", href: next.href, capture: false } : null;
}

export function TeachingProgressCard({ tasks, step, capture }: { tasks: TeachingTask[]; step: ProgressStep; capture?: ProgressStep }) {
  const location = useLocation();
  const back = returnState(location, "进度总览");
  const completed = tasks.filter(task => task.phase === "done").length;
  const selection = tasks.filter(task => task.phase === "select").length;
  const exceptions = tasks.filter(task => task.exception);
  const teaching = tasks.length - completed - selection - exceptions.length;
  const next = tasks.find(task => task.phase !== "done");
  const action = teachingAction(tasks, capture, step);
  const recapture = tasks.filter(task => task.phase === "recapture");
  return <section className="wp-check-row wp-teach-progress" aria-label="单帧示教进度">
    <div className="wp-teach-progress-head"><strong>{step.label}</strong><Badge tone={stepStatusTone[step.status]}>{stepStatusText[step.status]}</Badge><span className="wp-teach-count">{completed}<small> / {tasks.length} 组已示教</small></span></div>
    {tasks.length > 0 && <progress className="wp-teach-meter" aria-label="单帧示教完成进度" value={completed} max={tasks.length}/>}
    <div className="wp-teach-counts"><span>待选图 <b>{selection}</b></span><span>待示教 <b>{teaching}</b></span><span className={exceptions.length ? "c-warn" : ""}>异常 <b>{exceptions.length}</b></span></div>
    {action?.capture ? <div className="wp-teach-alert"><AlertTriangle size={17} aria-hidden="true"/><div><strong>{recapture.length || capture?.status === "blocked" ? "采集需要更新" : "先完成整圈采集"}</strong><p>{recapture.length ? `${recapture[0].poseId} · ${recapture[0].message}${recapture.length > 1 ? `，另有 ${recapture.length - 1} 组需重采。` : ""}` : capture?.detail ?? "尚未取得完整采集进度，请前往采集页确认。"}</p></div></div>
      : next && <div className="wp-teach-todo"><span>当前待办</span><strong>{next.poseId} · {next.label}</strong>{next.exception && <p>{next.message}</p>}</div>}
    <div className="wp-actions">{action && <Link className="btn primary" to={action.href} state={back}>{action.label}<ArrowRight size={14} aria-hidden="true"/></Link>}{tasks.length > 0 && <button className="btn" onClick={() => { const target = document.getElementById("teaching-groups"); target?.scrollIntoView({ block: "start" }); target?.focus({ preventScroll: true }); }}>查看分组明细</button>}</div>
    {step.blockers.length > 0 && <details className="wp-progress-diagnostics"><summary>原始诊断</summary><ul className="wp-blockers">{step.blockers.map((blocker, i) => <li key={i}><Link to={targetPath(blocker.target)} state={back}>{blocker.message}</Link></li>)}</ul></details>}
  </section>;
}

type Filter = "unfinished" | "all" | "select" | "teaching" | "exception" | "done";
const filters: [Filter, string][] = [["unfinished", "未完成"], ["all", "全部"], ["select", "待选图"], ["teaching", "待示教"], ["exception", "异常"], ["done", "已完成"]];
const matches = (task: TeachingTask, filter: Filter) => filter === "all" || (filter === "unfinished" ? task.phase !== "done" : filter === "exception" ? task.exception : filter === "teaching" ? ["teach", "trial", "save"].includes(task.phase) : task.phase === filter);

export function TeachingGroupTable({ tasks, shots }: { tasks: TeachingTask[]; shots: ShotSpec[] }) {
  const [filter, setFilter] = useState<Filter>("unfinished");
  const location = useLocation();
  const back = returnState(location, "进度总览");
  const visible = tasks.filter(task => matches(task, filter));
  return <div id="teaching-groups" className="wp-teaching-groups" tabIndex={-1}><Panel title="逐拍照组状态" actions={<span className="muted">{visible.length} / {tasks.length} 组</span>}>
    <div className="wp-teach-filters" role="group" aria-label="拍照组状态筛选">{filters.map(([key, label]) => <button key={key} className={"btn small" + (filter === key ? " primary" : "")} aria-pressed={filter === key} onClick={() => setFilter(key)}>{label}<span>{tasks.filter(task => matches(task, key)).length}</span></button>)}</div>
    {visible.length ? <div className="table-wrap"><table className="table" aria-label="逐拍照组状态"><thead><tr><th>拍照组</th><th>当前检测图</th><th>示教状态</th><th>标定验证</th><th>操作</th></tr></thead><tbody>
      {visible.map(task => <tr key={task.poseId}><td><strong>{task.poseId}</strong></td><td>{task.choice ? `${shots[task.choice.k].camera} · 图 ${task.choice.view}` : task.phase === "conflict" ? "选择冲突" : "未选定"}</td><td><Badge tone={task.tone}>{task.label}</Badge>{task.exception && <p className="wp-teach-row-hint">{task.message}</p>}</td>
        <td>{task.view ? calibrationText(task.view.calibration) : task.choice ? "状态待确认" : "选择检测图后验证"}</td><td><Link className="btn small" to={task.href} state={back} aria-label={`${task.poseId} · ${task.action}`}>{task.action}</Link></td></tr>)}
    </tbody></table></div> : <div className="wp-teach-empty" role="status">{filter === "unfinished" ? "所有拍照组已完成示教。" : "没有符合当前筛选的拍照组。"}</div>}
  </Panel></div>;
}
