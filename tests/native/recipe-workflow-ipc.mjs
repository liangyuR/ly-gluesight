import assert from 'node:assert/strict';
import { mkdir, writeFile, copyFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

const { values } = parseArgs({ options: {
  'playwright-module': { type: 'string' }, images: { type: 'string' },
  output: { type: 'string' }, cdp: { type: 'string', default: 'http://127.0.0.1:9351' },
  restart: { type: 'boolean', default: false },
  'skip-engine': { type: 'boolean', default: false },
} });
for (const key of ['playwright-module', 'images', 'output']) assert(values[key], `Missing --${key}`);
assert(values['skip-engine'], 'This workflow check requires --skip-engine; algorithm acceptance is separate');
const { chromium } = await import(pathToFileURL(resolve(values['playwright-module'])).href);
const browser = await chromium.connectOverCDP(values.cdp);
const output = resolve(values.output), id = 'RECIPE-WORKFLOW-IPC';
const report = { passed: false, fixtureSource: 'Generated persisted complete capture records; no live camera capture',
  physicalAcceptance: false, algorithmQualityAcceptance: false, engineSkipped: values['skip-engine'], startedAt: new Date().toISOString(), checks: [] };
await mkdir(output, { recursive: true });
try {
  const pages = [];
  for (const page of browser.contexts().flatMap(context => context.pages())) {
    if (await page.evaluate(() => !!window.__TAURI_INTERNALS__).catch(() => false)) pages.push(page);
  }
  assert.equal(pages.length, 1);
  const page = pages[0];
  const invoke = (command, args) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const records = await invoke('records_list'), profile = dirname(records.root);
  assert(profile.endsWith('com.xyzrobotics.gluesight.recipe-workflow-20261011'));
  assert(/^[cC]:[\\/]/.test(profile));
  let plc = await invoke('plc_get_config');
  if (!values.restart && plc.connection.protocol !== 'simulator') {
    assert.equal((await invoke('plc_get_status')).state, 'disconnected');
    const specs = [
      ['partStart', 'C10', 'bool'], ['partEnd', 'C11', 'bool'], ['resultAck', 'C12', 'bool'],
      ['faultReset', 'C13', 'bool'], ['partSn', 'HR100', 'u32'], ['productCode', 'HR102', 'u16'],
      ['shotCount', 'HR103', 'u16'], ['visionReady', 'C20', 'bool'], ['armed', 'C21', 'bool'],
      ['busy', 'C22', 'bool'], ['done', 'C23', 'bool'], ['resultCode', 'HR110', 'u16'],
      ['faultCode', 'HR111', 'u16'], ['resultSn', 'HR112', 'u32'],
    ];
    plc = { ...plc, connection: { ...plc.connection, protocol: 'simulator', host: '127.0.0.1' },
      autoConnect: false, heartbeat: { pointId: null, intervalMs: 500 },
      points: specs.map(([tag, address, dataType]) => ({ id: `p_${tag}`, name: tag, address, dataType,
        access: 'readWrite', edge: ['partStart', 'partEnd', 'resultAck', 'faultReset'].includes(tag) ? 'rising' : 'none',
        tags: [tag], wordOrder: dataType === 'bool' ? null : 'ABCD', logChanges: true, description: '隔离软件回归' })) };
    await invoke('plc_save_config', { config: plc });
  }
  assert.equal(plc.connection.protocol, 'simulator');
  await invoke('plc_connect');
  assert((await invoke('camera_rig_config')).every(camera => camera.source === 'sim'));
  report.profile = profile;
  if (values.restart) {
    const restored = await invoke('workspace_get', { id });
    assert.equal(restored.workspace.doc.shots.length, 20);
    assert.equal(restored.productionVersion, null);
    assert.equal(restored.workspace.pending, null);
    assert.deepEqual(restored.workspace.lastPosition, { k: 7, view: 2 });
    assert.equal(restored.workspace.doc.shots[7].views.filter(view => view.enabled).length, 2);
    assert(restored.workspace.frames[7].viewStates.filter(view => view.view <= 2).every(view => !view.saved));
    report.checks.push('Application restart restores explicit capture identity, selected views, persisted teaching draft and editor position');
  } else {
    assert(!(await invoke('workspace_list')).some(workspace => workspace.doc.id === id), 'Do not overwrite an existing test candidate');
    assert.equal((await invoke('history_query', { query: { limit: 1 } })).items.length, 0);
    let camera = (await invoke('camera_rig_config'))[0];
    camera = { ...camera, viewCount: 3 };
    await invoke('camera_save_config', { cam: 0, config: camera });
    const template = await invoke('recipe_template');
    const doc = { ...template, id, name: '配方创建真实接口回归', productCode: 62011, triggerMode: 'fly', shots: [], limits: { ...template.limits, position: null } };
    let state = await invoke('workspace_create', { doc });
    assert.equal(state.workspace.frames.length, 0);
    report.checks.push('Empty draft creation through real IPC');
    const round = async suffix => {
      const roundId = `capture-20261011-${suffix}`, frames = [];
      for (let k = 0; k < 20; k++) {
        const shot = join(profile, 'recipe-capture', roundId, `shot-${String(k + 1).padStart(4, '0')}`);
        await mkdir(shot, { recursive: true });
        const originalPath = join(shot, 'original.png');
        await copyFile(join(resolve(values.images), 'original.png'), originalPath);
        const views = [];
        for (let view = 1; view <= 3; view++) {
          const path = join(shot, `view-${view}.png`);
          await copyFile(join(resolve(values.images), 'view.png'), path);
          views.push({ view, path, width: 300, height: 300 });
        }
        frames.push({ shotId: `S${String(k + 1).padStart(4, '0')}`, ordinal: k + 1, frameCounter: k + 1, triggerCounter: k + 1, originalPath, views });
      }
      const record = { roundId, recipeId: id, cameraId: camera.id, cameraConfig: camera, deviceSession: 1, plannedCount: 20,
        plcPlannedCount: 20, plcActualCount: 20, plcPlanVersion: 1, receivedCount: 20, state: 'complete',
        createdAt: Date.now(), endedAt: Date.now(), frames, error: null, simulated: true };
      const text = JSON.stringify(record).replace(/"(fps|triggerDelayUs|exposureUs|gainDb)":(-?\d+)(?=[,}])/g, '"$1":$2.0');
      await writeFile(join(profile, 'recipe-capture', roundId, 'round.json'), text, { flag: 'wx' });
      return roundId;
    };
    const teaching = await round(1), sample = await round(2);
    const mutate = async (command, args) => state = await invoke(command, { id, revision: state.workspace.revision, ...args });
    await mutate('workspace_adopt_capture', { roundId: teaching, reuseTeaching: false, correspondenceConfirmed: false });
    assert.equal(state.workspace.doc.shots.length, 20);
    assert(state.workspace.frames.every(frame => frame.views.length === 3));
    report.checks.push('Adopt 20 complete persisted shots with three independent views');
    await assert.rejects(invoke('workspace_capture_sample', { id, revision: state.workspace.revision, roundId: teaching, expected: 'OK', correspondenceConfirmed: true }), /示教采集不能/);
    await assert.rejects(invoke('workspace_restore_teach', { id, revision: state.workspace.revision, k: 0 }), /整圈示教不能/);
    await assert.rejects(invoke('workspace_publish', { id, revision: state.workspace.revision }), /完整验证/);
    report.checks.push('Reject teaching-source self-validation, single-frame restore and unvalidated publication');
    for (let k = 0; k < 20; k++) {
      const selected = Array.from({ length: k % 3 + 1 }, (_, i) => i + 1);
      await mutate('workspace_set_views', { k, views: selected, skip: false });
      for (const view of selected) {
        await mutate('workspace_select_view', { k, view });
        await mutate('workspace_save_draft', { k, params: { path: [[270, 150], [30, 150]], mmPerPx: 0.1, detect: null, limits: null } });
        await mutate('workspace_check_calibration', { k, referenceMm: 10, measuredMm: 10, reuseMatching: true });
      }
    }
    report.checks.push('Independent persisted drafts and calibration-check IPC for 39 selected views; algorithm trials skipped');
    await mutate('workspace_capture_sample', { roundId: sample, expected: 'OK', correspondenceConfirmed: true });
    await mutate('workspace_validate', { samples: [{ historyId: null, sampleId: sample, expected: 'OK' }], sourceConfirmed: true });
    report.validation = state.workspace.validation;
    assert.equal(state.workspace.validation.passed, false, JSON.stringify(state.workspace.validation));
    await mutate('workspace_progress', { k: 7, view: 2 });
    await assert.rejects(invoke('workspace_publish', { id, revision: state.workspace.revision }), /完整验证/);
    report.checks.push('Independent sample accepted; incomplete teaching/engine validation prevents publication');
    assert.equal(state.workspace.pending, null, state.workspace.publishError);
    assert.equal(state.productionVersion, null);
    assert.equal((await invoke('history_query', { query: { limit: 1 } })).items.length, 0);
    report.checks.push('Draft and validation operations do not create production versions or inspection records');
    report.publication = { productionVersion: state.productionVersion, captureId: state.workspace.captureId };
  }
  report.passed = true;
} catch (error) {
  report.error = error.stack ?? String(error);
  throw error;
} finally {
  report.finishedAt = new Date().toISOString();
  await writeFile(join(output, values.restart ? 'restart-report.json' : 'ipc-report.json'), JSON.stringify(report, null, 2));
  await browser.close();
}
console.log(JSON.stringify({ passed: report.passed, checks: report.checks }));
