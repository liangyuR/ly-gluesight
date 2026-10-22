import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  captureRound, client, gateRetries, identifier, instanceCdp, MISSING_INPUT_EXIT, missingInputs, OK_CLASS, prepareSampleSet, produce, teach, uiPage, until, workspaceCalls,
} from "./lib.mjs";

// 夹具（FIXTURES）、样本集副本、整圈采集、分组示教与投产等共用流程在 ./lib.mjs。
const verdictLabel = {
  OK: "OK", OK_WITH_EXCURSION: "OK · 局部超差", NG_POSITION: "NG · 位置超差", NG_WIDTH: "NG · 胶宽超差",
  NG_ABSOLUTE: "NG · 超绝对限", NG_GAP: "NG · 断胶", ERR_INSPECT: "ERR · 未测成",
};
const sampleLabelText = { ok: "合格", ng: "不合格", unknown: "未标注" };

function assertSampleSetCalibration(view, taught, setId, mmPerPx) {
  const step = view.progress.steps.find(item => item.id === "calibration");
  assert.equal(step.status, "done", `标定步骤未完成：${JSON.stringify(step)}`);
  for (const { k, shotId } of taught) {
    const views = view.progress.shots[k].views.filter(v => v.enabled);
    assert(views.length && views.every(v => v.calibration.state === "sampleSet" && v.calibration.setId === setId
      && Math.abs(v.calibration.mmPerPx - mmPerPx) < 1e-6), `${shotId} 标定依据不是样本集声明当量：${JSON.stringify(views)}`);
  }
  return { status: step.status, detail: step.detail };
}

async function exportHistory(page, call, recipeId) {
  await page.getByRole("button", { name: "导出 CSV", exact: true }).click();
  const notice = page.getByText(/^已导出：/);
  await notice.waitFor();
  const file = (await notice.locator(".mono").innerText()).trim();
  const lines = (await readFile(file, "utf8")).trim().split(/\r?\n/);
  const { total } = await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } });
  assert.equal(lines.length - 1, total, `导出 CSV 行数与历史记录不符：${file}`);
  return { file, rows: lines.length - 1 };
}

async function replayInHistory(page, call, recipeId, summary) {
  const before = { detail: await call("history_detail", { id: summary.id }), workspace: (await call("workspace_get", { id: recipeId })).workspace };
  await page.getByRole("button", { name: `查看 SN ${summary.sn} 的记录`, exact: true }).click();
  await page.getByRole("button", { name: "使用该配方候选", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "切换配方工作台的当前配方", exact: true });
  await dialog.waitFor();
  const confirm = dialog.getByRole("button", { name: `切换到 ${recipeId}`, exact: true });
  if (await confirm.isEnabled()) await confirm.click();
  else await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  const titles = { original: "原发布包重现结果", rules: "规则重判结果", raw: "候选原图复测结果" };
  const results = {};
  for (const [button, source] of [["按原发布包重现", "original"], ["按候选规则重判", "rules"], ["从原图复测整件", "raw"]]) {
    const prior = await call("workspace_comparisons", { id: recipeId, historyId: summary.id });
    const control = page.getByRole("button", { name: button, exact: true });
    await until(() => control.isEnabled(), Boolean, `历史详情“${button}”不可用`, 15000);
    await control.click();
    const result = await until(async () => (await call("workspace_comparisons", { id: recipeId, historyId: summary.id })).find(c => c.source === source && !prior.some(p => p.id === c.id)),
      Boolean, `“${button}”未生成对照结果`, 90000);
    await page.getByRole("heading", { name: titles[source], exact: true }).waitFor();
    assert.equal(result.cycleId, summary.cycleId);
    assert.equal(result.originalVerdict, summary.verdict);
    assert.equal(result.judgement.verdict, summary.verdict, `${titles[source]}与原结论不一致：${result.judgement.reason}`);
    if (source === "original") {
      assert.equal(result.bundleId, summary.bundleId, "原发布包重现未使用本件冻结的发布包");
      assert.equal(result.candidateRecipe.revisionId, summary.recipeRevision);
    }
    results[source] = { id: result.id, verdict: result.judgement.verdict, reason: result.judgement.reason, via: "ui" };
  }
  for (const [source, { id }] of Object.entries(results)) {
    await page.locator('select[aria-label="已保存对照结果"]').selectOption(id);
    await page.getByRole("heading", { name: titles[source], exact: true }).waitFor();
  }
  assert.deepEqual(await call("history_detail", { id: summary.id }), before.detail, "复测对照改动了原始生产记录");
  assert.deepEqual((await call("workspace_get", { id: recipeId })).workspace, before.workspace, "复测对照改动了候选示教");
  return results;
}

export async function runOfflineE2E(page, options) {
  const startedAt = Date.now();
  const steps = [];
  const step = (name, detail) => steps.push({ name, atMs: Date.now() - startedAt, ...(detail ? { detail } : {}) });
  const call = client(page);
  const navigation = page.getByRole("navigation", { name: "操作导航" });

  const records = await call("records_list");
  assert(records.root.includes(identifier), `数据目录不属于隔离实例：${records.root}`);
  const initialSite = await call("site_status");
  assert.equal(initialSite.mode, "real", "软件启动必须处于真机模式");
  assert.equal((await call("history_query", { query: { limit: 1 } })).total, 0, "隔离实例应从全新站点开始");
  assert((await call("workspace_list")).every(w => !w.captureId), "全新站点不应已有采集建立的候选");
  const before = { plc: await call("plc_get_config"), cameras: await call("camera_rig_config") };
  const settings = await call("cycle_get_settings");
  if (settings.productSource !== "plc" || options.armMs) {
    const timeouts = options.armMs ? { ...settings.timeouts, armMs: options.armMs } : settings.timeouts;
    await call("cycle_save_settings", { settings: { ...settings, productSource: "plc", timeouts } });
  }
  const engine = await until(() => call("engine_status"), e => e.ready, "lyFlow 引擎未就绪", 30000);
  step("lyFlow 就绪", { version: engine.version ?? null });

  const prepared = await prepareSampleSet(options.sampleSet, options.workDir);
  const { fixture } = prepared;
  const preview = await call("site_preview_sample_set", { dir: prepared.dir });
  const normalPart = fixture.normalPart, defect = fixture.defect;
  for (const partId of [normalPart, defect?.partId].filter(Boolean)) assert(preview.parts.some(p => p.partId === partId), `样本集缺少 ${partId}`);
  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: prepared.dir } });
  assert.equal(entered.mode, "offline");
  assert.equal(entered.source, "sampleSet");
  assert.equal(entered.measurement, "lyFlow");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const cameras = await call("camera_rig_config");
  assert.deepEqual(cameras.map(c => [c.id, c.source]), prepared.manifest.cameras.map(c => [c.id, "sampleSet"]), "离线相机组应按样本集相机生成");
  const cameraIds = cameras.map(c => c.id);
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("heading", { name: "当前：离线 · 样本集", exact: true }).waitFor();
  const noRecipe = page.getByRole("note").filter({ hasText: /尚无可(开工|在当前样本集 \/ 相机组运行)的生产配方/ });
  await noRecipe.waitFor();
  const noRecipeText = (await noRecipe.innerText()).trim();
  step("进入离线·样本集", { port: entered.port, cameraIds, noRecipeText });

  const template = await call("recipe_template");
  const created = await call("workspace_create", { doc: template });
  const id = created.workspace.doc.id;
  const ws = workspaceCalls(call, id);
  const teachRound = await captureRound(call, id, cameraIds, normalPart);
  assert.equal(teachRound.plannedCount * cameraIds.length, preview.shotCount, "每次触发各相机一帧，拍照点数应等于样本集拍照点数");
  const adopted = await ws.edit("workspace_adopt_capture", { roundId: teachRound.roundId, reuseTeaching: false, correspondenceConfirmed: false });
  assert.equal(adopted.workspace.doc.shots.length, preview.shotCount);
  step("首次建站采集并采用", { recipeId: id, roundId: teachRound.roundId, triggers: teachRound.plannedCount, shots: preview.shotCount });

  const taught = await teach(ws, adopted, fixture);
  const calibration = assertSampleSetCalibration(await ws.get(), taught, prepared.manifest.id, fixture.mmPerPx);
  step("示教完成", { groups: taught.map(t => `${t.pose}:${t.shotId}`), calibration });

  const samples = [];
  for (const [partId, expected] of [[normalPart, "OK"], ...(defect ? [[defect.partId, defect.expected]] : [])]) {
    const round = await captureRound(call, id, cameraIds, partId);
    await ws.edit("workspace_capture_sample", { roundId: round.roundId, expected, correspondenceConfirmed: true });
    samples.push({ partId, roundId: round.roundId, expected });
  }
  const validation = (await ws.edit("workspace_validate", { samples: samples.map(s => ({ historyId: null, sampleId: s.roundId, expected: s.expected })), sourceConfirmed: true })).workspace.validation;
  assert(validation.passed, `验证未通过：${JSON.stringify(validation.checks.filter(c => !c.passed))}`);
  const normalSample = validation.samples.find(s => s.sampleId === samples[0].roundId);
  assert.equal(normalSample.expected, "OK");
  assert(OK_CLASS.includes(normalSample.actual) && normalSample.passed, `合格样本应判为符合：${normalSample.actual} ${normalSample.reason}`);
  if (defect) {
    const defectSample = validation.samples.find(s => s.sampleId === samples[1].roundId);
    assert.equal(defectSample.actual, defect.expected, `缺陷样本验证结论不符：${defectSample.reason}`);
  }
  const branches = { calibration: "sampleSetDeclared", normalExpectation: normalSample.actual === "OK" ? "ok" : "okClass" };
  step("验证通过", { branches, samples: validation.samples.map(s => ({ name: s.name, expected: s.expected, actual: s.actual, passed: s.passed })) });

  await ws.edit("workspace_publish", {});
  await until(async () => (await call("cycle_recipes")).find(r => r.id === id), Boolean, "发布后生产配方未生效", 30000);
  const active = await until(() => ws.get(), v => !v.workspace.pending && v.workspace.publish.state === "active", "待生效版本未提交", 30000);
  assert.equal(active.workspace.publish.validationSite, "offline", "离线发布的版本必须标记离线验证");
  const recipe = await call("cycle_layout", { recipeId: id });
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  const runSelect = page.locator('select[aria-label="离线配方"]');
  await until(() => runSelect.inputValue(), value => value === id, "发布后离线配方未默认选中新配方", 15000);
  step("发布生效", { recipeId: id, publish: active.workspace.publish });

  const produced = {};
  for (const partId of [normalPart, defect?.partId].filter(Boolean)) produced[partId] = await produce(call, id, partId);
  const normal = produced[normalPart];
  assert(OK_CLASS.includes(normal.summary.verdict), `投产 ${normalPart} 应为合格类：${normal.summary.verdict} ${normal.summary.reason}`);
  const defective = defect && produced[defect.partId];
  if (defective) {
    assert.equal(defective.summary.verdict, defect.expected, `投产 ${defect.partId} 应为 ${defect.expected}：${defective.summary.reason}`);
    assert(defective.summary.reason.includes(`${defect.pose}-${defect.camera}`), `断胶原因未指向 ${defect.pose}-${defect.camera}：${defective.summary.reason}`);
  }
  for (const { summary, run } of Object.values(produced)) {
    assert.equal(run.verdict, summary.verdict);
    assert.equal(summary.sample?.partId, run.partId, `历史记录缺少样本件引用：${JSON.stringify(summary.sample)}`);
    assert.equal(summary.sample.setId, prepared.manifest.id);
    assert.equal(summary.sample.label, run.label);
  }
  assert.equal((await call("history_query", { query: { scope: "offline", limit: 1 } })).total, Object.keys(produced).length,
    "示教与验证的整圈采集不得生成检测记录");
  step("投产", Object.fromEntries(Object.entries(produced).map(([partId, { summary }]) => [partId, summary.verdict])));

  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  const recentTable = page.getByRole("table", { name: "最近样本件", exact: true });
  for (const { run } of Object.values(produced)) {
    const row = recentTable.getByRole("row").filter({ has: page.getByRole("cell", { name: run.partSn, exact: true }) });
    await row.waitFor();
    const cells = (await row.getByRole("cell").allInnerTexts()).map(text => text.trim());
    assert.equal(cells[1], run.partId);
    assert.equal(cells[5], sampleLabelText[run.label], `最近样本件 ${run.partId} 期望列不符`);
    assert.equal(cells[6], verdictLabel[run.verdict], `最近样本件 ${run.partId} 实际列不符`);
  }
  assert.equal((await call("history_query", { query: { limit: 1 } })).total, 0, "离线件不得进入真机历史与统计");
  await navigation.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  await page.getByRole("combobox", { name: "历史配方", exact: true }).click();
  await page.getByRole("option", { name: id, exact: true }).click();
  for (const { summary } of Object.values(produced)) {
    const row = page.getByRole("row").filter({ has: page.getByRole("button", { name: `查看 SN ${summary.sn} 的记录`, exact: true }) });
    await row.getByText(`样本 ${summary.sample.partId}（${sampleLabelText[summary.sample.label]}）`, { exact: true }).waitFor();
    await row.getByText(verdictLabel[summary.verdict], { exact: true }).waitFor();
  }
  step("界面核对");
  const exported = await exportHistory(page, call, id);
  const replayed = (defective ?? normal).summary;
  const replay = await replayInHistory(page, call, id, replayed);
  step("历史导出与复测对照", { exported: exported.rows, replay: Object.fromEntries(Object.entries(replay).map(([k, v]) => [k, v.verdict])) });

  if (options.output) {
    await mkdir(path.dirname(options.output), { recursive: true });
    await page.screenshot({ path: path.join(path.dirname(options.output), "offline-e2e-history.png"), fullPage: true });
  }
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("button", { name: "退出离线", exact: true }).click();
  await until(() => call("site_status"), s => s.mode === "real", "未退出离线模式");
  await page.getByRole("combobox", { name: "离线来源", exact: true }).waitFor();
  assert.deepEqual(await call("plc_get_config"), before.plc, "退出离线后 PLC 配置未恢复");
  assert.deepEqual(await call("camera_rig_config"), before.cameras, "退出离线后相机组未恢复");
  step("退出离线并恢复");

  const result = {
    passed: true,
    identifier,
    inputs: { sampleSet: options.sampleSet, offlineSampleSet: prepared.dir, sampleSetId: prepared.manifest.id, mmPerPx: fixture.mmPerPx, lyflow: options.lyflow, lyflowVersion: engine.version ?? null, armBudgetMs: options.armMs ?? null },
    elapsedMs: Date.now() - startedAt,
    recipe: { id, revisionId: recipe.revisionId, teachingId: recipe.teachingId ?? null },
    taught,
    calibration,
    branches,
    gateRetries: Object.entries(gateRetries.reduce((counts, r) => ({ ...counts, [`${r.command}：${r.error}`]: (counts[`${r.command}：${r.error}`] ?? 0) + 1 }), {})).map(([what, count]) => ({ what, count })),
    validation: { checks: validation.checks.map(c => ({ name: c.name, passed: c.passed })), samples: validation.samples.map(s => ({ name: s.name, expected: s.expected, actual: s.actual, passed: s.passed })) },
    production: Object.fromEntries(Object.values(produced).map(({ summary, run }) => [run.partId, { historyId: summary.id, sn: summary.sn, partSn: run.partSn, verdict: summary.verdict, plcCode: summary.plcCode, reason: summary.reason, cycleId: summary.cycleId, recipeRevision: summary.recipeRevision, bundleId: summary.bundleId, sample: summary.sample }])),
    exported,
    replayPart: replayed.sample.partId,
    replay: { historyId: replayed.id, originalVerdict: replayed.verdict, ...replay },
    steps,
    boundary: "进程内 S7 服务与虚拟产线驱动真实 lyFlow 测量现场样本原图；像素当量为模拟标定，断胶件为副本内人工擦除胶段，笔划与检测参数是演示夹具，结论不代表算法质量、现场标定或现场节拍",
  };
  if (options.output) await writeFile(options.output, JSON.stringify(result, null, 2) + "\n");
  return result;
}

async function main() {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) {
    assert(process.argv[i].startsWith("--") && process.argv[i + 1], "参数必须为 --name value");
    args[process.argv[i].slice(2)] = process.argv[i + 1];
  }
  const inputs = {
    sampleSet: args["sample-set"] ?? process.env.GLUESIGHT_SAMPLE_SET,
    lyflow: args["lyflow-runtime"] ?? process.env.GLUESIGHT_LYFLOW_RUNTIME,
  };
  const problems = missingInputs(inputs);
  if (problems.length) {
    process.stderr.write(`离线端到端输入缺失，未运行（不算通过）：\n${problems.join("\n")}\n`);
    process.exitCode = MISSING_INPUT_EXIT;
    return;
  }
  const instance = args.instance ?? "output/playwright/e2e-regression/instance.json";
  const cdp = await instanceCdp(instance, args.cdp, id => id === identifier);
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = await uiPage(browser);
    const output = args.output ?? "output/playwright/e2e-regression/result.json";
    const armMs = args["arm-ms"] === undefined ? undefined : Number(args["arm-ms"]);
    assert(armMs === undefined || (Number.isInteger(armMs) && armMs > 0), `--arm-ms 必须是正整数：${args["arm-ms"]}`);
    const result = await runOfflineE2E(page, { ...inputs, armMs, output, workDir: path.dirname(output) });
    process.stdout.write(JSON.stringify({ passed: result.passed, elapsedMs: result.elapsedMs, branches: result.branches,
      production: Object.fromEntries(Object.entries(result.production).map(([k, v]) => [k, v.verdict])), replay: Object.fromEntries(["original", "rules", "raw"].map(k => [k, `${result.replay[k].verdict} via ${result.replay[k].via}`])) }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
