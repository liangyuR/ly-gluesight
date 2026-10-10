import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { appendFile, mkdir, readFile, readdir, realpath, stat, writeFile } from 'node:fs/promises';
import { basename, dirname, isAbsolute, join, resolve, win32 } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { recipeContract } from '../../scripts/robot-plc-demo/camera-bridge.mjs';

const execute = promisify(execFile);
const positive = value => typeof value === 'number' && Number.isFinite(value) && value >= 0;


export function distribution(values) {
  assert(values.length > 0 && values.every(positive), 'Missing or invalid timing samples');
  const sorted = [...values].sort((a, b) => a - b);
  return { count: sorted.length, unit: 'ms', percentile: 'nearest rank',
    min: sorted[0], p50: sorted[Math.ceil(sorted.length * .5) - 1],
    p95: sorted[Math.ceil(sorted.length * .95) - 1], max: sorted.at(-1),
    mean: sorted.reduce((sum, value) => sum + value, 0) / sorted.length };
}

export function memoryTrend(samples, key) {
  assert(samples.length >= 2 && samples.every(s => Number.isInteger(s[key]) && s[key] > 0));
  const values = samples.map(s => s[key]);
  const x = samples.reduce((sum, s) => sum + s.part, 0) / samples.length;
  const y = values.reduce((sum, value) => sum + value, 0) / values.length;
  const denominator = samples.reduce((sum, s) => sum + (s.part - x) ** 2, 0);
  assert(denominator > 0);
  return { startBytes: values[0], endBytes: values.at(-1), deltaBytes: values.at(-1) - values[0],
    minBytes: Math.min(...values), maxBytes: Math.max(...values),
    linearSlopeBytesPerPart: samples.reduce((sum, s) => sum + (s.part - x) * (s[key] - y), 0) / denominator };
}

export function validatePart(row, layout, mode, scenario) {
  const summary = row.detail.summary, n = layout.shots.length;
  assert(summary.cycleId && summary.bundleId && summary.recipeRevision === layout.revisionId);
  assert(summary.framesExpected === n && summary.framesReceived === n && row.detail.triggers === n);
  assert(summary.delivery.state === 'acknowledged' && row.detail.recording.state === 'complete');
  assert(positive(summary.drainMs), 'CycleHost did not persist partEnd-to-PLC timing');
  assert(row.detail.shots.length === n && row.measurements.length === n);
  assert(row.originals.complete && row.originals.frames.length === n * (mode === 'tricam' ? 3 : 1));
  assert(row.originals.frames.every(f => f.available && !f.error));
  for (let k = 0; k < n; k++) {
    const planned = layout.shots[k], shot = row.detail.shots[k];
    const measured = row.measurements.find(m => m.k === k);
    assert(shot.k === k && shot.shotId === planned.id && shot.camera === planned.camera &&
      shot.view === planned.view && shot.status === 'done' && !shot.error &&
      shot.ordinal === layout.shots.slice(0, k + 1).filter(s => s.camera === planned.camera).length);
    assert(measured && measured.cycleId === summary.cycleId && measured.sn === summary.sn &&
      measured.shotId === planned.id && measured.camera === planned.camera &&
      measured.bundleId === summary.bundleId && measured.located && !measured.error);
    for (const key of ['ms', 'queueMs', 'engineMs', 'coreMs']) {
      assert(Object.hasOwn(measured, key) && positive(measured[key]), 'Missing real image metric: ' + key);
    }
    const views = row.originals.frames.filter(f => f.k === k).map(f => f.view).sort();
    assert.deepEqual(views, mode === 'tricam' ? [1, 2, 3] : [1]);
    assert(shot.rawFiles.length === views.length && shot.rawFiles.every(f => typeof f.file === 'string'));
    const indices = layout.points.k.flatMap((owner, index) => owner === k ? [index] : []);
    assert.deepEqual(measured.idx, indices, 'Missing or reordered per-shot measurement points');
  }
  assert(positive(row.armMs) && row.armMs <= 200, 'Arming exceeded the unchanged 200 ms requirement');
  const expected = scenario === 'gap' ? ['NG_GAP', 13, 0] : ['OK', 1, 0];
  const actual = [summary.verdict, summary.plcCode, summary.faultCode];
  return JSON.stringify(actual) === JSON.stringify(expected) ? [] : [{ expected, actual }];
}

async function artifact(path) {
  const bytes = await readFile(path);
  return { path: resolve(path), bytes: bytes.length };
}

export async function recordedArtifact(root, file) {
  assert(/^c:[\\/]/i.test(root) && !isAbsolute(file) && !file.includes('\\') &&
    !file.includes(':') && file.split('/').every(part => part && part !== '.' && part !== '..'));
  const path = join(root, file), bytes = await readFile(path);
  const header = Buffer.from('P5\n1280 1024\n255\n', 'ascii');
  assert(bytes.subarray(0, header.length).equals(header) && bytes.length === header.length + 1280 * 1024,
    'Recorded image is not a full-resolution 1280x1024 Gray8 PGM');
  return { path: resolve(path), bytes: bytes.length,
    size: [1280, 1024] };
}

async function tree(directory) {
  const output = [];
  for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
    const path = join(directory, entry.name);
    assert(!entry.isSymbolicLink(), 'Frozen release must not contain symlinks: ' + path);
    if (entry.isDirectory()) output.push(...await tree(path));
    else if (entry.isFile()) output.push(await artifact(path));
  }
  return output;
}

export function validateCameraSource(camera, source, mode, replayDirectory) {
  assert(['sim', 'replay'].includes(source));
  assert(camera.source === source && camera.acquisition === 'triggered' &&
    camera.viewCount === (mode === 'tricam' ? 3 : 1), 'Camera source, acquisition or view count differs');
  if (source === 'replay') {
    assert(camera.id === 'cam1' && camera.replayChannel === 1, 'Replay control requires cam1/channel 1');
    assert(typeof camera.replayDir === 'string' && /^c:[\\/]/i.test(camera.replayDir) &&
      resolve(camera.replayDir).toLowerCase() === resolve(replayDirectory).toLowerCase(),
      'Camera replay directory differs from the explicit input directory');
  }
}

export async function scanReplayInputs(directory, mode) {
  assert(isAbsolute(directory) && /^c:[\\/]/i.test(directory) && ['single', 'tricam'].includes(mode));
  const root = await realpath(directory);
  assert(/^c:[\\/]/i.test(root), 'Replay directory resolves outside the C drive');
  const entries = await readdir(root, { withFileTypes: true });
  assert(entries.every(entry => !entry.isSymbolicLink()), 'Replay input tree must not contain symlinks');
  const files = [];
  for (let k = 0; k < 4; k++) {
    for (let view = 1; view <= (mode === 'tricam' ? 3 : 1); view++) {
      const file = 'cam1_' + (k + 1) + '_v' + view + '.pgm';
      files.push({ k, view, camera: 'cam1', file, ...await recordedArtifact(root, file) });
    }
  }
  const names = files.map(file => file.file).sort();
  const actual = entries
    .filter(entry => /\.(pgm|jpg|jpeg|png|bmp|tif|tiff)$/i.test(entry.name)).map(entry => entry.name).sort();
  assert.deepEqual(actual, names, 'Replay directory contains missing, extra or ambiguously named images');
  const snapshot = { directory: root, channel: 1, mode, files, tree: await tree(root) };
  validateReplayInputsSnapshot(snapshot, mode);
  return snapshot;
}

export function validateReplayInputsSnapshot(inputs, mode) {
  assert(inputs.mode === mode && inputs.channel === 1 && /^c:[\\/]/i.test(inputs.directory));
  const expected = [];
  for (let k = 0; k < 4; k++) {
    for (let view = 1; view <= (mode === 'tricam' ? 3 : 1); view++) expected.push([k, view]);
  }
  assert.deepEqual(inputs.files.map(file => [file.k, file.view]), expected);
  for (const file of inputs.files) {
    assert(file.camera === 'cam1' && file.file === 'cam1_' + (file.k + 1) + '_v' + file.view + '.pgm');
    assert(resolve(file.path) === resolve(join(inputs.directory, file.file)) &&
      JSON.stringify(file.size) === '[1280,1024]' && file.bytes === 1280 * 1024 + Buffer.byteLength('P5\n1280 1024\n255\n'));
    const entries = inputs.tree.filter(entry => resolve(entry.path) === resolve(file.path));
    assert(entries.length === 1 && entries[0].bytes === file.bytes);
  }
  const imageNames = inputs.tree.filter(file => /\.(pgm|jpg|jpeg|png|bmp|tif|tiff)$/i.test(file.path))
    .map(file => basename(file.path)).sort();
  assert.deepEqual(imageNames, inputs.files.map(file => file.file).sort());
}

export function validateReplayOutputs(row, inputs, layout) {
  assert(inputs.mode === 'single' || inputs.mode === 'tricam');
  assert(row.recordedArtifacts.length === inputs.files.length);
  const metadata = row.recordingMetadata.document, summary = row.detail.summary;
  assert(metadata.available === true && metadata.cycleId === summary.cycleId && metadata.sn === summary.sn &&
    metadata.bundleId === summary.bundleId && metadata.recipeRevision === summary.recipeRevision);
  assert(metadata.errors.length === 0 && metadata.missingShots.length === 0 && metadata.droppedFrames === 0);
  assert(metadata.frames.length === inputs.files.length);
  const comparisons = [];
  for (const input of inputs.files) {
    const outputs = row.recordedArtifacts.filter(file => file.k === input.k && file.view === input.view);
    assert(outputs.length === 1);
    const output = outputs[0], shot = row.detail.shots[input.k], planned = layout.shots[input.k];
    assert.deepEqual(output.size, input.size);
    const frames = metadata.frames.filter(frame => frame.k === input.k && frame.view === input.view);
    assert(frames.length === 1);
    const frame = frames[0];
    assert(frame.counter === 'synthetic' && frame.manual === false && frame.lostPackets === 0 &&
      frame.width === 1280 && frame.height === 1024 && frame.available === true && !frame.error);
    assert(frame.cycleId === summary.cycleId && frame.bundleId === summary.bundleId &&
      frame.recipeRevision === summary.recipeRevision && frame.camera === input.camera &&
      frame.shotId === planned.id && frame.selectedView === planned.view &&
      frame.session === shot.session && frame.ordinal === shot.ordinal &&
      frame.frameCounter === shot.frameCounter && frame.triggerCounter === shot.triggerCounter &&
      frame.frameCounter === frame.triggerCounter && Number.isSafeInteger(frame.frameCounter) && frame.frameCounter > 0);
    assert(basename(output.path) === frame.file);
    comparisons.push({ k: input.k, view: input.view, inputPath: input.path, outputPath: output.path,
      size: output.size, identityVerified: true });
  }
  return comparisons;
}

async function recordingMetadata(root, frames) {
  const directories = new Set(frames.map(frame => dirname(frame.file)));
  assert(directories.size === 1, 'Recorded frames span multiple part directories');
  const path = join(root, [...directories][0], 'part.json'), bytes = await readFile(path);
  return { path: resolve(path), bytes: bytes.length, document: JSON.parse(bytes.toString('utf8')) };
}

async function enrichFailedAttempt(attempt, root) {
  const observed = attempt.row ?? attempt.lastPoll;
  const frames = observed?.originals?.frames;
  if (typeof root !== 'string' || !/^c:[\\/]/i.test(root) || !Array.isArray(frames) || !frames.length) return;
  observed.recordedArtifacts ??= [];
  const errors = [];
  for (const frame of frames) {
    if (observed.recordedArtifacts.some(image => image.k === frame.k && image.view === frame.view)) continue;
    try {
      observed.recordedArtifacts.push({ k: frame.k, view: frame.view, ...await recordedArtifact(root, frame.file) });
    } catch (error) {
      errors.push({ k: frame.k, view: frame.view, error: String(error) });
    }
  }
  if (!observed.recordingMetadata) {
    try {
      observed.recordingMetadata = await recordingMetadata(root, frames);
    } catch (error) {
      errors.push({ metadata: true, error: String(error) });
    }
  }
  if (errors.length) attempt.evidenceReadErrors = errors;
}

export async function persistFailedAttempt(output, attempt, error, completedParts) {
  const path = join(output, 'failed-attempt.json');
  const document = { schemaVersion: 1, failedAt: new Date().toISOString(), completedParts,
    attempt, error: error.stack ?? String(error) };
  const bytes = Buffer.from(JSON.stringify(document, null, 2) + '\n');
  await writeFile(path, bytes, { flag: 'wx' });
  return { part: attempt.part, stage: attempt.stage, accepted: attempt.accepted,
    artifact: { path: resolve(path), bytes: bytes.length } };
}

export async function sampleProcess(pid, executable, expectedStart) {
  assert(Number.isSafeInteger(pid) && pid > 0 && isAbsolute(executable));
  const ps = "$p = Get-Process -Id " + pid + " -ErrorAction Stop; [pscustomobject]@{pid=$p.Id;path=$p.Path;start=$p.StartTime.ToUniversalTime().ToString('o');workingSetBytes=$p.WorkingSet64;privateBytes=$p.PrivateMemorySize64} | ConvertTo-Json -Compress";
  const { stdout } = await execute('pwsh', ['-NoProfile', '-NonInteractive', '-Command', ps], { windowsHide: true });
  const result = JSON.parse(stdout);
  assert(resolve(result.path).toLowerCase() === resolve(executable).toLowerCase(), 'PID belongs to a different executable');
  if (expectedStart) assert(result.start === expectedStart, 'Native application restarted during the run');
  return result;
}

export function validateTestRecordsRoot(recordsRoot, identifier = 'com.xyzrobotics.tujiaovision.p0-tests.performance', appdata = process.env.APPDATA) {
  assert(['com.xyzrobotics.tujiaovision.p0-tests.performance', 'com.xyzrobotics.tujiaovision.p0-tests.performance.nohash'].includes(identifier), 'Only explicit isolated performance profiles are accepted');
  assert(typeof appdata === 'string' && /^c:[\\/]/i.test(appdata) && win32.isAbsolute(appdata), 'APPDATA must be an absolute C-drive path');
  assert(typeof recordsRoot === 'string' && /^c:[\\/]/i.test(recordsRoot) && win32.isAbsolute(recordsRoot), 'Actual records root must be on C:');
  const expected = win32.join(win32.resolve(appdata), identifier, 'records');
  assert.equal(win32.resolve(recordsRoot).toLowerCase(), expected.toLowerCase(), 'Actual records root differs from the current explicit isolated profile');
  return expected;
}

export async function runCycleHostPerformance(page, options) {
  const { mode, scenario, fixture, releaseDir, executable, pid, output } = options;
  const parts = options.parts ?? 100, timeoutMs = options.timeoutMs ?? 30000;
  const source = options.source ?? 'sim';
  const now = options.now ?? Date.now;
  assert(['sim', 'replay'].includes(source), 'Only explicit simulator or replay controls are supported');
  assert(source === 'replay' ? typeof options.replayDir === 'string' : options.replayDir === undefined,
    '--replay-dir is required only for --source replay');
  assert(['single', 'tricam'].includes(mode) && ['normal', 'gap'].includes(scenario));
  assert(Number.isInteger(parts) && parts >= 100 && parts <= 1000);
  assert(Number.isInteger(timeoutMs) && timeoutMs >= 1000 && timeoutMs <= 120000);
  for (const path of [fixture, releaseDir, executable, output]) assert(isAbsolute(path), 'Use explicit absolute C-drive paths');
  assert([fixture, releaseDir, executable, output].every(path => /^c:[\\/]/i.test(path)), 'This recovery run must not access the D drive');
  assert(!await stat(output).then(() => true, error => { if (error.code === 'ENOENT') return false; throw error; }), 'Evidence directory already exists');
  await mkdir(output, { recursive: true });
  const reportPath = join(output, 'cyclehost-report.json'), rowsPath = join(output, 'parts.jsonl');
  const report = { schemaVersion: 1, startedAt: new Date().toISOString(), passed: false, completed: false,
    scope: source === 'replay'
      ? 'Actual CycleHost, real LyFlow DLL, explicit replay control pixels, Synthetic counters and development PLC simulator'
      : 'Actual CycleHost, real LyFlow DLL, generated simulator pixels and development PLC simulator',
    physicalValidation: false, s7HardwareValidation: false, source, mode, scenario, requestedParts: parts,
    replayScenarioSemantics: source === 'replay'
      ? 'Scenario selects the expected judgement only; replay pixels come entirely from the explicit input directory. No random pose, loss or locate-failure injection is claimed.' : null,
    timingDefinitions: {
      partEndToPlcSubmissionMs: 'Persisted drainMs: CycleHost observes partEnd through awaited PLC result/done writes; excludes result ACK and recording completion',
      queueMs: 'Submission through blocking worker entry, including channel, permit and blocking executor wait',
      engineMs: 'Image runner wall time inside worker, including wrapper',
      coreMs: 'Successful native image measurer duration',
      ms: 'Submission through returned measurement',
      uiObservedCycleMs: 'UI click through persisted ACK, complete recording and simulator IDLE observation; polling and UI overhead included' },
    memoryScope: 'Native application main process only, after recording and handshake settle; WebView2 child processes excluded',
    coldScope: 'First part in this script is measured; no claim of cold process, DLL, graph or OS cache',
    accuracyFailures: [], samples: [], completedParts: 0, rowsArtifact: rowsPath };
  const read = (command, args) => page.evaluate(async ({ command, args }) =>
    window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const processInfo = options.sampleProcess ?? sampleProcess;
  let layout;
  let attempt = { part: null, stage: 'initialGuard', accepted: false, lastPoll: null, row: null };
  try {
    const guard = { records: await read('records_list'), cameras: await read('camera_rig_config'),
      plc: await read('plc_get_config'), status: await read('plc_get_status'), settings: await read('cycle_get_settings'),
      cycle: await read('cycle_snapshot'), engine: await read('engine_status'), app: await read('app_info') };
    report.guard = guard;
    const identifier = options.identifier ?? 'com.xyzrobotics.tujiaovision.p0-tests.performance';
    report.recordsRoot = validateTestRecordsRoot(guard.records.root, identifier);
    report.identifier = identifier;
    if (guard.app.identifier !== undefined) assert.equal(guard.app.identifier, identifier);
    assert(guard.app.version && guard.engine.version);
    assert(guard.plc.connection.protocol === 'simulator' && guard.status.state === 'connected' && guard.cycle.phase === 'IDLE');
    assert(guard.settings.timeouts.armMs === 200 && guard.settings.vision === true && guard.settings.record === 'all');
    assert(guard.settings.recordKeep >= parts, 'Recording retention cannot preserve the whole requested run');
    const requiredBytes = parts * 4 * (mode === 'tricam' ? 3 : 1) * (1280 * 1024 + 64);
    assert(guard.settings.recordMaxGb * 1024 ** 3 > requiredBytes * 1.1, 'Recording byte budget is too small for the run');
    assert(guard.engine.backend === 'LyFlow' && guard.engine.ready && guard.engine.measuring && /^c:[\\/]/i.test(guard.engine.path));
    const fixtureDoc = JSON.parse(await readFile(fixture, 'utf8'));
    layout = await read('cycle_layout', { recipeId: fixtureDoc.id });
    assert.deepEqual(recipeContract(layout), recipeContract(fixtureDoc), 'Published fixture contract differs');
    assert(layout.shots.length === 4 && new Set(layout.shots.map(s => s.camera)).size === 1);
    assert.deepEqual(layout.shots.map(s => s.view), mode === 'tricam' ? [1, 2, 3, 1] : [1, 1, 1, 1]);
    for (const cameraId of new Set(layout.shots.map(s => s.camera))) {
      const cameras = guard.cameras.filter(c => c.id === cameraId);
      assert(cameras.length === 1);
      validateCameraSource(cameras[0], source, mode, options.replayDir);
      if (source === 'replay') assert((await realpath(cameras[0].replayDir)).toLowerCase() ===
        (await realpath(options.replayDir)).toLowerCase());
    }
    const manifestBytes = await readFile(join(releaseDir, 'manifest.json'));
    const manifest = JSON.parse(manifestBytes.toString('utf8'));
    assert(manifest.schemaVersion === 1 && manifest.recipeId === layout.id && manifest.recipeRevision === layout.revisionId);
    assert(manifest.shots.length === layout.shots.length && manifest.shots.every((shot, k) =>
      shot.k === k && shot.shotId === layout.shots[k].id && shot.camera === layout.shots[k].camera &&
      shot.view === layout.shots[k].view && JSON.stringify(shot.size) === '[1280,1024]'));
    report.provenance = { manifest, bundleId: manifest.bundleId, fixture: await artifact(fixture), executable: await artifact(executable),
      dll: await artifact(guard.engine.path), release: await tree(releaseDir), layout };
    assert(report.provenance.release.length > 0);
    if (source === 'replay') report.replayInputs = await scanReplayInputs(options.replayDir, mode);
    await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: '在线检测', exact: true }).click();
    await page.getByRole('combobox', { name: '模拟配方', exact: true }).selectOption(fixtureDoc.id);
    await page.getByRole('combobox', { name: '模拟工况', exact: true }).selectOption(scenario);
    const started = performance.now();
    const firstProcess = await processInfo(pid, executable);
    report.process = { pid, path: firstProcess.path, start: firstProcess.start };
    report.samples.push({ part: 0, elapsedMs: performance.now() - started,
      workingSetBytes: firstProcess.workingSetBytes, privateBytes: firstProcess.privateBytes });
    const rows = [], seen = new Set();
    for (let part = 1; part <= parts; part++) {
      attempt = { part, stage: 'partPreflight', accepted: false, lastPoll: null, row: null };
      assert((await read('cycle_snapshot')).phase === 'IDLE' && !(await read('sim_status')).running);
      if (source === 'replay') assert.deepEqual(await read('camera_rig_config'), guard.cameras,
        'Camera configuration changed during replay control run');
      const previous = (await read('history_query', { query: { recipeId: fixtureDoc.id, limit: 1 } })).items[0]?.id ?? -1;
      const began = now();
      Object.assign(attempt, { startedAt: began, previousHistoryId: previous, stage: 'clickRun' });
      await page.getByRole('button', { name: '运行一件', exact: true }).click();
      const deadline = now() + timeoutMs;
      attempt.stage = 'pollResult';
      let row;
      while (now() < deadline) {
        attempt.lastPoll = { ...attempt.lastPoll, pollStartedAt: now() };
        attempt.lastPoll.snapshot = await read('cycle_snapshot');
        attempt.lastPoll.snapshotObservedAt = now();
        attempt.lastPoll.simulator = await read('sim_status');
        attempt.lastPoll.simulatorObservedAt = now();
        const summary = (await read('history_query', { query: { recipeId: fixtureDoc.id, limit: 1 } })).items[0];
        attempt.lastPoll.summary = summary ?? null;
        if (summary?.id > previous) {
          const detail = await read('history_detail', { id: summary.id });
          attempt.lastPoll.detail = detail;
          const originals = await read('workspace_record_images', { historyId: summary.id });
          attempt.lastPoll.originals = originals;
          const measurements = await read('cycle_part_data');
          attempt.lastPoll.measurements = measurements;
          if (detail.summary.delivery.state === 'acknowledged' && detail.recording.state === 'complete' &&
            originals.complete && !attempt.lastPoll.simulator.running && attempt.lastPoll.snapshot.phase === 'IDLE') {
            row = { part, scenario, uiObservedCycleMs: now() - began, detail, originals, measurements };
            attempt.row = row;
            break;
          }
        }
        await page.waitForTimeout(50);
      }
      assert(row, 'No settled, recorded, acknowledged new part within timeout');
      attempt.stage = 'partValidation';
      const log = (await read('cycle_logs')).filter(line => line.ts >= began).find(line => line.ev === 'armed↑ busy↑');
      row.armMs = Number(log?.msg.match(/布防耗时 (\d+) ms/)?.[1]);
      row.armingLog = log;
      assert(row.detail.summary.bundleId === report.provenance.bundleId, 'Running bundle differs from the explicit release ID');
      const failures = validatePart(row, layout, mode, scenario);
      assert(!seen.has(row.detail.summary.cycleId), 'Cycle identity reused');
      seen.add(row.detail.summary.cycleId);
      if (rows.length) assert(row.detail.summary.bundleId === rows[0].detail.summary.bundleId, 'Frozen bundle changed mid-run');
      report.accuracyFailures.push(...failures.map(failure => ({ part, ...failure })));
      attempt.stage = 'readRecordedImages';
      row.recordedArtifacts = [];
      for (const frame of row.originals.frames) {
        row.recordedArtifacts.push({ k: frame.k, view: frame.view,
          ...await recordedArtifact(guard.records.root, frame.file) });
      }
      if (source === 'replay') {
        attempt.stage = 'readRecordingMetadata';
        row.recordingMetadata = await recordingMetadata(guard.records.root, row.originals.frames);
        attempt.stage = 'replayValidation';
        row.replayComparisons = validateReplayOutputs(row, report.replayInputs, layout);
      }
      attempt.stage = 'persistAcceptedPart';
      await appendFile(rowsPath, JSON.stringify(row) + '\n', 'utf8');
      rows.push(row);
      attempt.accepted = true;
      report.completedParts = rows.length;
      if (part % 10 === 0 || part === parts) {
        attempt.stage = 'sampleMemory';
        const sample = await processInfo(pid, executable, firstProcess.start);
        report.samples.push({ part, elapsedMs: performance.now() - started,
          workingSetBytes: sample.workingSetBytes, privateBytes: sample.privateBytes });
      }
      attempt.stage = 'reportCheckpoint';
      await writeFile(reportPath, JSON.stringify(report, null, 2) + '\n', 'utf8');
    }
    attempt.stage = 'finalVerification';
    report.metrics = Object.fromEntries(['ms', 'queueMs', 'engineMs', 'coreMs'].map(key =>
      [key, distribution(rows.flatMap(row => row.measurements.map(m => m[key])))]));
    report.metrics.partEndToPlcSubmissionMs = distribution(rows.map(row => row.detail.summary.drainMs));
    report.metrics.armMs = distribution(rows.map(row => row.armMs));
    report.metrics.uiObservedCycleMs = distribution(rows.map(row => row.uiObservedCycleMs));
    report.memory = { workingSet: memoryTrend(report.samples, 'workingSetBytes'), private: memoryTrend(report.samples, 'privateBytes') };
    assert.deepEqual(await tree(releaseDir), report.provenance.release, 'Frozen release files changed');
    assert.deepEqual(await artifact(fixture), report.provenance.fixture, 'Fixture changed');
    assert.deepEqual(await artifact(executable), report.provenance.executable, 'Application executable changed');
    assert.deepEqual(await artifact(guard.engine.path), report.provenance.dll, 'Native DLL changed');
    if (source === 'replay') {
      report.replayInputsAfter = await scanReplayInputs(options.replayDir, mode);
      assert.deepEqual(report.replayInputsAfter, report.replayInputs, 'Replay input tree changed during run');
      assert.deepEqual(await read('camera_rig_config'), guard.cameras, 'Replay camera configuration changed');
    }
    report.completed = true;
    report.passed = report.accuracyFailures.length === 0;
  } catch (error) {
    report.error = error.stack ?? String(error);
    try {
      await enrichFailedAttempt(attempt, report.guard?.records?.root);
      report.failedAttempt = await persistFailedAttempt(output, attempt, error, report.completedParts);
    } catch (persistenceError) {
      report.failedAttemptPersistenceError = persistenceError.stack ?? String(persistenceError);
    }
    throw error;
  } finally {
    report.finishedAt = new Date().toISOString();
    await writeFile(reportPath, JSON.stringify(report, null, 2) + '\n', 'utf8');
  }
  return report;
}

export async function connectAndRun(modulePath, endpoint, options) {
  const address = new URL(endpoint);
  assert(address.protocol === 'http:' && ['127.0.0.1', 'localhost'].includes(address.hostname), 'Only local native CDP is allowed');
  const runtime = await import(pathToFileURL(resolve(modulePath)).href);
  const browser = await (runtime.chromium ?? runtime.default?.chromium).connectOverCDP(endpoint);
  try {
    const pages = browser.contexts().flatMap(context => context.pages());
    const candidates = [];
    for (const page of pages) {
      if (await page.evaluate(() => Boolean(window.__TAURI_INTERNALS__)).catch(() => false)) candidates.push(page);
    }
    assert(candidates.length === 1, 'Expected exactly one native Tauri page');
    return await runCycleHostPerformance(candidates[0], options);
  } finally {
    await browser.close();
  }
}


if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { values } = parseArgs({ options: {
    'playwright-module': { type: 'string' }, cdp: { type: 'string', default: 'http://127.0.0.1:9338' },
    identifier: { type: 'string', default: 'com.xyzrobotics.tujiaovision.p0-tests.performance' },
    source: { type: 'string', default: 'sim' }, 'replay-dir': { type: 'string' },
    mode: { type: 'string' }, scenario: { type: 'string' }, fixture: { type: 'string' },
    release: { type: 'string' }, executable: { type: 'string' }, pid: { type: 'string' },
    output: { type: 'string' }, parts: { type: 'string', default: '100' } } });
  assert(values['playwright-module'], '--playwright-module must point to an installed Playwright index.mjs');
  const report = await connectAndRun(values['playwright-module'], values.cdp, {
    mode: values.mode, scenario: values.scenario, fixture: values.fixture,
    identifier: values.identifier, source: values.source, replayDir: values['replay-dir'],
    releaseDir: values.release, executable: values.executable, pid: Number(values.pid),
    output: values.output, parts: Number(values.parts) });
  console.log(JSON.stringify({ completed: report.completed, passed: report.passed,
    completedParts: report.completedParts, accuracyFailures: report.accuracyFailures.length,
    report: join(values.output, 'cyclehost-report.json') }));
  if (!report.passed) process.exitCode = 1;
}
