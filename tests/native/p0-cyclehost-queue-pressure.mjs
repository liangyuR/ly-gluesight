import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createConnection } from 'node:net';
import { mkdir, readFile, writeFile, lstat, realpath, readdir } from 'node:fs/promises';
import { join, dirname, resolve, basename } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { configureReplay, captureTeachingSample, isQuiescentCycleSnapshot, waitForPublishedRecipeReady } from './p0-clean-cyclehost-setup.mjs';
import { sampleProcess, recordedArtifact } from './p0-cyclehost-performance.mjs';

const identifier = 'com.xyzrobotics.tujiaovision.p0-tests.pressure';
const cPath = value => { const path = resolve(value); assert(/^C:\\/i.test(path), 'Explicit C-drive path required: ' + path); return path; };
const sleep = ms => new Promise(done => setTimeout(done, ms));
const json = async path => JSON.parse((await readFile(cPath(path), 'utf8')).replace(/^\uFEFF/, ''));
const absent = async path => { try { await lstat(path); return false; } catch (error) { if (error.code === 'ENOENT') return true; throw error; } };
async function until(check, label, timeout = 20000) {
  const deadline = Date.now() + timeout;
  do { const result = await check(); if (result) return result; await sleep(20); } while (Date.now() < deadline);
  throw new Error('Timed out: ' + label);
}
async function bounded(promise, ms, label) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(label)), ms); })]); }
  finally { clearTimeout(timer); }
}
class Modbus {
  constructor(socket) {
    this.socket = socket; this.next = 0; this.buffer = Buffer.alloc(0); this.pending = new Map(); this.tail = Promise.resolve(); this.error = null;
    socket.on('error', error => this.fail(error));
    socket.on('close', () => this.fail(new Error('Modbus fixture socket closed')));
    socket.on('data', data => {
      this.buffer = Buffer.concat([this.buffer, data]);
      while (this.buffer.length >= 7) {
        const length = this.buffer.readUInt16BE(4);
        if (length < 2 || length > 254) { this.fail(new Error('Invalid Modbus response size')); return; }
        if (this.buffer.length < 6 + length) return;
        const frame = this.buffer.subarray(0, 6 + length); this.buffer = this.buffer.subarray(6 + length);
        const request = this.pending.get(frame.readUInt16BE(0));
        if (!request || frame.readUInt16BE(2) !== 0 || frame[6] !== 1) { this.fail(new Error('Unexpected Modbus transaction')); return; }
        this.pending.delete(frame.readUInt16BE(0)); clearTimeout(request.timer);
        const pdu = frame.subarray(7);
        if (pdu[0] !== request.fn) request.reject(new Error('Modbus exception/function mismatch: ' + pdu.toString('hex')));
        else request.accept(pdu);
      }
    });
  }
  fail(error) { this.error ??= error; for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(error); } this.pending.clear(); }
  request(pdu) {
    const operation = async () => {
      if (this.error) throw this.error;
      const tid = this.next = (this.next + 1) % 65536, header = Buffer.alloc(7);
      header.writeUInt16BE(tid); header.writeUInt16BE(pdu.length + 1, 4); header[6] = 1;
      return new Promise((accept, reject) => {
        const timer = setTimeout(() => { this.pending.delete(tid); const error = new Error('Modbus wire timeout'); this.fail(error); reject(error); }, 2000);
        this.pending.set(tid, { accept, reject, timer, fn: pdu[0] }); this.socket.write(Buffer.concat([header, pdu]));
      });
    };
    const result = this.tail.then(operation); this.tail = result.catch(() => {}); return result;
  }
  async coil(address, value) {
    const pdu = Buffer.alloc(5); pdu[0] = 5; pdu.writeUInt16BE(address, 1); pdu.writeUInt16BE(value ? 0xff00 : 0, 3);
    assert.deepEqual(await this.request(pdu), pdu);
  }
  async registers(address, values) {
    const pdu = Buffer.alloc(6 + values.length * 2); pdu[0] = 16; pdu.writeUInt16BE(address, 1); pdu.writeUInt16BE(values.length, 3); pdu[5] = values.length * 2;
    values.forEach((value, index) => pdu.writeUInt16BE(value, 6 + index * 2)); assert.deepEqual(await this.request(pdu), pdu.subarray(0, 5));
  }
  async snapshot() {
    const read = async (fn, start, count) => { const pdu = Buffer.alloc(5); pdu[0] = fn; pdu.writeUInt16BE(start, 1); pdu.writeUInt16BE(count, 3); return this.request(pdu); };
    const coils = await read(1, 0, 24), registers = await read(3, 100, 14);
    assert.equal(coils[0], 1); assert.equal(registers[0], 3);
    assert.equal(coils.length, 5); assert.equal(coils[1], 3); assert.equal(registers.length, 30); assert.equal(registers[1], 28);
    const flag = bit => Boolean(coils[2 + Math.floor(bit / 8)] & (1 << (bit % 8))), word = n => registers.readUInt16BE(2 + n * 2);
    return { heartbeat: flag(0), partStart: flag(10), partEnd: flag(11), resultAck: flag(12), faultReset: flag(13), visionReady: flag(20), armed: flag(21), busy: flag(22), done: flag(23),
      partSn: word(0) * 65536 + word(1), productCode: word(2), shotCount: word(3), resultCode: word(10), faultCode: word(11), resultSn: word(12) * 65536 + word(13) };
  }
  close() { this.socket.destroy(); }
}

export async function teachPressureFixture(page, options) {
  const { views, inputs, output, recordsRoot, count, allowUnpublished = false, allowDisconnected = false } = options;
  assert([1, 3].includes(views));
  const id = options.id ?? `P0-CYCLEHOST-${views}V-CLEAN`;
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const workspace = () => read('workspace_get', { id });
  const until = async (predicate, message, timeoutMs = 30000) => {
    const deadline = Date.now() + timeoutMs;
    do {
      const state = await workspace();
      if (predicate(state)) return state;
      await page.waitForTimeout(100);
    } while (Date.now() < deadline);
    throw new Error(message + ': ' + await page.locator('main').innerText());
  };
  const records = await read('records_list');
  assert(recordsRoot ? resolve(records.root).toLowerCase() === resolve(recordsRoot).toLowerCase() : records.root.includes('com.xyzrobotics.tujiaovision.p0-tests.performance'));
  const settings = await read('cycle_get_settings'), engine = await read('engine_status');
  assert(settings.vision && settings.timeouts.armMs === 200 && settings.recordKeep >= 500 && engine.backend === 'LyFlow' && engine.ready && engine.measuring);
  const provenance = JSON.parse(await readFile(join(inputs, 'provenance.json'), 'utf8'));
  assert(provenance.physicalValidation === false && provenance.files.length === count * 2);
  const source = { directory: resolve(inputs), physicalValidation: false, imageCount: provenance.files.length, size: [1280, 1024], source: 'Independent synthetic clean Gray8 PGM replay inputs; original source metadata is not used for content matching' };
  await mkdir(output, { recursive: false });
  const report = { id, views, startedAt: new Date().toISOString(), passed: false, source, scope: 'Fresh pressure-test desktop candidate taught, trialled, validated and published from explicit independent clean replay pixels', physicalValidation: false, captures: [], captureRetries: [], trials: [] };
  try {
    report.camera = await configureReplay(page, { views, directory: join(inputs, 'normal'), recordsRoot, allowUnpublished, allowDisconnected });
    await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: '配方库', exact: true }).click();
    await page.getByRole('button', { name: '复制配方 MTR-HSG-B', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: '建立候选配方', exact: true });
    await dialog.getByRole('textbox', { name: '配方编号', exact: true }).fill(id);
    await dialog.getByRole('textbox', { name: '配方名称', exact: true }).fill(`P0 队列压力 ${count}拍照点洁净对照`);
    await dialog.getByRole('spinbutton', { name: '产品代码', exact: true }).fill(String(720 + count));
    await dialog.getByRole('button', { name: '创建候选', exact: true }).click();
    await page.waitForURL('**/#/recipe/geometry');
    await page.getByRole('combobox', { name: '触发方式', exact: true }).selectOption('fly');
    for (let n = 4; n < count; n++) await page.getByRole('button', { name: '添加拍照点', exact: true }).click();
    await page.getByRole('spinbutton', { name: '站距（mm）', exact: true }).fill('1');
    await page.getByRole('spinbutton', { name: '中值滤波窗口（点，奇数）', exact: true }).fill('5');
    for (let k = 0; k < count; k++) {
      await page.getByRole('textbox', { name: `拍照点 ${k + 1} · Pose`, exact: true }).fill(`P${k + 1}`);
      await page.getByRole('combobox', { name: `拍照点 ${k + 1} · 相机`, exact: true }).selectOption('cam1');
      await page.getByRole('combobox', { name: `拍照点 ${k + 1} · 视角`, exact: true }).selectOption(String(views === 3 ? [1, 2, 3, 1][k] : 1));
    }
    await page.getByRole('spinbutton', { name: '搜索半宽（mm）', exact: true }).fill('8');
    await page.getByRole('spinbutton', { name: '胶宽下限（mm）', exact: true }).fill('1.5');
    await page.getByRole('spinbutton', { name: '胶宽上限（mm）', exact: true }).fill('6.5');
    await page.getByRole('button', { name: '保存候选配置', exact: true }).last().click();
    await until(v => v.workspace.doc.spacing === 1 && v.workspace.doc.triggerMode === 'fly' && v.workspace.doc.shots.every((s,k) => s.camera === 'cam1' && s.view === (views === 3 ? [1,2,3,1][k] : 1)), 'Candidate geometry was not saved');
    await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: '单帧示教', exact: true }).click();
    for (let k = 0; k < count; k++) {
      await page.getByRole('button', { name: `选择帧 k${k + 1}`, exact: true }).click();
      assert(!(await workspace()).workspace.frames[k].image, 'Do not overwrite an existing teaching frame');
      const captured = await captureTeachingSample(page, { readWorkspace: workspace, k, views, retryEvidence: report.captureRetries });
      report.captures.push({ k, image: captured.workspace.frames[k].image, views: captured.workspace.frames[k].views });
      const svg = page.locator('svg.wp-gray-image.editable');
      await svg.waitFor();
      await svg.scrollIntoViewIfNeeded();
      const clear = page.getByRole('button', { name: '清空中线', exact: true });
      if (await clear.isEnabled()) await clear.click();
      for (const point of [[1040, 432 + (k % 4) * 32], [240, 432 + (k % 4) * 32]]) {
        await svg.scrollIntoViewIfNeeded();
        const location = await svg.locator('g').first().evaluate((g, p) => {
          const mapped = new DOMPoint(...p).matrixTransform(g.getScreenCTM());
          return { x: mapped.x, y: mapped.y };
        }, point);
        await page.mouse.click(location.x, location.y);
      }
      await page.getByRole('spinbutton', { name: '像素当量', exact: true }).fill('0.125');
      await page.getByRole('button', { name: '保存中线', exact: true }).click();
      await until(v => v.workspace.doc.shots[k].mmPerPx === .125 && v.workspace.doc.shots[k].path.length === 2, 'Teaching line was not saved');
      await page.getByRole('button', { name: '试测当前帧', exact: true }).click();
      const tested = await until(v => !!v.workspace.frames[k].trial, 'Real DLL trial did not return');
      assert(tested.workspace.frames[k].trial.passed && tested.workspace.frames[k].trial.coverage >= .99, 'Clean real DLL trial failed');
      report.trials.push({ k, shot: tested.workspace.doc.shots[k], trial: tested.workspace.frames[k].trial });
      await page.getByRole('button', { name: '保存本帧示教', exact: true }).click();
      await until(v => v.workspace.frames[k].saved, 'Teaching frame was not saved');
      if (k === 0 || k === count - 1) await page.screenshot({ path: join(output, `teaching-k${k}.png`), fullPage: true });
      if ((k + 1) % 5 === 0) console.log(JSON.stringify({ stage: 'teaching', id, completed: k + 1, count }));
    }
    await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: '工件总览', exact: true }).click();
    await page.getByRole('button', { name: '保存总览', exact: true }).click();
    await until(v => v.workspace.overview.saved, 'Overview was not saved');
    await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: '验证与发布', exact: true }).click();
    for (const [scenario, expected] of [['normal', 'OK'], ['gap', 'NG_GAP']]) {
      const name = `${id}-${scenario}`;
      await page.getByRole('button', { name: '导入原图样本组', exact: true }).click();
      const sample = page.getByRole('dialog', { name: '导入代表性原图样本组', exact: true });
      await sample.getByRole('textbox', { name: '样本名称', exact: true }).fill(name);
      await sample.getByRole('combobox', { name: '人工确认的期望结论', exact: true }).selectOption(expected);
      for (let k = 0; k < count; k++) {
        const view = views === 3 ? [1, 2, 3, 1][k] : 1;
        await sample.locator(`input[aria-label="k${k + 1} 原图"]`).setInputFiles(join(inputs, scenario, `cam1_${k + 1}_v${view}.pgm`));
      }
      await sample.getByRole('button', { name: '保存样本组', exact: true }).click();
      await until(v => v.workspace.sampleBank.some(s => s.name === name && s.expected === expected), 'Sample group was not imported');
      await page.getByRole('checkbox', { name: '选用样本 ' + name, exact: true }).check();
    }
    await page.getByRole('button', { name: '运行规则与图像验证', exact: true }).click();
    const validated = await until(v => v.workspace.validation?.samples.length === 2, 'Real image validation did not complete');
    report.validation = validated.workspace.validation;
    assert(report.validation.passed && report.validation.samples.every(s => s.passed && s.actual === s.expected), JSON.stringify(report.validation));
    await page.screenshot({ path: join(output, 'validated.png'), fullPage: true });
    await page.getByRole('button', { name: '发布生产配方', exact: true }).click();
    await page.getByRole('dialog', { name: '发布生产配方', exact: true }).getByRole('button', { name: `确认发布 v${validated.workspace.doc.version}`, exact: true }).click();
    const published = await until(v => v.productionVersion === validated.workspace.doc.version && !v.workspace.pending, 'Validated desktop release did not activate');
    report.publication = published;
    report.layout = await read('cycle_layout', { recipeId: id });
    await page.screenshot({ path: join(output, 'published.png'), fullPage: true });
    report.passed = true;
    return report;
  } catch (error) {
    report.error = error.stack ?? String(error);
    await page.screenshot({ path: join(output, 'failure.png'), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    report.finishedAt = new Date().toISOString();
    await writeFile(join(output, 'setup-report.json'), JSON.stringify(report, null, 2) + '\n', 'utf8');
  }
}


const { values } = parseArgs({ options: {
  executable: { type: 'string' }, instance: { type: 'string' }, appdata: { type: 'string' }, output: { type: 'string' },
  settings: { type: 'string' }, cameras: { type: 'string' }, template: { type: 'string' }, inputs: { type: 'string' },
  python: { type: 'string' }, fixture: { type: 'string' }, 'fixture-config': { type: 'string' }, 'playwright-module': { type: 'string' },
  'source-gate': { type: 'string' }, 'debug-port': { type: 'string', default: '9343' },
} });
for (const key of ['executable', 'instance', 'appdata', 'output', 'settings', 'cameras', 'template', 'inputs', 'python', 'fixture', 'fixture-config', 'playwright-module', 'source-gate']) assert(values[key], 'Missing --' + key);
const paths = Object.fromEntries(Object.entries(values).filter(([key]) => !['source-gate', 'debug-port'].includes(key)).map(([key, value]) => [key, cPath(value)]));
const output = paths.output, profile = paths.appdata, recordsRoot = join(profile, 'records'), debugPort = Number(values['debug-port']);
assert.equal(profile.toLowerCase(), join(cPath(process.env.APPDATA), identifier).toLowerCase());
assert.equal(basename(profile), identifier); assert(Number.isInteger(debugPort) && debugPort > 1024 && debugPort < 65536);
assert(await absent(output), 'Never overwrite an existing attempt'); await mkdir(output, { recursive: true });
const report = { schemaVersion: 1, sourceGate: values['source-gate'], identifier, startedAt: new Date().toISOString(), passed: false,
  scope: 'Actual Replay CameraRig callbacks, bounded64/32 CycleHost queues, real DLL and external loopback Modbus TCP outputs/ACK/recovery',
  physicalValidation: false, s7HardwareValidation: false, benchmark: false, counterSource: 'synthetic',
  algorithmScope: 'Independent clean replay control only; does not resolve default noisy-metal normal P0-09', profile, stages: [], lifecycle: [], setup: [], cases: [] };
const save = (name, value) => writeFile(join(output, name), JSON.stringify(value, null, 2) + '\n', { flag: 'wx' });
const env = { ...process.env, PATH: (process.env.PATH ?? '').split(';').filter(entry => entry && !/^D:/i.test(entry)).join(';') };
const owned = []; let app, plc, browser, page, wire, stage = 'guard', attempt = null, cleanup = false;
const read = (command, args) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
function progress(next) { stage = next; report.stages.push({ stage, at: new Date().toISOString() }); console.log(JSON.stringify({ stage, output })); }
function lifecycle(event, details = {}) { report.lifecycle.push({ event, at: new Date().toISOString(), stage, cleanup, ...details }); }
function sourceGuard() {
  const repository = cPath(resolve(dirname(fileURLToPath(import.meta.url)), '..', '..'));
  const execute = args => {
    const result = spawnSync('git', args, { cwd: repository, env, encoding: 'utf8', windowsHide: true, timeout: 10000 });
    assert.equal(result.status, 0, result.error?.message ?? result.stderr); return result.stdout.trim();
  };
  const root = cPath(execute(['rev-parse', '--show-toplevel'])), head = execute(['rev-parse', 'HEAD']);
  assert.equal(root.toLowerCase(), repository.toLowerCase()); assert.equal(head, values['source-gate'], 'Current C: source must equal the sealed build source gate');
  const businessDirty = execute(['status', '--porcelain', '--untracked-files=all', '--', 'src', 'src-tauri', 'scripts', 'tests', 'package.json', 'pnpm-lock.yaml', 'Cargo.toml', 'Cargo.lock']);
  assert.equal(businessDirty, '', 'Business source or native tools changed after the source gate');
  return { repository, head, businessDirty, checkedAt: new Date().toISOString() };
}
async function start(name, executable, args, customEnv = env) {
  const child = spawn(executable, args, { cwd: dirname(paths.executable), env: customEnv, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  const state = { name, child, executable, stdout: [], stderr: [], identity: null, error: null, stopped: null }; owned.push(state);
  child.on('error', error => { state.error = String(error); }); child.stdout.on('data', data => state.stdout.push(data)); child.stderr.on('data', data => state.stderr.push(data));
  child.on('exit', (code, signal) => lifecycle('owned-child-exit', { name, pid: child.pid ?? null, executable, code, signal }));
  state.identity = await sampleProcess(child.pid, executable); return state;
}
async function stop(state) {
  if (!state) return;
  if (state.child.exitCode !== null || state.child.signalCode !== null) { state.stopped ??= { alreadyExited: true, exitCode: state.child.exitCode, signal: state.child.signalCode }; return; }
  assert(state.identity, 'No process may be stopped without recorded PID/path/start identity');
  const verified = await sampleProcess(state.child.pid, state.executable, state.identity.start), ended = new Promise(done => state.child.once('exit', done));
  state.child.kill(); await bounded(ended, 10000, 'Owned process stop timeout');
  state.stopped = { at: new Date().toISOString(), verified, exitCode: state.child.exitCode, signal: state.child.signalCode };
}
async function pressure() {
  const value = await read('pressure_test_status');
  assert(value.enabled && value.identifier === identifier && value.schemaVersion === 1);
  assert.equal(value.callbackCapacity, 64); assert.equal(value.measureCapacity, 32);
  return value;
}
async function configure(callbackHold, measureHold, timeoutMs = 10000) {
  await read('pressure_test_configure', { callbackHold, measureHold, timeoutMs });
  return until(async () => { const state = await pressure(); return state.callbackHold === callbackHold && state.measureHold === measureHold && (!callbackHold || state.callbackConsumerHeld) && state; }, 'pressure gates reached actual consumer boundary', 3000);
}
async function rig() { const state = (await read('camera_rig_status')).find(camera => camera.id === 'cam1'); assert(state?.ready && state.source === 'replay' && state.acquisition === 'triggered'); return state; }
async function emit(count, sn = null) {
  assert(Number.isInteger(count) && count >= 1 && count <= 65);
  const before = await rig(), countersBefore = (await pressure()).cameras.find(camera => camera.id === 'cam1');
  assert(countersBefore?.counterSource === 'synthetic' && countersBefore.session > 0 && Number.isSafeInteger(countersBefore.lastTriggerCounter));
  const returned = await read('pressure_test_emit', { cameraId: 'cam1', count, sn });
  const after = await until(async () => { const state = await rig(); return state.frames >= before.frames + count && state; }, 'all actual replay callbacks delivered', 4000);
  assert.equal(after.frames - before.frames, count);
  const gates = await pressure(), countersAfter = gates.cameras.find(camera => camera.id === 'cam1');
  assert.equal(countersAfter.session, countersBefore.session); assert.equal(countersAfter.counterSource, 'synthetic');
  assert.equal(countersAfter.lastTriggerCounter - countersBefore.lastTriggerCounter, count);
  return { count, sn, before, after, countersBefore, countersAfter, droppedDelta: after.droppedFrames - before.droppedFrames, returned, pressure: gates };
}
async function released(sn) {
  assert(Number.isSafeInteger(sn) && sn > 0, 'Require the expected acknowledged SN');
  return until(async () => { const state = await wire.snapshot(), cycle = await read('cycle_snapshot'), gates = await pressure();
    const spool = (await readdir(join(profile, 'audit-spool'))).filter(name => name !== '.health');
    return !state.partStart && !state.partEnd && !state.resultAck && !state.done && !state.busy && !state.armed && cycle.phase === 'IDLE' && isQuiescentCycleSnapshot(cycle) && cycle.result?.sn === sn && !gates.callbackQueued && !gates.measureQueued && !gates.callbackHold && !gates.measureHold && spool.length === 0 && { wire: state, cycle, pressure: gates, spool };
  }, 'ACK released, queues empty, audit durable and Machine IDLE', 20000);
}
async function waitDisconnected() {
  return until(async () => {
    const plc = await read('plc_get_status'), cycle = await read('cycle_snapshot');
    return plc.state === 'disconnected' && cycle.phase === 'FAULT' && cycle.fault === 'PLC 未连接' && isQuiescentCycleSnapshot(cycle) && !(await read('sim_status')).running && { plc, cycle };
  }, 'disconnected PLC, no workpiece and unlocked native PLC 未连接 FAULT');
}
async function uiPlcConnection(connected) {
  await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: 'PLC 通讯', exact: true }).click();
  const before = await read('plc_get_status');
  if ((before.state === 'connected') !== connected) {
    await page.getByRole('button', { name: connected ? /^(连接|使用已保存配置重连|重连)$/ : '断开', exact: true }).click();
  }
  const after = connected
    ? await until(async () => { const plc = await read('plc_get_status'); return plc.state === 'connected' && plc; }, 'normal UI PLC connection established')
    : await waitDisconnected();
  report.plcTransitions ??= []; report.plcTransitions.push({ connected, before, after, at: new Date().toISOString() });
  return after;
}
async function releaseFor(setup) {
  const directory = join(profile, 'vision', 'releases', setup.id), found = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    assert(!entry.isSymbolicLink(), 'Release directory may not contain links');
    if (!entry.isDirectory()) continue;
    const releaseDir = join(directory, entry.name), manifest = await json(join(releaseDir, 'manifest.json'));
    if (manifest.recipeId === setup.id && manifest.recipeRevision === setup.layout.revisionId) {
      assert.equal(manifest.bundleId, entry.name); assert.equal(manifest.recipeVersion, setup.layout.version);
      found.push({ releaseDir, manifest });
    }
  }
  assert.equal(found.length, 1, 'Fresh published recipe must resolve to exactly one explicit release ID');
  return found[0];
}
async function waitReady() {
  return until(async () => { const state = await wire.snapshot(), cycle = await read('cycle_snapshot'); return state.visionReady && !state.done && !state.busy && !state.armed && cycle.phase === 'IDLE' && isQuiescentCycleSnapshot(cycle) && { wire: state, cycle }; }, 'actual external PLC ready and native IDLE');
}
async function ready() {
  for (const address of [10, 11, 12, 13]) await wire.coil(address, false);
  await wire.coil(13, true); await sleep(150); await wire.coil(13, false);
  return waitReady();
}
async function begin(recipe, sn) {
  await wire.registers(100, [Math.floor(sn / 65536), sn % 65536, recipe.productCode, recipe.shots.length, 0, 0]); await wire.coil(10, true);
  return until(async () => { const state = await wire.snapshot(), cycle = await read('cycle_snapshot');
    assert(!state.done, 'Unexpected pre-arm refusal: ' + JSON.stringify({ state, cycle }));
    return state.armed && state.busy && cycle.phase === 'ACQUIRE' && cycle.part?.sn === sn && { wire: state, cycle, pressure: await pressure() };
  }, 'actual Modbus armed/busy and native ACQUIRE');
}
async function finish(recipe, sn, expected, tag) {
  const actual = await until(async () => { const state = await wire.snapshot(); return state.done && state; }, 'actual PLC DONE', 20000);
  attempt.done = actual;
  assert.deepEqual([actual.resultCode, actual.faultCode, actual.resultSn], [expected[0], expected[1], sn], 'Actual wire result differs');
  assert(!actual.armed);
  await wire.coil(12, true);
  await until(async () => { const state = await wire.snapshot(); return !state.done && !state.busy && !state.armed && state; }, 'actual ACK clears DONE/busy');
  for (const address of [10, 11, 12]) await wire.coil(address, false);
  const settled = await released(sn);
  const detail = await until(async () => {
    const row = (await read('history_query', { query: { sn: String(sn), recipeId: recipe.id, limit: 1 } })).items[0];
    if (!row || row.sn !== sn) return false;
    const value = await read('history_detail', { id: row.id });
    return value.summary.delivery.state === 'acknowledged' && !['pending'].includes(value.recording.state) && value;
  }, 'settled acknowledged history');
  assert.deepEqual([detail.summary.plcCode, detail.summary.faultCode, detail.summary.sn], [expected[0], expected[1], sn]);
  assert.equal(settled.cycle.result.cycleId, detail.summary.cycleId, 'Retained final result must match the acknowledged history cycle');
  assert.equal(detail.shots.length, recipe.shots.length);
  for (const [k, shot] of detail.shots.entries()) assert.deepEqual([shot.k, shot.shotId, shot.camera, shot.view], [k, recipe.shots[k].id, recipe.shots[k].camera, recipe.shots[k].view]);
  const artifacts = [];
  for (const shot of detail.shots) for (const raw of shot.rawFiles) artifacts.push({ k: shot.k, view: raw.view, ...await recordedArtifact(recordsRoot, raw.file) });
  const directories = [...new Set(artifacts.map(file => dirname(file.path)))];
  const metadata = [];
  for (const directory of directories) {
    const document = await json(join(directory, 'part.json'));
    assert.equal(document.cycleId, detail.summary.cycleId); assert.equal(document.sn, sn);
    assert.equal(document.recipeRevision, detail.summary.recipeRevision); assert.equal(document.bundleId, detail.summary.bundleId);
    for (const frame of document.frames) assert(frame.counter === 'synthetic' && frame.manual === false && frame.session > 0 && frame.frameCounter === frame.triggerCounter && frame.lostPackets === 0);
    metadata.push({ path: join(directory, 'part.json'), document });
  }
  const result = { tag, passed: true, expected: { resultCode: expected[0], faultCode: expected[1] }, actual, settled, detail, artifacts, metadata, rig: await rig(), measurements: await read('cycle_part_data'), logs: await read('cycle_logs') };
  if (expected[0] === 1) {
    assert.equal(detail.summary.verdict, 'OK'); assert.equal(detail.summary.framesReceived, recipe.shots.length);
    assert(detail.shots.every(shot => shot.status === 'done' && shot.session > 0 && !shot.error));
    assert.equal(artifacts.length, recipe.shots.length); assert.equal(detail.recording.state, 'complete');
    assert.equal(result.measurements.length, recipe.shots.length);
    for (const measured of result.measurements) {
      assert(measured.cycleId === detail.summary.cycleId && measured.bundleId === detail.summary.bundleId && measured.located && !measured.error);
      for (const field of ['ms', 'coreMs', 'engineMs', 'queueMs']) assert(typeof measured[field] === 'number' && Number.isFinite(measured[field]) && measured[field] >= 0, 'Unknown or invalid real measurement timing: ' + field);
    }
  }
  await save(tag + '.json', result); return result;
}
async function alignReplay(recipe, sn, previous, expectedPadding) {
  const before = (await pressure()).cameras.find(camera => camera.id === 'cam1');
  assert(before?.counterSource === 'synthetic' && Number.isSafeInteger(before.lastTriggerCounter));
  const nextIndex = before.lastTriggerCounter % recipe.shots.length, padding = (recipe.shots.length - nextIndex) % recipe.shots.length;
  assert.equal(padding, expectedPadding, 'Actual session counter yields an unexpected Replay alignment');
  const alignment = { source: 'Ordinary PGM Replay advances once per actual trigger and starts index0/seq0 on each new session', before, frameCount: recipe.shots.length, nextIndex, padding, callbacks: null };
  if (padding) {
    alignment.beforeRelease = await released(sn); alignment.gate = await configure(true, false);
    alignment.callbacks = await emit(padding); assert.equal(alignment.callbacks.droppedDelta, 0); assert.equal(alignment.callbacks.pressure.callbackQueued, padding);
    alignment.releasedGate = await configure(false, false); alignment.settled = await released(sn);
  }
  const after = (await pressure()).cameras.find(camera => camera.id === 'cam1');
  assert.equal(after.session, before.session); assert.equal(after.lastTriggerCounter - before.lastTriggerCounter, padding);
  assert.equal(after.lastTriggerCounter % recipe.shots.length, 0); alignment.after = after;
  await save(previous + '-replay-alignment.json', alignment); return alignment;
}
async function recovery(recipe, sn, previous) {
  attempt = { tag: 'recovery-' + previous, sn, stage: 'ready' }; await waitReady(); attempt.started = await begin(recipe, sn);
  const callbacks = [];
  for (let k = 0; k < recipe.shots.length; k++) {
    callbacks.push(await emit(1, sn));
    await until(async () => { const cycle = await read('cycle_snapshot'); return cycle.part?.frames[k]?.status === 'done' && cycle; }, 'real DLL next-part measurement k=' + k, 3000);
  }
  assert(callbacks.every(item => item.droppedDelta === 0));
  await wire.coil(11, true); const result = await finish(recipe, sn, [1, 0], attempt.tag);
  result.callbacks = callbacks; result.cameraSession = callbacks[0].countersBefore.session;
  assert(callbacks.every(item => item.countersBefore.session === result.cameraSession && item.countersAfter.session === result.cameraSession));
  await save(attempt.tag + '-callbacks.json', callbacks); report.cases.push(result); return result;
}
try {
  for (const [key, path] of Object.entries(paths).filter(([key]) => !['appdata', 'output', 'inputs'].includes(key))) {
    const metadata = await lstat(path); assert(metadata.isFile() && !metadata.isSymbolicLink()); assert(/^C:\\/i.test(await realpath(path)));
  }
  const manifest = await json(paths.instance), settings = await json(paths.settings), cameras = await json(paths.cameras), exported = await json(paths.template), fixtureConfig = await json(paths['fixture-config']);
  report.initialSourceGuard = sourceGuard();
  assert.equal(manifest.identifier, identifier); assert.equal(cPath(manifest.executable).toLowerCase(), paths.executable.toLowerCase()); assert.equal(manifest.sourceGate ?? manifest.runtimeSource, values['source-gate']);
  assert(await absent(profile), 'Require a fresh absent pressure profile; never copy a DB or published bundle');
  assert(/^C:\\/i.test(await realpath(dirname(profile))) && /^C:\\/i.test(await realpath(output)), 'Profile/evidence paths must resolve within C:');
  assert(settings.vision === true && settings.record === 'all' && settings.productSource === 'plc' && settings.recordKeep >= 500 && settings.recordMaxGb === 20);
  assert.deepEqual(settings.timeouts, { armMs: 200, motionMs: 30000, drainMs: 1000, procMs: 3000, ackMs: 5000 });
  assert(settings.lyflowCore && /^C:\\/i.test(settings.lyflowCore));
  const core = await lstat(settings.lyflowCore); assert(core.isFile() && !core.isSymbolicLink() && /^C:\\/i.test(await realpath(settings.lyflowCore)));
  for (const camera of cameras.cameras ?? []) assert(Object.values(camera).every(value => typeof value !== 'string' || !/^D:/i.test(value))); 
  assert(Array.isArray(cameras.cameras) && cameras.cameras.length === 1 && cameras.cameras[0].id === 'cam1' && cameras.cameras[0].source === 'sim' && cameras.cameras[0].acquisition === 'triggered' && !cameras.cameras[0].replayDir);
  assert.equal(fixtureConfig.schemaVersion, 1); assert.equal(fixtureConfig.plc.unitId, 1);
  for (const path of [fixtureConfig.runtimeDir, fixtureConfig.robot.recipe]) assert(/^C:\\/i.test(path), 'Fixture config paths must be explicitly C:');
  report.buildManifest = manifest; report.settings = settings;
  let cdpBusy = false; try { await fetch(`http://127.0.0.1:${debugPort}/json/version`, { signal: AbortSignal.timeout(500) }); cdpBusy = true; } catch {}
  assert(!cdpBusy, 'Native CDP port already occupied');
  await mkdir(profile); await writeFile(join(profile, 'cycle.json'), JSON.stringify(settings), { flag: 'wx' }); await writeFile(join(profile, 'cameras.json'), JSON.stringify(cameras), { flag: 'wx' });
  progress('external-modbus-fixture');
  plc = await start('modbus-fixture', paths.python, ['-u', paths.fixture, '--config', paths['fixture-config'], '--port', '0', '--output', join(output, 'modbus-wire')]);
  const port = await until(() => { if (plc.error || plc.child.exitCode !== null) throw new Error(plc.error ?? 'PLC fixture exited'); const match = Buffer.concat(plc.stdout).toString('utf8').match(/PLC Modbus TCP ready on 127\.0\.0\.1:(\d+), unit 1/); return match && Number(match[1]); }, 'owned external Modbus fixture listening');
  const socket = createConnection({ host: '127.0.0.1', port }); wire = new Modbus(socket); await bounded(new Promise((accept, reject) => { socket.once('connect', accept); socket.once('error', reject); }), 3000, 'Modbus TCP connect timeout');
  const config = structuredClone(exported.template ?? exported);
  assert(config.connection.protocol.toLowerCase().includes('modbus'));
  Object.assign(config.connection, { host: '127.0.0.1', port, timeoutMs: 1000, pollIntervalMs: 20, reconnectIntervalMs: 200 }); config.heartbeat.intervalMs = 100; config.autoConnect = false;
  await writeFile(join(profile, 'plc.json'), JSON.stringify(config), { flag: 'wx' }); report.plc = { config, fixturePid: plc.child.pid, port };
  progress('fresh-pressure-native');
  app = await start('native-pressure', paths.executable, [], { ...env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort} --remote-debugging-address=127.0.0.1` });
  await until(async () => { if (app.error || app.child.exitCode !== null) throw new Error(app.error ?? 'Native app exited'); try { return (await fetch(`http://127.0.0.1:${debugPort}/json/version`, { signal: AbortSignal.timeout(500) })).ok; } catch { return false; } }, 'owned native CDP');
  const { chromium } = await import(pathToFileURL(paths['playwright-module']).href); browser = await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`);
  browser.on('disconnected', () => lifecycle('browser-disconnected'));
  report.cdpConnectedAt = new Date().toISOString(); lifecycle('browser-connected', { debugPort, pid: app.child.pid });
  const native = await until(async () => { const candidates = []; for (const candidate of browser.contexts().flatMap(context => context.pages())) if (await candidate.evaluate(() => !!window.__TAURI_INTERNALS__?.invoke).catch(() => false)) candidates.push(candidate); return candidates.length && candidates; }, 'native invoke page');
  assert.equal(native.length, 1); page = native[0];
  page.on('crash', () => lifecycle('page-crash', { url: page.url() }));
  page.on('close', () => lifecycle('page-close', { url: page.url() }));
  lifecycle('native-page-selected', { url: page.url() });
  await page.waitForURL('http://tauri.localhost/**'); await page.getByRole('navigation', { name: '操作导航' }).waitFor();
  assert.equal(cPath((await read('records_list')).root).toLowerCase(), recordsRoot.toLowerCase()); await sampleProcess(app.child.pid, paths.executable, app.identity.start);
  assert.equal((await read('history_query', { query: { limit: 1 } })).total, 0);
  assert.deepEqual(await read('cycle_get_settings'), settings); report.initialPressure = await pressure();
  report.offlinePreparation = await waitDisconnected();
  report.engine = await until(async () => { const engine = await read('engine_status'); return engine.ready && engine.measuring && engine.backend === 'LyFlow' && engine; }, 'real DLL ready');
  assert(/^C:\\/i.test(report.engine.path));
  const prepared = [];
  for (const count of [4, 40]) {
    progress('fresh-ui-teach-publish-' + count);
    const expanded = join(output, 'input-' + count); await mkdir(expanded);
    const files = [];
    for (const scenario of ['normal', 'gap']) {
      await mkdir(join(expanded, scenario));
      for (let k = 0; k < count; k++) {
        const source = join(paths.inputs, '1-view', scenario, `cam1_${k % 4 + 1}_v1.pgm`), file = join(expanded, scenario, `cam1_${k + 1}_v1.pgm`);
        const metadata = await lstat(source); assert(metadata.isFile() && !metadata.isSymbolicLink() && /^C:\\/i.test(await realpath(source)));
        await recordedArtifact(dirname(source), basename(source)); await writeFile(file, await readFile(source), { flag: 'wx' }); files.push({ source, file, bytes: metadata.size, k, scenario });
      }
    }
    await writeFile(join(expanded, 'provenance.json'), JSON.stringify({ physicalValidation: false, source: 'Explicit clean synthetic PGM fixture copied as source pixels into new pressure input groups; no production assets or DB copied', files }), { flag: 'wx' });
    const setup = await teachPressureFixture(page, { views: 1, inputs: expanded, output: join(output, 'setup-' + count), recordsRoot, allowUnpublished: true, allowDisconnected: true, count, id: 'P0-PRESSURE-' + count });
    assert(setup.passed && setup.layout.shots.length === count); const release = await releaseFor(setup); prepared.push({ count, input: expanded, recipe: await read('recipe_doc', { id: setup.id }), setup, release }); report.setup.push({ count, id: setup.id, report: join(output, 'setup-' + count, 'setup-report.json') });
  }
  report.offlinePublicationComplete = await waitDisconnected();
  await uiPlcConnection(true); await ready();
  for (const specification of [{ tag: 'callback-missing-91', count: 4, prefill: 64, emits: 4, drops: 4, expected: [90, 91] }, { tag: 'callback-extra-96', count: 4, prefill: 60, emits: 5, drops: 1, expected: [90, 96] }, { tag: 'measure-full-99', count: 40, expected: [90, 99] }]) {
    const selected = prepared.find(item => item.count === specification.count), recipe = selected.recipe, sn = 750000100 + report.cases.length;
    report.lastSourceGuard = sourceGuard();
    progress(specification.tag); await ready(); await uiPlcConnection(false);
    await configureReplay(page, { views: 1, directory: join(selected.input, 'normal'), recordsRoot, allowDisconnected: true });
    await uiPlcConnection(true); await ready();
    attempt = { ...specification, sn, stage: 'recipe-preparation', before: { rig: await rig(), pressure: await pressure() } };
    attempt.recipeReady = await waitForPublishedRecipeReady(page, { layout: selected.setup.layout, bundleId: selected.release.manifest.bundleId });
    attempt.readyWire = await waitReady(); attempt.stage = 'hold';
    if (specification.prefill) {
      attempt.gate = await configure(true, false); attempt.prefill = await emit(specification.prefill);
      assert.equal(attempt.prefill.droppedDelta, 0); assert.equal(attempt.prefill.pressure.callbackQueued, specification.prefill);
      attempt.started = await begin(recipe, sn); attempt.callbacks = await emit(specification.emits, sn);
      assert.equal(attempt.callbacks.droppedDelta, specification.drops); assert.equal(attempt.callbacks.pressure.callbackQueued, 64);
      attempt.releasedGate = await configure(false, false);
      attempt.consumed = await until(async () => { const state = await pressure(), cycle = await read('cycle_snapshot'); return state.callbackQueued === 0 && cycle.part?.received === (specification.expected[1] === 96 ? 4 : 0) && cycle.part.queue === 0 && { pressure: state, cycle }; }, 'old callbacks consumed and planned real DLL frames settled before partEnd', 2500);
      await wire.coil(11, true);
    } else {
      attempt.gate = await configure(false, true); attempt.started = await begin(recipe, sn); attempt.firstEmitStartedAt = Date.now(); attempt.first = await emit(1, sn);
      attempt.held = await until(async () => { const state = await pressure(); return state.measureBlockedConsumer && state.heldJob?.sn === sn && state.heldJob.k === 0 && state.heldJob.cycleId === attempt.started.cycle.part.cycleId && state; }, 'first actual measurement job held before consumer advances', 1000);
      attempt.callbacks = await emit(39, sn); assert.equal(attempt.callbacks.droppedDelta, 0);
      attempt.saturated = await until(async () => { const state = await pressure(), cycle = await read('cycle_snapshot'); return state.measureQueued === 32 && state.measureQueueFull - attempt.before.pressure.measureQueueFull === 7 && cycle.part?.received === 40 && { pressure: state, cycle }; }, 'real measure queue32 and seven actual try_send Full', 1500);
      attempt.releaseRequestedAt = Date.now(); attempt.queueHeldUpperBoundMs = attempt.releaseRequestedAt - attempt.firstEmitStartedAt;
      assert(attempt.queueHeldUpperBoundMs < settings.timeouts.procMs, 'Pressure gate must release before the actual first-job proc budget');
      attempt.queueAccounting = { emitted: 40, heldOutsideQueue: 1, queued: 32, rejectedFull: attempt.saturated.pressure.measureQueueFull - attempt.before.pressure.measureQueueFull };
      assert.equal(attempt.queueAccounting.emitted, attempt.queueAccounting.heldOutsideQueue + attempt.queueAccounting.queued + attempt.queueAccounting.rejectedFull);
      attempt.releasedGate = await configure(false, false);
      attempt.measured = await until(async () => { const cycle = await read('cycle_snapshot'); return cycle.part?.received === 40 && cycle.part?.queue === 0 && cycle; }, 'original real DLL jobs settled after release', 2500);
      await wire.coil(11, true);
    }
    attempt.stage = 'actual-result-ack'; const result = await finish(recipe, sn, specification.expected, specification.tag); result.observations = attempt;
    if (specification.expected[1] === 91) assert(result.detail.shots.every(shot => shot.status === 'missing') && result.artifacts.length === 0);
    if (specification.expected[1] === 96) assert.equal(result.detail.summary.framesReceived, 4);
    if (specification.expected[1] === 99) {
      const rejected = result.detail.shots.filter(shot => shot.error?.includes('测量队列已满'));
      assert.equal(rejected.length, 7, 'Exactly seven real jobs must be rejected by try_send Full');
      assert(result.detail.shots.filter(shot => !rejected.includes(shot)).every(shot => shot.status === 'done' && !shot.error), 'All held/queued real DLL jobs must complete');
      assert(result.detail.shots.every(shot => !shot.error?.includes('超时')) && result.measurements.every(measurement => !measurement.error?.includes('超时')), 'T_proc timeouts cannot count as queue-full acceptance');
    }
    await save(specification.tag + '-observations.json', attempt); report.cases.push(result);
    const failedCameraSession = attempt.callbacks.countersBefore.session;
    const alignment = await alignReplay(recipe, sn, specification.tag, specification.expected[1] === 96 ? 3 : 0); result.replayAlignment = alignment;
    const recovered = await recovery(recipe, sn + 1, specification.tag);
    assert.equal(recovered.cameraSession, failedCameraSession, 'Recovery must retain the actual pressure-case camera session');
    const failedSessions = result.metadata.flatMap(entry => entry.document.frames.map(frame => frame.session)), recoverySessions = recovered.metadata.flatMap(entry => entry.document.frames.map(frame => frame.session));
    if (failedSessions.length) assert(failedSessions.every(session => recoverySessions.includes(session)), 'Recovery must use the same actual camera session');
    await sampleProcess(app.child.pid, paths.executable, app.identity.start);
  }
  report.finalSourceGuard = sourceGuard();
  report.final = { cycle: await read('cycle_snapshot'), pressure: await pressure(), rig: await rig(), wire: await wire.snapshot() };
  assert(report.cases.length === 6 && report.cases.every(result => result.passed)); assert.equal(report.final.cycle.phase, 'IDLE'); report.passed = true;
} catch (error) {
  report.failedStage = stage; report.error = error.stack ?? String(error); report.failedAttempt = attempt; process.exitCode = 1;
  if (page) {
    try { report.lastNative = { cycle: await read('cycle_snapshot'), pressure: await pressure(), cameras: await read('camera_rig_status'), history: await read('history_query', { query: { limit: 10 } }), logs: await read('cycle_logs') }; } catch (failure) { report.nativeReadError = String(failure); }
    await page.screenshot({ path: join(output, 'failure.png'), fullPage: true }).catch(failure => { report.screenshotError = String(failure); });
  }
  if (wire) try { report.lastWire = await wire.snapshot(); } catch (failure) { report.wireReadError = String(failure); }
} finally {
  cleanup = true; lifecycle('cleanup-start');
  const cleanupErrors = [];
  if (page) try { await read('pressure_test_configure', { callbackHold: false, measureHold: false, timeoutMs: 100 }); } catch (error) { cleanupErrors.push('release gates: ' + error); }
  if (browser) await browser.close().catch(error => { cleanupErrors.push('CDP: ' + error); });
  try { await stop(app); } catch (error) { cleanupErrors.push('owned app: ' + error); }
  if (wire) wire.close();
  try { await stop(plc); } catch (error) { cleanupErrors.push('owned fixture: ' + error); }
  for (const state of [...owned].reverse()) if (state !== app && state !== plc) try { await stop(state); } catch (error) { cleanupErrors.push('other owned process: ' + error); }
  for (const state of owned) for (const stream of ['stdout', 'stderr']) try { await writeFile(join(output, `${state.name}-${state.child.pid}-${stream}.log`), Buffer.concat(state[stream]), { flag: 'wx' }); } catch (error) { cleanupErrors.push('process evidence: ' + error); }
  report.processCleanup = owned.map(state => ({ name: state.name, identity: state.identity, stopped: state.stopped, error: state.error }));
  if (cleanupErrors.length) { report.cleanupErrors = cleanupErrors; report.passed = false; process.exitCode = 1; }
  report.finishedAt = new Date().toISOString(); await save('report.json', report); console.log(JSON.stringify({ passed: report.passed, failedStage: report.failedStage, output, error: report.error }));
}
