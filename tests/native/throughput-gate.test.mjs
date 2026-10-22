import assert from "node:assert/strict";
import test from "node:test";
import { assessThroughput, MEASURE_QUEUE_CAPACITY } from "../../scripts/soak/throughput.mjs";

// 3 × 25 Hz 吞吐门槛是合并门槛（规格 §0 不裁帧、所有帧都测）。判定函数认两类条件：
//   分项：Unmeasured = 0、内存预算丢帧 = 0、测量队列峰值低于容量的 50%（另列：测量失败、帧通道满、没有出件）；
//   持续满负载：产线没有中途停、实际帧率 ≥ 需求 3 × hz 的 90%。分项只在满负载下才有意义，
//   否则空转或只有突发（样本集平均 25.9 帧/s、目标 75）的运行会被判成“余量充足”。
// 每条规则一个用例，只断言 passed 与失败条件的种类（failureKinds），不断言文案。
const half = MEASURE_QUEUE_CAPACITY / 2;
const ok = { parts: 40, frames: { total: 1440, measured: 1440, unmeasured: 0, error: 0, missing: 0, outOfCorridor: 0 },
  camera: { channelDropped: 0, budgetDropped: 0 }, queue: { capacity: MEASURE_QUEUE_CAPACITY, peak: 80, peakRatio: 80 / MEASURE_QUEUE_CAPACITY },
  run: { stoppedEarly: false, framesPerSecond: 74, demandFps: 75 } };
const with_ = patch => ({ ...ok, ...patch,
  frames: { ...ok.frames, ...patch.frames }, camera: { ...ok.camera, ...patch.camera }, queue: { ...ok.queue, ...patch.queue }, run: { ...ok.run, ...patch.run } });

test("分项与持续满负载都满足 → 达标，持续负载已验证", () => {
  const verdict = assessThroughput(ok);
  assert.equal(verdict.passed, true);
  assert.deepEqual(verdict.failureKinds, []);
  assert.equal(verdict.sustainedVerified, true);
});

for (const [name, patch, kind] of [
  ["有帧没测成（Unmeasured）", { frames: { unmeasured: 1 } }, "unmeasured"],
  ["图像内存预算丢帧", { camera: { budgetDropped: 1 } }, "budgetDropped"],
  ["测量队列峰值恰为容量的一半（要求低于 50%，边界不含）", { queue: { peak: half, peakRatio: 0.5 } }, "queuePeak"],
  ["lyFlow 测量失败", { frames: { error: 2 } }, "measureError"],
  ["帧通道满丢帧", { camera: { channelDropped: 3 } }, "channelDropped"],
  ["没有生产出任何件", { parts: 0 }, "noParts"],
  ["产线没跑满设定时长就停了", { run: { stoppedEarly: true } }, "stoppedEarly"],
  ["实际帧率只有需求的 88%（要求至少 90%）", { run: { framesPerSecond: 66 } }, "notSustained"],
]) {
  test(`${name} → 不达标，失败种类只有 ${kind}`, () => {
    const verdict = assessThroughput(with_(patch));
    assert.equal(verdict.passed, false);
    assert.deepEqual(verdict.failureKinds, [kind]);
  });
}

test("测量队列峰值略低于容量的一半 → 达标", () => {
  const verdict = assessThroughput(with_({ queue: { peak: half - 1, peakRatio: (half - 1) / MEASURE_QUEUE_CAPACITY } }));
  assert.equal(verdict.passed, true);
});

test("样本集式的突发（实际帧率 25.9，需求 75）、显式放行 → 分项达标，但持续负载仍标未验证", () => {
  const verdict = assessThroughput({ ...with_({ run: { framesPerSecond: 25.9 } }), allowBurstOnly: true });
  assert.equal(verdict.passed, true);
  assert.equal(verdict.sustainedVerified, false);
});

test("产线没跑满设定时长就停了、即使显式放行突发 → 仍不达标", () => {
  const verdict = assessThroughput({ ...with_({ run: { stoppedEarly: true, framesPerSecond: 25.9 } }), allowBurstOnly: true });
  assert.equal(verdict.passed, false);
  assert.deepEqual(verdict.failureKinds, ["stoppedEarly"]);
  assert.equal(verdict.sustainedVerified, false);
});
