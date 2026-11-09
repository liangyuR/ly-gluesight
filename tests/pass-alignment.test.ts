import { describe, expect, it } from "vitest";
import { foldRows, link, linkable, linksOf, unlink } from "../src/features/workspace/passAlignment";
import type { PassPair } from "../src/features/workspace/types";

const pair = (old: number, next: number, change: "same" | "changed" = "same"): PassPair => ({ old, new: next, change });
const removed = (old: number): PassPair => ({ old, change: "removed" });
const added = (next: number): PassPair => ({ new: next, change: "added" });

/** 旧 5 笔、新 5 笔：0↔0、2↔1、4↔4 对上；旧 1、3 删除，新 2、3 新增。 */
const rows: PassPair[] = [pair(0, 0), removed(1), pair(2, 1, "changed"), removed(3), added(2), added(3), pair(4, 4)];

describe("逐笔对照的编辑", () => {
  // 后端把没列出的笔当删除 / 新增：把删除、新增行也回传，或丢了改动标记，迁移就按错的对照走
  it("回传的对应 → 只取对上的笔，改动标记取自状态；断开后那一对不再回传", () => {
    expect(linksOf(rows)).toEqual([{ old: 0, new: 0, changed: false }, { old: 2, new: 1, changed: true }, { old: 4, new: 4, changed: false }]);
    expect(unlink(rows, 2)).toEqual([{ old: 0, new: 0, changed: false }, { old: 4, new: 4, changed: false }]);
  });

  // 对应须在两件上都保持先后顺序，否则后端拒绝保存；界面只能给出连上后仍有序的新增笔
  it("连接删除的旧笔与新增的新笔 → 只能连到前后两对对应之间的新笔，越过已有对应的被拒绝；连上后按旧笔顺序回传并算改动", () => {
    expect(linkable(rows, 1)).toEqual([]);
    expect(link(rows, 1, 2)).toBeNull();
    expect(linkable(rows, 3)).toEqual([2, 3]);
    expect(link(rows, 3, 3)).toEqual([
      { old: 0, new: 0, changed: false }, { old: 2, new: 1, changed: true }, { old: 3, new: 3, changed: true }, { old: 4, new: 4, changed: false },
    ]);
  });

  // 折叠只为缩短上百笔的表：改动、删除、新增的笔被并进“不变”的折叠行，人就看不到要处理的笔
  it("连续不变的笔 → 三笔以上折叠成一行，遇到其他状态就断开，不足三笔照常逐行；展开后逐行列出", () => {
    const long: PassPair[] = [0, 1, 2, 3, 4].map(i => pair(i, i)).concat([pair(5, 5, "changed"), pair(6, 6), pair(7, 7)]);
    const shape = (expanded: Set<number>) => foldRows(long, expanded).map(line => line.kind === "fold" ? `fold ${line.old.join("-")}` : `${line.row.old}:${line.row.change}`);
    expect(shape(new Set())).toEqual(["fold 0-4", "5:changed", "6:same", "7:same"]);
    expect(shape(new Set([0]))).toEqual(["fold 0-4", "0:same", "1:same", "2:same", "3:same", "4:same", "5:changed", "6:same", "7:same"]);
  });
});
