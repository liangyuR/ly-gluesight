import { Link } from "react-router-dom";
import { Select } from "qing-steel";
import { planScaleText } from "../../plan/scale";
import { targetLinks } from "../readiness";
import type { RecipeOption, RecipeSummary } from "../types";

interface Props {
  options: RecipeOption[] | null;
  /** 配方库摘要（cycle_recipes）：段数与 ΣN_exp 取自这里 */
  summaries?: RecipeSummary[];
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
}

/** 段数与 ΣN_exp ± Σtol（待标定显示“待标定”）；摘要还没取到时不显示规模。 */
function scaleText(summary: RecipeSummary | undefined): string {
  return summary?.segmentCount ? planScaleText({ segmentCount: summary.segmentCount, expectedTotal: summary.expectedTotal, toleranceTotal: summary.toleranceTotal }) : "";
}

export default function RecipeRunSelect({ options, summaries, value, onChange, disabled }: Props) {
  const none = !!options && !options.some(o => o.runnable);
  const items = (options ?? []).map((r) => {
    const scale = scaleText(summaries?.find(s => s.id === r.id));
    return {
      value: r.id,
      disabled: !r.runnable,
      label: `${r.id} · 型号 ${r.productCode}${scale && ` · ${scale}`}${r.runnable ? (r.waiting ? `（${r.waiting}）` : "") : `（不可运行：${r.reason ?? "原因未知"}）`}`,
    };
  });
  return (
    <>
      <Select aria-label="离线配方" value={value} onValueChange={onChange} disabled={disabled || !options || none}
        placeholder={options ? "没有可运行配方" : "正在检查配方…"} options={items} />
      {none && (
        <span className="offline-guide" role="note">
          {options.length ? "尚无可在当前样本集 / 相机组运行的生产配方" : "尚无可开工的生产配方"}：新建并发布配方后再运行；整圈采集不受影响。
          <Link to={targetLinks.workspace.path}>{targetLinks.workspace.label}</Link>
        </span>
      )}
    </>
  );
}
