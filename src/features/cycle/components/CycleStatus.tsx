import { useEffect, useMemo, useState } from "react";
import { InspectionStages } from "qing-steel";
import { plcApi, usePlcValues, type PlcConfig } from "../../plc";
import { codeName, pcStateNames, plcStateNames } from "../../plc/handshake";
import type { Phase, Snapshot } from "../types";

const steps: { id: Phase; label: string }[] = [
  { id: "IDLE", label: "空闲" },
  { id: "VALIDATE", label: "接件" },
  { id: "ACQUIRE", label: "采集" },
  { id: "DRAIN", label: "收尾" },
  { id: "JUDGE", label: "判定" },
  { id: "REPORT", label: "回写" },
  { id: "RELEASE", label: "释放" },
];

export function CycleStepper({ snapshot }: { snapshot: Snapshot | null }) {
  const phase = snapshot?.phase ?? "IDLE";
  const at = steps.findIndex(step => step.id === phase);
  const err = snapshot?.result?.verdict === "ERR_INSPECT" && (!snapshot.part || snapshot.result.cycleId === snapshot.part.cycleId);
  const fault = phase === "FAULT" ? { message: `故障 · ${snapshot?.fault ?? ""}` } : err && at >= 4 ? { stepId: phase, message: "本件未测成" } : null;
  return <InspectionStages steps={steps} currentStepId={phase === "IDLE" ? null : phase} completedStepIds={steps.slice(0, Math.max(0, at)).map(step => step.id)} fault={fault} />;
}

const sides = [
  { tag: "plcState", side: "PLC", names: plcStateNames, pc: false },
  { tag: "pcState", side: "PC", names: pcStateNames, pc: true },
];

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

  return (
    <div className="lamps">
      {sides.map(({ tag, side, names, pc }) => {
        const id = byTag.get(tag);
        const value = id === undefined ? null : values[id]?.value;
        const state = typeof value === "number" ? codeName(names, value) : "—";
        const lit = typeof value === "number" && value !== 0;
        return <span key={tag} className={`lamp${pc ? " pc" : ""}${value === 90 ? " fault" : lit ? " on" : ""}${id ? "" : " unbound"}`} title={id ? `点位 ${id} · ${tag}` : "地址表中未绑定该标签"}>
          {side}：{state}
        </span>;
      })}
    </div>
  );
}
