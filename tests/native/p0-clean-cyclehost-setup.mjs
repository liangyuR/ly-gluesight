import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';

export async function configureReplay(page, { views, directory, recordsRoot, allowUnpublished = false, allowDisconnected = false }) {
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const records = await read('records_list'), cycle = await read('cycle_snapshot');
  assert(recordsRoot ? resolve(records.root).toLowerCase() === resolve(recordsRoot).toLowerCase() : records.root.includes('com.xyzrobotics.tujiaovision.p0-tests.performance'));
  assert(/^[cC]:[\\/]/.test(directory));
  const disconnected = allowDisconnected && cycle.phase === 'FAULT' && cycle.fault === 'PLC 未连接' && !cycle.part && cycle.plcLocked === false && (await read('plc_get_status')).state === 'disconnected';
  assert((cycle.phase === 'IDLE' || disconnected || allowUnpublished && cycle.phase === 'FAULT' && !cycle.part && cycle.fault?.includes('没有一个配方开得了工')) && !(await read('sim_status')).running);
  await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: '设备与采集', exact: true }).click();
  await page.getByRole('button', { name: '回放目录', exact: true }).click();
  await page.getByRole('combobox', { name: '设备视角', exact: true }).selectOption(String(views));
  await page.getByRole('button', { name: '触发（飞拍）', exact: true }).click();
  await page.getByRole('textbox', { name: '图片目录', exact: true }).fill(directory);
  await page.getByRole('spinbutton', { name: views === 3 ? '回放设备编号（0=首设备）' : '通道', exact: true }).fill('1');
  await page.getByRole('button', { name: '保存并应用', exact: true }).click();
  await page.waitForFunction(async ({ views, directory }) => {
    const camera = (await window.__TAURI_INTERNALS__.invoke('camera_rig_config'))[0];
    return camera.source === 'replay' && camera.viewCount === views && camera.replayDir === directory && camera.replayChannel === 1;
  }, { views, directory });
  return (await read('camera_rig_config'))[0];
}

export async function teachCleanFixture(page, options) {
  const { views, inputs, output, recordsRoot, allowUnpublished = false } = options;
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
  assert(provenance.physicalValidation === false && provenance.files.length === 32);
  const source = { directory: resolve(inputs), physicalValidation: false, imageCount: provenance.files.length, size: [1280, 1024], source: 'Independent synthetic clean Gray8 PGM replay inputs; original source metadata is not used for content matching' };
  await mkdir(output, { recursive: false });
  const report = { id, views, startedAt: new Date().toISOString(), passed: false, source, scope: 'Independent desktop candidate taught, trialled, validated and published from CLEAN Prepared pixels via replay camera', physicalValidation: false, captures: [], trials: [] };
  try {
    report.camera = await configureReplay(page, { views, directory: join(inputs, `${views}-view`, 'normal'), recordsRoot, allowUnpublished });
    await page.getByRole('navigation', { name: '操作导航' }).getByRole('link', { name: '配方库', exact: true }).click();
    await page.getByRole('button', { name: '复制配方 MTR-HSG-B', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: '建立候选配方', exact: true });
    await dialog.getByRole('textbox', { name: '配方编号', exact: true }).fill(id);
    await dialog.getByRole('textbox', { name: '配方名称', exact: true }).fill(`P0 CycleHost ${views}视角洁净性能对照`);
    await dialog.getByRole('spinbutton', { name: '产品代码', exact: true }).fill(String(710 + views));
    await dialog.getByRole('button', { name: '创建候选', exact: true }).click();
    await page.waitForURL('**/#/recipe/geometry');
    await page.getByRole('combobox', { name: '触发方式', exact: true }).selectOption('fly');
    await page.getByRole('spinbutton', { name: '站距（mm）', exact: true }).fill('1');
    await page.getByRole('spinbutton', { name: '中值滤波窗口（点，奇数）', exact: true }).fill('5');
    for (let k = 0; k < 4; k++) {
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
    for (let k = 0; k < 4; k++) {
      await page.getByRole('button', { name: `选择帧 k${k + 1}`, exact: true }).click();
      assert(!(await workspace()).workspace.frames[k].image, 'Do not overwrite an existing teaching frame');
      await page.getByRole('button', { name: '取新样本', exact: true }).click();
      const captured = await until(v => v.workspace.frames[k].views.length === views, 'Replay capture did not complete');
      report.captures.push({ k, image: captured.workspace.frames[k].image, views: captured.workspace.frames[k].views });
      const svg = page.locator('svg.wp-gray-image.editable');
      await svg.waitFor();
      await svg.scrollIntoViewIfNeeded();
      const clear = page.getByRole('button', { name: '清空中线', exact: true });
      if (await clear.isEnabled()) await clear.click();
      for (const point of [[1040, 432 + k * 32], [240, 432 + k * 32]]) {
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
      await page.screenshot({ path: join(output, `teaching-k${k}.png`), fullPage: true });
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
      for (let k = 0; k < 4; k++) {
        const view = views === 3 ? [1, 2, 3, 1][k] : 1;
        await sample.locator(`input[aria-label="k${k + 1} 原图"]`).setInputFiles(join(inputs, `${views}-view`, scenario, `cam1_${k + 1}_v${view}.pgm`));
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

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { values } = parseArgs({ options: { 'playwright-module': { type: 'string' }, views: { type: 'string' }, inputs: { type: 'string' }, output: { type: 'string' }, cdp: { type: 'string', default: 'http://127.0.0.1:9338' } } });
  const { chromium } = await import(pathToFileURL(resolve(values['playwright-module'])).href);
  const browser = await chromium.connectOverCDP(values.cdp);
  try {
    const pages = browser.contexts().flatMap(c => c.pages());
    const native = [];
    for (const page of pages) if (await page.evaluate(() => !!window.__TAURI_INTERNALS__).catch(() => false)) native.push(page);
    assert(native.length === 1, 'Require one isolated native Tauri page');
    const report = await teachCleanFixture(native[0], { views: Number(values.views), inputs: resolve(values.inputs), output: resolve(values.output) });
    process.stdout.write(JSON.stringify({ id: report.id, passed: report.passed }) + '\n');
  } finally { await browser.close(); }
}
