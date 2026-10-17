import { describe, expect, it } from "vitest";
import { defaultCameraConfig } from "../src/features/camera/api";
import { emptySync, syncExclusion, syncFrame, syncSettle, type SyncMember, type SyncState } from "../src/features/camera/sync";
import type { CameraConfig, CameraStatus, CounterSource, Frame } from "../src/features/camera/types";

const frame = (cam: number, triggerCounter: number, over: { session?: number; counter?: CounterSource } = {}): Frame =>
  ({ cam, session: over.session ?? 1, counter: over.counter ?? "chunkTrigger", frameCounter: triggerCounter, triggerCounter, lostPackets: 0, ts: 0 });
const members = (...excluded: string[]): SyncMember[] => excluded.map((reason, cam) => ({ cam, excluded: reason }));

/** 一次触发：这些帧在窗口开头到达，窗口过后结算。 */
function trigger(state: SyncState, at: number, frames: Frame[], who: SyncMember[]) {
  for (const f of frames) state = syncFrame(state, f, who, at);
  return syncSettle(state, who, at + 500);
}
/** 先来一次全部出图的触发建立基准。 */
const baseline = (who: SyncMember[], counters: number[]) => trigger(emptySync, 0, counters.map((c, cam) => frame(cam, c)), who);

describe("同步触发核对", () => {
  it("两台相机同一次触发各加一 → 判同步，计一次触发、零次不同步", () => {
    const who = members("", "");
    const state = trigger(baseline(who, [10, 20]), 1000, [frame(0, 11), frame(1, 21)], who);
    expect(state.last?.ok).toBe(true);
    expect(state.last?.rows.map(r => r.delta)).toEqual([1, 1]);
    expect([state.triggers, state.mismatches]).toEqual([1, 0]);
  });

  it.each([
    { name: "一台这次没出图", counters: [11, null], result: "lag" },
    { name: "一台计数跳了 2", counters: [11, 22], result: "jump" },
  ])("$name → 判不同步，记在这台相机上", ({ counters, result }) => {
    const who = members("", "");
    const frames = counters.flatMap((c, cam) => c == null ? [] : [frame(cam, c)]);
    const state = trigger(baseline(who, [10, 20]), 1000, frames, who);
    expect(state.last?.ok).toBe(false);
    expect(state.last?.rows[1].result).toBe(result);
    expect(state.mismatches).toBe(1);
    expect(state.misses).toEqual({ 1: 1 });
  });

  it("未绑定的相机没出图 → 排除在核对外，不判不同步", () => {
    const who = members("", "", "未绑定");
    const state = trigger(baseline(who, [10, 20]), 1000, [frame(0, 11), frame(1, 21)], who);
    expect(state.last?.ok).toBe(true);
    expect(state.last?.rows[2]).toMatchObject({ result: "excluded", reason: "未绑定" });
  });

  it("相机重连后会话变了 → 重新建立基准，不判落后", () => {
    const who = members("", "", "");
    const state = trigger(baseline(who, [10, 20, 30]), 1000, [frame(0, 11), frame(1, 21), frame(2, 35, { session: 2 })], who);
    expect(state.last?.ok).toBe(true);
    expect(state.last?.rows[2].result).toBe("pending");
    expect(state.mismatches).toBe(0);
  });

  it("帧里没有 Chunk 触发计数 → 这台相机排除，不拿帧计数冒充触发计数", () => {
    const who = members("", "", "");
    const state = trigger(baseline(who, [10, 20, 30]), 1000, [frame(0, 11), frame(1, 21), frame(2, 35, { counter: "sdkFrame" })], who);
    expect(state.last?.ok).toBe(true);
    expect(state.last?.rows[2].result).toBe("excluded");
  });

  it("两次触发落在同一窗口、各相机都加二 → 仍判同步", () => {
    const who = members("", "");
    let state = baseline(who, [10, 20]);
    for (const f of [frame(0, 11), frame(1, 21), frame(0, 12), frame(1, 22)]) state = syncFrame(state, f, who, 1000);
    state = syncSettle(state, who, 1500);
    expect(state.last?.ok).toBe(true);
    expect(state.last?.rows.map(r => r.delta)).toEqual([2, 2]);
  });
});

describe("同步核对的参与相机", () => {
  const ready: CameraStatus = { cam: 0, id: "cam1", name: "相机 1", source: "mvs", ready: true, released: false, message: "", device: null, sdkVersion: null, frames: 1, fps: 0, maxFps: null, lostPackets: 0, droppedFrames: 0, warnings: [] };
  const bound: CameraConfig = { ...defaultCameraConfig, id: "cam1", serial: "SN1" };
  it.each([
    { name: "未绑定设备", config: { serial: "" }, status: {}, reason: "未绑定" },
    { name: "已释放给 MVS", config: {}, status: { ready: false, released: true }, reason: "已释放给 MVS" },
    { name: "未连接", config: {}, status: { ready: false }, reason: "未连接" },
    { name: "软触发", config: { triggerSource: "Software" as const }, status: {}, reason: "软触发，不随 PLC 触发" },
    { name: "Chunk 关闭", config: { chunk: false }, status: {}, reason: "Chunk 关闭，没有触发计数" },
  ])("$name → 排除在核对外", ({ config, status, reason }) => {
    expect(syncExclusion({ ...bound, ...config }, { ...ready, ...status })).toBe(reason);
  });

  it("已绑定、在线、Line0 且开 Chunk → 参与核对", () => {
    expect(syncExclusion(bound, ready)).toBe("");
  });
});
