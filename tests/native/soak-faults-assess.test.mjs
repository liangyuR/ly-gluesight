import assert from "node:assert/strict";
import test from "node:test";
import { assess } from "../../scripts/soak/faults.mjs";

// 长稳故障注入的判定函数 assess 对“脉冲数波动”件（pulseJitter）的核对。
// 首尾各 ±1 个脉冲，总数最多偏离名义 2 个；长稳用样本集标定，Σtol 只有 1，所以帧组数可以低于 ΣN_exp − Σtol，
// 此时标“疑似提前终止”是规格要求的告警（E1），不是缺陷，不能把它判成长稳失败（否则按脉冲种子约 1/9 的概率随机失败）。
const jitter = (verdict, groupCount, shortPart) => ({
  pulseJitter: { record: { verdict, flags: { shortPart } }, count: { groupCount, expectedTotal: 12, toleranceTotal: 1 }, recovered: true },
});

test("脉冲少了 2 个、帧组数 10 低于下限 11 并标疑似提前终止、结论仍合格 → 不算失败", () => {
  assert.deepEqual(assess(jitter("OK", 10, true)).failures, []);
});

test("帧组数 12 在容差内却标了疑似提前终止 → 失败", () => {
  assert.equal(assess(jitter("OK", 12, true)).failures.length, 1);
});

test("脉冲数波动让本件结论变成 NG → 失败", () => {
  assert.equal(assess(jitter("NG_GAP", 12, false)).failures.length, 1);
});
