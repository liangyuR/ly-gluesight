import { NavLink, useLocation } from "react-router-dom";
import { LayoutDashboard, Lock } from "lucide-react";
import SelectionTrack from "../../components/SelectionTrack";
import { corridorTasks } from "./cameraTasks";
import { useWorkspace } from "./context";
import { stagesV2, stageStatusText, stageStatusV2 } from "./stages";

export function PlanSteps() {
  const { data } = useWorkspace();
  const { pathname } = useLocation();
  const tasks = data?.plan ? corridorTasks(data.plan) : [];
  // 走廊示教页：进行中时以“已示教 / 全部（段 × 检测相机）条”代替阶段状态
  const teaching = pathname === "/recipe/teach" && tasks.length ? { done: tasks.filter(task => task.status === "done").length, total: tasks.length } : null;
  return <SelectionTrack as="nav" className="wp-steps" trackId="workspace-steps" activeKey={pathname} aria-label="配方配置步骤">
    <NavLink to="/recipe/progress" end className={({ isActive }) => "wp-step-home" + (isActive ? " active" : "")}><span className="wp-step-no"><LayoutDashboard size={13} aria-hidden="true" /></span><span className="wp-step-label">总览</span></NavLink>
    {stagesV2.map((stage, i) => {
      const status = data ? stageStatusV2(data, stage) : null;
      return <NavLink key={stage.path} to={stage.path} end title={stage.optional ? stage.label + "（可选）" : undefined} className={({ isActive }) => (isActive ? "active" : "") + (stage.optional ? " optional" : "")}>
        <span className="wp-step-no">{i + 1}</span><span className="wp-step-label">{stage.label}</span>
        {status === "waiting"
          ? <small className="wp-step-status waiting"><Lock size={11} aria-hidden="true" />{stageStatusText.waiting}</small>
          : teaching && stage.path === pathname && (status === "inProgress" || status === "notStarted")
            ? <small className="wp-step-status wp-step-count">{teaching.done} / {teaching.total} 条</small>
            : status && status !== "optional" && <small className={"wp-step-status " + status}>{stageStatusText[status]}</small>}
      </NavLink>;
    })}
  </SelectionTrack>;
}
