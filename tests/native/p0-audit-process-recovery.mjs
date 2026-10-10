import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, readFile, writeFile, readdir, lstat, realpath } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { join, dirname, resolve, basename } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { sampleProcess } from './p0-cyclehost-performance.mjs';

const identifier = 'com.xyzrobotics.tujiaovision.p0-tests.audit';
const { values } = parseArgs({ options: {
  executable: { type: 'string' }, appdata: { type: 'string' }, output: { type: 'string' },
  template: { type: 'string' }, fixture: { type: 'string' }, instance: { type: 'string' },
  recipe: { type: 'string' }, settings: { type: 'string' }, cameras: { type: 'string' },
  python: { type: 'string' }, 'playwright-module': { type: 'string' },
  'source-gate': { type: 'string' }, 'debug-port': { type: 'string', default: '9342' },
} });
for (const key of ['executable', 'appdata', 'output', 'template', 'fixture', 'instance', 'recipe', 'settings', 'cameras', 'python', 'playwright-module', 'source-gate']) {
  assert(values[key], 'Missing --' + key);
}
const cPath = value => { const absolute = resolve(value); assert(/^c:\\/i.test(absolute), 'Only absolute C-drive paths are allowed'); return absolute; };
const paths = Object.fromEntries(Object.entries(values).filter(([key]) => !['source-gate', 'debug-port'].includes(key)).map(([key, value]) => [key, cPath(value)]));
const profile = paths.appdata, output = paths.output, db = join(profile, 'inspection.db');
const spoolRoot = join(profile, 'audit-spool'), journalPath = join(profile, 'plc-handshake.json');
const debugPort = Number(values['debug-port']);
assert(Number.isInteger(debugPort) && debugPort > 1024 && debugPort < 65536);
assert.equal(profile.toLowerCase(), join(cPath(process.env.APPDATA), identifier).toLowerCase(), 'Require exact dedicated AppData identity');
assert.equal(basename(profile), identifier);
const json = async file => JSON.parse((await readFile(file, 'utf8')).replace(/^\uFEFF/, ''));
const absent = async file => { try { await lstat(file); return false; } catch (error) { if (error.code === 'ENOENT') return true; throw error; } };
const save = (name, value) => writeFile(join(output, name), JSON.stringify(value, null, 2) + '\n', { flag: 'wx' });
assert(await absent(output), 'Do not overwrite any existing attempt evidence');
await mkdir(output, { recursive: true });
const report = {
  schemaVersion: 1, startedAt: new Date().toISOString(), passed: false,
  scope: 'Actual S7 loopback, production Synthetic refusal, SQLite writer lock before request, durable spool and real ACK, process interruption and startup recovery',
  physicalValidation: false, s7HardwareValidation: false, imageProcessingValidated: false, benchmark: false,
  sourceGate: values['source-gate'], identifier, profile, stages: [], ownedProcesses: [],
  drainSemantics: 'Pre-arm refusal has no observed partEnd, so drainMs must remain null; no elapsed value is invented',
};
const sleep = ms => new Promise(done => setTimeout(done, ms));
async function bounded(promise, milliseconds, message) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(message)), milliseconds); })]); }
  finally { clearTimeout(timer); }
}
let app, browser, page, plc, lock, commandId = 0, stage = 'inputGuard';
const requests = new Map(), owned = [], children = new Map();
const cleanEnv = { ...process.env, PATH: (process.env.PATH ?? '').split(';').filter(entry => !/^d:/i.test(entry)).join(';') };
function progress(next) { stage = next; report.stages.push({ stage, at: new Date().toISOString() }); console.log(JSON.stringify({ stage, output })); }
async function waitFor(check, label, timeout = 20000) {
  const deadline = Date.now() + timeout;
  do { const result = await check(); if (result) return result; await sleep(20); } while (Date.now() < deadline);
  throw new Error('Timed out waiting for ' + label);
}
async function inputFile(file) {
  const metadata = await lstat(file);
  assert(metadata.isFile() && !metadata.isSymbolicLink(), 'Require ordinary input file: ' + file);
  assert(/^c:\\/i.test(await realpath(file)), 'Input resolves outside C drive');
  return { path: file, bytes: metadata.size };
}
function startChild(name, executable, arguments_, options = {}) {
  const child = spawn(executable, arguments_, { cwd: dirname(paths.executable), env: cleanEnv, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'], ...options });
  const state = { name, child, executable, stdout: [], stderr: [], error: null, identity: null, stopped: null };
  child.on('error', error => { state.error = String(error); });
  child.stdout.on('data', data => state.stdout.push(data));
  child.stderr.on('data', data => state.stderr.push(data));
  children.set(child, state); owned.push(state);
  return child;
}
async function identify(child) {
  const state = children.get(child);
  assert(child.pid && child.exitCode === null && !state.error, state.error ?? 'Owned child exited before identity check');
  state.identity = await sampleProcess(child.pid, state.executable);
  report.ownedProcesses.push({ name: state.name, ...state.identity });
  return state.identity;
}
async function stopChild(child, force = true) {
  if (!child) return;
  const state = children.get(child);
  if (child.exitCode !== null || child.signalCode !== null) { state.stopped ??= { at: new Date().toISOString(), alreadyExited: true, exitCode: child.exitCode, signal: child.signalCode }; return; }
  assert(state.identity, 'Cannot terminate a process without recorded path/start identity');
  const ended = new Promise(done => child.once('exit', done));
  const verified = force ? await sampleProcess(child.pid, state.executable, state.identity.start) : state.identity;
  if (force) child.kill();
  await bounded(ended, 10000, 'Owned child did not exit: ' + state.name);
  state.stopped = { at: new Date().toISOString(), verified, exitCode: child.exitCode, signal: child.signalCode };
}
function control(command) {
  const id = ++commandId;
  return new Promise((accept, reject) => {
    const timer = setTimeout(() => { requests.delete(id); reject(new Error('S7 control timeout: ' + command.op)); }, 5000);
    requests.set(id, { accept: result => { clearTimeout(timer); accept(result); }, reject: error => { clearTimeout(timer); reject(error); } });
    plc.stdin.write(JSON.stringify({ ...command, id }) + '\n');
  });
}
function queryDb(cycleId = '') {
  const code = `import sqlite3,json,sys\nc=sqlite3.connect('file:'+sys.argv[1].replace('\\\\','/')+'?mode=ro',uri=True,timeout=1)\nc.row_factory=sqlite3.Row\nr=c.execute('select * from parts where cycle_id=?',(sys.argv[2],)).fetchone()\nshots=c.execute('select * from part_shots where part_id=? order by k',(r['id'],)).fetchall() if r else []\npoints=c.execute('select format,data from part_points where part_id=?',(r['id'],)).fetchone() if r else None\nrecipe=c.execute('select json from recipe_snapshots where revision_id=?',(r['recipe_revision'],)).fetchone() if r else None\nprint(json.dumps({'part':dict(r) if r else None,'shots':[dict(s) for s in shots],'points':{'format':points[0],'values':list(points[1])} if points else None,'recipe':json.loads(recipe[0]) if recipe else None,'totalParts':c.execute('select count(*) from parts').fetchone()[0],'journalMode':c.execute('pragma journal_mode').fetchone()[0]}))\nc.close()`;
  const result = spawnSync(paths.python, ['-c', code, db, cycleId], { cwd: dirname(paths.executable), env: cleanEnv, encoding: 'utf8', windowsHide: true, timeout: 5000 });
  assert.equal(result.status, 0, result.error?.message ?? result.stderr);
  return JSON.parse(result.stdout);
}
async function spoolSnapshot() {
  const result = [];
  for (const name of (await readdir(spoolRoot)).sort()) {
    if (name === '.health') continue;
    assert(/^\d{20}\.(json|tmp)$/.test(name), 'Unknown spool file must be preserved: ' + name);
    try {
      const file = join(spoolRoot, name), metadata = await lstat(file);
      assert(metadata.isFile() && !metadata.isSymbolicLink() && metadata.size <= 4 * 1024 * 1024);
      result.push({ name, bytes: metadata.size, pending: name.endsWith('.tmp'), document: name.endsWith('.json') ? await json(file) : null });
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  return result;
}
function event(snapshot, key, predicate = () => true) {
  return snapshot.map(file => file.document?.[key]).find(value => value !== undefined && predicate(value));
}
const read = (command, args) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
async function launch(label) {
  progress(label);
  app = startChild(label, paths.executable, [], { env: { ...cleanEnv, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort} --remote-debugging-address=127.0.0.1` } });
  await identify(app);
  await waitFor(async () => {
    if (app.exitCode !== null || app.signalCode !== null || children.get(app).error) throw new Error('Native application exited during startup: ' + children.get(app).error);
    try { return (await fetch(`http://127.0.0.1:${debugPort}/json/version`, { signal: AbortSignal.timeout(500) })).ok; } catch { return false; }
  }, 'owned native CDP');
  const { chromium } = await import(pathToFileURL(paths['playwright-module']).href);
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`);
  const pages = await waitFor(async () => {
    const native = [];
    for (const candidate of browser.contexts().flatMap(context => context.pages())) {
      if (await candidate.evaluate(() => !!window.__TAURI_INTERNALS__?.invoke).catch(() => false)) native.push(candidate);
    }
    return native.length ? native : false;
  }, 'native invoke page');
  assert.equal(pages.length, 1, 'Require exactly one native page');
  page = pages[0]; await page.waitForURL('http://tauri.localhost/**');
  await page.getByRole('navigation', { name: '操作导航' }).waitFor();
  assert.equal(cPath((await read('records_list')).root).toLowerCase(), join(profile, 'records').toLowerCase());
  await sampleProcess(app.pid, paths.executable, children.get(app).identity.start);
  return page;
}
async function stopApp() {
  if (browser) { await browser.close().catch(() => {}); browser = null; }
  await stopChild(app); app = null; page = null;
}
function verifyRecovered(record, original, recording, submission, sn) {
  const part = record.part;
  assert(part && record.totalParts === 1);
  const fields = { ts: original.ts, sn, cycle_id: original.cycle_id, recipe_id: original.recipe.id,
    recipe_version: original.recipe.version, recipe_revision: original.recipe.revisionId,
    verdict: original.judgement.verdict, plc_code: original.judgement.plcCode,
    fault_code: original.judgement.faultCode, reason: original.judgement.reason,
    drain_ms: submission[2], frames_expected: original.frames_expected, frames_received: original.frames_received,
    triggers: original.triggers, software_version: original.software_version, bundle_id: original.bundle_id };
  for (const [key, value] of Object.entries(fields)) assert.deepEqual(part[key], value, 'Recovered original differs at ' + key);
  assert.deepEqual(JSON.parse(part.judgement), original.judgement);
  assert.deepEqual(JSON.parse(part.frames), original.frames);
  assert.deepEqual(record.recipe, original.recipe);
  assert.equal(part.delivery_state, 'acknowledged');
  assert.equal(part.recording_state, recording.state);
  assert.equal(Boolean(part.recording_available), recording.available);
  assert.equal(part.recording_directory, recording.directory);
  assert.deepEqual(JSON.parse(part.recording_errors), recording.errors);
  assert.equal(record.points, null); assert.equal(original.table, null);
  assert.equal(record.shots.length, 4);
  for (const [k, shot] of record.shots.entries()) {
    const expected = original.shots[k];
    assert.deepEqual({ k: shot.k, shotId: shot.shot_id, camera: shot.camera, view: shot.view, session: shot.session,
      ordinal: shot.ordinal, frameCounter: shot.frame_counter, triggerCounter: shot.trigger_counter,
      status: shot.status, error: shot.error, score: shot.score, ms: shot.ms, rawFiles: JSON.parse(shot.raw_files) }, expected);
  }
}
try {
  const inputs = {};
  for (const [key, file] of Object.entries(paths).filter(([key]) => !['appdata', 'output'].includes(key))) inputs[key] = await inputFile(file);
  const manifest = await json(paths.instance), recipe = await json(paths.recipe), settings = await json(paths.settings), cameras = await json(paths.cameras);
  assert.equal(manifest.identifier, identifier);
  assert.equal(cPath(manifest.executable).toLowerCase(), paths.executable.toLowerCase());
  assert.equal(manifest.sourceGate ?? manifest.runtimeSource, values['source-gate']);
  assert(await absent(profile), 'Require absent dedicated profile; never reuse/copy production or previous test data');
  assert.equal(recipe.schemaVersion, 5); assert.equal(recipe.version, 1);
  assert(/^P0-AUDIT-[A-Z0-9_-]+$/.test(recipe.id) && recipe.id.length <= 32);
  assert(!recipe.teachingId && recipe.triggerMode === 'fly' && recipe.shots.length === 4);
  assert(Number.isInteger(recipe.productCode) && recipe.productCode > 0 && recipe.productCode <= 65535);
  assert(settings.vision === false && settings.record === 'all' && settings.productSource === 'plc');
  assert(settings.timeouts.armMs === 200 && settings.recordMaxGb === 20 && settings.timeouts.procMs >= 200);
  assert(settings.lyflowCore === null || /^c:\\/i.test(settings.lyflowCore), 'No non-C engine configuration');
  assert(Array.isArray(cameras.cameras) && cameras.cameras.length > 0 && cameras.cameras.length <= 3);
  assert(cameras.cameras.every(camera => camera.source === 'sim' && camera.acquisition === 'triggered' && !camera.replayDir));
  assert(recipe.shots.every(shot => cameras.cameras.some(camera => camera.id === shot.camera && camera.viewCount >= shot.view)));
  assert(recipe.shots.every(shot => !shot.skip && shot.path.length >= 2 && shot.mmPerPx > 0));
  report.inputs = inputs; report.buildManifest = manifest; report.settings = settings; report.explicitRefusalFixture = recipe;
  report.fixtureQualification = 'Explicit geometry only, no teaching resources or release bundle; never qualified as an OK production recipe';
  let cdpBusy = false;
  try { await fetch(`http://127.0.0.1:${debugPort}/json/version`, { signal: AbortSignal.timeout(500) }); cdpBusy = true; } catch {}
  assert(!cdpBusy, 'Stop previous native CDP instance first');
  const processScan = spawnSync('pwsh', ['-NoProfile', '-NonInteractive', '-Command', 'Get-Process | Where-Object { $_.Path -eq $env:P0_AUDIT_EXE } | Select-Object Id,Path | ConvertTo-Json -Compress'], { encoding: 'utf8', env: { ...cleanEnv, P0_AUDIT_EXE: paths.executable }, windowsHide: true });
  assert.equal(processScan.status, 0, processScan.stderr); assert(!processScan.stdout.trim(), 'Executable already running');
  await mkdir(profile); await mkdir(join(profile, 'recipes'));
  await writeFile(join(profile, 'recipes', recipe.id + '.json'), JSON.stringify(recipe, null, 2), { flag: 'wx' });
  await writeFile(join(profile, 'cycle.json'), JSON.stringify(settings, null, 2), { flag: 'wx' });
  await writeFile(join(profile, 'cameras.json'), JSON.stringify(cameras, null, 2), { flag: 'wx' });
  progress('start-loopback-fixture');
  const fixtureReady = new Promise((accept, reject) => {
    plc = startChild('s7-fixture', paths.python, ['-u', paths.fixture, '--port', '0']);
    createInterface({ input: plc.stdout }).on('line', line => {
      let value; try { value = JSON.parse(line); } catch { return; }
      if (value.event === 'ready') accept(value);
      else if (requests.has(value.id)) { const request = requests.get(value.id); requests.delete(value.id); value.ok ? request.accept(value.result) : request.reject(new Error(value.error)); }
    });
    plc.once('error', reject); plc.once('exit', code => reject(new Error('Fixture exited: ' + code)));
  });
  await identify(plc);
  const ready = await bounded(fixtureReady, 10000, 'Fixture startup timeout');
  assert(ready.pid === plc.pid && ready.host === '127.0.0.1' && ready.protocol === 's7' && ready.db === 100 && ready.port > 0);
  report.fixture = ready;
  await control({ op: 'heartbeat', enabled: true, interval_ms: 100 });
  const exported = await json(paths.template), config = structuredClone(exported.template ?? exported);
  assert.equal(config.connection.protocol, 's7');
  const done = config.points.find(point => point.tags?.includes('done'));
  assert.equal(done?.address.toUpperCase(), 'DB100.DBX64.3', 'Require known fixture DONE address');
  Object.assign(config.connection, { host: '127.0.0.1', port: ready.port, timeoutMs: 2000, pollIntervalMs: 20, reconnectIntervalMs: 200 });
  config.heartbeat.intervalMs = 100; config.autoConnect = false;
  await writeFile(join(profile, 'plc.json'), JSON.stringify(config, null, 2), { flag: 'wx' });
  report.actualPlcConfig = config;
  await launch('initial-native');
  report.loadedRecipe = await read('recipe_doc', { id: recipe.id });
  assert.deepEqual(await read('cycle_layout', { recipeId: recipe.id }), await read('recipe_preview', { doc: recipe }));
  const actualCameras = await read('camera_rig_config');
  assert.equal(actualCameras.length, cameras.cameras.length);
  for (const expected of cameras.cameras) {
    const actual = actualCameras.find(camera => camera.id === expected.id); assert(actual);
    for (const [key, value] of Object.entries(expected)) assert.deepEqual(actual[key], value, 'Configured camera field differs: ' + key);
  }
  assert((await read('history_query', { query: { limit: 1 } })).items.length === 0);
  assert.equal((await read('cycle_get_settings')).vision, false);
  await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: 'PLC 通讯', exact: true }).click();
  await page.getByRole('button', { name: '连接', exact: true }).click();
  await page.waitForFunction(async () => (await window.__TAURI_INTERNALS__.invoke('plc_get_status')).state === 'connected');
  await sleep(1200);
  await control({ op: 'plc_values', values: { faultReset: true } });
  report.ready = await waitFor(async () => { const state = await control({ op: 'status' }); return state.fields.visionReady && state; }, 'real S7 reset/ready and preallocated cycle identities');
  await control({ op: 'plc_values', values: { faultReset: false } });
  const plan = await read('plc_recipe_plan', { recipeId: recipe.id });
  assert.equal(plan.shotCount, 4); assert(!Object.hasOwn(plan, 'planHash'));
  const before = queryDb(); assert.equal(before.totalParts, 0); assert.equal(before.journalMode, 'wal');
  report.before = before; report.plan = plan;
  progress('lock-sqlite-before-request');
  const lockReady = new Promise((accept, reject) => {
    const code = "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1],timeout=5)\nc.execute('BEGIN IMMEDIATE')\nprint('LOCKED',flush=True)\nsys.stdin.readline()\nc.rollback()\nc.close()";
    lock = startChild('sqlite-writer-lock', paths.python, ['-u', '-c', code, db]);
    lock.stdout.on('data', data => { if (data.toString().includes('LOCKED')) accept(); });
    lock.once('error', reject); lock.once('exit', code => reject(new Error('Lock process exited before held: ' + code)));
  });
  await identify(lock);
  await bounded(lockReady, 10000, 'SQLite writer lock timeout');
  report.lock = { identity: children.get(lock).identity, acquiredAt: new Date().toISOString(), mode: 'BEGIN IMMEDIATE, no row writes' };
  report.doneObservationDelay = await control({ op: 'fault', function: 'write', kind: 'delay', db: 100, byte: 64, bit: 3, delay_ms: 600, count: 1 });
  const sn = 740000101, seq = 101;
  await control({ op: 'plc_request', values: { protocolVersion: 1, requestSeq: seq, partSn: sn, productCode: recipe.productCode,
    shotCount: plan.shotCount, planVersion: plan.planVersion, planReserved: 0,
    camera1Shots: plan.cameraShots[0], camera2Shots: plan.cameraShots[1], camera3Shots: plan.cameraShots[2] } });
  progress('observe-durable-insert-before-done');
  const beforeDone = await waitFor(async () => {
    const spool = await spoolSnapshot(), original = event(spool, 'Insert', value => value.sn === sn);
    if (!original) return false;
    const database = queryDb(original.cycle_id);
    assert.equal(database.part, null); assert.equal(database.totalParts, 0);
    const state = await control({ op: 'status' });
    assert(!state.fields.done, 'DONE observed before the required pre-DONE spool observation; keep attempt and diagnose instrumentation');
    return { observedAt: new Date().toISOString(), spool, original, database, fields: state.fields };
  }, 'complete Insert before delayed actual DONE');
  const original = beforeDone.original, cycleId = original.cycle_id;
  assert(cycleId && original.recipe.id === recipe.id && original.shots.length === 4 && original.frames_expected === 4);
  assert(original.judgement.verdict === 'ERR_INSPECT' && original.judgement.plcCode === 90);
  assert(original.judgement.faultCode === 98 && original.judgement.reason.includes('模拟 / 回放相机') && original.judgement.reason.includes('生产节拍不能用它的计数'), 'Require real production Synthetic-counter refusal, not an unrelated fixture error');
  assert.equal(original.bundle_id, null); assert.equal(original.drain_ms, null);
  assert.equal(original.frames_received, 0); assert.equal(original.triggers, 0);
  assert.equal(original.delivery.state, 'pending');
  report.preDone = beforeDone; await save('spool-before-done.json', beforeDone);
  const doneFields = await waitFor(async () => { const state = await control({ op: 'status' }); return state.fields.done && state.fields.resultSeq === seq && state.fields.resultSn === sn && state.fields; }, 'actual safe refusal DONE');
  assert(doneFields.resultCode === 90 && !doneFields.armed && doneFields.faultCode === original.judgement.faultCode);
  const completedSpool = await waitFor(async () => {
    const snapshot = await spoolSnapshot(), recording = event(snapshot, 'Recording', value => value.cycleId === cycleId), submission = event(snapshot, 'Submission', value => value[0] === cycleId);
    return recording && submission && { snapshot, recording, submission };
  }, 'durable recording and actual submission events');
  const { recording, submission } = completedSpool;
  assert(recording.state === 'failed' && recording.available === false && recording.directory === null && recording.files.length === 0 && recording.errors.length > 0);
  assert(submission[1].state === 'submitted' && submission[2] === null);
  const whileLocked = queryDb(cycleId); assert.equal(whileLocked.part, null); assert.equal(whileLocked.totalParts, 0);
  report.done = { fields: doneFields, completedSpool, database: whileLocked, sqliteWriterStillLocked: true };
  progress('actual-ack-and-process-interruption');
  await control({ op: 'plc_ack' });
  const acknowledged = await waitFor(async () => {
    const journal = await json(journalPath);
    return journal.pending?.cycleId === cycleId && journal.pending.acknowledged === true && journal;
  }, 'actual S7 ACK durable in handshake journal');
  assert.equal(acknowledged.pending.request.requestSeq, seq); assert.equal(acknowledged.pending.request.sn, sn);
  const crashSpool = await waitFor(async () => { const snapshot = await spoolSnapshot(); return snapshot.every(file => !file.pending) && snapshot; }, 'published spool receipts before controlled process termination');
  const crashDb = queryDb(cycleId);
  assert.equal(crashDb.part, null); assert.equal(crashDb.totalParts, 0);
  assert(event(crashSpool, 'Insert', value => value.cycle_id === cycleId));
  report.crashWindow = { at: new Date().toISOString(), app: children.get(app).identity, journal: acknowledged, database: crashDb, spool: crashSpool,
    sqliteWriterStillLocked: lock.exitCode === null,
    acknowledgedSpoolEventObserved: !!event(crashSpool, 'Delivery', value => value[0] === cycleId && value[1].state === 'acknowledged') };
  assert(report.crashWindow.sqliteWriterStillLocked);
  await save('crash-window.json', report.crashWindow);
  report.wireBeforeCrash = await control({ op: 'trace' });
  await stopApp();
  lock.stdin.end('\n'); await stopChild(lock, false); lock = null;
  const afterStop = queryDb(cycleId); assert.equal(afterStop.part, null); assert.equal(afterStop.totalParts, 0);
  report.afterStopBeforeRestart = afterStop;
  await launch('restart-native');
  const recovered = await waitFor(() => { const record = queryDb(cycleId); return record.part?.delivery_state === 'acknowledged' && record; }, 'spool replay and actual ACK recovery');
  verifyRecovered(recovered, original, recording, submission, sn);
  const history = await read('history_detail', { id: recovered.part.id });
  assert(history.summary.cycleId === cycleId && history.summary.sn === sn && history.summary.delivery.state === 'acknowledged');
  assert.deepEqual(history.judgement, original.judgement); assert.equal(history.summary.drainMs, null);
  assert.equal(history.shots.length, 4); assert.equal(history.recording.state, 'failed'); assert.equal(history.recording.available, false);
  const remaining = await spoolSnapshot(); assert.equal(remaining.length, 0, 'Successful startup replay must acknowledge all spool receipts');
  report.after = { recovered, history, remainingSpool: remaining, journal: await json(journalPath), cycle: await read('cycle_snapshot') };
  report.ackRecoveryPath = report.crashWindow.acknowledgedSpoolEventObserved
    ? 'Actual S7 ACK present in both journal and spool; startup spool replay restored it, so no separate journal fallback execution is claimed'
    : 'Actual S7 ACK in journal; spool restored the missing part first and Machine startup associated the exact persisted ACK';
  report.originalDetectionEvidencePreserved = true;
  report.passed = true; progress('passed');
} catch (error) {
  report.failedStage = stage; report.error = error.stack ?? String(error); process.exitCode = 1;
  try { if (!(await absent(spoolRoot))) report.failedSpool = await spoolSnapshot(); } catch (failure) { report.failedSpoolReadError = String(failure); }
  try { if (!(await absent(journalPath))) report.failedJournal = await json(journalPath); } catch (failure) { report.failedJournalReadError = String(failure); }
  if (plc && plc.exitCode === null) try { report.failedPlcState = await control({ op: 'status' }); report.failedWire = await control({ op: 'trace' }); } catch (failure) { report.failedFixtureReadError = String(failure); }
} finally {
  const cleanupErrors = [];
  try { await stopApp(); } catch (error) { cleanupErrors.push('app: ' + error); }
  if (lock) { try { lock.stdin.end('\n'); await stopChild(lock, false); } catch (error) { cleanupErrors.push('sqlite lock: ' + error); } }
  if (plc) { try { if (plc.exitCode === null) { await control({ op: 'shutdown' }); plc.stdin.end(); } await stopChild(plc, false); } catch (error) { cleanupErrors.push('fixture: ' + error); try { await stopChild(plc); } catch (failure) { cleanupErrors.push('fixture force stop: ' + failure); } } }
  for (const state of owned) {
    for (const stream of ['stdout', 'stderr']) {
      try { await writeFile(join(output, `${state.name}-${state.child.pid ?? 'unstarted'}-${stream}.log`), Buffer.concat(state[stream]), { flag: 'wx' }); }
      catch (error) { cleanupErrors.push(`${state.name} ${stream} evidence: ${error}`); }
    }
  }
  report.processCleanup = owned.map(state => ({ name: state.name, pid: state.child.pid, identity: state.identity, stopped: state.stopped, error: state.error }));
  if (cleanupErrors.length) { report.cleanupErrors = cleanupErrors; report.passed = false; process.exitCode = 1; }
  report.finishedAt = new Date().toISOString();
  await save('report.json', report);
  console.log(JSON.stringify({ passed: report.passed, failedStage: report.failedStage, error: report.error, output }));
}

