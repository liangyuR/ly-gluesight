import { Select } from "qing-steel";
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
  const options = [
    { value: "", label: `按顺序（下一件 ${nextPart ?? "—"}）` },
    ...sampleSet.parts.map((p) => ({ value: p.partId, label: `${p.partId} · ${p.sn} · 期望 ${sampleLabelText[p.label]}` })),
  ];
  return <Select aria-label={label} className="sample-part-select" value={value} onValueChange={onChange} disabled={disabled} options={options} />;
}
