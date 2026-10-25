import { Link } from "react-router-dom";
import { Select } from "qing-steel";
import { planScaleText } from "../../plan/scale";
import { targetLinks } from "../readiness";
import type { RecipeOption, RecipeSummary } from "../types";

interface Props {
  options: RecipeOption[] | null;
  /** 配方库摘要（cycle_recipes）：段配方的段数与 ΣN_exp 取自这里；开工判定的 RecipeOption 只带 shotCount，段配方恒为 0 */
  summaries?: RecipeSummary[];
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
}

/** 段配方显示段数与 ΣN_exp ± Σtol（待标定显示“待标定”），旧拍照点配方显示拍照点数；段配方的摘要还没取到时（shotCount 为 0）不显示规模，不写 N=0。 */
function scaleText(option: RecipeOption, summary: RecipeSummary | undefined): string {
  if (summary?.segmentCount) return planScaleText({ segmentCount: summary.segmentCount, expectedTotal: summary.expectedTotal, toleranceTotal: summary.toleranceTotal });
  return option.shotCount > 0 ? `N=${option.shotCount}` : "";
}

export default function RecipeRunSelect({ options, summaries, value, onChange, disabled }: Props) {
  const none = !!options && !options.some(o => o.runnable);
  const items = (options ?? []).map((r) => {
    const scale = scaleText(r, summaries?.find(s => s.id === r.id));
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
