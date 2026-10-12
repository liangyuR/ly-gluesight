import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { existsSync, statSync } from "node:fs";
import { cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export const identifier = "com.xyzrobotics.tujiaovision.e2e-tests";
export const MISSING_INPUT_EXIT = 3;
export const defaultInputs = {
  sampleSet: "C:\\Users\\11601\\gluesight-samples\\mx11-demo",
  lyflow: "C:\\Users\\11601\\project\\ly-gluesight\\src-tauri\\target\\dependency-runtime\\runtime\\lyflow\\lyflow_core.dll",
};

// 演示夹具：只为让 mx11-demo 跑通离线全流程而手工选定的 ROI、像素当量与检测参数，不代表算法调优结果或现场标定。
// 像素当量写进本次输出目录里样本集副本的 shots[].mmPerPx，按样本集声明当量走标定验证。
const TEACH = {
  S001: { roi: [0, 450, 560, 110], mmPerPx: 0.0853 },
  S002: { roi: [0, 470, 400, 100], mmPerPx: 0.0916 },
  S004: { roi: [850, 180, 120, 240], mmPerPx: 0.1232, detect: { searchMm: 4.312, polarity: "dark", widthRange: [2, 6] } },
  S005: { roi: [860, 240, 130, 200], mmPerPx: 0.1284 },
  S008: { roi: [500, 420, 380, 130], mmPerPx: 0.156 },
};
const SKIPPED = new Set(["S003", "S006", "S007"]);
const CENTERLINE_SEGMENTS = 12;
const TEACH_PART = "p01";
const NORMAL_PART = "p01";
const DEFECT_PART = "p02";
const OK_CLASS = ["OK", "OK_WITH_EXCURSION"];
const verdictLabel = {
  OK: "OK", OK_WITH_EXCURSION: "OK · 局部超差", NG_POSITION: "NG · 位置超差", NG_WIDTH: "NG · 胶宽超差",
  NG_ABSOLUTE: "NG · 超绝对限", NG_GAP: "NG · 断胶", ERR_INSPECT: "ERR · 未测成",
};
const sampleLabelText = { ok: "合格", ng: "不合格", unknown: "未标注" };

export function missingInputs(inputs) {
  const problems = [];
  if (!inputs.sampleSet || !existsSync(path.join(inputs.sampleSet, "sample-set.json"))) problems.push(`样本集目录缺少 sample-set.json：${inputs.sampleSet || "（未设置 GLUESIGHT_SAMPLE_SET）"}`);
  if (!inputs.lyflow || !existsSync(inputs.lyflow) || !statSync(inputs.lyflow).isFile()) problems.push(`lyFlow 核心库不存在：${inputs.lyflow || "（未设置 LYFLOW_CORE_DLL）"}`);
  return problems;
}

export function monotoneCenterline(points, segments = CENTERLINE_SEGMENTS) {
  assert(points.length >= 3, `提取的中线点过少：${points.length}`);
  const mean = points.reduce((sum, [x, y]) => [sum[0] + x / points.length, sum[1] + y / points.length], [0, 0]);
  let xx = 0, xy = 0, yy = 0;
  for (const [x, y] of points) {
    const dx = x - mean[0], dy = y - mean[1];
    xx += dx * dx; xy += dx * dy; yy += dy * dy;
  }
  const angle = 0.5 * Math.atan2(2 * xy, xx - yy);
  const axis = [Math.cos(angle), Math.sin(angle)];
  const t = points.map(([x, y]) => (x - mean[0]) * axis[0] + (y - mean[1]) * axis[1]);
  const lo = Math.min(...t), hi = Math.max(...t);
  const bins = Array.from({ length: segments }, () => ({ x: 0, y: 0, n: 0 }));
  points.forEach(([x, y], i) => {
    const bin = bins[Math.min(segments - 1, Math.floor(((t[i] - lo) / (hi - lo || 1)) * segments))];
    bin.x += x; bin.y += y; bin.n += 1;
  });
  const path = bins.filter(b => b.n).map(b => [b.x / b.n, b.y / b.n]);
  assert(path.length >= 3, "中线沿主方向分段后少于 3 个点");
  return path;
}

export async function prepareSampleSet(source, outputDir) {
  const root = path.resolve(outputDir);
  const target = path.join(root, "sample-set");
  assert.equal(path.dirname(target), root);
  await rm(target, { recursive: true, force: true });
  await cp(source, target, { recursive: true });
  const file = path.join(target, "sample-set.json");
  const manifest = JSON.parse((await readFile(file, "utf8")).trim());
  for (const shot of manifest.shots) if (TEACH[shot.shotId]) shot.mmPerPx = TEACH[shot.shotId].mmPerPx;
  await writeFile(file, JSON.stringify(manifest, null, 2) + "\n");
  return target;
}

export async function until(read, accepts, message, timeout = 30000, interval = 150) {
  const deadline = Date.now() + timeout;
  let value;
  while (Date.now() < deadline) {
    value = await read();
    if (accepts(value)) return value;
    await new Promise(resolve => setTimeout(resolve, interval));
  }
  throw new Error(`${message}：${JSON.stringify(value)?.slice(0, 1500)}`);
}

const GATE_BUSY = /正在处理 PLC 事务|检测仍有在途事务|设备配置正在变化|尚未回到空闲|正在预热/;
export const gateRetries = [];

export function client(page) {
  const once = async (command, args) => {
    const result = await page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args)
      .then(ok => ({ ok }), error => ({ error: typeof error === "string" ? error : String(error?.message ?? error) })), { command, args });
    if ("error" in result) throw new Error(`${command} 失败：${result.error}`);
    return result.ok;
  };
  return async (command, args) => {
    const deadline = Date.now() + 60000;
    for (;;) {
      try { return await once(command, args); } catch (error) {
        if (!GATE_BUSY.test(error.message) || Date.now() > deadline) throw error;
        gateRetries.push({ command, error: error.message.replace(/^.*失败：/, "") });
        await new Promise(resolve => setTimeout(resolve, 250));
      }
    }
  };
}

function workspaceCalls(call, id) {
  const current = async () => (await call("workspace_get", { id })).workspace.revision;
  return {
    get: () => call("workspace_get", { id }),
    edit: async (command, args) => call(command, { id, revision: await current(), ...args }),
  };
}

async function captureRound(call, id, cameraId, partId) {
  const round = await call("recipe_capture_start", { recipeId: id, cameraId, drainTimeoutMs: 1000 });
  assert.equal(round.state, "waitingStart");
  await call("offline_capture", { partId });
  const done = await until(() => call("recipe_capture_get", { roundId: round.roundId }), r => ["complete", "failed"].includes(r?.state), `整圈采集 ${partId} 未结束`, 60000);
  assert.equal(done.state, "complete", `整圈采集 ${partId} 失败：${done.error}`);
  await until(() => call("site_status"), s => !s.running, "离线产线采集后未停止", 15000);
  return done;
}

async function teach(ws, view) {
  const shots = view.workspace.doc.shots;
  const taught = [];
  for (const [k, shot] of shots.entries()) {
    const skip = SKIPPED.has(shot.id);
    assert(skip || TEACH[shot.id], `夹具未覆盖拍照点 ${shot.id}`);
    await ws.edit("workspace_set_views", { k, views: [1], skip });
  }
  for (const [k, shot] of shots.entries()) {
    const fixture = TEACH[shot.id];
    if (!fixture) continue;
    const imageId = (await ws.get()).workspace.frames[k].image.id;
    const declared = (await ws.get()).workspace.doc.shots[k].mmPerPx;
    assert(Math.abs(declared - fixture.mmPerPx) < 1e-6, `${shot.id} 未采用样本集声明当量：${declared}`);
    const extracted = await ws.edit("workspace_extract_centerline", { k, imageId, roi: fixture.roi });
    const centerline = monotoneCenterline(extracted.path);
    await ws.edit("workspace_save_params", { k, params: { path: centerline, mmPerPx: declared, ...(fixture.detect ? { detect: fixture.detect } : {}) } });
    const tried = await ws.edit("workspace_trial", { k, imageId });
    const trial = tried.workspace.frames[k].trial;
    assert(trial?.passed, `${shot.id} 试测未通过：${trial?.reason}`);
    await ws.edit("workspace_save_teach", { k, imageId });
    taught.push({ k, shotId: shot.id, points: centerline.length, extracted: extracted.path.length, trialVerdict: trial.verdict ?? null, coverage: trial.coverage });
  }
  return taught;
}

function assertSampleSetCalibration(view, taught, setId) {
  const step = view.progress.steps.find(item => item.id === "calibration");
  assert.equal(step.status, "done", `标定步骤未完成：${JSON.stringify(step)}`);
  for (const { k, shotId } of taught) {
    const views = view.progress.shots[k].views.filter(v => v.enabled);
    assert(views.length && views.every(v => v.calibration.state === "sampleSet" && v.calibration.setId === setId
      && Math.abs(v.calibration.mmPerPx - TEACH[shotId].mmPerPx) < 1e-6), `${shotId} 标定依据不是样本集声明当量：${JSON.stringify(views)}`);
  }
  return { status: step.status, detail: step.detail };
}

export async function produce(call, recipeId, partId) {
  const previous = await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } });
  await call("offline_run", { recipeId, scenario: "normal", continuous: false, partId });
  const summary = await until(async () => (await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } })).items.find(i => i.id !== previous.items[0]?.id),
    Boolean, `样本件 ${partId} 未生成检测记录`, 60000);
  const site = await until(() => call("site_status"), s => !s.running, `样本件 ${partId} 握手未结束`, 30000);
  const run = site.recent.find(r => r.purpose === "production" && r.partId === partId && r.cycleId === summary.cycleId);
  assert(run, `最近样本件里没有 ${partId}（cycleId ${summary.cycleId}）`);
  await until(() => call("workspace_record_images", { historyId: summary.id }), v => v.complete, `样本件 ${partId} 原图录制未完成`, 30000);
  return { summary, run };
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
  await page.getByRole("row", { name: `查看 SN ${summary.sn} 的记录`, exact: true }).click();
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
      Boolean, `“${button}”未生成对照结果`, 60000);
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
    await page.getByRole("combobox", { name: "已保存对照结果", exact: true }).selectOption(id);
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
  if (settings.lyflowCore !== options.lyflow || settings.productSource !== "plc") {
    await call("cycle_save_settings", { settings: { ...settings, lyflowCore: options.lyflow, productSource: "plc" } });
  }
  const engine = await until(() => call("engine_status"), e => e.ready, "lyFlow 引擎未就绪", 30000);
  step("lyFlow 就绪", { version: engine.version ?? null });

  const offlineSampleSet = await prepareSampleSet(options.sampleSet, options.workDir);
  const preview = await call("site_preview_sample_set", { dir: offlineSampleSet });
  for (const partId of [TEACH_PART, NORMAL_PART, DEFECT_PART]) assert(preview.parts.some(p => p.partId === partId), `样本集缺少 ${partId}`);
  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: offlineSampleSet } });
  assert.equal(entered.mode, "offline");
  assert.equal(entered.source, "sampleSet");
  assert.equal(entered.measurement, "lyFlow");
  await page.getByRole("link", { name: `站点模式：离线 · 样本集 ${preview.name}`, exact: true }).waitFor();
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const cameras = await call("camera_rig_config");
  assert.equal(cameras.length, 1);
  assert.equal(cameras[0].source, "sampleSet");
  const cameraId = cameras[0].id;
  await navigation.getByRole("link", { name: "离线控制台", exact: true }).click();
  const noRecipe = page.getByRole("note").filter({ hasText: /尚无可(开工|在当前样本集 \/ 相机组运行)的生产配方/ });
  await noRecipe.waitFor();
  const noRecipeText = (await noRecipe.innerText()).trim();
  step("进入离线·样本集", { port: entered.port, cameraId, noRecipeText });

  const template = await call("recipe_template");
  const created = await call("workspace_create", { doc: template });
  const id = created.workspace.doc.id;
  const ws = workspaceCalls(call, id);
  const teachRound = await captureRound(call, id, cameraId, TEACH_PART);
  const adopted = await ws.edit("workspace_adopt_capture", { roundId: teachRound.roundId, reuseTeaching: false, correspondenceConfirmed: false });
  assert.equal(adopted.workspace.doc.shots.length, preview.shotCount);
  step("首次建站采集并采用", { recipeId: id, roundId: teachRound.roundId, shots: preview.shotCount });

  const taught = await teach(ws, adopted);
  const calibration = assertSampleSetCalibration(await ws.get(), taught, preview.id);
  step("示教完成", { taught: taught.map(t => t.shotId), skipped: [...SKIPPED], calibration });

  const samples = [];
  for (const [partId, expected] of [[NORMAL_PART, "OK"], [DEFECT_PART, "NG_GAP"]]) {
    const round = await captureRound(call, id, cameraId, partId);
    await ws.edit("workspace_capture_sample", { roundId: round.roundId, expected, correspondenceConfirmed: true });
    samples.push({ partId, roundId: round.roundId, expected });
  }
  const validation = (await ws.edit("workspace_validate", { samples: samples.map(s => ({ historyId: null, sampleId: s.roundId, expected: s.expected })), sourceConfirmed: true })).workspace.validation;
  assert(validation.passed, `验证未通过：${JSON.stringify(validation.checks.filter(c => !c.passed))}`);
  const normalSample = validation.samples.find(s => s.sampleId === samples[0].roundId);
  assert.equal(normalSample.expected, "OK");
  assert.equal(normalSample.actual, "OK_WITH_EXCURSION", `合格样本的实际结论变了：${normalSample.reason}`);
  assert(normalSample.passed, "期望合格时 OK_WITH_EXCURSION 应判为符合");
  const defectSample = validation.samples.find(s => s.sampleId === samples[1].roundId);
  assert.equal(defectSample.actual, "NG_GAP", `缺陷样本验证结论不符：${defectSample.reason}`);
  const branches = { calibration: "sampleSetDeclared", normalExpectation: "okClass" };
  step("验证通过", { branches, samples: validation.samples.map(s => ({ name: s.name, expected: s.expected, actual: s.actual, passed: s.passed })) });

  await ws.edit("workspace_publish", {});
  await until(async () => (await call("cycle_recipes")).find(r => r.id === id), Boolean, "发布后生产配方未生效", 30000);
  const active = await until(() => ws.get(), v => !v.workspace.pending && v.workspace.publish.state === "active", "待生效版本未提交", 30000);
  const recipe = await call("cycle_layout", { recipeId: id });
  await navigation.getByRole("link", { name: "离线控制台", exact: true }).click();
  const runSelect = page.getByRole("combobox", { name: "离线配方", exact: true });
  await until(() => runSelect.inputValue(), value => value === id, "发布后离线配方未默认选中新配方", 15000);
  step("发布生效", { recipeId: id, publish: active.workspace.publish });

  const produced = {};
  for (const partId of [NORMAL_PART, DEFECT_PART]) produced[partId] = await produce(call, id, partId);
  const normal = produced[NORMAL_PART], defect = produced[DEFECT_PART];
  assert(OK_CLASS.includes(normal.summary.verdict), `投产 ${NORMAL_PART} 应为合格类：${normal.summary.verdict} ${normal.summary.reason}`);
  assert.equal(defect.summary.verdict, "NG_GAP", `投产 ${DEFECT_PART} 应为断胶：${defect.summary.reason}`);
  assert(defect.summary.reason.includes("S004"), `断胶原因未指向 S004：${defect.summary.reason}`);
  for (const { summary, run } of [normal, defect]) {
    assert.equal(run.verdict, summary.verdict);
    assert.equal(summary.sample?.partId, run.partId, `历史记录缺少样本件引用：${JSON.stringify(summary.sample)}`);
    assert.equal(summary.sample.setId, preview.id);
    assert.equal(summary.sample.label, run.label);
  }
  step("投产", { [NORMAL_PART]: normal.summary.verdict, [DEFECT_PART]: defect.summary.verdict });

  await navigation.getByRole("link", { name: "离线控制台", exact: true }).click();
  const recentTable = page.getByRole("table", { name: "最近样本件", exact: true });
  for (const { run } of [normal, defect]) {
    const row = recentTable.getByRole("row").filter({ has: page.getByRole("cell", { name: String(run.partSn), exact: true }) });
    await row.waitFor();
    const cells = (await row.getByRole("cell").allInnerTexts()).map(text => text.trim());
    assert.equal(cells[1], run.partId);
    assert.equal(cells[5], sampleLabelText[run.label], `最近样本件 ${run.partId} 期望列不符`);
    assert.equal(cells[6], verdictLabel[run.verdict], `最近样本件 ${run.partId} 实际列不符`);
  }
  assert.equal((await call("history_query", { query: { limit: 1 } })).total, 0, "离线件不得进入真机历史与统计");
  await navigation.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  await page.getByRole("combobox", { name: "历史配方", exact: true }).selectOption(id);
  for (const { summary } of [normal, defect]) {
    const row = page.getByRole("row", { name: `查看 SN ${summary.sn} 的记录`, exact: true });
    await row.getByText(`样本 ${summary.sample.partId}（${sampleLabelText[summary.sample.label]}）`, { exact: true }).waitFor();
    await row.getByText(verdictLabel[summary.verdict], { exact: true }).waitFor();
  }
  step("界面核对");
  const exported = await exportHistory(page, call, id);
  const replay = await replayInHistory(page, call, id, defect.summary);
  step("历史导出与复测对照", { exported: exported.rows, replay: Object.fromEntries(Object.entries(replay).map(([k, v]) => [k, v.verdict])) });

  if (options.output) {
    await mkdir(path.dirname(options.output), { recursive: true });
    await page.screenshot({ path: path.join(path.dirname(options.output), "offline-e2e-history.png"), fullPage: true });
  }
  await navigation.getByRole("link", { name: "离线控制台", exact: true }).click();
  await page.getByRole("button", { name: "退出离线", exact: true }).click();
  await until(() => call("site_status"), s => s.mode === "real", "未退出离线模式");
  await page.getByRole("link", { name: "站点模式：真机", exact: true }).waitFor();
  assert.deepEqual(await call("plc_get_config"), before.plc, "退出离线后 PLC 配置未恢复");
  assert.deepEqual(await call("camera_rig_config"), before.cameras, "退出离线后相机组未恢复");
  step("退出离线并恢复");

  const result = {
    passed: true,
    identifier,
    inputs: { sampleSet: options.sampleSet, offlineSampleSet, sampleSetId: preview.id, lyflow: options.lyflow, lyflowVersion: engine.version ?? null },
    elapsedMs: Date.now() - startedAt,
    recipe: { id, revisionId: recipe.revisionId, teachingId: recipe.teachingId ?? null },
    taught,
    calibration,
    branches,
    gateRetries: Object.entries(gateRetries.reduce((counts, r) => ({ ...counts, [`${r.command}：${r.error}`]: (counts[`${r.command}：${r.error}`] ?? 0) + 1 }), {})).map(([what, count]) => ({ what, count })),
    validation: { checks: validation.checks.map(c => ({ name: c.name, passed: c.passed })), samples: validation.samples.map(s => ({ name: s.name, expected: s.expected, actual: s.actual, passed: s.passed })) },
    production: Object.fromEntries([normal, defect].map(({ summary, run }) => [run.partId, { historyId: summary.id, sn: summary.sn, partSn: run.partSn, verdict: summary.verdict, plcCode: summary.plcCode, reason: summary.reason, cycleId: summary.cycleId, recipeRevision: summary.recipeRevision, bundleId: summary.bundleId, sample: summary.sample }])),
    exported,
    replay: { historyId: defect.summary.id, originalVerdict: defect.summary.verdict, ...replay },
    steps,
    boundary: "进程内 S7 服务与虚拟产线驱动真实 lyFlow 测量现场样本原图；ROI、像素当量与检测参数是演示夹具，结论不代表算法质量、现场标定或现场节拍",
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
    sampleSet: args["sample-set"] ?? process.env.GLUESIGHT_SAMPLE_SET ?? defaultInputs.sampleSet,
    lyflow: args.lyflow ?? process.env.LYFLOW_CORE_DLL ?? defaultInputs.lyflow,
  };
  const problems = missingInputs(inputs);
  if (problems.length) {
    process.stderr.write(`离线端到端输入缺失，未运行（不算通过）：\n${problems.join("\n")}\n`);
    process.exitCode = MISSING_INPUT_EXIT;
    return;
  }
  const cdp = args.cdp ?? "http://127.0.0.1:9341";
  const url = new URL(cdp);
  assert(["127.0.0.1", "localhost", "[::1]"].includes(url.hostname), "仅允许本机 CDP");
  const instance = args.instance ?? "output/playwright/e2e-regression/instance.json";
  const manifest = JSON.parse(await readFile(instance, "utf8"));
  assert.equal(manifest.identifier, identifier, "仅允许隔离端到端实例");
  assert.equal(Number(url.port), manifest.debugPort);
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = browser.contexts().flatMap(context => context.pages()).find(p => /tauri\.localhost|localhost:1420|^tauri:\/\//.test(p.url()));
    assert(page, "未找到 Tauri UI 页面");
    page.setDefaultTimeout(30000);
    await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
    const output = args.output ?? "output/playwright/e2e-regression/result.json";
    const result = await runOfflineE2E(page, { ...inputs, output, workDir: path.dirname(output) });
    process.stdout.write(JSON.stringify({ passed: result.passed, elapsedMs: result.elapsedMs, branches: result.branches,
      production: Object.fromEntries(Object.entries(result.production).map(([k, v]) => [k, v.verdict])), replay: Object.fromEntries(["original", "rules", "raw"].map(k => [k, `${result.replay[k].verdict} via ${result.replay[k].via}`])) }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
