import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { createSharedSnapshot } from "../../lib/desktopSnapshot";
import type { PlcHandshakeState } from "../plc/types";
import type { Maintenance } from "./types";

export interface CycleControlState {
  productionBusy: boolean;
  plcLocked: boolean;
  plcHandshake: PlcHandshakeState | null;
  maintenance: Maintenance | null;
}

const subscribeControl = createSharedSnapshot<CycleControlState>("cycle://control-state", () =>
  isTauri() ? invoke<CycleControlState | null>("cycle_control_state") : Promise.resolve(null),
);

export function useCycleControl<T>(
  select: (state: CycleControlState | null) => T,
  initial: T,
  equal: (current: T, next: T) => boolean = Object.is,
) {
  const [value, setValue] = useState(() => initial);
  const projected = useRef(initial);
  useEffect(() => {
    return subscribeControl(state => {
      const next = select(state);
      if (equal(projected.current, next)) return;
      projected.current = next;
      setValue(() => next);
    });
  }, [equal, select]);
  return value;
}

const selectBusy = (state: CycleControlState | null) => state?.productionBusy === true;
const selectMaintenance = (state: CycleControlState | null) => state?.maintenance ?? null;
const sameMaintenance = (a: Maintenance | null, b: Maintenance | null) => a?.since === b?.since && a?.reason === b?.reason;

export function useProductionBusy() {
  return useCycleControl(selectBusy, false);
}

export function useMaintenance() {
  return useCycleControl(selectMaintenance, null, sameMaintenance);
}
