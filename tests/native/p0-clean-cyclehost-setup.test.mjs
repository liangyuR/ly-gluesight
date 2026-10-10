import assert from 'node:assert/strict';
import test from 'node:test';
import { isQuiescentCycleSnapshot } from './p0-clean-cyclehost-setup.mjs';

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
