import { NumberField, TextField } from "qing-steel";
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

/** 比例分段边界两侧的过渡帧数（0–5，D-SB-40）。段列表页。 */
export function BoundaryGuardField({ locked = "" }: { locked?: string }) {
  const { doc, set, disabled } = useDocField();
  if (!doc) return null;
  return <span title={locked || "按标定比例推算的段边界两侧各留几帧，只判每帧胶宽、断胶、溢胶，不判胶偏"}>
    <NumberField label="边界过渡帧数" unit="帧" aria-label="边界过渡帧数" step={1} min={0} max={5} value={doc.boundaryGuard ?? 2} disabled={disabled || !!locked}
      onValueChange={value => set("boundaryGuard", value === "" ? NaN : value)} />
  </span>;
}

/** 每笔开头、结尾跳过的帧组数（0–50，D-SB-62）；都为 0 时不写入配方。段列表页。 */
export function PassSkipFields({ locked = "" }: { locked?: string }) {
  const { doc, set, disabled } = useDocField();
  const passes = useWorkspace().data?.workspace.teachingPiece?.passes ?? doc?.passFrames;
  if (!doc) return null;
  const skip = doc.passSkip ?? { head: 0, tail: 0 };
  const change = (end: "head" | "tail", value: number | "") => {
    const next = { ...skip, [end]: value === "" ? NaN : value };
    set("passSkip", next.head === 0 && next.tail === 0 ? undefined : next);
  };
  const hint = "机械臂每次起涂打开、收胶关闭拍照；每笔开头胶条还没铺满，这几帧不判；整笔帧数不超过首尾之和的笔整笔不检";
  return <span className="wp-pass-skip" title={locked || hint}>
    <NumberField label="每笔开头跳过" unit="帧组" aria-label="每笔开头跳过" step={1} min={0} max={50} value={Number.isFinite(skip.head) ? skip.head : ""} disabled={disabled || !!locked}
      onValueChange={value => change("head", value)} />
    <NumberField label="每笔结尾跳过" unit="帧组" aria-label="每笔结尾跳过" step={1} min={0} max={50} value={Number.isFinite(skip.tail) ? skip.tail : ""} disabled={disabled || !!locked}
      onValueChange={value => change("tail", value)} />
    {!!passes?.length && <span className="muted" aria-label="示教件笔数">示教件 {passes.length} 笔</span>}
  </span>;
}
