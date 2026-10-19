import { sampleLabelText } from "../../history/meta";
import type { SampleSetSummary } from "../types";

interface Props {
  sampleSet: SampleSetSummary;
  nextPart: string | null;
  value: string;
  onChange: (partId: string) => void;
  disabled?: boolean;
  label?: string;
}

export default function SamplePartSelect({ sampleSet, nextPart, value, onChange, disabled, label = "样本件" }: Props) {
  return (
    <select aria-label={label} className="input sample-part-select" value={value} onChange={(e) => onChange(e.target.value)} disabled={disabled}>
      <option value="">按顺序（下一件 {nextPart ?? "—"}）</option>
      {sampleSet.parts.map((p) => (
        <option key={p.partId} value={p.partId}>
          {p.partId} · {p.sn} · 期望 {sampleLabelText[p.label]}
        </option>
      ))}
    </select>
  );
}
