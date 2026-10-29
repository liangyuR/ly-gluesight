import { Link, useLocation } from "react-router-dom";
import { LayoutDashboard, Lock } from "lucide-react";
import { Steps, type StepItem } from "qing-steel";
import { corridorTasks } from "./cameraTasks";
import { useWorkspace } from "./context";
import { stagesV2, stageStatusText, stageStatusTone, stageStatusV2 } from "./stages";

export function PlanSteps() {
  const { data } = useWorkspace();
  const { pathname } = useLocation();
  const tasks = data ? corridorTasks(data.plan) : [];
  const teaching = pathname === "/recipe/teach" && tasks.length ? { done: tasks.filter(task => task.status === "done").length, total: tasks.length } : null;
  const items: StepItem[] = [
    { value: "/recipe/progress", href: "/recipe/progress", label: "总览", number: <LayoutDashboard size={13} aria-hidden="true" /> },
    ...stagesV2.map((stage, i): StepItem => {
      const status = data ? stageStatusV2(data, stage) : null;
      return {
        value: stage.path,
        href: stage.path,
        label: stage.label,
        number: i + 1,
        optional: stage.optional,
        tone: status ? stageStatusTone[status] : "neutral",
        status: status === "waiting"
          ? <><Lock size={11} aria-hidden="true" />{stageStatusText.waiting}</>
          : teaching && stage.path === pathname && (status === "inProgress" || status === "notStarted")
            ? `${teaching.done} / ${teaching.total} 条`
            : status && stageStatusText[status],
      };
    }),
  ];
  return <Steps label="配方配置步骤" currentValue={pathname} items={items}
    renderLink={({ href, ...props }) => <Link {...props} to={href} />} />;
}
