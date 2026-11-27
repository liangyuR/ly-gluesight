import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import type { PlcHandshakeState } from "../plc/types";
import type { Maintenance } from "./types";

export interface CycleControlState {
  productionBusy: boolean;
  plcLocked: boolean;
  plcHandshake: PlcHandshakeState | null;
  maintenance: Maintenance | null;
}

export function useCycleControl<T>(
  select: (state: CycleControlState | null) => T,
  initial: T,
  equal: (current: T, next: T) => boolean = Object.is,
) {
  const [value, setValue] = useState(() => initial);
  const projected = useRef(initial);
  useEffect(() => {
    let active = true, receivedEvent = false;
    let unlisten: UnlistenFn | undefined;
    const update = (state: CycleControlState | null) => {
      if (!active) return;
      const next = select(state);
      if (equal(projected.current, next)) return;
      projected.current = next;
      setValue(() => next);
    };

    async function start() {
      if (!isTauri()) {
        update(null);
        return;
      }
      try {
        const stop = await listen<CycleControlState>("cycle://control-state", event => {
          receivedEvent = true;
          update(event.payload);
        });
        if (!active) {
          stop();
          return;
        }
        unlisten = stop;
      } catch {
        update(null);
        return;
      }
      try {
        const state = await invoke<CycleControlState | null>("cycle_control_state");
        if (!receivedEvent) update(state);
      } catch {
        if (!receivedEvent) update(null);
      }
    }

    void start();
    return () => { active = false; unlisten?.(); };
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
