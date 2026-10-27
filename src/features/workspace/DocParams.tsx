import { NumberField, Switch, TextField } from "qing-steel";
import type { RecipeDoc } from "../cycle/types";
import { useWorkspace } from "./context";

/**
 * 配方级参数的就地编辑（原“默认参数”页拆分到用到它们的页面，D-SB-44 后续）：改动交给工作台的候选自动保存（setDoc）。
 * `locked` 非空时只读并显示原因（例如本页还有未保存的其他改动，避免两份草稿互相覆盖）。
 */
function useDocField() {
  const { doc, data, busy, setDoc } = useWorkspace();
  const frozen = !!data?.workspace.pending;
  const set = <K extends keyof RecipeDoc>(key: K, value: RecipeDoc[K]) => { if (doc) setDoc({ ...doc, [key]: value }); };
  return { doc, set, disabled: busy || frozen };
}

/** 配方名称与 PLC 型号。工件总览页。 */
export function RecipeIdentity() {
  const { doc, set, disabled } = useDocField();
  if (!doc) return null;
  return <div className="wp-doc-identity">
    <TextField label="配方编号" className="mono" value={doc.id} readOnly disabled />
    <TextField label="名称" aria-label="配方名称" value={doc.name} disabled={disabled} onChange={e => set("name", e.target.value)} />
    <NumberField label="型号（PLC 下发）" aria-label="型号" step={1} min={1} max={32767} value={doc.productCode} disabled={disabled}
      onValueChange={value => set("productCode", value === "" ? NaN : value)} />
  </div>;
}

/** 标定轮数（10–100）。多轮标定页。 */
export function CalibrationRoundsField() {
  const { doc, set, disabled } = useDocField();
  if (!doc) return null;
  return <NumberField label="标定轮数" unit="轮" aria-label="标定轮数" step={1} min={10} max={100} value={doc.calibrationRounds ?? 10} disabled={disabled}
    onValueChange={value => set("calibrationRounds", value === "" ? NaN : value)} />;
}

/** 比例分段边界两侧的过渡帧数（0–5，D-SB-40）。段列表页。 */
export function BoundaryGuardField({ locked = "" }: { locked?: string }) {
  const { doc, set, disabled } = useDocField();
  if (!doc) return null;
  return <span title={locked || "按标定比例推算的段边界两侧各留几帧，只判每帧胶宽、断胶、溢胶，不判胶偏"}>
    <NumberField label="边界过渡帧数" unit="帧" aria-label="边界过渡帧数" step={1} min={0} max={5} value={doc.boundaryGuard ?? 2} disabled={disabled || !!locked}
      onValueChange={value => set("boundaryGuard", value === "" ? NaN : value)} />
  </span>;
}

/** 按逐帧定向的胶条方向序列细化比例边界（D-SB-47）。只有开启逐帧定向的段时可用。段列表页。 */
export function BoundaryRefineField({ locked = "", available }: { locked?: string; available: boolean }) {
  const { doc, set, disabled } = useDocField();
  if (!doc) return null;
  const reason = locked || (available ? "" : "没有开启逐帧定向的段，按比例边界分段");
  return <span title={reason || "在比例边界附近，按胶条方向开始变化与停止变化的帧细化段边界；比例分段仍作兜底"}>
    <Switch label="按胶条方向细化边界" aria-label="按胶条方向细化边界" checked={!!doc.boundaryRefine} disabled={disabled || !!reason}
      onCheckedChange={on => set("boundaryRefine", on)} />
  </span>;
}
