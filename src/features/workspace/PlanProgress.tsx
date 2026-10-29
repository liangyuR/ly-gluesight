import { Link, useLocation } from "react-router-dom";
import { AlertTriangle, ArrowRight, Lock } from "lucide-react";
import { Button, Progress } from "qing-steel";
import { corridorTasks, nextTeachingHref } from "./cameraTasks";
import { useWorkspace } from "./context";
import { RecipeIdentity } from "./DocParams";
import { Badge, Panel, WorkspaceBar, WorkspaceEmpty } from "./components";
import { stepOfV2, stepStatusText, stepStatusTone, targetPathV2 } from "./progress";
import { pieceLabel } from "./pieces";
import { returnState } from "./returnNavigation";
import { stageOfV2, stagesV2, stageStatusText, stageStatusTone, stageStatusV2 } from "./stages";
import "./segments.css";

const actions: Record<string, string> = {
  "/recipe/capture": "查看示教件", "/recipe/segments": "编辑段列表", "/recipe/teach": "查看走廊示教",
  "/recipe/calibration": "查看多轮标定", "/recipe/validation": "查看发布",
};

/** 配方进度（段配方）：示教件 → 段列表 → 走廊示教 → 多轮标定 → 发布（独立样本验证可选）。 */
export default function PlanProgressPage() {
  const { data } = useWorkspace();
  const location = useLocation();
  if (!data) return <WorkspaceEmpty />;
  const plan = data.plan;
  const { progress } = plan;
  const tasks = corridorTasks(plan);
  const taught = tasks.filter(task => task.status === "done").length;
  const nextTeaching = nextTeachingHref(plan);
  const calibrationPieces = plan.pieces.filter(piece => piece.inCalibration && piece.role !== "standard").length;
  const teachingPiece = data.workspace.teachingPiece;
  const next = progress.next;
  const nextStep = next && stepOfV2(progress, next.step);
  const warning = nextStep?.status === "blocked";
  const reasons = [...new Set(nextStep?.blockers.map(blocker => blocker.message) ?? [])].filter(message => message !== nextStep?.detail);
  const back = returnState(location, "进度总览");
  return <div className="wp-page"><WorkspaceBar />
    <Panel title="配方信息">
      <RecipeIdentity />
      <p className="muted" aria-label="当前示教件">示教件：{teachingPiece ? pieceLabel(teachingPiece) : "未选"}</p>
    </Panel>
    <Panel title="配方进度" className="wp-progress-board">
    {next && <section className={"wp-progress-next " + (warning ? "warn" : "info")} aria-label="下一步">
      {warning ? <AlertTriangle size={22} aria-hidden="true" /> : <ArrowRight size={22} aria-hidden="true" />}
      <div className="wp-progress-next-body">
        <small>下一步{nextStep && " · " + (stageOfV2(nextStep.id)?.label ?? nextStep.label)}</small>
        <strong>{nextStep?.detail || "继续完成当前候选配置"}</strong>
        {reasons.length > 0 && <p>{reasons[0]}{reasons.length > 1 && `（另有 ${reasons.length - 1} 项）`}</p>}
      </div>
      <Button asChild><Link to={targetPathV2(next.target)} state={back}>{next.label}<ArrowRight size={15} aria-hidden="true" /></Link></Button>
    </section>}
    <div className="wp-progress-list">{stagesV2.map((stage, index) => {
      const status = stageStatusV2(data, stage);
      const waiting = status === "waiting";
      const main = stepOfV2(progress, stage.ids[0]);
      const subs = stage.ids.slice(1).flatMap(id => stepOfV2(progress, id) ?? []);
      const isCorridor = stage.ids.includes("corridor");
      const isCalibration = stage.ids.includes("calibration");
      const link = isCorridor && nextTeaching ? { href: nextTeaching, label: "继续示教" } : { href: stage.path, label: actions[stage.path] };
      return <section key={stage.path} className={"wp-progress-row" + (waiting ? " waiting" : "")} aria-label={stage.label + "进度"}>
        <span className={"wp-progress-number" + (stage.optional ? " optional" : "")} aria-hidden="true">{index + 1}</span>
        <div className="wp-progress-row-body">
          <h3>{stage.label}{stage.optional && <small>可选</small>}</h3>
          <p>{waiting ? "选好示教件后可继续" : isCorridor ? "为各段检测相机示教走廊" : isCalibration ? "应用多轮采集结果并确认容差" : main?.detail || stage.hint}</p>
          {!waiting && subs.map(step => <p key={step.id} className="wp-progress-sub"><span>{step.id === "publish" ? "发布" : step.label}</span><Badge tone={stepStatusTone[step.status]}>{stepStatusText[step.status]}</Badge>{step.detail}</p>)}
        </div>
        <div className="wp-progress-meter">
          {isCorridor && <>
            <Progress label="走廊示教进度" value={taught} max={Math.max(1, tasks.length)} />
            <output aria-label="已示教走廊数">{taught} / {tasks.length} 条已示教</output>
          </>}
          {isCalibration && !waiting && plan.calibration && <output aria-label="标定件数">{calibrationPieces} / {plan.calibration.minRounds} 件（至少）</output>}
        </div>
        <output className="wp-progress-state" aria-label={stage.label + "状态"}>{waiting
          ? <Badge tone="neutral"><Lock size={12} aria-hidden="true" />{stageStatusText.waiting}</Badge>
          : status && <Badge tone={stageStatusTone[status]}>{isCalibration && status === "inProgress" && plan.calibration && calibrationPieces >= plan.calibration.minRounds ? "待完成标定" : stageStatusText[status]}</Badge>}</output>
        <Link className="wp-progress-link" to={link.href} state={back}>{link.label}<ArrowRight size={14} aria-hidden="true" /></Link>
      </section>;
    })}</div>
  </Panel></div>;
}
