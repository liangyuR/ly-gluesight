import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import test from 'node:test';
import { distribution, fnv1a64, memoryTrend, recordedArtifact, validatePart } from './p0-cyclehost-performance.mjs';

function evidence(mode = 'tricam', verdict = 'OK', plcCode = 1) {
  const layout = { id: 'fixture', hash: 'recipe', points: { k: [0, 1, 2, 3] },
    shots: [1, 2, 3, 1].map((view, k) => ({ id: 'P' + (k + 1), camera: 'cam1', view: mode === 'single' ? 1 : view })) };
  const row = { armMs: 10, detail: {
    summary: { cycleId: 'cycle', bundleHash: 'bundle', recipeHash: 'recipe', framesExpected: 4,
      framesReceived: 4, drainMs: 8, sn: 41, verdict, plcCode, faultCode: 0, delivery: { state: 'acknowledged' } },
    triggers: 4, recording: { state: 'complete' },
    shots: layout.shots.map((shot, k) => ({ k, shotId: shot.id, camera: shot.camera, view: shot.view,
      ordinal: k + 1, status: 'done', error: null,
      rawFiles: (mode === 'single' ? [1] : [1, 2, 3]).map(view => ({ view, hash: 'hash' })) })) },
    originals: { complete: true, frames: layout.shots.flatMap((_, k) =>
      (mode === 'single' ? [1] : [1, 2, 3]).map(view => ({ k, view, available: true, error: null }))) },
    measurements: layout.shots.map((shot, k) => ({ k, cycleId: 'cycle', bundleHash: 'bundle', sn: 41,
      shotId: shot.id, camera: shot.camera, located: true, error: null, idx: [k],
      ms: 12, queueMs: 1, engineMs: 11, coreMs: 9 })) };
  return { layout, row };
}

test('nearest-rank latency uses raw samples and accepts measured zero', () => {
  assert.deepEqual(distribution([8, 0, 4, 2]), { count: 4, unit: 'ms', percentile: 'nearest rank',
    min: 0, p50: 2, p95: 8, max: 8, mean: 3.5 });
  for (const values of [[], [null], [undefined], [NaN], [Infinity], [-1]]) assert.throws(() => distribution(values));
});

test('memory trend is fitted against part number, including unequal final interval', () => {
  const samples = [0, 10, 15].map(part => ({ part, bytes: 1000 + part * 20 }));
  const trend = memoryTrend(samples, 'bytes');
  assert(Math.abs(trend.linearSlopeBytesPerPart - 20) < 1e-9);
  delete trend.linearSlopeBytesPerPart;
  assert.deepEqual(trend, { startBytes: 1000, endBytes: 1300,
    deltaBytes: 300, minBytes: 1000, maxBytes: 1300 });
  assert.throws(() => memoryTrend([{ part: 0, bytes: null }, { part: 10, bytes: null }], 'bytes'));
});

for (const mode of ['single', 'tricam']) {
  test(mode + ' settled normal and gap retain actual image metrics', () => {
    const { row, layout } = evidence(mode);
    assert.deepEqual(validatePart(row, layout, mode, 'normal'), []);
    row.detail.summary.verdict = 'NG_GAP';
    row.detail.summary.plcCode = 13;
    assert.deepEqual(validatePart(row, layout, mode, 'gap'), []);
  });
}

test('normal excursion stays an explicit accuracy failure without changing measured data', () => {
  const { row, layout } = evidence('tricam', 'OK_WITH_EXCURSION', 2);
  const before = structuredClone(row);
  assert.deepEqual(validatePart(row, layout, 'tricam', 'normal'),
    [{ expected: ['OK', 1, 0], actual: ['OK_WITH_EXCURSION', 2, 0] }]);
  assert.deepEqual(row, before);
});

const invalid = [
  ['measurement from another cycle', row => row.measurements[1].cycleId = 'other'],
  ['wrong SN', row => row.measurements[1].sn++],
  ['wrong frozen bundle', row => row.measurements[1].bundleHash = 'new'],
  ['missing nullable metric', row => delete row.measurements[1].queueMs],
  ['unknown core duration', row => row.measurements[1].coreMs = null],
  ['incorrect point ownership', row => row.measurements[1].idx = [0]],
  ['missing physical view', row => row.originals.frames.splice(5, 1)],
  ['duplicated physical view', row => row.originals.frames[5].view = 2],
  ['recording still pending', row => row.detail.recording.state = 'pending'],
  ['PLC delivery unconfirmed', row => row.detail.summary.delivery.state = 'submitted'],
  ['wrong device ordinal', row => row.detail.shots[1].ordinal = 1],
  ['arm deadline exceeded', row => row.armMs = 201],
  ['unknown tail duration', row => row.detail.summary.drainMs = null],
];
for (const [name, mutate] of invalid) {
  test('rejects ' + name, () => {
    const { row, layout } = evidence();
    mutate(row);
    assert.throws(() => validatePart(row, layout, 'tricam', 'normal'));
  });
}

test('recorded pixels must be full-resolution raw Gray8; unsafe paths are refused', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'p0-cyclehost-selftest-'));
  try {
    const bytes = Buffer.concat([Buffer.from('P5\n1280 1024\n255\n'), Buffer.alloc(1280 * 1024, 255)]);
    await writeFile(join(directory, 'frame.pgm'), bytes);
    const full = await recordedArtifact(directory, 'frame.pgm');
    assert.deepEqual(full.size, [1280, 1024]);
    assert(full.bytes === bytes.length && /^[a-f0-9]{64}$/.test(full.sha256));
    await writeFile(join(directory, 'frame.pgm'), bytes.subarray(0, bytes.length - 1));
    await assert.rejects(() => recordedArtifact(directory, 'frame.pgm'), /full-resolution/);
    for (const path of ['../frame.pgm', 'x/../../frame.pgm', 'C:/frame.pgm', 'x\\frame.pgm']) {
      await assert.rejects(() => recordedArtifact(directory, path));
    }
  } finally {
    assert(dirname(resolve(directory)) === resolve(tmpdir()) &&
      directory.startsWith(join(tmpdir(), 'p0-cyclehost-selftest-')));
    await rm(directory, { recursive: true, force: true });
  }
});


function reportEvidence() {
  const { row, layout } = evidence('single');
  const root = 'C:/p0-cyclehost-selftest';
  const rows = Array.from({ length: 100 }, (_, index) => {
    const value = structuredClone(row), cycleId = 'cycle' + (index + 1);
    value.part = index + 1;
    value.scenario = 'normal';
    value.uiObservedCycleMs = 2500;
    value.detail.summary.cycleId = cycleId;
    for (const measured of value.measurements) measured.cycleId = cycleId;
    value.originals.frames.forEach(frame => frame.file = 'day/part_cycle_' + cycleId + '/k' + frame.k + '.pgm');
    value.recordedArtifacts = value.originals.frames.map(frame => ({ k: frame.k, view: frame.view,
      path: join(root, frame.file), bytes: 1280 * 1024 + Buffer.byteLength('P5\n1280 1024\n255\n'),
      sha256: '0'.repeat(64), size: [1280, 1024] }));
    return value;
  });
  const samples = Array.from({ length: 11 }, (_, index) => ({ part: index * 10,
    elapsedMs: index * 25000, workingSetBytes: 10000 + index * 100, privateBytes: 8000 + index * 200 }));
  const artifact = { path: join(root, 'fixture.json'), bytes: 0, sha256: '0'.repeat(64) };
  const report = { schemaVersion: 1, completed: true, passed: true, mode: 'single', scenario: 'normal',
    physicalValidation: false, s7HardwareValidation: false, requestedParts: 100, completedParts: 100,
    accuracyFailures: [], guard: { records: { root } }, samples,
    provenance: { layout, bundleHash: 'bundle', manifest: { recipeId: 'fixture', recipeHash: 'recipe' }, fixture: artifact, executable: artifact, dll: artifact, release: [artifact] },
    metrics: Object.fromEntries(['ms', 'queueMs', 'engineMs', 'coreMs'].map(key =>
      [key, distribution(rows.flatMap(value => value.measurements.map(m => m[key])))])),
    memory: { workingSet: memoryTrend(samples, 'workingSetBytes'), private: memoryTrend(samples, 'privateBytes') } };
  report.metrics.armMs = distribution(rows.map(value => value.armMs));
  report.metrics.partEndToPlcSubmissionMs = distribution(rows.map(value => value.detail.summary.drainMs));
  report.metrics.uiObservedCycleMs = distribution(rows.map(value => value.uiObservedCycleMs));
  return { report, rows };
}

test('report validator recomputes all 100 parts, four frame metrics and settled memory', async () => {
  const { validateCycleHostEvidence } = await import('../../scripts/p0-cyclehost-report.mjs');
  const { report, rows } = reportEvidence();
  assert.equal(validateCycleHostEvidence(report, rows).length, 404);
});

for (const [name, mutate] of [
  ['fabricated percentile', report => report.metrics.queueMs.p95++],
  ['fabricated memory slope', report => report.memory.private.linearSlopeBytesPerPart++],
  ['missing tenth-part memory sample', report => report.samples.splice(4, 1)],
  ['hidden accuracy failure', report => report.accuracyFailures.push({ part: 1 })],
  ['wrong overall pass status', report => report.passed = false],
  ['false hardware validation', report => report.s7HardwareValidation = true],
  ['incomplete run relabelled complete', report => report.completedParts--],
  ['borrowed image from another part', (_report, rows) => rows[2].recordedArtifacts[0].path = rows[1].recordedArtifacts[0].path],
  ['duplicate cycle identity', (_report, rows) => rows[2].detail.summary.cycleId = rows[1].detail.summary.cycleId],
]) {
  test('report validator refuses ' + name, async () => {
    const { validateCycleHostEvidence } = await import('../../scripts/p0-cyclehost-report.mjs');
    const { report, rows } = reportEvidence();
    mutate(report, rows);
    assert.throws(() => validateCycleHostEvidence(report, rows));
  });
}


test('manifest fingerprint uses the release FNV algorithm', () => {
  assert.equal(fnv1a64(Buffer.from('hello')), 'a430d84680aabd0b');
});
