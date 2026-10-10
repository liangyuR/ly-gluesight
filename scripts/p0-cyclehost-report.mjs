import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { distribution, memoryTrend, scanReplayInputs, validateCameraSource,
  validatePart, validateReplayInputsSnapshot, validateReplayOutputs } from '../tests/native/p0-cyclehost-performance.mjs';


export function validateCycleHostEvidence(report, rows) {
  assert(report.schemaVersion === 1 && report.completed === true);
  assert(report.physicalValidation === false && report.s7HardwareValidation === false);
  assert(Number.isInteger(report.requestedParts) && report.requestedParts >= 100);
  assert(report.completedParts === report.requestedParts);
  assert(rows.length === report.requestedParts);
  assert(['single', 'tricam'].includes(report.mode) && ['normal', 'gap'].includes(report.scenario));
  const source = report.source ?? 'sim';
  assert(['sim', 'replay'].includes(source));
  if (source === 'replay') {
    validateReplayInputsSnapshot(report.replayInputs, report.mode);
    assert.deepEqual(report.replayInputsAfter, report.replayInputs);
    const cameras = report.guard.cameras.filter(camera => camera.id === 'cam1');
    assert(cameras.length === 1);
    validateCameraSource(cameras[0], 'replay', report.mode, report.replayInputs.directory);
    assert(report.replayScenarioSemantics && report.scope.includes('replay control'));
  }
  const layout = report.provenance.layout, ids = new Set(), failures = [];
  if (report.guard.cameras) {
    for (const id of new Set(layout.shots.map(shot => shot.camera))) {
      const cameras = report.guard.cameras.filter(camera => camera.id === id);
      assert(cameras.length === 1);
      validateCameraSource(cameras[0], source, report.mode, report.replayInputs?.directory);
    }
  }
  const files = [report.provenance.fixture, report.provenance.executable,
    report.provenance.dll, ...report.provenance.release];
  if (source === 'replay') files.push(...report.replayInputs.tree, ...report.replayInputs.files);
  for (const [index, row] of rows.entries()) {
    assert(row.part === index + 1 && row.scenario === report.scenario);
    assert(!ids.has(row.detail.summary.cycleId), 'Cycle identity repeated');
    ids.add(row.detail.summary.cycleId);
    assert(row.detail.summary.bundleId === rows[0].detail.summary.bundleId &&
      row.detail.summary.bundleId === report.provenance.bundleId);
    assert(report.provenance.manifest.recipeId === layout.id && report.provenance.manifest.recipeRevision === layout.revisionId);
    failures.push(...validatePart(row, layout, report.mode, report.scenario).map(failure => ({ part: row.part, ...failure })));
    assert(row.recordedArtifacts.length === row.originals.frames.length);
    assert.deepEqual(row.recordedArtifacts.map(f => [f.k, f.view]), row.originals.frames.map(f => [f.k, f.view]));
    for (const [imageIndex, image] of row.recordedArtifacts.entries()) {
      const raw = row.originals.frames[imageIndex];
      assert(resolve(image.path) === resolve(join(report.guard.records.root, raw.file)));
      assert(raw.file.split('/').some(part => part.endsWith('_cycle_' + row.detail.summary.cycleId)));
      assert.deepEqual(image.size, [1280, 1024]);
      assert(image.bytes === 1280 * 1024 + Buffer.byteLength('P5\n1280 1024\n255\n'));
      files.push(image);
    }
    if (source === 'replay') {
      assert.deepEqual(row.replayComparisons, validateReplayOutputs(row, report.replayInputs, layout));
      assert(resolve(row.recordingMetadata.path) === resolve(join(
        report.guard.records.root, row.originals.frames[0].file, '..', 'part.json')));
      files.push(row.recordingMetadata);
    }
  }
  for (const key of ['ms', 'queueMs', 'engineMs', 'coreMs']) {
    assert.deepEqual(report.metrics[key], distribution(rows.flatMap(row => row.measurements.map(m => m[key]))));
  }
  assert.deepEqual(report.metrics.partEndToPlcSubmissionMs, distribution(rows.map(row => row.detail.summary.drainMs)));
  assert.deepEqual(report.metrics.armMs, distribution(rows.map(row => row.armMs)));
  assert.deepEqual(report.metrics.uiObservedCycleMs, distribution(rows.map(row => row.uiObservedCycleMs)));
  const expected = [0, ...Array.from({ length: Math.floor(rows.length / 10) }, (_, i) => (i + 1) * 10)];
  if (expected.at(-1) !== rows.length) expected.push(rows.length);
  assert.deepEqual(report.samples.map(sample => sample.part), expected);
  assert(report.samples.every((sample, index) => Number.isFinite(sample.elapsedMs) &&
    sample.elapsedMs >= 0 && (!index || sample.elapsedMs > report.samples[index - 1].elapsedMs)));
  assert.deepEqual(report.memory.workingSet, memoryTrend(report.samples, 'workingSetBytes'));
  assert.deepEqual(report.memory.private, memoryTrend(report.samples, 'privateBytes'));
  assert.deepEqual(report.accuracyFailures, failures);
  assert(report.passed === (failures.length === 0), 'Report hides non-nominal verdicts');
  return files;
}

export async function verifyCycleHostReport(path) {
  const report = JSON.parse(await readFile(path, 'utf8'));
  assert(isAbsolute(report.rowsArtifact));
  const data = await readFile(report.rowsArtifact);
  const rows = data.toString('utf8').trimEnd().split('\n').map(line => JSON.parse(line));
  const files = validateCycleHostEvidence(report, rows);
  const manifests = report.provenance.release.filter(file => /[\\/]manifest.json$/.test(file.path));
  assert(manifests.length === 1);
  const manifestBytes = await readFile(manifests[0].path);
  assert(JSON.parse(manifestBytes.toString('utf8')).bundleId === report.provenance.bundleId);
  assert.deepEqual(JSON.parse(manifestBytes.toString('utf8')), report.provenance.manifest);
  if ((report.source ?? 'sim') === 'replay') {
    assert.deepEqual(await scanReplayInputs(report.replayInputs.directory, report.mode), report.replayInputs,
      'Replay input tree no longer matches the run');
  }
  const checked = new Set();
  for (const file of files) {
    assert(isAbsolute(file.path) && /^c:[\\/]/i.test(file.path));
    const bytes = await readFile(file.path);
    assert(bytes.length === file.bytes, 'Artifact size differs: ' + file.path);
    if (file.size) {
      const header = Buffer.from('P5\n1280 1024\n255\n');
      assert(bytes.subarray(0, header.length).equals(header) && bytes.length === header.length + 1280 * 1024);
    }
    if (file.document) {
      const actual = JSON.parse(bytes.toString('utf8'));
      assert(actual.cycleId === file.document.cycleId && actual.sn === file.document.sn);
      assert(actual.recipeRevision === file.document.recipeRevision && actual.bundleId === file.document.bundleId);
    }
    checked.add(resolve(file.path).toLowerCase());
  }
  return { valid: true, passed: report.passed, parts: rows.length, mode: report.mode,
    scenario: report.scenario, accuracyFailures: report.accuracyFailures.length, artifacts: checked.size };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert(process.argv.length === 3, 'Usage: node scripts/p0-cyclehost-report.mjs <absolute cyclehost-report.json>');
  const result = await verifyCycleHostReport(process.argv[2]);
  console.log(JSON.stringify(result));
  if (!result.passed) process.exitCode = 1;
}

