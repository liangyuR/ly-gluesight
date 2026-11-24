import { useEffect, useMemo, useState } from "react";
import { InspectionStages } from "qing-steel";
import { plcApi, usePlcValues, type PlcConfig } from "../../plc";
import type { Phase, Snapshot } from "../types";

const steps: { id: Phase; label: string }[] = [
  { id: "IDLE", label: "空闲" },
  { id: "VALIDATE", label: "接件" },
  { id: "ACQUIRE", label: "采集" },
  { id: "DRAIN", label: "收尾" },
  { id: "JUDGE", label: "判定" },
];

export function CycleStepper({ snapshot }: { snapshot: Snapshot | null }) {
  const phase = snapshot?.phase ?? "IDLE";
  const at = steps.findIndex(step => step.id === phase);
  const err = snapshot?.result?.verdict === "ERR_INSPECT" && (!snapshot.part || snapshot.result.cycleId === snapshot.part.cycleId);
  const fault = phase === "FAULT" ? { message: `故障 · ${snapshot?.fault ?? ""}` } : err && at >= 4 ? { stepId: phase, message: "本件未测成" } : null;
  return <InspectionStages steps={steps} currentStepId={phase === "IDLE" ? null : phase} completedStepIds={steps.slice(0, Math.max(0, at)).map(step => step.id)} fault={fault} />;
}

export function SignalLamps() {
  const [config, setConfig] = useState<PlcConfig | null>(null);
  const values = usePlcValues();
  useEffect(() => {
    plcApi.getConfig().then(setConfig).catch(() => setConfig(null));
  }, []);

  const byTag = useMemo(() => {
    const map = new Map<string, string>();
    config?.points.forEach((p) => p.tags.forEach((t) => map.set(t, p.id)));
    return map;
  }, [config]);
  const read = (tag: string) => {
    const id = byTag.get(tag);
    return id === undefined ? null : values[id]?.value ?? null;
  };

  const inspecting = read("inspecting");
  const partSeq = read("partSeq");
  const resultSeq = read("resultSeq");
  const matched = typeof resultSeq === "number" && resultSeq !== 0 && resultSeq === partSeq;
  const part = typeof partSeq === "number" && partSeq !== 0 ? ` · 件 ${partSeq}` : "";
  const lamps = [
    { tags: ["inspecting", "partSeq"], pc: false, lit: !!inspecting,
      text: inspecting === null ? "PLC：—" : `PLC：${inspecting ? "在检" : "未在检"}${part}` },
    { tags: ["resultSeq"], pc: true, lit: matched,
      text: typeof resultSeq !== "number" ? "PC：—" : matched ? "PC：本件结果已回写" : resultSeq === 0 ? "PC：无结果" : `PC：结果为件 ${resultSeq}` },
  ];

  return (
    <div className="lamps">
      {lamps.map(({ tags, pc, lit, text }) => {
        const bound = tags.every(tag => byTag.has(tag));
        return <span key={tags[0]} className={`lamp lamp-state${pc ? " pc" : ""}${lit ? " on" : ""}${bound ? "" : " unbound"}`} title={bound ? tags.join(" · ") : "地址表中未绑定该标签"}>
          {text}
        </span>;
      })}
    </div>
  );
}
