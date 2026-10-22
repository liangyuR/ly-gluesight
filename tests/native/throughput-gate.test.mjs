import assert from "node:assert/strict";
import test from "node:test";
import { assessThroughput, MEASURE_QUEUE_CAPACITY } from "../../scripts/soak/throughput.mjs";

// 3 × 25 Hz 吞吐门槛是合并门槛（规格 §0 不裁帧、所有帧都测）：Unmeasured = 0、内存预算丢帧 = 0、测量队列峰值低于容量的 50%。
// 判定函数只认这几个数；边界（恰好 50% 不算达标）和“任一条不满足即不达标”是容易改错的地方。
const ok = { parts: 40, frames: { total: 1440, measured: 1440, unmeasured: 0, error: 0, missing: 0, outOfCorridor: 0 },
  camera: { channelDropped: 0, budgetDropped: 0 }, queue: { capacity: MEASURE_QUEUE_CAPACITY, peak: 80, peakRatio: 80 / MEASURE_QUEUE_CAPACITY } };
const with_ = patch => ({ ...ok, ...patch,
  frames: { ...ok.frames, ...patch.frames }, camera: { ...ok.camera, ...patch.camera }, queue: { ...ok.queue, ...patch.queue } });

test("吞吐门槛：各条件逐一不满足 → 不达标并点明原因，全部满足 → 达标", () => {
  const half = MEASURE_QUEUE_CAPACITY / 2;
  for (const [name, input, passed, reason] of [
    ["全部满足", ok, true, null],
    ["有帧没测成（Unmeasured）", with_({ frames: { unmeasured: 1 } }), false, /Unmeasured 1/],
    ["图像内存预算丢帧", with_({ camera: { budgetDropped: 1 } }), false, /预算丢帧 1/],
    ["测量队列峰值恰为容量的一半（要求低于 50%，边界不含）", with_({ queue: { peak: half, peakRatio: 0.5 } }), false, /50\.0%/],
    ["测量队列峰值略低于一半", with_({ queue: { peak: half - 1, peakRatio: (half - 1) / MEASURE_QUEUE_CAPACITY } }), true, null],
    ["lyFlow 测量失败", with_({ frames: { error: 2 } }), false, /测量失败 2/],
    ["帧通道满丢帧", with_({ camera: { channelDropped: 3 } }), false, /帧通道满丢帧 3/],
    ["没有生产出任何件", with_({ parts: 0 }), false, /没有生产出任何件/],
  ]) {
    const verdict = assessThroughput(input);
    assert.equal(verdict.passed, passed, `${name}：${verdict.failures.join("；")}`);
    assert.equal(verdict.failures.length === 0, passed, name);
    if (reason) assert(verdict.failures.some(f => reason.test(f)), `${name} 的原因应点明：${verdict.failures.join("；")}`);
  }
});
