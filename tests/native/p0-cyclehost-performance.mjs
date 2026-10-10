import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { appendFile, mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { recipeContract } from '../../scripts/robot-plc-demo/camera-bridge.mjs';

const execute = promisify(execFile);
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const positive = value => typeof value === 'number' && Number.isFinite(value) && value >= 0;

export function fnv1a64(bytes) {
  let hash = 0xcbf29ce484222325n;
  for (const byte of bytes) hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * 0x100000001b3n);
  return hash.toString(16).padStart(16, '0');
}

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
  assert(summary.cycleId && summary.bundleHash && summary.recipeHash === layout.hash);
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
      measured.bundleHash === summary.bundleHash && measured.located && !measured.error);
    for (const key of ['ms', 'queueMs', 'engineMs', 'coreMs']) {
      assert(Object.hasOwn(measured, key) && positive(measured[key]), 'Missing real image metric: ' + key);
    }
    const views = row.originals.frames.filter(f => f.k === k).map(f => f.view).sort();
    assert.deepEqual(views, mode === 'tricam' ? [1, 2, 3] : [1]);
    assert(shot.rawFiles.length === views.length && shot.rawFiles.every(f => f.hash));
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
  return { path: resolve(path), bytes: bytes.length, sha256: digest(bytes) };
}

export async function recordedArtifact(root, file) {
  assert(/^c:[\\/]/i.test(root) && !isAbsolute(file) && !file.includes('\\') &&
    !file.includes(':') && file.split('/').every(part => part && part !== '.' && part !== '..'));
  const path = join(root, file), bytes = await readFile(path);
  const header = Buffer.from('P5\n1280 1024\n255\n', 'ascii');
  assert(bytes.subarray(0, header.length).equals(header) && bytes.length === header.length + 1280 * 1024,
    'Recorded image is not a full-resolution 1280x1024 Gray8 PGM');
  return { path: resolve(path), bytes: bytes.length, sha256: digest(bytes), size: [1280, 1024] };
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

export async function sampleProcess(pid, executable, expectedStart) {
  assert(Number.isSafeInteger(pid) && pid > 0 && isAbsolute(executable));
  const ps = "$p = Get-Process -Id " + pid + " -ErrorAction Stop; [pscustomobject]@{pid=$p.Id;path=$p.Path;start=$p.StartTime.ToUniversalTime().ToString('o');workingSetBytes=$p.WorkingSet64;privateBytes=$p.PrivateMemorySize64} | ConvertTo-Json -Compress";
  const { stdout } = await execute('pwsh', ['-NoProfile', '-NonInteractive', '-Command', ps], { windowsHide: true });
  const result = JSON.parse(stdout);
  assert(resolve(result.path).toLowerCase() === resolve(executable).toLowerCase(), 'PID belongs to a different executable');
  if (expectedStart) assert(result.start === expectedStart, 'Native application restarted during the run');
  return result;
}

export async function runCycleHostPerformance(page, options) {
  const { mode, scenario, fixture, releaseDir, executable, pid, output } = options;
  const parts = options.parts ?? 100, timeoutMs = options.timeoutMs ?? 30000;
  assert(['single', 'tricam'].includes(mode) && ['normal', 'gap'].includes(scenario));
  assert(Number.isInteger(parts) && parts >= 100 && parts <= 1000);
  assert(Number.isInteger(timeoutMs) && timeoutMs >= 1000 && timeoutMs <= 120000);
  for (const path of [fixture, releaseDir, executable, output]) assert(isAbsolute(path), 'Use explicit absolute C-drive paths');
  assert([fixture, releaseDir, executable, output].every(path => /^c:[\\/]/i.test(path)), 'This recovery run must not access the D drive');
  assert(!await stat(output).then(() => true, error => { if (error.code === 'ENOENT') return false; throw error; }), 'Evidence directory already exists');
  await mkdir(output, { recursive: true });
  const reportPath = join(output, 'cyclehost-report.json'), rowsPath = join(output, 'parts.jsonl');
  const report = { schemaVersion: 1, startedAt: new Date().toISOString(), passed: false, completed: false,
    scope: 'Actual CycleHost, real LyFlow DLL, generated simulator pixels and development PLC simulator',
    physicalValidation: false, s7HardwareValidation: false, mode, scenario, requestedParts: parts,
    timingDefinitions: {
      partEndToPlcSubmissionMs: 'Persisted drainMs: CycleHost observes partEnd through awaited PLC result/done writes; excludes result ACK and recording completion',
      queueMs: 'Submission through blocking worker entry, including channel, permit and blocking executor wait',
      engineMs: 'Image runner wall time inside worker, including wrapper',
      coreMs: 'Successful native image measurer duration',
      ms: 'Submission through returned measurement',
      uiObservedCycleMs: 'UI click through persisted ACK, complete recording and simulator IDLE observation; polling and UI overhead included' },
    memoryScope: 'Native application main process only, after recording and handshake settle; WebView2 child processes excluded',
    coldScope: 'First part in this script is measured; no claim of cold process, DLL, graph or OS cache',
    accuracyFailures: [], samples: [], rowsArtifact: rowsPath };
  const read = (command, args) => page.evaluate(async ({ command, args }) =>
    window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const processInfo = options.sampleProcess ?? sampleProcess;
  let layout;
  try {
    const guard = { records: await read('records_list'), cameras: await read('camera_rig_config'),
      plc: await read('plc_get_config'), status: await read('plc_get_status'), settings: await read('cycle_get_settings'),
      cycle: await read('cycle_snapshot'), engine: await read('engine_status'), app: await read('app_info') };
    report.guard = guard;
    assert(guard.records.root.includes('com.xyzrobotics.tujiaovision.p0-tests') && /^c:[\\/]/i.test(guard.records.root));
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
      assert(cameras.length === 1 && cameras[0].source === 'sim' && cameras[0].acquisition === 'triggered' &&
        cameras[0].viewCount === (mode === 'tricam' ? 3 : 1));
    }
    const manifestBytes = await readFile(join(releaseDir, 'manifest.json'));
    const manifest = JSON.parse(manifestBytes.toString('utf8'));
    assert(manifest.schemaVersion === 1 && manifest.recipeId === layout.id && manifest.recipeHash === layout.hash);
    assert(manifest.shots.length === layout.shots.length && manifest.shots.every((shot, k) =>
      shot.k === k && shot.shotId === layout.shots[k].id && shot.camera === layout.shots[k].camera &&
      shot.view === layout.shots[k].view && JSON.stringify(shot.size) === '[1280,1024]'));
    report.provenance = { manifest, bundleHash: fnv1a64(manifestBytes), fixture: await artifact(fixture), executable: await artifact(executable),
      dll: await artifact(guard.engine.path), release: await tree(releaseDir), layout };
    assert(report.provenance.release.length > 0);
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
      assert((await read('cycle_snapshot')).phase === 'IDLE' && !(await read('sim_status')).running);
      const previous = (await read('history_query', { query: { recipeId: fixtureDoc.id, limit: 1 } })).items[0]?.id ?? -1;
      const began = Date.now();
      await page.getByRole('button', { name: '运行一件', exact: true }).click();
      const deadline = Date.now() + timeoutMs;
      let row;
      while (Date.now() < deadline) {
        const summary = (await read('history_query', { query: { recipeId: fixtureDoc.id, limit: 1 } })).items[0];
        if (summary?.id > previous) {
          const detail = await read('history_detail', { id: summary.id });
          const originals = await read('workspace_record_images', { historyId: summary.id });
          if (detail.summary.delivery.state === 'acknowledged' && detail.recording.state === 'complete' &&
            originals.complete && !(await read('sim_status')).running && (await read('cycle_snapshot')).phase === 'IDLE') {
            row = { part, scenario, uiObservedCycleMs: Date.now() - began, detail, originals,
              measurements: await read('cycle_part_data') };
            break;
          }
        }
        await page.waitForTimeout(50);
      }
      assert(row, 'No settled, recorded, acknowledged new part within timeout');
      const log = (await read('cycle_logs')).filter(line => line.ts >= began).find(line => line.ev === 'armed↑ busy↑');
      row.armMs = Number(log?.msg.match(/布防耗时 (\d+) ms/)?.[1]);
      row.armingLog = log;
      assert(row.detail.summary.bundleHash === report.provenance.bundleHash, 'Running bundle differs from the hashed release manifest');
      const failures = validatePart(row, layout, mode, scenario);
      assert(!seen.has(row.detail.summary.cycleId), 'Cycle identity reused');
      seen.add(row.detail.summary.cycleId);
      if (rows.length) assert(row.detail.summary.bundleHash === rows[0].detail.summary.bundleHash, 'Frozen bundle changed mid-run');
      report.accuracyFailures.push(...failures.map(failure => ({ part, ...failure })));
      row.recordedArtifacts = [];
      for (const frame of row.originals.frames) {
        row.recordedArtifacts.push({ k: frame.k, view: frame.view,
          ...await recordedArtifact(guard.records.root, frame.file) });
      }
      await appendFile(rowsPath, JSON.stringify(row) + '\n', 'utf8');
      rows.push(row);
      if (part % 10 === 0 || part === parts) {
        const sample = await processInfo(pid, executable, firstProcess.start);
        report.samples.push({ part, elapsedMs: performance.now() - started,
          workingSetBytes: sample.workingSetBytes, privateBytes: sample.privateBytes });
      }
      report.completedParts = rows.length;
      await writeFile(reportPath, JSON.stringify(report, null, 2) + '\n', 'utf8');
    }
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
    report.rowsSha256 = (await artifact(rowsPath)).sha256;
    report.completed = true;
    report.passed = report.accuracyFailures.length === 0;
  } catch (error) {
    report.error = error.stack ?? String(error);
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
    mode: { type: 'string' }, scenario: { type: 'string' }, fixture: { type: 'string' },
    release: { type: 'string' }, executable: { type: 'string' }, pid: { type: 'string' },
    output: { type: 'string' }, parts: { type: 'string', default: '100' } } });
  assert(values['playwright-module'], '--playwright-module must point to an installed Playwright index.mjs');
  const report = await connectAndRun(values['playwright-module'], values.cdp, {
    mode: values.mode, scenario: values.scenario, fixture: values.fixture,
    releaseDir: values.release, executable: values.executable, pid: Number(values.pid),
    output: values.output, parts: Number(values.parts) });
  console.log(JSON.stringify({ completed: report.completed, passed: report.passed,
    completedParts: report.completedParts, accuracyFailures: report.accuracyFailures.length,
    report: join(values.output, 'cyclehost-report.json') }));
  if (!report.passed) process.exitCode = 1;
}
