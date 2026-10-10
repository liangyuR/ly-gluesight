import assert from 'node:assert/strict';
import test from 'node:test';
import { isQuiescentCycleSnapshot, isPublishedRecipeReady, waitForPublishedRecipeReady } from './p0-clean-cyclehost-setup.mjs';

function retainedFinal() {
  return { phase: 'IDLE', plcLocked: false, fault: null,
    part: { cycleId: '6bc85829b4ad25e083eda3fe80863032', sn: 750000100, recipeId: 'P0-PRESSURE-4',
      received: 0, triggers: 0, queue: 0, filled: 404, total: 404,
      frames: [1, 2, 3, 4].map(k => ({ shotId: 'P' + k, status: 'missing' })) },
    result: { cycleId: '6bc85829b4ad25e083eda3fe80863032', sn: 750000100, recipeId: 'P0-PRESSURE-4',
      verdict: 'ERR_INSPECT', plcCode: 90, faultCode: 91 } };
}

test('actual retained 90/91 final snapshot is quiescent in IDLE or disconnected FAULT', () => {
  const cycle = retainedFinal();
  assert.equal(isQuiescentCycleSnapshot(cycle), true);
  cycle.phase = 'FAULT'; cycle.fault = 'PLC 未连接';
  assert.equal(isQuiescentCycleSnapshot(cycle), true);
});

for (const phase of ['IDLE', 'FAULT']) test('fresh no-part ' + phase + ' requires an explicitly unlocked PLC', () => {
  assert.equal(isQuiescentCycleSnapshot({ phase, plcLocked: false, part: null }), true);
  assert.equal(isQuiescentCycleSnapshot({ phase, plcLocked: false }), true);
  assert.equal(isQuiescentCycleSnapshot({ phase, plcLocked: true }), false);
  assert.equal(isQuiescentCycleSnapshot({ phase }), false);
});

const rejected = [
  ...['ACQUIRE', 'DRAIN', 'WaitAck', 'REPORT', 'RELEASE', 'VALIDATE'].map(phase => ['active phase ' + phase, cycle => { cycle.phase = phase; }]),
  ['PLC transaction locked', cycle => { cycle.plcLocked = true; }],
  ['unknown PLC transaction lock', cycle => { delete cycle.plcLocked; }],
  ['no matching result', cycle => { cycle.result = null; }],
  ['different cycle ID', cycle => { cycle.result.cycleId = 'other-cycle'; }],
  ['different SN', cycle => { cycle.result.sn++; }],
  ['different recipe ID', cycle => { cycle.result.recipeId = 'other-recipe'; }],
  ['missing part cycle ID', cycle => { delete cycle.part.cycleId; delete cycle.result.cycleId; }],
  ['missing part recipe ID', cycle => { delete cycle.part.recipeId; delete cycle.result.recipeId; }],
  ['unknown SN', cycle => { cycle.part.sn = null; cycle.result.sn = null; }],
  ['pending measurement queue', cycle => { cycle.part.queue = 1; }],
  ['unknown measurement queue', cycle => { cycle.part.queue = null; }],
  ['unfilled result table', cycle => { cycle.part.filled--; }],
  ['empty result table', cycle => { cycle.part.total = 0; cycle.part.filled = 0; }],
  ['unknown result table size', cycle => { cycle.part.total = null; cycle.part.filled = null; }],
  ['fractional result table size', cycle => { cycle.part.total = 1.5; cycle.part.filled = 1.5; }],
];
for (const [name, change] of rejected) test('quiescent snapshot rejects ' + name, () => {
  const cycle = retainedFinal(); change(cycle);
  assert.equal(isQuiescentCycleSnapshot(cycle), false);
});

test('missing cycle snapshot is not quiescent', () => {
  for (const cycle of [null, undefined, {}]) assert.equal(isQuiescentCycleSnapshot(cycle), false);
});

const publishedLayout = { id: 'P0-PRESSURE-40', version: 1, revisionId: 'P0-PRESSURE-40-v1' };
const publishedOptions = { layout: publishedLayout, bundleId: 'release-40-explicit' };
function publishedReady() {
  return { layout: structuredClone(publishedLayout), cycle: { ...retainedFinal(), alarms: [] },
    sim: { running: false }, engine: { backend: 'LyFlow', ready: true, measuring: true },
    logs: [{ ts: 1000, level: 'ok', ev: '生产预热', msg: '发布包 release-40-explicit 已就绪，耗时 120 ms' }] };
}

test('published recipe requires its exact layout and positive bundle warmup log', () => {
  const observation = publishedReady();
  observation.cycle.alarms.push('配方 OTHER 开不了工：没有发布包');
  assert.equal(isPublishedRecipeReady(observation, publishedOptions), true);
});

for (const [name, change] of [
  ['no positive warmup log', value => { value.logs = []; }],
  ['another bundle', value => { value.logs[0].msg = '发布包 release-other 已就绪，耗时 120 ms'; }],
  ['bundle prefix collision', value => { value.logs[0].msg = '发布包 release-40-explicit-extra 已就绪，耗时 120 ms'; }],
  ['unknown completion time', value => { value.logs[0].msg = '发布包 release-40-explicit 已就绪，耗时 null ms'; }],
  ['wrong log event', value => { value.logs[0].ev = '检测结果'; }],
  ['failed warmup log', value => { value.logs[0].level = 'err'; }],
  ['still warming', value => { value.cycle.alarms.push('配方 P0-PRESSURE-40 开不了工：发布资源与图像引擎正在预热，完成前不能布防'); }],
  ['temporary warming', value => { value.cycle.alarms.push('配方 P0-PRESSURE-40 暂时开不了工：发布资源与图像引擎正在预热，完成前不能布防'); }],
  ['active cycle', value => { value.cycle.phase = 'ACQUIRE'; }],
  ['fault phase', value => { value.cycle.phase = 'FAULT'; }],
  ['locked PLC', value => { value.cycle.plcLocked = true; }],
  ['unfilled retained cycle', value => { value.cycle.part.filled--; }],
  ['running simulator', value => { value.sim.running = true; }],
  ['unknown simulator state', value => { value.sim = {}; }],
  ['wrong backend', value => { value.engine.backend = 'Mock'; }],
  ['engine not ready', value => { value.engine.ready = false; }],
  ['engine cannot measure', value => { value.engine.measuring = false; }],
  ['unknown alarms', value => { delete value.cycle.alarms; }],
]) test('published readiness rejects ' + name, () => {
  const observation = publishedReady(); change(observation);
  assert.equal(isPublishedRecipeReady(observation, publishedOptions), false);
});

for (const field of ['id', 'version', 'revisionId']) test('published readiness fails immediately for changed ' + field, () => {
  const observation = publishedReady(); observation.layout[field] = field === 'version' ? 2 : 'other';
  assert.throws(() => isPublishedRecipeReady(observation, publishedOptions), /layout changed/);
});

for (const prefix of ['开不了工', '暂时开不了工']) test('published readiness fails immediately for non-warming target ' + prefix, () => {
  const observation = publishedReady(); observation.cycle.alarms.push('配方 P0-PRESSURE-40 ' + prefix + '：图像引擎损坏');
  assert.throws(() => isPublishedRecipeReady(observation, publishedOptions), /图像引擎损坏/);
});

function readOnlyPage(observations) {
  const commands = []; let index = 0;
  return { commands,
    evaluate(_callback, { command, args }) {
      commands.push({ command, args });
      const observation = observations[index];
      return Promise.resolve(observation[{ cycle_layout: 'layout', cycle_snapshot: 'cycle', cycle_logs: 'logs', engine_status: 'engine', sim_status: 'sim' }[command]]);
    },
    async waitForTimeout(ms) { if (index < observations.length - 1) index++; else await new Promise(accept => setTimeout(accept, ms)); },
  };
}

test('bounded wait polls only read commands and returns full positive evidence', async () => {
  const warming = publishedReady(); warming.logs = []; warming.cycle.alarms.push('配方 P0-PRESSURE-40 开不了工：发布资源与图像引擎正在预热，完成前不能布防');
  const page = readOnlyPage([warming, publishedReady()]);
  const result = await waitForPublishedRecipeReady(page, { ...publishedOptions, timeoutMs: 1000 });
  assert.equal(result.polls, 2); assert.deepEqual(result.layout, publishedLayout);
  assert.deepEqual(result.warmupLog, publishedReady().logs[0]); assert.equal(result.sim.running, false);
  assert.equal(page.commands.length, 10);
  assert.deepEqual(page.commands[0], { command: 'cycle_layout', args: { recipeId: publishedLayout.id, revisionId: publishedLayout.revisionId } });
  assert(page.commands.every(({ command }) => ['cycle_layout', 'cycle_snapshot', 'cycle_logs', 'engine_status', 'sim_status'].includes(command)));
});

test('bounded wait never treats global readiness without target bundle log as completion', async () => {
  const observation = publishedReady(); observation.logs = [];
  await assert.rejects(waitForPublishedRecipeReady(readOnlyPage([observation]), { ...publishedOptions, timeoutMs: 10 }), /did not become ready/);
});

test('bounded wait fails on a real target error without polling again', async () => {
  const observation = publishedReady(); observation.cycle.alarms.push('配方 P0-PRESSURE-40 开不了工：发布包丢失');
  const page = readOnlyPage([observation]);
  await assert.rejects(waitForPublishedRecipeReady(page, publishedOptions), /发布包丢失/);
  assert.equal(page.commands.length, 5);
});

test('bounded wait also limits a stalled native read', async () => {
  const page = { evaluate: () => new Promise(() => {}), waitForTimeout: () => { throw new Error('Unexpected poll'); } };
  await assert.rejects(waitForPublishedRecipeReady(page, { ...publishedOptions, timeoutMs: 10 }), /read exceeded/);
});
for (const [name, ts, expected] of [
  ['older completion log', 999, false],
  ['missing completion timestamp', undefined, false],
  ['null completion timestamp', null, false],
  ['fractional completion timestamp', 1000.5, false],
  ['completion at exact boundary', 1000, true],
  ['newer completion log', 1001, true],
]) test('warmup timestamp lower bound checks ' + name, () => {
  const observation = publishedReady(); observation.logs[0].ts = ts;
  assert.equal(isPublishedRecipeReady(observation, { ...publishedOptions, warmupAfterTs: 1000 }), expected);
});

for (const warmupAfterTs of [-1, null, 0.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1, '1000']) test('invalid warmup timestamp lower bound ' + String(warmupAfterTs) + ' rejects before native reads', async () => {
  assert.throws(() => isPublishedRecipeReady(publishedReady(), { ...publishedOptions, warmupAfterTs }), /nonnegative safe integer/);
  const page = readOnlyPage([publishedReady()]);
  await assert.rejects(waitForPublishedRecipeReady(page, { ...publishedOptions, warmupAfterTs }), /nonnegative safe integer/);
  assert.equal(page.commands.length, 0);
});

test('bounded wait ignores previous warmup and returns a new timestamped completion', async () => {
  const previous = publishedReady(), current = publishedReady(); current.logs[0].ts = 1001;
  const page = readOnlyPage([previous, current]);
  const result = await waitForPublishedRecipeReady(page, { ...publishedOptions, warmupAfterTs: 1001, timeoutMs: 1000 });
  assert.equal(result.polls, 2); assert.equal(result.warmupAfterTs, 1001);
  assert.deepEqual(result.logs, current.logs); assert.deepEqual(result.warmupLog, current.logs[0]);
  assert.deepEqual(result.cycle, current.cycle); assert.deepEqual(result.engine, current.engine); assert.deepEqual(result.sim, current.sim);
});

test('default zero timestamp bound still requires an actual log timestamp', () => {
  const observation = publishedReady(); delete observation.logs[0].ts;
  assert.equal(isPublishedRecipeReady(observation, publishedOptions), false);
  observation.logs[0].ts = 0;
  assert.equal(isPublishedRecipeReady(observation, publishedOptions), true);
});