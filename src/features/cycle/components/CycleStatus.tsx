import { useEffect, useMemo, useState } from "react";
import { plcApi, usePlcValues, type PlcConfig } from "../../plc";
import type { Phase, Snapshot } from "../types";

const steps: [Phase, string][] = [
  ["IDLE", "空闲"],
  ["VALIDATE", "校验"],
  ["ACQUIRE", "采集"],
  ["DRAIN", "收尾"],
  ["JUDGE", "判定"],
  ["REPORT", "回写"],
  ["RELEASE", "释放"],
];

export function CycleStepper({ snapshot }: { snapshot: Snapshot | null }) {
  const phase = snapshot?.phase ?? "IDLE";
  if (phase === "FAULT") {
    return <span className="step-fault">故障 · {snapshot?.fault}</span>;
  }
  const at = steps.findIndex(([p]) => p === phase);
  const err = snapshot?.result?.verdict === "ERR_INSPECT" && (!snapshot.part || snapshot.result.cycleId === snapshot.part.cycleId);
  return (
    <div className="stepper">
      {steps.map(([p, label], i) => (
        <span key={p} className={i < at ? "done" : i === at && phase !== "IDLE" ? `act${err && i >= 4 ? " err" : ""}` : ""}>
          {label}
        </span>
      ))}
    </div>
  );
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
