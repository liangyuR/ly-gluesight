import { Link, useLocation } from "react-router-dom";
import { AlertTriangle, ArrowRight, Lock } from "lucide-react";
import { Progress } from "qing-steel";
import { cameraTeachingTasks } from "./cameraTasks";
import { useWorkspace } from "./context";
import { Badge, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { stepOfV2, stepStatusText, stepStatusTone, targetPathV2 } from "./progress";
import { returnState } from "./returnNavigation";
import { stageOfV2, stagesV2, stageStatusText, stageStatusTone, stageStatusV2 } from "./stages";

const actions: Record<string, string> = {
  "/recipe/capture": "查看采集", "/recipe/segments": "编辑段列表", "/recipe/geometry": "配置参数", "/recipe/teach": "查看走廊示教",
  "/recipe/calibration": "查看多轮标定", "/recipe/validation": "查看验证与发布",
};

/** 配方进度（段配方）：采集 → 段列表 → 走廊示教 → 多轮标定 → 验证与发布。 */
export default function PlanProgressPage() {
  const { data } = useWorkspace();
  const location = useLocation();
  if (!data) return <WorkspaceEmpty />;
  const plan = data.plan;
  const { progress } = plan;
  const tasks = cameraTeachingTasks(plan);
  const taught = tasks.filter(task => task.status === "done").length;
  const nextTask = tasks.find(task => task.status !== "done");
  const next = progress.next;
  const nextStep = next && stepOfV2(progress, next.step);
  const warning = nextStep?.status === "blocked";
  const reasons = [...new Set(nextStep?.blockers.map(blocker => blocker.message) ?? [])].filter(message => message !== nextStep?.detail).slice(0, 3);
  const back = returnState(location, "进度总览");
  return <div className="wp-page"><WorkspaceBar /><Panel title="配方进度" className="wp-progress-board">
    {next && <section className={"wp-progress-next " + (warning ? "warn" : "info")} aria-label="下一步">
      {warning ? <AlertTriangle size={22} aria-hidden="true" /> : <ArrowRight size={22} aria-hidden="true" />}
      <div className="wp-progress-next-body">
        <small>下一步{nextStep && " · " + (stageOfV2(nextStep.id)?.label ?? nextStep.label)}</small>
        <strong>{nextStep?.detail || "继续完成当前候选配置"}</strong>
        {reasons.length > 0 && <ul>{reasons.map(reason => <li key={reason}>{reason}</li>)}</ul>}
      </div>
      <Link className="qs-btn" to={targetPathV2(next.target)} state={back}>{next.label}<ArrowRight size={15} aria-hidden="true" /></Link>
    </section>}
    <div className="wp-progress-list">{stagesV2.map((stage, index) => {
      const status = stageStatusV2(data, stage);
      const waiting = status === "waiting";
      const main = stepOfV2(progress, stage.ids[0]);
      const subs = stage.ids.slice(1).flatMap(id => stepOfV2(progress, id) ?? []);
      const isCorridor = stage.ids.includes("corridor");
      const isCalibration = stage.ids.includes("calibration");
      const link = isCorridor && nextTask ? { href: nextTask.href, label: "继续示教" } : { href: stage.path, label: actions[stage.path] };
      return <section key={stage.path} className={"wp-progress-row" + (waiting ? " waiting" : "")} aria-label={stage.label + "进度"}>
        <span className={"wp-progress-number" + (stage.optional ? " optional" : "")} aria-hidden="true">{index + 1}</span>
        <div className="wp-progress-row-body">
          <h3>{stage.label}{stage.optional && <small>可选</small>}</h3>
          <p>{waiting ? "整圈采集完成后可继续" : main?.detail ? main.detail : stage.hint}</p>
          {!waiting && subs.map(step => <p key={step.id} className="wp-progress-sub"><span>{step.id === "publish" ? "发布" : step.label}</span><Badge tone={stepStatusTone[step.status]}>{stepStatusText[step.status]}</Badge>{step.detail}</p>)}
        </div>
        <div className="wp-progress-meter">
          {isCorridor && <>
            <Progress label="走廊示教进度" value={taught} max={Math.max(1, tasks.length)} />
            <output aria-label="已示教相机数">{taught} / {tasks.length} 台已示教</output>
          </>}
          {isCalibration && !waiting && plan.calibration && <output aria-label="标定轮数">{plan.calibration.rounds.length} / {plan.calibration.minRounds} 轮</output>}
        </div>
        <output className="wp-progress-state" aria-label={stage.label + "状态"}>{waiting
          ? <Badge tone="neutral"><Lock size={12} aria-hidden="true" />{stageStatusText.waiting}</Badge>
          : status && <Badge tone={stageStatusTone[status]}>{stageStatusText[status]}</Badge>}</output>
        <Link className="wp-progress-link" to={link.href} state={back}>{link.label}<ArrowRight size={14} aria-hidden="true" /></Link>
      </section>;
    })}</div>
  </Panel></div>;
}
