import { Link, useLocation } from "react-router-dom";
import { Check, LayoutDashboard, Lock } from "lucide-react";
import { Steps, type StepItem } from "qing-steel";
import { corridorTasks } from "./cameraTasks";
import { useWorkspace } from "./context";
import { stageOptionalV2, stagesV2, stageStatusText, stageStatusTone, stageStatusV2 } from "./stages";
import { stepPathOf, useRecipeHref } from "./recipeRoutes";

export function PlanSteps() {
  const { data } = useWorkspace();
  const pathname = stepPathOf(useLocation().pathname);
  const href = useRecipeHref();
  const tasks = data ? corridorTasks(data.plan) : [];
  const teaching = pathname === "/recipe/teach" && tasks.length ? { done: tasks.filter(task => task.status === "done").length, total: tasks.length } : null;
  const items: StepItem[] = [
    { value: "/recipe/progress", href: href("/recipe/progress"), label: "总览", number: <LayoutDashboard size={13} aria-hidden="true" /> },
    ...stagesV2.map((stage, i): StepItem => {
      const status = data ? stageStatusV2(data, stage) : null;
      // 完成的步骤不再挂“完成”徽标，编号圈换成勾，省出一行的宽度
      const done = status === "done";
      return {
        value: stage.path,
        href: href(stage.path),
        label: done ? <>{stage.label}<span className="qs-sr-only">（{stageStatusText.done}）</span></> : stage.label,
        number: done ? <span className="wp-step-done" title={stageStatusText.done}><Check size={13} aria-hidden="true" /></span> : i + 1,
        optional: stageOptionalV2(data, stage),
        tone: status ? stageStatusTone[status] : "neutral",
        status: status === "waiting"
          ? <><Lock size={11} aria-hidden="true" />{stageStatusText.waiting}</>
          : teaching && stage.path === pathname && (status === "inProgress" || status === "notStarted")
            ? `${teaching.done} / ${teaching.total} 条`
            : status && !done ? stageStatusText[status] : undefined,
      };
    }),
  ];
  return <Steps label="配方配置步骤" currentValue={pathname} items={items}
    renderLink={({ href, ...props }) => <Link {...props} to={href} />} />;
}
