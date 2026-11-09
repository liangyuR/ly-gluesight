import { describe, expect, it } from "vitest";
import { ThumbPages } from "../src/features/workspace/thumbPages";
import type { ThumbStrip } from "../src/features/workspace/workbenchDraft";

const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
const strip = (value: number, count = 32): ThumbStrip => ({ width: 1, height: 1, channels: 1, frames: Array.from({ length: count }, () => Uint8Array.of(value)) });

describe("分段缩略图分页", () => {
  it("千帧工件快速切换 → 只读当前视口和邻页，旧页不显示且并发有界", async () => {
    const reads: { round: string; start: number; count: number; resolve: (value: ThumbStrip) => void }[] = [];
    let active = 0, peak = 0;
    const pages = new ThumbPages((round, _camera, start, count) => {
      active++; peak = Math.max(peak, active);
      return new Promise<ThumbStrip>(resolve => reads.push({ round, start, count, resolve: value => { active--; resolve(value); } }));
    });
    let oldNotifications = 0;
    const leave = pages.watch("old", ["a", "b", "c"], 1000, 0, 12, () => oldNotifications++);
    await flush();
    leave();
    const stop = pages.watch("new", ["a", "b", "c"], 1000, 640, 652, () => {});
    reads.slice().forEach(read => read.resolve(strip(1)));
    await flush();
    expect(pages.frame("old", "a", 0).status).toBe("loading");
    expect(oldNotifications).toBe(0);
    let completed = 2;
    while (completed < reads.length) {
      const batch = reads.slice(completed);
      completed = reads.length;
      batch.forEach(read => read.resolve(strip(9, read.count)));
      await flush();
    }
    expect(peak).toBe(2);
    expect(reads.filter(read => read.round === "old")).toHaveLength(2);
    expect(reads.filter(read => read.round === "new").map(read => read.start)).toEqual([641, 641, 641, 609, 609, 609, 673, 673, 673]);
    expect(reads.every(read => read.count <= 32)).toBe(true);
    expect(pages.frame("new", "a", 640).pixels?.[0]).toBe(9);
    expect(pages.frame("new", "a", 96).status).toBe("loading");
    stop();
  });

  it("跨工件浏览超过字节预算 → 淘汰旧页，重新打开时重新读取", async () => {
    let reads = 0;
    const pages = new ThumbPages(async () => strip(++reads), 90);
    const first = pages.watch("first", ["a"], 32, 0, 10, () => {});
    await flush();
    expect(pages.frame("first", "a", 0).pixels?.[0]).toBe(1);
    first();
    const second = pages.watch("second", ["a"], 32, 0, 10, () => {});
    await flush();
    expect(pages.frame("second", "a", 0).pixels?.[0]).toBe(2);
    expect(pages.frame("first", "a", 0).status).toBe("loading");
    second();
    const reopened = pages.watch("first", ["a"], 32, 0, 10, () => {});
    await flush();
    expect(pages.frame("first", "a", 0).pixels?.[0]).toBe(3);
    reopened();
  });

  it("邻页预取超过预算 → 保留当前可见页且不循环重读", async () => {
    const starts: number[] = [];
    const pages = new ThumbPages(async (_round, _camera, start) => { starts.push(start); return strip(start); }, 90);
    const stop = pages.watch("part", ["a"], 1000, 32, 40, () => {});
    await flush();
    expect(pages.frame("part", "a", 32).pixels?.[0]).toBe(33);
    expect(pages.frame("part", "a", 0).status).toBe("loading");
    expect(pages.frame("part", "a", 64).status).toBe("loading");
    expect(starts).toEqual([33, 1, 65]);
    stop();
  });

});
