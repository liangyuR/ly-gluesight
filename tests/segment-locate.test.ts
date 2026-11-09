import { describe, expect, it } from "vitest";
import { teachingSeqFor } from "../src/features/workspace/segmentLocate";

// 防的回归：从历史件去分段页时定位到的示教件帧组与生产换算段边界的口径不一致，操作员对着错的位置调边界
describe("历史件帧组在示教件上的对应位置", () => {
  it.each([
    // 笔数相同：按所在笔的笔内进度对应（第 2 笔开头 → 示教件第 2 笔开头），不按整件比例
    { name: "笔数相同 → 按笔内进度", seq: 11, groupCount: 30, passes: [10, 20], teaching: [20, 10], expected: { seq: 21, byPass: true } },
    // 笔尾多出帧：笔中间按离笔起点的帧数对应，不按笔长比例前移；超出示教件这一笔的帧对到这一笔末帧，不跨到下一笔
    { name: "笔尾多帧 → 按离笔起点的帧数", seq: 6, groupCount: 16, passes: [12, 4], teaching: [10, 4], expected: { seq: 6, byPass: true } },
    { name: "超出示教件笔长 → 这一笔末帧", seq: 12, groupCount: 16, passes: [12, 4], teaching: [10, 4], expected: { seq: 10, byPass: true } },
    // 笔数不同：按整件进度
    { name: "笔数不同 → 按整件进度", seq: 11, groupCount: 30, passes: [10, 20], teaching: [30], expected: { seq: 11, byPass: false } },
  ])("$name", ({ seq, groupCount, passes, teaching, expected }) => {
    expect(teachingSeqFor({ seq, groupCount, passes }, teaching.reduce((a, b) => a + b, 0), teaching)).toEqual(expected);
  });
});
