import assert from "node:assert/strict";
import test from "node:test";
import { assessThroughput, MEASURE_QUEUE_CAPACITY } from "../../scripts/soak/throughput.mjs";

// 3 × 25 Hz 吞吐门槛是合并门槛（规格 §0 不裁帧、所有帧都测）。判定函数认两类条件：
//   分项：Unmeasured = 0、内存预算丢帧 = 0、测量队列峰值低于容量的 50%（另列：测量失败、帧通道满、没有出件）；
//   持续满负载：产线没有中途停；只按相机使能为高的时间窗计帧率（每件第一帧到达到最后一帧到达、首个帧组不计），
//   Σ窗口内帧数 ÷ Σ使能时长 ≥ 需求 3 × hz 的 90%，且至少连续 10 件逐件达到。件间握手间隙不计入。
// 每条规则一个用例，只断言 passed 与失败条件的种类（failureKinds），不断言文案。
const half = MEASURE_QUEUE_CAPACITY / 2;

/** 运行日志里的“帧到达”：每件 groups 个帧组、三台相机同一时刻到帧，使能窗口内帧组间隔 3000 / fps ms，件间握手 gapMs；slow 里的件按 slowFps 到帧。 */
function arrivals({ parts = 12, groups = 12, fps = 75, gapMs = 900, slow = [], slowFps = 60 } = {}) {
  const records = [];
  let t = 1_000_000;
  for (let part = 0; part < parts; part++) {
    const period = 3000 / (slow.includes(part) ? slowFps : fps);
    for (let seq = 1; seq <= groups; seq++) {
      for (let camera = 0; camera < 3; camera++) records.push({ cycleId: `cycle-${part}`, ts: Math.round(t), seq });
      if (seq < groups) t += period;
    }
    t += gapMs;
  }
  return records;
}

const ok = { parts: 12, frames: { total: 432, measured: 432, unmeasured: 0, error: 0, missing: 0, outOfCorridor: 0 },
  camera: { channelDropped: 0, budgetDropped: 0 }, queue: { capacity: MEASURE_QUEUE_CAPACITY, peak: 80, peakRatio: 80 / MEASURE_QUEUE_CAPACITY },
  run: { stoppedEarly: false, demandFps: 75, arrivals: arrivals() } };
const with_ = patch => ({ ...ok, ...patch,
  frames: { ...ok.frames, ...patch.frames }, camera: { ...ok.camera, ...patch.camera }, queue: { ...ok.queue, ...patch.queue }, run: { ...ok.run, ...patch.run } });

test("件间握手间隙 0.9 s（含间隙的平均帧率只有需求约三分之一）、使能窗口内满 3 × 25 Hz、分项都满足 → 达标，持续负载已验证", () => {
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
  ["使能窗口内帧率只有需求的 88%（要求至少 90%）", { run: { arrivals: arrivals({ fps: 66 }) } }, "notSustained"],
  ["只有 9 件、件件满负载（要求连续至少 10 件）", { run: { arrivals: arrivals({ parts: 9 }) } }, "sustainedTooShort"],
  ["15 件里第 8 件掉到需求的 80%、最长只连续 7 件满负载", { run: { arrivals: arrivals({ parts: 15, slow: [7] }) } }, "sustainedTooShort"],
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

test("使能窗口内帧率不到需求的 90%、显式放行 → 分项达标，持续负载标未验证，记为按突发放行", () => {
  const verdict = assessThroughput({ ...with_({ run: { arrivals: arrivals({ fps: 66 }) } }), allowBurstOnly: true });
  assert.equal(verdict.passed, true);
  assert.equal(verdict.sustainedVerified, false);
  assert.equal(verdict.burstOnlyAccepted, true);
});

test("显式放行、但有帧没测成 → 不达标，也不记为按突发放行", () => {
  const verdict = assessThroughput({ ...with_({ frames: { unmeasured: 1 }, run: { arrivals: arrivals({ fps: 66 }) } }), allowBurstOnly: true });
  assert.equal(verdict.passed, false);
  assert.equal(verdict.burstOnlyAccepted, false);
});

test("产线没跑满设定时长就停了、即使显式放行 → 仍不达标", () => {
  const verdict = assessThroughput({ ...with_({ run: { stoppedEarly: true, arrivals: arrivals({ fps: 66 }) } }), allowBurstOnly: true });
  assert.equal(verdict.passed, false);
  assert.deepEqual(verdict.failureKinds, ["stoppedEarly"]);
  assert.equal(verdict.sustainedVerified, false);
});
