import { Link } from "react-router-dom";
import { Select } from "qing-steel";
import { targetLinks } from "../readiness";
import type { RecipeOption } from "../types";

interface Props {
  options: RecipeOption[] | null;
  value: string;
  onChange: (id: string) => void;
  disabled?: boolean;
}

export default function RecipeRunSelect({ options, value, onChange, disabled }: Props) {
  const none = !!options && !options.some(o => o.runnable);
  const items = (options ?? []).map((r) => ({
    value: r.id,
    disabled: !r.runnable,
    label: `${r.id} · 代码 ${r.productCode} · N=${r.shotCount}${r.runnable ? (r.waiting ? `（${r.waiting}）` : "") : `（不可运行：${r.reason ?? "原因未知"}）`}`,
  }));
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
