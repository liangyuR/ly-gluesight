import { useEffect, useMemo, useState } from "react";
import { InspectionStages } from "qing-steel";
import { plcApi, usePlcValues, type PlcConfig } from "../../plc";
import type { Phase, Snapshot } from "../types";

const steps: { id: Phase; label: string }[] = [
  { id: "IDLE", label: "空闲" },
  { id: "VALIDATE", label: "校验" },
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

const plcToPc = ["partStart", "partEnd", "resultAck"];
const pcToPlc = ["visionReady", "armed", "busy", "done"];

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

  const lamp = (tag: string, pc: boolean) => {
    const id = byTag.get(tag);
    const on = id !== undefined && !!values[id]?.value;
    return (
      <span key={tag} className={`lamp${pc ? " pc" : ""}${on ? " on" : ""}${id ? "" : " unbound"}`} title={id ? `点位 ${id}` : "地址表中未绑定该标签"}>
        {tag}
      </span>
    );
  };

  return (
    <div className="lamps">
      {plcToPc.map((t) => lamp(t, false))}
      <span className="lamp-sep">|</span>
      {pcToPlc.map((t) => lamp(t, true))}
    </div>
  );
}
