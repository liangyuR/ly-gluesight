import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { lstat, mkdir, readFile, readdir, realpath, writeFile } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { configureReplay, teachCleanFixture, isQuiescentCycleSnapshot } from './p0-clean-cyclehost-setup.mjs';
import { runCycleHostPerformance, sampleProcess, scanReplayInputs } from './p0-cyclehost-performance.mjs';
import { verifyCycleHostReport } from '../../scripts/p0-cyclehost-report.mjs';

const identifier = 'com.xyzrobotics.tujiaovision.p0-tests.performance';
const cPath = value => { assert.equal(typeof value, 'string'); const path = resolve(value); assert(/^C:\\/i.test(path), 'Explicit C: path required: ' + path); return path; };
const json = async path => JSON.parse((await readFile(cPath(path), 'utf8')).replace(/^\uFEFF/, ''));
const sleep = ms => new Promise(done => setTimeout(done, ms));
async function absent(path) { try { await lstat(path); return false; } catch (error) { if (error.code === 'ENOENT') return true; throw error; } }
async function until(check, label, timeout = 90000) {
  const deadline = Date.now() + timeout;
  do { const value = await check(); if (value) return value; await sleep(100); } while (Date.now() < deadline);
  throw new Error('Timed out: ' + label);
}
async function bounded(promise, ms, label) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(label)), ms); })]); }
  finally { clearTimeout(timer); }
}
async function fileArtifact(path) {
  const metadata = await lstat(path);
  assert(metadata.isFile() && !metadata.isSymbolicLink(), 'Regular file required: ' + path);
  const actual = cPath(await realpath(path));
  return { path: cPath(path), resolvedPath: actual, bytes: metadata.size };
}
const { values } = parseArgs({ options: {
  exe: { type: 'string' }, manifest: { type: 'string' }, settings: { type: 'string' }, cameras: { type: 'string' },
  plc: { type: 'string' }, inputs: { type: 'string' }, output: { type: 'string' }, playwright: { type: 'string' },
  'debug-port': { type: 'string', default: '9344' },
} });
for (const key of ['exe', 'manifest', 'settings', 'cameras', 'plc', 'inputs', 'output', 'playwright']) assert(values[key], 'Missing --' + key);
const paths = Object.fromEntries(Object.entries(values).filter(([key]) => key !== 'debug-port').map(([key, value]) => [key, cPath(value)]));
const profile = join(cPath(process.env.APPDATA), identifier), recordsRoot = join(profile, 'records');
const repository = cPath(resolve(dirname(fileURLToPath(import.meta.url)), '..', '..'));
const debugPort = Number(values['debug-port']);
assert(Number.isInteger(debugPort) && debugPort > 1024 && debugPort < 65536);
assert(await absent(paths.output), 'Never overwrite a suite attempt');
await mkdir(paths.output, { recursive: true });
const env = { ...process.env, PATH: (process.env.PATH ?? '').split(';').filter(entry => entry && !/^D:/i.test(entry)).join(';') };
const report = { schemaVersion: 1, identifier, profile, startedAt: new Date().toISOString(), passed: false,
  requestedParts: 400, completedParts: 0, physicalValidation: false, s7HardwareValidation: false,
  scope: 'Current source, fresh profile, actual CycleHost and real LyFlow DLL; independently UI-taught 1V/3V clean replay controls, simulator PLC, four groups of 100',
  algorithmScope: 'Clean replay control only; default noisy-metal normal P0-09 remains independent and unresolved',
  stages: [], plcTransitions: [], setup: [], groups: [] };
let app, browser, page, stage = 'preflight';
const read = (command, args) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
const save = (name, value) => writeFile(join(paths.output, name), JSON.stringify(value, null, 2) + '\n', { flag: 'wx' });
function progress(next) { stage = next; report.stages.push({ stage, at: new Date().toISOString() }); console.log(JSON.stringify({ stage, output: paths.output })); }
function sourceGuard(expected) {
  const git = args => {
    const result = spawnSync('git', args, { cwd: repository, env, encoding: 'utf8', windowsHide: true, timeout: 10000 });
    assert.equal(result.status, 0, result.error?.message ?? result.stderr); return result.stdout.trim();
  };
  assert.equal(cPath(git(['rev-parse', '--show-toplevel'])).toLowerCase(), repository.toLowerCase());
  const head = git(['rev-parse', 'HEAD']);
  assert.equal(head, expected, 'Current source must equal the native manifest source gate');
  const businessDirty = git(['status', '--porcelain', '--untracked-files=all', '--', 'src', 'src-tauri', 'scripts', 'tests', 'package.json', 'pnpm-lock.yaml', 'Cargo.toml', 'Cargo.lock']);
  assert.equal(businessDirty, '', 'Business source or native tools changed after the source gate');
  return { repository, head, businessDirty, checkedAt: new Date().toISOString() };
}
async function warmReady() {
  return until(async () => {
    const cycle = await read('cycle_snapshot'), simulator = await read('sim_status'), plc = await read('plc_get_status'), engine = await read('engine_status');
    return cycle?.phase === 'IDLE' && isQuiescentCycleSnapshot(cycle) && !simulator.running && plc.state === 'connected' && engine.ready && engine.measuring && { cycle, simulator, plc, engine };
  }, 'actual production warm-up and IDLE');
}
async function waitDisconnected() {
  return until(async () => {
    const cycle = await read('cycle_snapshot'), simulator = await read('sim_status'), plc = await read('plc_get_status');
    return plc.state === 'disconnected' && cycle?.phase === 'FAULT' && cycle.fault === 'PLC 未连接' && isQuiescentCycleSnapshot(cycle) && !simulator.running && { cycle, simulator, plc };
  }, 'explicit disconnected PLC and neutral FAULT');
}
async function plcTransition(connected, label) {
  const transition = { label, target: connected ? 'connected' : 'disconnected', startedAt: new Date().toISOString(), before: { cycle: await read('cycle_snapshot'), plc: await read('plc_get_status'), simulator: await read('sim_status') }, uiAction: false };
  report.plcTransitions.push(transition);
  if (connected) await waitDisconnected();
  else if (transition.before.plc.state === 'connected') await warmReady();
  if (transition.before.plc.state !== transition.target) {
    assert(connected || transition.before.plc.state === 'connected', 'Disconnect transition requires an established connection');
    await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: 'PLC 通讯', exact: true }).click();
    await page.getByRole('button', { name: connected ? '连接' : '断开', exact: true }).click();
    transition.uiAction = true;
  }
  transition.after = connected ? await warmReady() : await waitDisconnected();
  transition.finishedAt = new Date().toISOString();
  return transition;
}
async function releaseFor(setup) {
  const directory = join(profile, 'vision', 'releases', setup.id), found = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    assert(!entry.isSymbolicLink(), 'Release directory may not contain links');
    if (!entry.isDirectory()) continue;
    const releaseDir = join(directory, entry.name), manifest = await json(join(releaseDir, 'manifest.json'));
    if (manifest.recipeId === setup.id && manifest.recipeRevision === setup.layout.revisionId) {
      assert.equal(manifest.bundleId, entry.name); assert.equal(manifest.recipeVersion, setup.layout.version);
      found.push({ releaseDir, fixture: join(releaseDir, 'recipe.json'), manifest });
    }
  }
  assert.equal(found.length, 1, 'Fresh published recipe must resolve to exactly one explicit release ID');
  return found[0];
}
async function validateReport(reportPath, resultPath) {
  let result;
  try { result = await verifyCycleHostReport(reportPath); }
  catch (error) { result = { valid: false, passed: false, report: reportPath, error: error.stack ?? String(error) }; }
  await writeFile(resultPath, JSON.stringify(result, null, 2) + '\n', { flag: 'wx' });
  return result;
}
async function stopOwnedApp() {
  if (!app) return;
  if (app.child.exitCode !== null || app.child.signalCode !== null) { app.stopped = { alreadyExited: true, exitCode: app.child.exitCode, signal: app.child.signalCode }; return; }
  assert(app.identity, 'Cannot stop a process without PID/path/start ownership evidence');
  const verified = await sampleProcess(app.child.pid, paths.exe, app.identity.start);
  const ended = new Promise(done => app.child.once('exit', done)); app.child.kill();
  await bounded(ended, 10000, 'Owned native stop timeout');
  app.stopped = { verified, at: new Date().toISOString(), exitCode: app.child.exitCode, signal: app.child.signalCode };
}
try {
  for (const key of ['exe', 'manifest', 'settings', 'cameras', 'plc', 'playwright']) await fileArtifact(paths[key]);
  const manifest = await json(paths.manifest), settings = await json(paths.settings), cameras = await json(paths.cameras), suppliedPlc = await json(paths.plc);
  const plc = structuredClone(suppliedPlc); plc.autoConnect = false;
  const sourceGate = manifest.sourceGate ?? manifest.runtimeSource;
  assert.equal(manifest.identifier, identifier); assert.equal(cPath(manifest.executable).toLowerCase(), paths.exe.toLowerCase());
  assert(typeof sourceGate === 'string' && /^[a-f0-9]{40}$/i.test(sourceGate), 'Manifest requires a full current source commit ID');
  report.sourceBefore = sourceGuard(sourceGate); report.buildManifest = manifest;
  assert(await absent(profile), 'Require a fresh absent performance profile; do not reuse old history, recipes or releases');
  assert(/^C:\\/i.test(await realpath(dirname(profile))) && /^C:\\/i.test(await realpath(paths.output)));
  assert.equal(plc.connection.protocol, 'simulator', 'Supply explicit legacy PLC simulator configuration; production S7 templates are not valid here');
  assert(Array.isArray(plc.points) && plc.points.length > 0 && plc.heartbeat);
  assert(settings.vision === true && settings.record === 'all' && settings.productSource === 'plc');
  assert.equal(settings.recordKeep, 1000); assert.equal(settings.recordMaxGb, 20);
  assert.deepEqual(settings.timeouts, { armMs: 200, motionMs: 30000, drainMs: 1000, procMs: 3000, ackMs: 5000 });
  await fileArtifact(cPath(settings.lyflowCore));
  assert(Array.isArray(cameras.cameras) && cameras.cameras.length === 1);
  assert(cameras.cameras[0].id === 'cam1' && cameras.cameras[0].source === 'sim' && cameras.cameras[0].acquisition === 'triggered' && !cameras.cameras[0].replayDir);
  report.configuration = { settings, cameras, suppliedPlc, plc };
  const inputs = await json(join(paths.inputs, 'provenance.json'));
  assert(inputs.physicalValidation === false && inputs.files.length === 32);
  report.inputControls = [];
  for (const [mode, views] of [['single', 1], ['tricam', 3]]) for (const scenario of ['normal', 'gap']) {
    report.inputControls.push({ mode, scenario, ...await scanReplayInputs(join(paths.inputs, `${views}-view`, scenario), mode) });
  }
  report.artifacts = { executable: await fileArtifact(paths.exe), manifest: await fileArtifact(paths.manifest), dll: await fileArtifact(cPath(settings.lyflowCore)),
    tools: await Promise.all(['tests/native/p0-current-cyclehost-suite.mjs', 'tests/native/p0-clean-cyclehost-setup.mjs', 'tests/native/p0-cyclehost-performance.mjs', 'scripts/p0-cyclehost-report.mjs', 'scripts/robot-plc-demo/camera-bridge.mjs', 'tests/native/tauri-current-performance.json'].map(file => fileArtifact(join(repository, file)))) };
  let busy = false; try { await fetch(`http://127.0.0.1:${debugPort}/json/version`, { signal: AbortSignal.timeout(500) }); busy = true; } catch {}
  assert(!busy, 'Native CDP port is already occupied');
  await mkdir(profile);
  for (const [name, value] of [['cycle', settings], ['cameras', cameras], ['plc', plc]]) await writeFile(join(profile, name + '.json'), JSON.stringify(value), { flag: 'wx' });
  progress('fresh-current-native');
  const child = spawn(paths.exe, [], { cwd: dirname(paths.exe), env: { ...env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort} --remote-debugging-address=127.0.0.1` }, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  app = { child, stdout: [], stderr: [], identity: null, error: null, stopped: null };
  child.on('error', error => { app.error = String(error); }); child.stdout.on('data', data => app.stdout.push(data)); child.stderr.on('data', data => app.stderr.push(data));
  await until(async () => { if (app.error || child.exitCode !== null) throw new Error(app.error ?? 'Owned native exited'); try { app.identity = await sampleProcess(child.pid, paths.exe); return app.identity; } catch { return false; } }, 'owned native PID/path/start identity', 5000);
  report.process = app.identity;
  await until(async () => { if (app.error || child.exitCode !== null) throw new Error(app.error ?? 'Owned native exited'); try { return (await fetch(`http://127.0.0.1:${debugPort}/json/version`, { signal: AbortSignal.timeout(500) })).ok; } catch { return false; } }, 'owned native CDP');
  const { chromium } = await import(pathToFileURL(paths.playwright).href);
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`);
  const native = await until(async () => { const found = []; for (const candidate of browser.contexts().flatMap(context => context.pages())) if (await candidate.evaluate(() => !!window.__TAURI_INTERNALS__?.invoke).catch(() => false)) found.push(candidate); return found.length && found; }, 'native invoke page');
  assert.equal(native.length, 1); page = native[0]; await page.waitForURL('http://tauri.localhost/**'); await page.getByRole('navigation', { name: '操作导航' }).waitFor();
  assert.equal(cPath((await read('records_list')).root).toLowerCase(), recordsRoot.toLowerCase());
  assert.equal((await read('history_query', { query: { limit: 1 } })).total, 0);
  assert.deepEqual(await read('cycle_get_settings'), settings); assert.deepEqual(await read('plc_get_config'), plc);
  await plcTransition(false, 'fresh-profile-offline');
  await until(async () => { const engine = await read('engine_status'); return engine.backend === 'LyFlow' && engine.ready && engine.measuring && engine; }, 'real DLL ready');
  for (const [mode, views] of [['single', 1], ['tricam', 3]]) {
    progress('fresh-ui-teach-publish-' + mode);
    await plcTransition(false, mode + '-offline-before-teaching');
    const setup = await teachCleanFixture(page, { views, inputs: paths.inputs, output: join(paths.output, 'setup-' + mode), recordsRoot, allowUnpublished: true, allowDisconnected: true });
    assert(setup.passed && setup.layout.shots.length === 4);
    const release = await releaseFor(setup);
    report.setup.push({ mode, id: setup.id, revisionId: setup.layout.revisionId, bundleId: release.manifest.bundleId, report: join(paths.output, 'setup-' + mode, 'setup-report.json') });
    await waitDisconnected();
    for (const scenario of ['normal', 'gap']) {
      const tag = mode + '-' + scenario, output = join(paths.output, tag), replayDir = join(paths.inputs, `${views}-view`, scenario);
      progress('current-100-' + tag);
      const offline = await plcTransition(false, tag + '-offline-before-camera-config');
      await configureReplay(page, { views, directory: replayDir, recordsRoot, allowDisconnected: true });
      await waitDisconnected();
      const online = await plcTransition(true, tag + '-online-after-camera-config');
      const ready = online.after;
      const identity = await sampleProcess(child.pid, paths.exe, app.identity.start);
      const run = await runCycleHostPerformance(page, { mode, scenario, source: 'replay', replayDir, fixture: release.fixture, releaseDir: release.releaseDir, executable: paths.exe, pid: child.pid, output, parts: 100 });
      const validation = await validateReport(join(output, 'cyclehost-report.json'), join(output, 'independent-validation.json'));
      report.groups.push({ mode, scenario, sourceGate, runtime: identity, offline, online, ready, output, report: join(output, 'cyclehost-report.json'), validation, completedParts: run.completedParts, passed: run.passed && validation.valid && validation.passed });
      report.completedParts += run.completedParts;
      await save(tag + '-association.json', report.groups.at(-1));
      assert(report.groups.at(-1).passed && run.completedParts === 100, 'Current clean control group failed');
      await writeFile(join(paths.output, 'suite-checkpoint.json'), JSON.stringify(report, null, 2) + '\n');
    }
  }
  progress('independent-final-revalidation');
  for (const group of report.groups) { group.finalValidation = await validateReport(group.report, join(group.output, 'final-independent-validation.json')); assert(group.finalValidation.valid && group.finalValidation.passed && group.finalValidation.parts === 100); }
  report.sourceAfter = sourceGuard(sourceGate); report.finalProcess = await sampleProcess(child.pid, paths.exe, app.identity.start); report.finalReady = await warmReady();
  assert(report.groups.length === 4 && report.completedParts === 400); report.passed = true;
} catch (error) {
  report.failedStage = stage; report.error = error.stack ?? String(error); process.exitCode = 1;
  if (page) {
    try { report.lastNative = { cycle: await read('cycle_snapshot'), simulator: await read('sim_status'), cameras: await read('camera_rig_status'), plc: await read('plc_get_status'), engine: await read('engine_status'), history: await read('history_query', { query: { limit: 10 } }), logs: await read('cycle_logs') }; } catch (failure) { report.nativeReadError = String(failure); }
    await page.screenshot({ path: join(paths.output, 'failure.png'), fullPage: true }).catch(failure => { report.screenshotError = String(failure); });
  }
} finally {
  const errors = [];
  if (browser) await browser.close().catch(error => errors.push('CDP close: ' + error));
  try { await stopOwnedApp(); } catch (error) { errors.push('Owned native cleanup: ' + error); }
  if (app) {
    report.processCleanup = { identity: app.identity, stopped: app.stopped, error: app.error };
    for (const stream of ['stdout', 'stderr']) try { await save('native-' + stream + '.json', { encoding: 'utf8', data: Buffer.concat(app[stream]).toString('utf8') }); } catch (error) { errors.push('Process evidence: ' + error); }
  }
  if (errors.length) { report.cleanupErrors = errors; report.passed = false; process.exitCode = 1; }
  report.finishedAt = new Date().toISOString(); await save('suite-report.json', report);
  console.log(JSON.stringify({ passed: report.passed, completedParts: report.completedParts, failedStage: report.failedStage, output: paths.output, error: report.error }));
}
