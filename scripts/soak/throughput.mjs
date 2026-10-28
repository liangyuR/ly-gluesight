import assert from "node:assert/strict";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { buildBurstRecipe, burstFacts } from "../../tests/native/lib.mjs";
import { connect, ensureOffline, offlineHistory, parseArgs, percentiles, processMetrics, RECIPE_ID, sleep, until, writeJson } from "./lib.mjs";

// 3 × 25 Hz 吞吐门槛（规格 §0：不裁帧、所有帧都测；决策记录 7.1 与 TESTING.md“3 × 25 Hz 吞吐门槛”）：
// 三台相机按给定频率同时出帧，连续跑给定秒数，核对没有帧没测成（Unmeasured）、没有因图像内存预算被丢的帧、测量队列峰值不到容量的一半。
// 达不到时本脚本退出码 1 并把实测值写进报告，由用户决定降 PLC 脉冲频率（决策记录 7.1 的降级方案）还是加测量并发；PC 不自行降频。
// “连续 60 s 满负载”本身也是判定条件：产线中途停了（stoppedEarly）不算达标；持续负载只统计相机使能为高的时间窗内的帧率
// （Σ窗口内帧数 ÷ Σ使能时长，见 enableWindows），要求 ≥ 需求 3 × hz 的 90%，并且至少连续 10 件（--min-parts）逐件达到这个帧率，
// 否则空转或只有零星几件的运行会把队列峰值判成“余量充足”。件间握手（使能为低：partEnd → 结果 → ACK → 下一件接件登记）衡量的是
// PLC 握手开销而不是 PC 的测量能力（离线虚拟 PLC 每件约 0.9 s，按含握手的平均帧率算任何来源都达不到），单独报告为
// achieved.handshakeGapSeconds，不参与判定。持续负载未验证时默认判不达标；只想看分项时显式传 `--burst true`
// （Test-Soak.ps1 的 -AllowBurstOnly）放行，此时报告里 sustainedVerified 仍为 false、burstOnlyAccepted 为 true，
// 结论只是“分项满足”，不能当作持续 3 × 25 Hz 门槛的结论。
//
// 两种图像来源，结果里都写明（boundary），不能互相替代：
//   sampleSet  离线·样本集：帧从现场原图读出，生产路径用真实 lyFlow 测量——这是唯一能在离线下压 lyFlow 的来源。样本集每件只有 12 个点位，
//              脉冲串按 12 个脉冲一件：使能窗口内是 3 × hz，件间握手期间没有帧，测量队列在间隙里排空，所以队列峰值只反映每件
//              12 个帧组的积压，不等于不间断 3 × hz 下的积压（projection 给出外推）。
//   synthetic  离线·合成图：每帧逐像素渲染，生产路径用模拟测量，所以这里测的是帧管线（节拍、落库、录制、界面）与渲染能力，lyFlow 没有参与；
//              渲染赶不上脉冲时末尾的帧会晚于 partEnd，记缺帧。
// “图像内存预算”只作用于海康回调来源（camera.rs 的 IMAGE_BUDGET_PER_CAMERA），两种离线来源按节拍生成、不受它约束，
// 所以离线下预算丢帧恒为 0，现场机要看真机来源的相机状态 budgetDroppedFrames；这里仍如实读出并核对。

/** 段配方测量队列容量（src-tauri/src/measure.rs 的 FRAME_MEASURE_QUEUE）。 */
export const MEASURE_QUEUE_CAPACITY = 1024;
/** 持续负载的判定：使能窗口内帧率至少达到需求 3 × hz 的这个比例，且产线没有中途停。 */
export const SUSTAINED_RATIO = 0.9;
/** 持续负载至少要连续这么多件逐件达到上面的帧率（--min-parts 可改）。 */
export const MIN_SUSTAINED_PARTS = 10;
export const GATE = { unmeasured: 0, budgetDropped: 0, queuePeakRatio: 0.5, sustainedRatio: SUSTAINED_RATIO, minSustainedParts: MIN_SUSTAINED_PARTS };

const median = values => values.length ? [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)] : null;

/**
 * 每件的相机使能窗口，取自运行日志里每帧一条“帧到达”（cycleId、seq、ts，毫秒）。PC 看不到使能信号本身，窗口取本件第一帧到达到
 * 最后一帧到达：窗口里经过的脉冲周期数 = 帧组数 − 1，所以首个帧组（最小 seq）的帧不计入窗口帧数（栅栏计数），否则每件 12 组时帧率偏高 12/11。
 * 不用接件登记 → partEnd：PLC 置 partEnd 前还要等一段（离线 300 ms），且接件登记早于 PLC 拉高 Line0，比到帧窗口更不准。
 * 相邻两件之间（上一件最后一帧到达 → 下一件第一帧到达）是件间握手间隙，单独返回，不计入使能时长。
 */
export function enableWindows(arrivals) {
  const byCycle = new Map();
  for (const { cycleId, ts, seq } of arrivals) {
    if (!cycleId || !Number.isFinite(ts) || !Number.isFinite(seq)) continue;
    const entry = byCycle.get(cycleId) ?? { cycleId, start: ts, end: ts, frames: [] };
    entry.start = Math.min(entry.start, ts); entry.end = Math.max(entry.end, ts); entry.frames.push(seq);
    byCycle.set(cycleId, entry);
  }
  const windows = [...byCycle.values()].map(({ cycleId, start, end, frames }) => {
    const firstSeq = Math.min(...frames);
    return { cycleId, start, end, frames: frames.filter(seq => seq > firstSeq).length, seconds: (end - start) / 1000 };
  }).sort((a, b) => a.start - b.start);
  const gaps = windows.slice(1).map((window, i) => (window.start - windows[i].end) / 1000);
  return { windows, gaps };
}

/** 持续负载指标：Σ窗口内帧数 ÷ Σ使能时长，与逐件达到 floorFps 的最长连续件数；件间握手间隙只报告。 */
function sustainedLoad(arrivals, demandFps) {
  const { windows, gaps } = enableWindows(arrivals);
  const floorFps = SUSTAINED_RATIO * demandFps;
  const seconds = windows.reduce((n, w) => n + w.seconds, 0);
  const enabledFps = seconds > 0 ? windows.reduce((n, w) => n + w.frames, 0) / seconds : null;
  let run = 0, consecutiveParts = 0;
  for (const w of windows) {
    run = w.seconds > 0 && w.frames / w.seconds >= floorFps ? run + 1 : 0;
    consecutiveParts = Math.max(consecutiveParts, run);
  }
  return { enabledFps, floorFps, consecutiveParts, parts: windows.length,
    handshakeGapSeconds: gaps.length ? { n: gaps.length, median: median(gaps), max: Math.max(...gaps) } : null };
}

/**
 * 判门槛：Unmeasured = 0、预算丢帧 = 0、测量队列峰值 < 容量的 50%；另把帧通道满丢帧和测量失败也作为失败列出（都是“不裁帧”被破坏）。
 * 这些分项只在满负载下才有意义，所以还要求 run：产线没有中途停（stoppedEarly），且持续负载已验证（sustainedVerified）——
 * 使能窗口内帧率（run.arrivals 按 enableWindows 切窗）≥ 需求 run.demandFps 的 90%，并有至少 minSustainedParts 件连续逐件达到。
 * 件间握手间隙不计入。持续负载未验证时默认判不达标（帧率不够记 notSustained，帧率够但连续件数不够记 sustainedTooShort）；
 * allowBurstOnly（--burst true）显式放行，此时 passed 只代表分项满足，sustainedVerified 仍为 false，burstOnlyAccepted 为 true。
 * 返回的 failureKinds 与 failures 一一对应，供测试按规则种类断言，不依赖文案。
 */
export function assessThroughput({ parts, frames, camera, queue, run, allowBurstOnly = false, minSustainedParts = MIN_SUSTAINED_PARTS }) {
  const failures = [], failureKinds = [];
  const fail = (kind, message) => { failureKinds.push(kind); failures.push(message); };
  if (!parts) fail("noParts", "没有生产出任何件");
  if (run.stoppedEarly) fail("stoppedEarly", "产线在跑满设定时长前就停了（site_status.running 变 false）：没有持续满负载，各分项不能算数");
  const load = sustainedLoad(run.arrivals, run.demandFps);
  const rateOk = load.enabledFps !== null && load.enabledFps >= load.floorFps;
  const sustainedVerified = !run.stoppedEarly && rateOk && load.consecutiveParts >= minSustainedParts;
  if (!sustainedVerified && !run.stoppedEarly && !allowBurstOnly) {
    const hint = "只想看分项时显式传 --burst true（Test-Soak.ps1 -AllowBurstOnly），该结果不是持续门槛的结论";
    if (!rateOk) {
      fail("notSustained", `持续负载未验证：使能窗口内帧率 ${load.enabledFps === null ? "无数据" : `${load.enabledFps.toFixed(1)} 帧/s`}，`
        + `需求 ${run.demandFps} 帧/s 的 ${SUSTAINED_RATIO * 100}% 是 ${load.floorFps} 帧/s；${hint}`);
    } else {
      fail("sustainedTooShort", `持续负载未验证：逐件达到 ${load.floorFps} 帧/s 的件最长只连续 ${load.consecutiveParts} 件，要求至少 ${minSustainedParts} 件；${hint}`);
    }
  }
  if (frames.unmeasured > GATE.unmeasured) fail("unmeasured", `Unmeasured ${frames.unmeasured} 帧（要求 ${GATE.unmeasured}）：测量队满或超时没测`);
  if (frames.error > 0) fail("measureError", `测量失败 ${frames.error} 帧：lyFlow 报错，检测不完整`);
  if (camera.budgetDropped > GATE.budgetDropped) fail("budgetDropped", `图像内存预算丢帧 ${camera.budgetDropped} 帧（要求 ${GATE.budgetDropped}）`);
  if (camera.channelDropped > 0) fail("channelDropped", `帧通道满丢帧 ${camera.channelDropped} 帧：检测节拍来不及取走`);
  if (queue.peakRatio >= GATE.queuePeakRatio) fail("queuePeak", `测量队列峰值 ${queue.peak} / ${queue.capacity}（${(queue.peakRatio * 100).toFixed(1)}%），要求低于 ${GATE.queuePeakRatio * 100}%`);
  const passed = failures.length === 0;
  // 按突发放行只在显式放行、持续负载未验证且分项确实满足时成立；分项不达标时不能记成“按突发放行”
  return { failures, failureKinds, passed, sustainedVerified, burstOnlyAccepted: allowBurstOnly && !sustainedVerified && passed, load };
}

/** 运行日志里某类事件（关键字）的全部记录，分页读出。 */
async function eventLogs(call, keyword, from, to) {
  const records = [];
  for (let offset = 0; offset < 20000; offset += 1000) {
    const page = await call("applog_query", { query: { from, to, keyword, offset, limit: 1000 } });
    records.push(...page.items);
    if (offset + 1000 >= page.total) break;
  }
  return records;
}

/**
 * 突发排空速率与持续压力下的外推：每件的脉冲串是一次突发（样本集每件 12 个脉冲），取该件第一帧到达到最后一帧测完的时间，
 * 得到测量排空速率；与 3 × hz 的需求相比，需求更大时按“排队深度 = (需求 − 排空) × 秒数”外推队列多久装满。
 * 只是估算：突发里前几帧到达时测量线程还没排满，所以排空速率是下界；持续压力下的真实余量以现场机连续跑的结果为准。
 */
export function projectSustained({ hz, capacity, parts }) {
  const demand = 3 * hz;
  const drain = median(parts.map(p => p.drainFps));
  const arrival = median(parts.map(p => p.arrivalFps));
  const deficit = drain === null ? null : Math.max(0, demand - drain);
  return { demandFps: demand, burstArrivalFps: arrival, drainFps: drain, deficitFps: deficit,
    secondsToFillQueue: deficit ? Number((capacity / deficit).toFixed(1)) : null, parts: parts.length };
}

async function cameraCounters(call) {
  const status = await call("camera_rig_status");
  return status.map(c => ({ frames: c.frames ?? 0, channelDropped: c.droppedFrames ?? 0, budgetDropped: c.budgetDroppedFrames ?? 0 }));
}

/** 合成图来源：进入离线·合成图并保证压测配方（THRU-SYN）已发布；段配方建站需要 lyFlow（走廊测量、标定、验证）。 */
async function ensureSynthetic(call) {
  const site = await call("site_status");
  if (site.mode === "offline" && site.source !== "synthetic") await call("site_leave_offline");
  if ((await call("site_status")).mode !== "offline") await call("site_enter_offline", { source: { kind: "synthetic" } });
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const id = "THRU-SYN";
  if (!(await call("cycle_recipes")).some(r => r.id === id)) {
    const template = await call("recipe_template");
    const built = await buildBurstRecipe(call, { sourceId: "synthetic", doc: { ...template, id, name: "吞吐压测" }, log: message => process.stdout.write(`[throughput] ${message}\n`) });
    assert(built.validation.passed, `压测配方验证未通过：${JSON.stringify(built.validation.checks.filter(c => !c.passed))}`);
  }
  await until(async () => (await call("cycle_readiness")).recipes.find(r => r.id === id)?.runnable === true, Boolean, `配方 ${id} 不可运行`, 90000, 500);
  return id;
}

export async function throughput(session, out, options) {
  const { call, manifest } = session;
  const hz = Number(options.hz ?? 25);
  const seconds = Number(options.seconds ?? 60);
  const source = options.source ?? "sampleSet";
  assert(["sampleSet", "synthetic"].includes(source), "--source 只能是 sampleSet 或 synthetic");
  assert(hz >= 1 && hz <= 66, "--hz 需在 1–66 之间（相机上限 66 fps）");
  assert(seconds >= 5, "--seconds 至少 5");
  // 显式放行“持续负载未验证”的运行：passed 只代表分项满足，sustainedVerified 仍如实写 false
  assert([undefined, "true", "false"].includes(options.burst), "--burst 只能是 true 或 false");
  const allowBurstOnly = options.burst === "true";
  const minSustainedParts = Number(options["min-parts"] ?? MIN_SUSTAINED_PARTS);
  assert(Number.isInteger(minSustainedParts) && minSustainedParts >= 1, "--min-parts 需为正整数");
  const settings = await call("cycle_get_settings");
  await call("cycle_save_settings", { settings: { ...settings, productSource: "plc" } });
  const recipeId = source === "synthetic" ? await ensureSynthetic(call) : (await ensureOffline(call, out), RECIPE_ID);
  const site = await call("site_status");
  const intervalMs = Math.round(1000 / hz);
  await call("offline_pace", { pace: { shotIntervalMs: intervalMs, pauseMs: 0 } });
  const before = { history: await offlineHistory(call, recipeId), cameras: await cameraCounters(call) };
  const known = new Set(before.history.items.map(i => i.id));
  const metrics = [await processMetrics(manifest.pid)];
  const startedAt = Date.now();
  let queuePeak = 0, peakGroups = 0, polls = 0;
  await call("offline_run", { recipeId, scenario: "normal", continuous: true, partId: null });
  let lastMetrics = Date.now();
  let stopped = false;
  while (Date.now() - startedAt < seconds * 1000) {
    await sleep(50);
    const snapshot = await call("cycle_snapshot");
    polls++;
    const burst = snapshot?.part?.burst;
    if (burst) { queuePeak = Math.max(queuePeak, burst.pendingMeasure ?? 0); peakGroups = Math.max(peakGroups, burst.groupCount ?? 0); }
    if (Date.now() - lastMetrics >= 10000) { metrics.push(await processMetrics(manifest.pid)); lastMetrics = Date.now(); }
    if (!(await call("site_status")).running) { stopped = true; break; }
  }
  await call("offline_stop", { now: false });
  await until(() => call("site_status"), s => !s.running, "离线产线未停止", 60000, 300);
  const endedAt = Date.now();
  metrics.push(await processMetrics(manifest.pid));
  const history = await offlineHistory(call, recipeId);
  const items = history.items.filter(i => !known.has(i.id)).sort((a, b) => a.ts - b.ts);
  const totals = { total: 0, measured: 0, unmeasured: 0, error: 0, missing: 0, outOfCorridor: 0 };
  const verdicts = {};
  const flags = { incomplete: 0, frameCountAnomaly: 0, shortPart: 0 };
  let groups = 0, firstFrames = 0;
  for (const [index, item] of items.entries()) {
    verdicts[item.verdict] = (verdicts[item.verdict] ?? 0) + 1;
    const detail = await call("history_detail", { id: item.id });
    if (!detail.burst) continue;
    const facts = burstFacts(detail);
    groups += facts.groupCount;
    for (const [state, n] of Object.entries(facts.frameStates)) {
      totals.total += n; totals[state] = (totals[state] ?? 0) + n;
      if (index === 0) firstFrames += n;
    }
    for (const flag of Object.keys(flags)) if (facts.flags[flag]) flags[flag]++;
  }
  // 测量耗时与排队耗时：运行日志里每帧一条“测量完成”；每件第一帧到达 → 最后一帧测完给出突发排空速率
  const latency = { ms: [], queueMs: [], engineMs: [] };
  const done = await eventLogs(call, "测量完成", startedAt, endedAt + 5000);
  const arrived = await eventLogs(call, "帧到达", startedAt, endedAt + 5000);
  for (const record of done) for (const key of Object.keys(latency)) if (Number.isFinite(record.data?.[key])) latency[key].push(record.data[key]);
  const byCycle = new Map();
  for (const [kind, records] of [["arrived", arrived], ["done", done]]) {
    for (const record of records) {
      if (!record.cycleId) continue;
      const entry = byCycle.get(record.cycleId) ?? { arrived: [], done: [] };
      entry[kind].push(record.ts);
      byCycle.set(record.cycleId, entry);
    }
  }
  const perPart = [...byCycle.values()].filter(e => e.arrived.length > 3 && e.done.length === e.arrived.length).map(e => {
    const first = Math.min(...e.arrived);
    const arrivalSpan = Math.max(...e.arrived) - first, drainSpan = Math.max(...e.done) - first;
    return { frames: e.arrived.length, arrivalFps: arrivalSpan > 0 ? e.arrived.length / (arrivalSpan / 1000) : null, drainFps: drainSpan > 0 ? e.done.length / (drainSpan / 1000) : null };
  }).filter(p => p.arrivalFps && p.drainFps);
  const projection = projectSustained({ hz, capacity: MEASURE_QUEUE_CAPACITY, parts: perPart });
  const after = await cameraCounters(call);
  const camera = {
    channelDropped: after.reduce((n, c, i) => n + c.channelDropped - before.cameras[i].channelDropped, 0),
    budgetDropped: after.reduce((n, c, i) => n + c.budgetDropped - before.cameras[i].budgetDropped, 0),
    perCamera: after.map((c, i) => ({ frames: c.frames - before.cameras[i].frames, channelDropped: c.channelDropped - before.cameras[i].channelDropped, budgetDropped: c.budgetDropped - before.cameras[i].budgetDropped })),
  };
  const queue = { capacity: MEASURE_QUEUE_CAPACITY, peak: queuePeak, peakRatio: queuePeak / MEASURE_QUEUE_CAPACITY, polls, note: "运行中每 50 ms 读一次在途件的 pendingMeasure 取最大值，是采样下界" };
  const span = items.length > 1 ? items.at(-1).ts - items[0].ts : endedAt - startedAt;
  const main = metrics.map(m => m.main).filter(Boolean);
  // 含件间握手的平均帧率只作参考：按第一件结束到最后一件结束这段时间里的整件算，第一件的帧在这段之前，不计入
  const spanFrames = items.length > 1 ? totals.total - firstFrames : totals.total;
  const demandFps = 3 * hz;
  const verdict = assessThroughput({ parts: items.length, frames: totals, camera, queue, run: { stoppedEarly: stopped, demandFps, arrivals: arrived }, allowBurstOnly, minSustainedParts });
  const round = (value, digits) => value === null ? null : Number(value.toFixed(digits));
  const gaps = verdict.load.handshakeGapSeconds;
  const achieved = {
    enabledFps: round(verdict.load.enabledFps, 1), sustainedParts: verdict.load.consecutiveParts, minSustainedParts, windows: verdict.load.parts,
    handshakeGapSeconds: gaps && { n: gaps.n, median: round(gaps.median, 3), max: round(gaps.max, 3) },
    framesPerSecond: span > 0 ? Number((spanFrames / (span / 1000)).toFixed(1)) : null, partsPerMinute: span > 0 ? Number(((items.length - 1) * 60000 / span).toFixed(1)) : null,
    target: demandFps,
    note: "enabledFps 是持续负载的判定依据：只统计相机使能为高的时间窗（每件第一帧到达到最后一帧到达，首个帧组不计），Σ窗口内帧数 ÷ Σ使能时长；"
      + "sustainedParts 是逐件达到需求 90% 的最长连续件数；handshakeGapSeconds 是件间握手间隙（上一件最后一帧到达 → 下一件第一帧到达），不参与判定；"
      + "framesPerSecond 是含件间握手的平均帧率，只作参考",
  };
  return {
    passed: verdict.passed, sustainedVerified: verdict.sustainedVerified, burstOnlyAccepted: verdict.burstOnlyAccepted,
    failures: verdict.failures, failureKinds: verdict.failureKinds, gate: { ...GATE, minSustainedParts },
    source, measurement: site.measurement, recipeId, hz, intervalMs, seconds, elapsedMs: endedAt - startedAt, stoppedEarly: stopped,
    parts: items.length, groups, verdicts, flags, frames: totals, camera, queue,
    achieved,
    latencyMs: { total: percentiles(latency.ms), queue: percentiles(latency.queueMs), engine: percentiles(latency.engineMs) },
    projection,
    process: { startPrivBytes: main[0]?.priv ?? null, peakPrivBytes: main.length ? Math.max(...main.map(m => m.priv)) : null, endPrivBytes: main.at(-1)?.priv ?? null,
      peakSystemLoad: Math.max(...metrics.map(m => m.systemLoad ?? 0)) },
    boundary: source === "sampleSet"
      ? "离线·样本集：现场原图（停稳拍，每件 12 点位）经真实 lyFlow 测量；每件 12 个脉冲，使能窗口内帧率见 achieved.enabledFps，件间握手期间没有帧、测量队列在间隙里排空（队列峰值只反映每件的积压，持续积压见 projection）；检测项全关（长稳配方）；预算丢帧只作用于海康回调来源，离线恒为 0；开发笔记本调试构建，不能外推现场工控机"
      : "离线·合成图：每帧逐像素渲染、生产路径用模拟测量，lyFlow 没有参与测量；渲染能力限制帧率，末尾帧可能缺；预算丢帧只作用于海康回调来源，离线恒为 0；开发笔记本调试构建，不能外推现场工控机",
  };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  assert(args.command === "run", "命令为 run");
  const out = path.resolve(args.out ?? "output/soak");
  const session = await connect(args.cdp ?? "http://[::1]:9347", args.instance ?? "output/playwright/soak-regression/instance.json");
  try {
    const result = await throughput(session, out, args);
    await writeJson(path.join(out, `throughput-${result.source}-${result.hz}hz.json`), result);
    process.stdout.write(JSON.stringify({ passed: result.passed, sustainedVerified: result.sustainedVerified, burstOnlyAccepted: result.burstOnlyAccepted, failures: result.failures, source: result.source, measurement: result.measurement, hz: result.hz, parts: result.parts, frames: result.frames,
      camera: { channelDropped: result.camera.channelDropped, budgetDropped: result.camera.budgetDropped }, queue: result.queue, achieved: result.achieved, latencyMs: result.latencyMs, projection: result.projection }, null, 1) + "\n");
    if (result.burstOnlyAccepted) process.stderr.write("按突发放行（--burst true）：持续满负载没有验证，这不是持续 3 × 25 Hz 门槛的结论。\n");
    if (!result.passed) process.exitCode = 1;
  } finally { await session.browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
