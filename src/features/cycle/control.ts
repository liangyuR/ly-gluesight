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

type ControlListener = (state: CycleControlState | null) => void;
const controlListeners = new Set<ControlListener>();
let controlState: CycleControlState | null | undefined;
let connection: ReturnType<typeof createControlConnection> | undefined;

function publish(state: CycleControlState | null) {
  controlState = state;
  controlListeners.forEach(listener => listener(state));
}

function createControlConnection() {
  let active = true, pending = false, resolved = false;
  let unlisten: UnlistenFn | undefined;
  const accept = (state: CycleControlState | null) => {
    if (!active) return;
    resolved = true;
    publish(state);
  };

  const start = async () => {
    if (!active || pending || resolved) return;
    if (!isTauri()) { accept(null); return; }
    pending = true;
    let acceptEvents = true;
    try {
      if (!unlisten) {
        const stop = await listen<CycleControlState>("cycle://control-state", event => {
          if (acceptEvents) accept(event.payload);
        });
        if (!active) { stop(); return; }
        unlisten = stop;
      }
      if (!resolved) {
        const state = await invoke<CycleControlState | null>("cycle_control_state");
        if (!resolved) accept(state);
      }
    } catch {
      if (!unlisten) { acceptEvents = false; resolved = false; }
      if (active && !resolved) publish(null);
    } finally {
      pending = false;
    }
  };
  const dispose = () => { active = false; unlisten?.(); };
  return { start, dispose };
}

function subscribeControl(listener: ControlListener) {
  controlListeners.add(listener);
  if (controlState !== undefined) listener(controlState);
  connection ??= createControlConnection();
  void connection.start();
  return () => {
    if (!controlListeners.delete(listener) || controlListeners.size) return;
    connection?.dispose();
    connection = undefined;
    controlState = undefined;
  };
}

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
