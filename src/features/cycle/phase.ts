import type { Phase, Snapshot } from "./types";

export const phaseOccupied = (phase: Phase | string) => phase !== "IDLE" && phase !== "FAULT";
export const productionBusy = (snapshot: Snapshot | null | undefined) => !!snapshot && phaseOccupied(snapshot.phase);
