import { useEffect, useRef, useState } from "react";
import { subscribe } from "../plc";
import type { CameraConfig, CameraStatus, Frame } from "./types";

/** 一次 PLC 触发后各相机的帧在这个窗口内到齐算同一次触发 */
export const SYNC_WINDOW_MS = 500;

/** 参与核对的相机；excluded 非空时排除并写明原因 */
export interface SyncMember { cam: number; excluded: string }

/** 不参与核对的原因：未绑定、不在线、软触发或没有触发计数的相机不随 PLC 触发计数。 */
export function syncExclusion(config: CameraConfig, status: CameraStatus | null): string {
  const mvs = config.source === "mvs";
  if (mvs && !config.serial) return "未绑定";
  if (status?.released) return "已释放给 MVS";
  if (!status?.ready) return "未连接";
  if (mvs && config.triggerSource === "Software") return "软触发，不随 PLC 触发";
  if (mvs && !config.chunk) return "Chunk 关闭，没有触发计数";
  return "";
}

export type SyncResult = "sync" | "lag" | "jump" | "pending" | "excluded";

export interface SyncRow {
  cam: number;
  before: number | null;
  after: number | null;
  delta: number | null;
  result: SyncResult;
  reason: string;
}

export interface SyncState {
  base: Record<number, { session: number; counter: number }>;
  open: { start: number; frames: Record<number, Frame> } | null;
  last: { at: number; rows: SyncRow[]; ok: boolean | null } | null;
  triggers: number;
  mismatches: number;
  misses: Record<number, number>;
}

export const emptySync: SyncState = { base: {}, open: null, last: null, triggers: 0, mismatches: 0, misses: {} };

const realTrigger = (f: Frame) => f.counter === "chunkTrigger" || f.counter === "synthetic";

/** 收到一帧：窗口已过的上一组先结算，再把这帧并入当前组。 */
export function syncFrame(state: SyncState, frame: Frame, members: SyncMember[], now: number): SyncState {
  const settled = state.open && now - state.open.start >= SYNC_WINDOW_MS ? syncSettle(state, members, now) : state;
  const open = settled.open ?? { start: now, frames: {} };
  return { ...settled, open: { ...open, frames: { ...open.frames, [frame.cam]: frame } } };
}

/** 窗口到期时结算：参与的相机触发计数增量都相同算同步，没出图或增量不一致记为不同步。 */
export function syncSettle(state: SyncState, members: SyncMember[], now: number): SyncState {
  const group = state.open;
  if (!group || now - group.start < SYNC_WINDOW_MS) return state;
  const base = { ...state.base };
  const rows: SyncRow[] = members.map(m => {
    const f = group.frames[m.cam], b = state.base[m.cam];
    if (f) base[m.cam] = { session: f.session, counter: f.triggerCounter };
    if (m.excluded) return { cam: m.cam, before: null, after: null, delta: null, result: "excluded", reason: m.excluded };
    if (f && !realTrigger(f)) return { cam: m.cam, before: null, after: null, delta: null, result: "excluded", reason: "相机没有送出 Chunk 触发计数" };
    if (!f) return b ? { cam: m.cam, before: b.counter, after: b.counter, delta: 0, result: "sync", reason: "" } : { cam: m.cam, before: null, after: null, delta: null, result: "pending", reason: "等待第一帧建立基准" };
    if (!b || b.session !== f.session || f.triggerCounter < b.counter) return { cam: m.cam, before: null, after: f.triggerCounter, delta: null, result: "pending", reason: "相机重新连接，已重新建立基准" };
    return { cam: m.cam, before: b.counter, after: f.triggerCounter, delta: f.triggerCounter - b.counter, result: "sync", reason: "" };
  });
  const compared = rows.filter(r => r.delta != null);
  const deltas = compared.map(r => r.delta!);
  if (compared.length >= 2 && deltas.every(d => d === 0)) return { ...state, base, open: null };
  const ok = compared.length >= 2 ? deltas.every(d => d === deltas[0]) : null;
  const enough = members.filter(m => !m.excluded).length >= 2;
  if (ok === null) for (const r of compared) { r.result = "pending"; r.reason = enough ? "等其他相机建立基准" : "参与核对的相机不足 2 台"; }
  const least = Math.min(...deltas.filter(d => d > 0));
  const misses = { ...state.misses };
  if (ok === false) for (const r of compared) {
    if (r.delta === 0) { r.result = "lag"; r.reason = "这次触发没有出图"; }
    else if (r.delta! > least) { r.result = "jump"; r.reason = `计数跳了 ${r.delta}，期间有触发没有出图`; }
    if (r.result !== "sync") misses[r.cam] = (misses[r.cam] ?? 0) + 1;
  }
  return {
    base,
    open: null,
    last: { at: group.start, rows, ok },
    triggers: state.triggers + (ok === null ? 0 : 1),
    mismatches: state.mismatches + (ok === false ? 1 : 0),
    misses,
  };
}

/** 打开本页起逐帧核对 PLC 一次触发后各相机触发计数是否同步加一。 */
export function useSyncCheck(members: SyncMember[]) {
  const [state, setState] = useState<SyncState>(emptySync);
  const current = useRef({ state, members });
  current.current.members = members;
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined, timerFor = -1;
    const off = subscribe<Frame>("camera://frame", f => {
      const next = syncFrame(current.current.state, f, current.current.members, Date.now());
      const settled = next.last !== current.current.state.last;
      current.current.state = next;
      if (settled) setState(next);
      const start = next.open!.start;
      if (timerFor === start) return;
      clearTimeout(timer);
      timerFor = start;
      timer = setTimeout(() => {
        current.current.state = syncSettle(current.current.state, current.current.members, Date.now());
        setState(current.current.state);
      }, Math.max(0, start + SYNC_WINDOW_MS - Date.now()));
    });
    return () => { clearTimeout(timer); off(); };
  }, []);
  return state;
}
