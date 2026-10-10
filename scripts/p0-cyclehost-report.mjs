import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { distribution, fnv1a64, memoryTrend, validatePart } from '../tests/native/p0-cyclehost-performance.mjs';

const digest = bytes => createHash('sha256').update(bytes).digest('hex');

export function validateCycleHostEvidence(report, rows) {
  assert(report.schemaVersion === 1 && report.completed === true);
  assert(report.physicalValidation === false && report.s7HardwareValidation === false);
  assert(Number.isInteger(report.requestedParts) && report.requestedParts >= 100);
  assert(report.completedParts === report.requestedParts);
  assert(rows.length === report.requestedParts);
  assert(['single', 'tricam'].includes(report.mode) && ['normal', 'gap'].includes(report.scenario));
  const layout = report.provenance.layout, ids = new Set(), failures = [];
  const files = [report.provenance.fixture, report.provenance.executable,
    report.provenance.dll, ...report.provenance.release];
  for (const [index, row] of rows.entries()) {
    assert(row.part === index + 1 && row.scenario === report.scenario);
    assert(!ids.has(row.detail.summary.cycleId), 'Cycle identity repeated');
    ids.add(row.detail.summary.cycleId);
    assert(row.detail.summary.bundleHash === rows[0].detail.summary.bundleHash &&
      row.detail.summary.bundleHash === report.provenance.bundleHash);
    assert(report.provenance.manifest.recipeId === layout.id && report.provenance.manifest.recipeHash === layout.hash);
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
  assert(digest(data) === report.rowsSha256, 'Per-part evidence changed');
  const rows = data.toString('utf8').trimEnd().split('\n').map(line => JSON.parse(line));
  const files = validateCycleHostEvidence(report, rows);
  const manifests = report.provenance.release.filter(file => /[\\/]manifest.json$/.test(file.path));
  assert(manifests.length === 1);
  const manifestBytes = await readFile(manifests[0].path);
  assert(fnv1a64(manifestBytes) === report.provenance.bundleHash);
  assert.deepEqual(JSON.parse(manifestBytes.toString('utf8')), report.provenance.manifest);
  const checked = new Map();
  for (const file of files) {
    assert(isAbsolute(file.path) && /^c:[\\/]/i.test(file.path));
    const key = resolve(file.path).toLowerCase();
    if (!checked.has(key)) {
      const bytes = await readFile(file.path);
      checked.set(key, { bytes: bytes.length, sha256: digest(bytes) });
    }
    assert.deepEqual(checked.get(key), { bytes: file.bytes, sha256: file.sha256 }, 'Artifact changed: ' + file.path);
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

