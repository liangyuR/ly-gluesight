import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { lstat, mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { dirname, join, resolve, win32 } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs, promisify } from 'node:util';
import { recipeContract } from '../../scripts/robot-plc-demo/camera-bridge.mjs';

const execute = promisify(execFile);
const commandEnvironment = { ...process.env, PATH: (process.env.PATH ?? '').split(';').filter(entry => !/^d:/i.test(entry)).join(';') };
const identifier = 'com.xyzrobotics.gluesight.robot-plc-demo';
const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const defaultsPath = join(repository, 'scripts/robot-plc-demo/fixtures/ROBOT-DEMO-TRICAM.json');
const json = async file => JSON.parse((await readFile(file, 'utf8')).replace(/^\uFEFF/, ''));
const save = (file, value) => writeFile(file, JSON.stringify(value, null, 2) + '\n', { flag: 'wx' });

export function cPath(value) {
  assert(typeof value === 'string' && /^[cC]:[\\/]/.test(value), 'Require an explicit absolute C-drive path');
  const path = win32.normalize(value);
  assert(!path.startsWith('\\\\') && !path.slice(2).includes(':'), 'Unsupported device path or alternate stream');
  return path;
}

export function assertDefaultContract(doc, defaults) {
  assert(Number.isSafeInteger(doc.version) && doc.version > 0, 'Missing actual candidate version');
  assert.deepEqual(recipeContract(doc), recipeContract({ ...defaults, version: doc.version }), 'Default Robot parameters must remain unchanged');
}

export function assertStrictValidation(validation, names) {
  assert(validation && validation.samples?.length === 2, 'Require exactly two real image validation results');
  for (const [name, expected] of [[names.good, 'OK'], [names.gap, 'NG_GAP']]) {
    const rows = validation.samples.filter(sample => sample.name === name);
    assert(rows.length === 1 && rows[0].expected === expected, 'Strict sample identity or expected label changed');
    assert(rows[0].passed && rows[0].actual === expected, `Strict accuracy failure: ${name}, expected ${expected}, actual ${rows[0].actual}; publication stopped`);
  }
  assert(validation.passed && validation.checks?.length && validation.checks.every(check => check.passed), 'Publication checks did not pass');
}

export function assertSite(site, profile, dll) {
  assert.equal(cPath(site.records.root).toLowerCase(), win32.join(cPath(profile), 'records').toLowerCase(), 'Wrong Demo profile');
  assert(!site.cycle.part && ['IDLE', 'FAULT'].includes(site.cycle.phase) && !site.sim.running, 'A cycle or simulator is active');
  assert(site.cameras.length && site.cameras.every(camera => camera.source === 'sim' && camera.acquisition === 'triggered'), 'Only triggered Sim devices are allowed');
  const cam1 = site.cameras.filter(camera => camera.id === 'cam1');
  assert(cam1.length === 1 && cam1[0].viewCount === 3, 'Require cam1 with three simultaneous views');
  const connection = site.plc.connection;
  assert(connection.protocol === 'modbusTcp' && connection.host === '127.0.0.1' && Number.isInteger(connection.port), 'Require loopback Modbus Demo PLC');
  assert(site.settings.productSource === 'plc' && site.settings.vision && site.settings.record === 'all', 'Require PLC product selection, actual vision and all raw recording');
  assert.equal(site.settings.timeouts.armMs, 200, 'Do not relax the arming limit');
  assert.equal(site.settings.timeouts.procMs, 3000, 'Do not relax the processing limit');
  assert(site.engine.backend === 'LyFlow' && site.engine.ready && site.engine.measuring, 'Require a ready real DLL');
  assert.equal(cPath(site.engine.path).toLowerCase(), cPath(dll).toLowerCase(), 'Unexpected DLL path');
}

async function ordinaryPath(file, requireFile = false) {
  file = cPath(file);
  for (let current = file;; current = dirname(current)) {
    const info = await lstat(current);
    assert(!info.isSymbolicLink(), 'Refuse reparse or symbolic-link paths: ' + current);
    if (current === file && requireFile) assert(info.isFile(), 'Require an ordinary file: ' + file);
    cPath(await realpath(current));
    if (dirname(current) === current) break;
  }
  return file;
}

async function absent(file) {
  try { await lstat(file); return false; } catch (error) { if (error.code === 'ENOENT') return true; throw error; }
}

async function processIdentity(instance, cdp) {
  assert(Number.isSafeInteger(instance.pid) && instance.pid > 0 && instance.processStartTime, 'Missing exact owned process identity');
  const url = new URL(cdp);
  assert(url.protocol === 'http:' && url.hostname === '127.0.0.1' && /^\d+$/.test(url.port) && url.pathname === '/' && !url.username && !url.password && !url.search && !url.hash, 'Require an explicit loopback CDP port');
  const port = Number(url.port);
  assert(port > 1024 && port < 65536);
  const script = `$ErrorActionPreference='Stop'; $app=Get-Process -Id ${instance.pid}; $owners=@(Get-NetTCPConnection -State Listen -LocalPort ${port} | Select-Object -ExpandProperty OwningProcess -Unique); if($owners.Count -ne 1){throw 'Ambiguous CDP listener'}; $cursor=[int]$owners[0]; $chain=@(); for($i=0;$i -lt 32;$i++){ $chain+=$cursor; if($cursor -eq ${instance.pid}){break}; $item=Get-CimInstance Win32_Process -Filter ('ProcessId='+$cursor); if(!$item){throw 'CDP owner disappeared'}; $cursor=[int]$item.ParentProcessId }; if($cursor -ne ${instance.pid}){throw 'CDP listener does not belong to the Demo process'}; [pscustomobject]@{pid=$app.Id;path=$app.Path;start=$app.StartTime.ToUniversalTime().ToString('o');cdpOwners=$owners;ancestorChain=$chain}|ConvertTo-Json -Compress`;
  const { stdout } = await execute('pwsh', ['-NoProfile', '-NonInteractive', '-Command', script], { cwd: repository, env: commandEnvironment, windowsHide: true, timeout: 15000 });
  const identity = JSON.parse(stdout);
  assert.equal(cPath(identity.path).toLowerCase(), cPath(instance.executable).toLowerCase(), 'PID executable mismatch');
  assert.equal(identity.start, instance.processStartTime, 'PID was recycled or the native process restarted');
  return identity;
}

async function selfTest() {
  const { test } = await import('node:test');
  const defaults = await json(defaultsPath);
  test('C paths reject relative, other drives, UNC and alternate streams', () => {
    for (const path of ['relative.json', 'D:\\old\\recipe.json', '\\\\server\\file', 'C:\\safe\\file:stream']) assert.throws(() => cPath(path));
    assert.equal(cPath('C:/safe/recipe.json'), 'C:\\safe\\recipe.json');
  });
  test('default contract accepts actual versions and inherited settings but rejects weakened limits', () => {
    const doc = structuredClone(defaults); doc.version = 7;
    for (const shot of doc.shots) { delete shot.detect; delete shot.limits; }
    assertDefaultContract(doc, defaults);
    doc.limits.width.tolUpper = 8;
    assert.throws(() => assertDefaultContract(doc, defaults));
  });
  test('site guard rejects another profile, replay pixels, remote PLC and relaxed processing budgets', () => {
    const profile = 'C:\\profiles\\' + identifier, dll = 'C:\\native\\lyflow_core.dll';
    const site = { records: { root: win32.join(profile, 'records') }, cycle: { phase: 'FAULT', part: null }, sim: { running: false },
      cameras: [{ id: 'cam1', source: 'sim', acquisition: 'triggered', viewCount: 3 }], plc: { connection: { protocol: 'modbusTcp', host: '127.0.0.1', port: 1502 } },
      settings: { productSource: 'plc', vision: true, record: 'all', timeouts: { armMs: 200, procMs: 3000 } },
      engine: { backend: 'LyFlow', ready: true, measuring: true, path: dll } };
    assertSite(site, profile, dll);
    for (const mutate of [value => { value.records.root = 'C:\\profiles\\production\\records'; }, value => { value.cameras[0].source = 'replay'; },
      value => { value.plc.connection.host = '192.168.1.1'; }, value => { value.settings.timeouts.procMs = 6000; }, value => { value.cycle.part = { sn: 42 }; }]) {
      const invalid = structuredClone(site); mutate(invalid); assert.throws(() => assertSite(invalid, profile, dll));
    }
  });
  test('strict normal 2 is retained as failure and cannot authorize publication', () => {
    const names = { good: 'strict-good', gap: 'strict-gap' };
    const value = { passed: true, checks: [{ passed: true }], samples: [{ name: names.good, expected: 'OK', actual: 'OK_WITH_EXCURSION', passed: true }, { name: names.gap, expected: 'NG_GAP', actual: 'NG_GAP', passed: true }] };
    assert.throws(() => assertStrictValidation(value, names), /Strict accuracy failure/);
    value.samples[0].expected = 'OK_WITH_EXCURSION';
    assert.throws(() => assertStrictValidation(value, names), /label changed/);
    value.samples[0] = { name: names.good, expected: 'OK', actual: 'OK', passed: true };
    assertStrictValidation(value, names);
    value.samples.push({ ...value.samples[0] });
    assert.throws(() => assertStrictValidation(value, names));
  });
}

async function main() {
  const { values } = parseArgs({ options: {
    stage: { type: 'string' }, instance: { type: 'string' }, output: { type: 'string' }, dll: { type: 'string' },
    samples: { type: 'string' }, 'playwright-module': { type: 'string' }, 'source-gate': { type: 'string' },
    cdp: { type: 'string', default: 'http://127.0.0.1:9340' }, 'self-test': { type: 'boolean' },
  } });
  if (values['self-test']) return selfTest();
  assert(process.platform === 'win32', 'This runner requires Windows');
  cPath(process.execPath);
  assert(['prepare', 'publish'].includes(values.stage), 'Use --stage prepare or publish');
  for (const key of ['instance', 'output', 'dll', 'playwright-module', 'source-gate']) assert(values[key], 'Missing --' + key);
  const profile = join(cPath(process.env.APPDATA), identifier), output = cPath(values.output), dll = cPath(values.dll);
  assert(output.toLowerCase() !== profile.toLowerCase() && !output.toLowerCase().startsWith(profile.toLowerCase() + '\\'), 'Evidence must be outside the existing Demo profile');
  for (const file of [values.instance, dll, values['playwright-module'], defaultsPath]) await ordinaryPath(file, true);
  await ordinaryPath(profile);
  const instance = await json(values.instance), defaults = await json(defaultsPath), id = defaults.id;
  assert.equal(instance.identifier, identifier);
  assert.equal(instance.sourceGate, values['source-gate'], 'Build manifest must attest the requested source gate');
  const { stdout } = await execute('git', ['rev-parse', 'HEAD'], { cwd: repository, env: commandEnvironment, windowsHide: true });
  assert(/^[a-f0-9]{7,40}$/i.test(values['source-gate']) && stdout.trim().startsWith(values['source-gate']), 'Source gate differs from current repository HEAD');
  const dirty = await execute('git', ['diff', 'HEAD', '--name-only', '--', 'src-tauri', 'src', 'scripts/robot-plc-demo'], { cwd: repository, env: commandEnvironment, windowsHide: true });
  assert.equal(dirty.stdout.trim(), '', 'Business source changed after the declared build gate');
  const untracked = await execute('git', ['ls-files', '--others', '--exclude-standard', '--', 'src-tauri', 'src', 'scripts/robot-plc-demo'], { cwd: repository, env: commandEnvironment, windowsHide: true });
  assert.equal(untracked.stdout.trim(), '', 'Untracked business source is outside the declared build gate');
  await ordinaryPath(cPath(instance.executable), true);
  const identity = await processIdentity(instance, values.cdp);
  if (values.stage === 'prepare') {
    assert(await absent(output), 'Never overwrite an earlier attempt directory');
    await ordinaryPath(dirname(output));
    await mkdir(output);
  } else await ordinaryPath(output);
  const reportPath = join(output, `${values.stage}-report.json`);
  assert(await absent(reportPath), 'Never overwrite stage evidence');
  const report = { schemaVersion: 1, startedAt: new Date().toISOString(), stage: values.stage, passed: false, published: false, strictValidationPassed: false, robotFiveScenariosPassed: false,
    scope: 'Actual default noisy Sim four-shot desktop teaching and strict image validation; no clean fixture or label relaxation',
    physicalValidation: false, s7HardwareValidation: false, benchmark: false, identifier, profile, sourceGate: values['source-gate'],
    sourceAttestation: 'Build manifest source gate plus current source/PID/start-time/CDP ancestry guards; no executable content fingerprint', instance, process: identity, captures: [], trials: [] };
  let browser, page, read;
  try {
    const { chromium } = await import(pathToFileURL(cPath(values['playwright-module'])).href);
    browser = await chromium.connectOverCDP(values.cdp);
    const pages = [];
    for (const candidate of browser.contexts().flatMap(context => context.pages())) if (await candidate.evaluate(() => !!window.__TAURI_INTERNALS__?.invoke).catch(() => false)) pages.push(candidate);
    assert.equal(pages.length, 1, 'Require exactly one native Demo page');
    page = pages[0]; assert(new URL(page.url()).hostname === 'tauri.localhost');
    read = (command, args) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
    const guard = async () => {
      await processIdentity(instance, values.cdp);
      const site = { records: await read('records_list'), cycle: await read('cycle_snapshot'), sim: await read('sim_status'),
        cameras: await read('camera_rig_config'), plc: await read('plc_get_config'), settings: await read('cycle_get_settings'), engine: await read('engine_status') };
      assertSite(site, profile, dll);
      await ordinaryPath(site.records.root);
      assert.equal(site.plc.connection.port, 1502, 'This default Robot fixture expects port 1502');
      const robot = await fetch('http://127.0.0.1:8766/state', { signal: AbortSignal.timeout(2000) }).then(response => { assert(response.ok); return response.json(); });
      assert(robot.contractVersion === 2 && robot.plcEndpoint === '127.0.0.1:1502' && robot.unitId === 1, 'Wrong Robot/PLC service');
      assert(robot.running === false && robot.plc && ['partStart','partEnd','resultAck','faultReset','armed','busy','done'].every(key => robot.plc[key] === false || robot.plc[key] === 0), 'Robot/PLC handshake is active or unknown');
      report.lastGuard = { ...site, robot: { running: robot.running, phase: robot.phase, plc: robot.plc }, app: await read('app_info') };
    };
    await guard();
    const nav = label => page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: label, exact: true }).click();
    const workspace = () => read('workspace_get', { id });
    const until = async (predicate, label, timeout = 30000) => {
      for (const deadline = Date.now() + timeout; Date.now() < deadline;) {
        const state = await workspace(); if (predicate(state)) return state;
        await page.waitForTimeout(100);
      }
      throw new Error(label + ': ' + await page.locator('main').innerText());
    };
    const checkWorkspace = state => {
      assertDefaultContract(state.workspace.doc, defaults);
      assert(state.workspace.frames.length === 4 && state.workspace.frames.every((frame, k) => frame.k === k && frame.saved && frame.trial?.passed && frame.image?.source === 'Sim' && frame.image?.camera === 'cam1' && frame.image?.view === defaults.shots[k].view && frame.image?.size?.[0] === 1280 && frame.image?.size?.[1] === 1024 && frame.views?.length === 3 && frame.views.every(image => image.source === 'Sim')), 'Require four saved, actually trialled, full-resolution default Sim frames');
      assert(state.workspace.overview.saved && !state.workspace.pending, 'Overview must be saved with no pending publication');
    };
    if (values.stage === 'prepare') {
      for (const file of [join(profile, 'workspaces', id), join(profile, 'recipes', `${id}.json`)]) assert(await absent(file), 'Refuse to replace existing recipe/workspace: ' + file);
      await nav('配方库');
      await page.getByRole('button', { name: '新建飞拍', exact: true }).click();
      const dialog = page.getByRole('dialog', { name: '建立候选配方', exact: true });
      await dialog.getByRole('textbox', { name: '配方编号', exact: true }).fill(id);
      await dialog.getByRole('textbox', { name: '配方名称', exact: true }).fill(defaults.name);
      await dialog.getByRole('spinbutton', { name: '产品代码', exact: true }).fill(String(defaults.productCode));
      await dialog.getByRole('button', { name: '创建候选', exact: true }).click();
      await page.waitForURL('**/#/recipe/geometry');
      const rows = page.getByRole('textbox', { name: /^拍照点 \d+ · 编号$/ });
      while (await rows.count() < 4) await page.getByRole('button', { name: '添加拍照点', exact: true }).click();
      while (await rows.count() > 4) await page.getByRole('button', { name: `删除拍照点 ${await rows.count()}`, exact: true }).click();
      await page.getByRole('combobox', { name: '触发方式', exact: true }).selectOption('fly');
      for (const [name, value] of [['站距（mm）', defaults.spacing], ['中值滤波窗口（点，奇数）', defaults.filterWindow], ['搜索半宽（mm）', defaults.detect.searchMm], ['胶宽下限（mm）', defaults.detect.widthRange[0]], ['胶宽上限（mm）', defaults.detect.widthRange[1]], ['允许断胶长度（mm）', defaults.limits.maxGapLen]]) await page.getByRole('spinbutton', { name, exact: true }).fill(String(value));
      await page.getByRole('combobox', { name: '极性', exact: true }).selectOption(defaults.detect.polarity);
      const paramLabels = { nominal: '名义', tolUpper: '上公差', tolLower: '下公差', absMin: '绝对下限', absMax: '绝对上限', maxExcursionLen: '允许超差长度' };
      for (const [kind, label] of [['position', '位置'], ['width', '胶宽']]) {
        const enable = page.getByRole('button', { name: `启用${label}判定`, exact: true });
        if (await enable.count()) await enable.click();
        for (const [field, name] of Object.entries(paramLabels)) await page.getByRole('spinbutton', { name: `${label} · ${name}`, exact: true }).fill(String(defaults.limits[kind][field]));
      }
      for (const [k, shot] of defaults.shots.entries()) {
        for (const [label, value] of [['编号', shot.id], ['Pose', shot.poseId], ['胶条', shot.bead], ['标定引用', '']]) await page.getByRole('textbox', { name: `拍照点 ${k + 1} · ${label}`, exact: true }).fill(value);
        await page.getByRole('combobox', { name: `拍照点 ${k + 1} · 相机`, exact: true }).selectOption('cam1');
        await page.getByRole('combobox', { name: `拍照点 ${k + 1} · 视角`, exact: true }).selectOption(String(shot.view === 1 ? 2 : 1));
      await page.getByRole('combobox', { name: `拍照点 ${k + 1} · 视角`, exact: true }).selectOption(String(shot.view));
        await page.getByRole('checkbox', { name: `拍照点 ${k + 1} · 不检`, exact: true }).uncheck();
      }
      await page.getByRole('button', { name: '保存候选配置', exact: true }).last().click();
      await until(state => state.workspace.doc.shots.length === 4 && state.workspace.doc.detect.searchMm === 15 && state.workspace.doc.shots.every((shot, k) => shot.poseId === defaults.shots[k].poseId && shot.view === defaults.shots[k].view), 'Default geometry was not saved');
      await nav('单帧示教');
      for (const [k, shot] of defaults.shots.entries()) {
        await guard();
        await page.getByRole('button', { name: `选择帧 k${k + 1}`, exact: true }).click();
        assert(!(await workspace()).workspace.frames[k].image, 'Do not replace an earlier teaching frame');
        const capture = async () => {
          const previous = (await workspace()).workspace.frames[k].image?.id;
          await page.getByRole('button', { name: '取新样本', exact: true }).click();
          const state = await until(state => state.workspace.frames[k].image && state.workspace.frames[k].image.id !== previous && state.workspace.frames[k].views.length === 3, 'Sim capture did not complete');
          const frame = state.workspace.frames[k];
          assert(frame.views.every(image => image.source === 'Sim' && image.size[0] === 1280 && image.size[1] === 1024));
          report.captures.push({ k, image: frame.image, views: frame.views });
        };
        await capture();
        const svg = page.locator('svg.wp-gray-image.editable'); await svg.waitFor();
        const clear = page.getByRole('button', { name: '清空中线', exact: true }); if (await clear.isEnabled()) await clear.click();
        for (const point of shot.path) {
          await svg.scrollIntoViewIfNeeded();
          const location = await svg.locator('g').first().evaluate((group, point) => { const mapped = new DOMPoint(...point).matrixTransform(group.getScreenCTM()); return { x: mapped.x, y: mapped.y }; }, point);
          await page.mouse.click(location.x, location.y);
        }
        await page.getByRole('spinbutton', { name: '像素当量', exact: true }).fill(String(shot.mmPerPx));
        await page.getByRole('button', { name: '保存中线', exact: true }).click();
        await until(state => { try { assert.deepEqual(state.workspace.doc.shots[k].path, shot.path); return state.workspace.doc.shots[k].mmPerPx === shot.mmPerPx; } catch { return false; } }, 'Default teaching geometry was not saved');
        await capture();
        await page.getByRole('button', { name: '试测当前帧', exact: true }).click();
        const tested = await until(state => !!state.workspace.frames[k].trial, 'Real DLL trial did not return');
        report.trials.push({ k, shot: tested.workspace.doc.shots[k], trial: tested.workspace.frames[k].trial });
        assert(tested.workspace.frames[k].trial.passed && tested.workspace.frames[k].trial.measurement, 'Actual image trial failed; do not save or publish');
        await page.getByRole('button', { name: '保存本帧示教', exact: true }).click();
        await until(state => state.workspace.frames[k].saved, 'Teaching frame was not saved');
        await page.screenshot({ path: join(output, `teaching-k${k + 1}.png`), fullPage: true });
      }
      await nav('工件总览'); await page.getByRole('button', { name: '保存总览', exact: true }).click();
      const prepared = await until(state => state.workspace.overview.saved, 'Overview was not saved');
      checkWorkspace(prepared);
      assert(!prepared.productionVersion, 'New candidate must not already be production');
      report.workspace = prepared;
      await save(join(output, 'robot-recipe.json'), prepared.workspace.doc);
      const config = await json(join(repository, 'scripts/robot-plc-demo/demo.config.json'));
      config.robot.recipe = join(output, 'robot-recipe.json'); config.runtimeDir = join(output, 'runtime');
      await save(join(output, 'demo.config.json'), config);
      report.next = { workspace: join(profile, 'workspaces', id), config: join(output, 'demo.config.json'), samples: join(output, 'samples'),
        action: 'Run make-samples.py externally with --config/--workspace/--output; then invoke this script --stage publish. No production version has been published.' };
    } else {
      assert(values.samples, 'Missing --samples exported by make-samples.py');
      const samples = await ordinaryPath(cPath(values.samples)), prepared = await json(await ordinaryPath(join(output, 'prepare-report.json'), true));
      assert(prepared.passed && prepared.identifier === identifier && prepared.sourceGate === values['source-gate'], 'Require a successful matching prepare stage');
      const fixture = await json(await ordinaryPath(join(output, 'robot-recipe.json'), true)); assertDefaultContract(fixture, defaults);
      const before = await workspace(); checkWorkspace(before);
      assert(!before.productionVersion && before.workspace.revision === prepared.workspace.workspace.revision, 'Candidate changed or was published after preparation');
      assert.deepEqual(recipeContract(before.workspace.doc), recipeContract(fixture));
      const provenance = await json(await ordinaryPath(join(samples, 'provenance.json'), true));
      assert(provenance.imageEngineValidated === false && provenance.physicalValidation === false && provenance.plannedDeviceTriggers?.cam1 === 4 && provenance.frames?.length === 12, 'Require external default-Sim sample export provenance');
      assert.deepEqual(recipeContract(provenance.recipe), recipeContract(fixture));
      for (const group of ['good', 'gap']) for (let k = 1; k <= 4; k++) await ordinaryPath(join(samples, group, `k${k}.pgm`), true);
      report.provenance = provenance;
      await nav('配方库');
      await page.locator('.wp-recipe-card').filter({ has: page.getByText(id, { exact: true }) }).getByRole('button', { name: '配置候选', exact: true }).click();
      await page.waitForURL('**/#/recipe/geometry');
      await nav('验证与发布');
      assert((await workspace()).workspace.sampleBank.length === 0, 'Do not modify a candidate with earlier validation samples');
      const names = { good: `${id}-STRICT-OK`, gap: `${id}-STRICT-GAP` };
      for (const [group, expected] of [['good', 'OK'], ['gap', 'NG_GAP']]) {
        await page.getByRole('button', { name: '导入原图样本组', exact: true }).click();
        const dialog = page.getByRole('dialog', { name: '导入代表性原图样本组', exact: true });
        await dialog.getByRole('textbox', { name: '样本名称', exact: true }).fill(names[group]);
        await dialog.getByRole('combobox', { name: '人工确认的期望结论', exact: true }).selectOption(expected);
        for (let k = 1; k <= 4; k++) await dialog.locator(`input[aria-label="k${k} 原图"]`).setInputFiles(join(samples, group, `k${k}.pgm`));
        await dialog.getByRole('button', { name: '保存样本组', exact: true }).click();
        await until(state => state.workspace.sampleBank.some(sample => sample.name === names[group] && sample.expected === expected), 'Strict original-image group was not imported');
        await page.getByRole('checkbox', { name: `选用样本 ${names[group]}`, exact: true }).check();
      }
      await page.getByRole('button', { name: '运行规则与图像验证', exact: true }).click();
      const validated = await until(state => state.workspace.validation?.samples.length === 2, 'Real strict image validation did not complete');
      report.validation = validated.workspace.validation;
      await page.screenshot({ path: join(output, 'strict-validation.png'), fullPage: true });
      assertStrictValidation(report.validation, names);
      report.strictValidationPassed = true;
      await guard(); assertDefaultContract(validated.workspace.doc, defaults);
      assert.deepEqual(recipeContract(validated.workspace.doc), recipeContract(fixture), 'Candidate version or actual parameters changed before publication');
      await page.getByRole('button', { name: '发布生产配方', exact: true }).click();
      await page.getByRole('dialog', { name: '发布生产配方', exact: true }).getByRole('button', { name: `确认发布 v${fixture.version}`, exact: true }).click();
      report.publication = await until(state => state.productionVersion === fixture.version && !state.workspace.pending, 'Validated release did not activate');
      const layout = await read('cycle_layout', { recipeId: id }); assert.deepEqual(recipeContract(layout), recipeContract(fixture));
      assert.equal(layout.revisionId, `${id}-v${fixture.version}`);
      report.layout = layout; report.published = true;
      await page.screenshot({ path: join(output, 'published.png'), fullPage: true });
      report.next = { config: join(output, 'demo.config.json'), executableSource: cPath(instance.executable), executableDestination: join(output, 'runtime', 'GlueSight-Robot-PLC.exe'),
        action: 'Stop only the owned original Demo services using their original config. Create this new runtime directory and copy the just-verified current Demo executable there without overwriting files; then Start-Demo.ps1 -Config this exported config, so both Robot and bridge load the actual-version fixture. Run run-regression.py with the same config; keep normal expected code 1/0. No services or processes were started by this runner.' };
    }
    report.passed = true;
  } catch (error) {
    report.error = error.stack ?? String(error);
    if (page) await page.screenshot({ path: join(output, `${values.stage}-failure.png`), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    report.finishedAt = new Date().toISOString();
    await save(reportPath, report);
    if (browser) await browser.close();
    console.log(JSON.stringify({ stage: values.stage, passed: report.passed, published: report.published, report: reportPath, next: report.next }));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) main().catch(error => { console.error(error.stack ?? error); process.exitCode = 1; });
