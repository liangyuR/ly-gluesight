import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { cp, mkdir, readFile, realpath, rm, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { client, monotoneCenterline, produce, until } from "./offline-e2e.mjs";

// 演示夹具：2026-10-17 单圈样本集（12 点位 × 3 相机）没有标定，像素当量 0.1 mm/px 是模拟标定（胶条在图中约 35 px，按 3–4 mm 常见胶宽推定）；
// ROI 只为让回放链路跑通而手工选定，不代表算法调优结果或现场标定。
const MM_PER_PX = 0.1;
const DETECT = { searchMm: 3, polarity: "dark", widthRange: [1.5, 5] };
// 三相机整圈采集采用后，拍照点编号为“点位-相机”，同一点位是一个拍照点组，每组只能选一台相机的图示教；
// 这里每组各选一个胶条清晰的拍照点（括号内为样本集 shotId），其余设为不检。
const TEACH = {
  "S001-cam3": [515, 430, 90, 160] /* S003 */, "S002-cam2": [670, 410, 70, 220] /* S005 */, "S003-cam1": [370, 230, 180, 130] /* S007 */,
  "S004-cam3": [718, 590, 60, 130] /* S012 */, "S005-cam1": [510, 148, 90, 70] /* S013 */, "S006-cam1": [470, 190, 120, 115] /* S016 */,
  "S007-cam2": [615, 300, 80, 170] /* S020 */, "S008-cam3": [600, 300, 70, 150] /* S024 */, "S009-cam1": [750, 200, 110, 90] /* S025 */,
  "S010-cam1": [470, 60, 115, 100] /* S028 */, "S011-cam3": [515, 430, 80, 100] /* S033 */, "S012-cam3": [515, 430, 90, 140] /* S036 */,
};
const PART = "p01";
const ARM_MS = 1000;
const STAGES = ["partStart", "armed", "partEnd", "result", "released"];

async function prepareSampleSet(source, outputDir) {
  await mkdir(outputDir, { recursive: true });
  const target = path.join(await realpath(outputDir), "sample-set");
  const realSource = await realpath(source);
  assert(!target.startsWith(realSource) && !realSource.startsWith(target), "样本集副本不能与原件重叠");
  await rm(target, { recursive: true, force: true });
  await cp(source, target, { recursive: true });
  const file = path.join(target, "sample-set.json");
  const manifest = JSON.parse((await readFile(file, "utf8")).trim());
  manifest.mmPerPx = MM_PER_PX;
  await writeFile(file, JSON.stringify(manifest, null, 2) + "\n");
  return target;
}

async function captureRound(call, id, cameraIds) {
  const round = await call("recipe_capture_start", { recipeId: id, cameraIds, drainTimeoutMs: 1000 });
  await call("offline_capture", { partId: PART });
  const done = await until(() => call("recipe_capture_get", { roundId: round.roundId }), r => ["complete", "failed"].includes(r?.state), "整圈采集未结束", 90000);
  assert.equal(done.state, "complete", `整圈采集失败：${done.error}`);
  await until(() => call("site_status"), s => !s.running, "离线产线采集后未停止", 15000);
  return done;
}

async function teach(call, id) {
  const revision = async () => (await call("workspace_get", { id })).workspace.revision;
  const edit = async (command, args) => call(command, { id, revision: await revision(), ...args });
  const shots = (await call("workspace_get", { id })).workspace.doc.shots;
  for (const [k, shot] of shots.entries()) await edit("workspace_set_views", { k, views: TEACH[shot.id] ? [1] : [], skip: !TEACH[shot.id] });
  const taught = [];
  for (const [k, shot] of shots.entries()) {
    const roi = TEACH[shot.id];
    if (!roi) continue;
    const imageId = (await call("workspace_get", { id })).workspace.frames[k].image.id;
    const extracted = await edit("workspace_extract_centerline", { k, imageId, roi });
    await edit("workspace_save_params", { k, params: { path: monotoneCenterline(extracted.path), mmPerPx: MM_PER_PX, detect: DETECT } });
    const trial = (await edit("workspace_trial", { k, imageId })).workspace.frames[k].trial;
    assert(trial?.passed, `${shot.id} 试测未通过：${trial?.reason}`);
    await edit("workspace_save_teach", { k, imageId });
    taught.push({ k, shotId: shot.id, verdict: trial.verdict ?? null, coverage: trial.coverage });
  }
  const bad = taught.filter(t => t.verdict && !["OK", "OK_WITH_EXCURSION"].includes(t.verdict));
  assert.equal(bad.length, 0, `正常样本示教试测结论不合格：${JSON.stringify(bad)}`);
  return taught;
}

function perShot(detail, layout, comparison) {
  return layout.shots.map((shot, k) => {
    const owned = layout.points.k.flatMap((owner, j) => owner === k ? [j] : []);
    const m = comparison.measurements.find(x => x.k === k);
    if (shot.skip || !m) return { k, shotId: shot.id, skip: shot.skip, compared: false };
    const at = new Map(m.idx.map((j, i) => [j, i]));
    let changed = 0, maxDd = 0, maxDw = 0;
    for (const j of owned) {
      const i = at.get(j), before = detail.points.st[j], after = i === undefined ? 2 : m.st[i];
      if (before !== after) { changed++; continue; }
      if (before !== 0) continue;
      maxDd = Math.max(maxDd, Math.abs(detail.points.d[j] - m.d[i]));
      if (detail.points.w[j] != null && m.w[i] != null) maxDw = Math.max(maxDw, Math.abs(detail.points.w[j] - m.w[i]));
    }
    return { k, shotId: shot.id, skip: false, compared: true, points: owned.length, changed, maxDd, maxDw };
  });
}

async function checkPart(call, summary) {
  const detail = await call("history_detail", { id: summary.id });
  const layout = await call("history_recipe", { revisionId: summary.recipeRevision, recipeId: summary.recipeId });
  assert.equal(detail.shots.length, layout.shots.length);
  assert(detail.shots.every(shot => shot.rawFiles.length > 0), "每个拍照点都应保留原图");
  const timeline = await call("history_timeline", { id: summary.id });
  const marks = Object.fromEntries(STAGES.map(stage => [stage, timeline.entries.filter(e => e.stage === stage).length]));
  for (const stage of STAGES) assert(marks[stage] >= 1, `时间线缺少 ${stage}：${JSON.stringify(marks)}`);
  const frames = timeline.entries.filter(e => e.stage === "frame");
  assert.deepEqual([...new Set(frames.map(e => e.k))].sort((a, b) => a - b), layout.shots.map((_, k) => k), "每个拍照点都应有帧到达记录");
  assert(frames.every(e => e.data?.triggerCounter != null && e.shotId && e.camera), "帧到达记录应带触发计数、shotId 与相机");
  assert(timeline.stages.every(s => s.ms != null && s.ms >= 0), `阶段耗时不完整：${JSON.stringify(timeline.stages)}`);
  assert.equal(timeline.requestSeq != null, true, "时间线应带 requestSeq");
  const plc = timeline.entries.filter(e => e.source === "plc");
  const appLogs = await call("applog_query", { query: { cycleId: summary.cycleId, limit: 1 } });
  assert.equal(appLogs.total, timeline.entries.length - plc.length, "运行日志按 cycleId 查询结果应与时间线一致");
  const comparison = await call("workspace_compare_original", { historyId: summary.id });
  assert.equal(comparison.bundleId, summary.bundleId);
  assert.equal(comparison.candidateRecipe.revisionId, summary.recipeRevision);
  assert.equal(comparison.judgement.verdict, summary.verdict, `原包重跑结论与原记录不一致：${comparison.judgement.reason}`);
  const shots = perShot(detail, layout, comparison);
  const differing = shots.filter(s => s.compared && (s.changed || s.maxDd > 0.01 || s.maxDw > 0.01));
  assert.equal(differing.length, 0, `原包重跑逐拍照点不一致：${JSON.stringify(differing)}`);
  return { id: summary.id, sn: summary.sn, cycleId: summary.cycleId, verdict: summary.verdict, requestSeq: timeline.requestSeq,
    stages: Object.fromEntries(timeline.stages.map(s => [s.key, s.ms])), timeline: { app: timeline.entries.length - plc.length, plc: plc.length, notes: timeline.notes },
    replay: { verdict: comparison.judgement.verdict, compared: shots.filter(s => s.compared).length, maxDd: Math.max(...shots.filter(s => s.compared).map(s => s.maxDd)), maxDw: Math.max(...shots.filter(s => s.compared).map(s => s.maxDw)) } };
}

async function checkUi(page, call, parts, output) {
  const nav = page.getByRole("navigation", { name: "操作导航" });
  await nav.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  const [first, second] = parts;
  await page.getByRole("row", { name: `查看 SN ${second.sn} 的记录`, exact: true }).click();
  await page.getByRole("heading", { name: "本件事件时间线", exact: true }).waitFor();
  await page.getByRole("table", { name: "本件事件", exact: true }).getByRole("cell", { name: "partStart↑", exact: true }).first().waitFor();
  await page.getByRole("region", { name: "本件事件时间线" }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: path.join(output, "history-timeline.png") });
  await page.getByRole("button", { name: "按原发布包重现", exact: true }).click();
  await page.getByRole("heading", { name: "原发布包重现结果", exact: true }).waitFor({ timeout: 90000 });
  await page.getByText(/个拍照点全部与原结果一致/).waitFor();
  await page.getByRole("checkbox", { name: "只看差异" }).uncheck();
  const panel = name => page.locator("section.wp-panel").filter({ has: page.getByRole("heading", { name, exact: true }) });
  await panel("原发布包重现结果").scrollIntoViewIfNeeded();
  await page.screenshot({ path: path.join(output, "history-replay.png") });
  const measuredShot = Object.keys(TEACH)[1];
  await page.getByRole("table", { name: "逐拍照点比对" }).getByRole("button", { name: new RegExp(`比对查看拍照点 k\\d+`) })
    .filter({ hasText: measuredShot }).click();
  await page.getByText(/横线为原发布包重现测得的胶边与胶宽/).waitFor();
  const zoomIn = panel("逐拍照点结果与原图").getByRole("button", { name: "放大原图" });
  for (let i = 0; i < 3; i++) await zoomIn.click();
  await panel("逐拍照点结果与原图").getByRole("img", { name: /原始 SN/ }).scrollIntoViewIfNeeded();
  await page.waitForTimeout(500);
  await page.screenshot({ path: path.join(output, "history-overlay.png") });
  await page.keyboard.press("[");
  await until(() => page.url(), url => url.endsWith(`/history/${first.id}`), "按 [ 未跳到前一件", 10000);
  await page.getByRole("heading", { name: "本件事件时间线", exact: true }).waitFor();
  await page.keyboard.press("]");
  await until(() => page.url(), url => url.endsWith(`/history/${second.id}`), "按 ] 未跳到后一件", 10000);
  await nav.getByRole("link", { name: "运行日志", exact: true }).click();
  await page.getByRole("table", { name: "运行日志", exact: true }).getByRole("row").nth(1).waitFor();
  await page.getByRole("searchbox", { name: "搜索日志" }).or(page.getByRole("textbox", { name: "搜索日志" })).fill(second.cycleId);
  await page.keyboard.press("Enter");
  await page.getByRole("table", { name: "运行日志", exact: true }).getByRole("cell", { name: "检测结果", exact: true }).waitFor();
  await page.screenshot({ path: path.join(output, "app-logs.png") });
  await page.getByRole("button", { name: "导出诊断包", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "导出诊断包", exact: true });
  await dialog.getByRole("combobox", { name: "诊断包记录范围" }).selectOption("offline");
  await dialog.getByRole("button", { name: "导出", exact: true }).click();
  await dialog.getByText(/^已导出/).waitFor({ timeout: 120000 });
  await page.screenshot({ path: path.join(output, "diagnostics-dialog.png") });
  const text = await dialog.innerText();
  await dialog.getByRole("button", { name: "完成", exact: true }).click();
  return text.match(/已导出\s*(\S+\.zip)/)?.[1] ?? null;
}

function zipEntries(file) {
  const tar = process.platform === "win32" ? path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe") : "tar";
  return execFileSync(tar, ["-tf", file], { encoding: "utf8" }).split(/\r?\n/).filter(Boolean);
}

export async function runLogReplay(page, options) {
  assert(Number.isInteger(options.parts) && options.parts >= 3, `--parts 至少为 3：第 2 件用于前后件导航与界面回放，最后一件用于删除原图，需互不重叠；实际 ${options.parts}`);
  const started = Date.now();
  const call = client(page);
  const records = await call("records_list");
  assert(records.root.includes("-tests"), `数据目录不属于隔离实例：${records.root}`);
  const settings = await call("cycle_get_settings");
  // 调试构建下布防实测约 250 ms（发布包核验约 56 ms、审计健康约 28 ms、PLC 布防确认约 160 ms），超过默认 200 ms 预算会判 ERR 98；验证回放时放宽到 1 s。
  await call("cycle_save_settings", { settings: { ...settings, productSource: "plc", timeouts: { ...settings.timeouts, armMs: ARM_MS } } });
  const engine = await until(() => call("engine_status"), e => e.ready, "lyFlow 引擎未就绪", 30000);
  const set = await prepareSampleSet(options.sampleSet, options.output);
  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: set } });
  assert.equal(entered.measurement, "lyFlow");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const cameras = await call("camera_rig_config");
  const created = await call("workspace_create", { doc: await call("recipe_template") });
  const id = created.workspace.doc.id;
  const round = await captureRound(call, id, cameras.map(c => c.id));
  const revision = async () => (await call("workspace_get", { id })).workspace.revision;
  await call("workspace_adopt_capture", { id, revision: await revision(), roundId: round.roundId, reuseTeaching: false, correspondenceConfirmed: false });
  const taught = await teach(call, id);
  const sample = await captureRound(call, id, cameras.map(c => c.id));
  await call("workspace_capture_sample", { id, revision: await revision(), roundId: sample.roundId, expected: "OK", correspondenceConfirmed: true });
  const validation = (await call("workspace_validate", { id, revision: await revision(), samples: [{ historyId: null, sampleId: sample.roundId, expected: "OK" }], sourceConfirmed: true })).workspace.validation;
  assert(validation.passed, `验证未通过：${JSON.stringify(validation.checks.filter(c => !c.passed))}`);
  await call("workspace_publish", { id, revision: await revision() });
  await until(async () => (await call("workspace_get", { id })).workspace.publish.state === "active", Boolean, "发布未生效", 60000);
  const produced = [];
  for (let i = 0; i < options.parts; i++) produced.push((await produce(call, id, PART)).summary);
  const parts = [];
  for (const summary of produced) parts.push(await checkPart(call, summary));
  const middle = produced[1];
  const neighbors = await call("history_neighbors", { id: middle.id, query: null });
  assert.deepEqual(neighbors, { older: produced[0].id, newer: produced[2].id }, "前后件导航与生产顺序不符");
  const missing = produced.at(-1);
  const missingDetail = await call("history_detail", { id: missing.id });
  const root = records.root.replace(/[\\/]records$/, "");
  const victim = path.join(root, "records-offline", missingDetail.shots.find(s => TEACH[s.shotId] && s.rawFiles.length).rawFiles[0].file);
  assert(existsSync(victim), `找不到原图文件：${victim}`);
  await unlink(victim);
  const images = await call("workspace_record_images", { historyId: missing.id });
  assert.equal(images.complete, false, "删除原图后应判为不完整");
  const refused = await call("workspace_compare_original", { historyId: missing.id }).then(() => null, error => error.message);
  assert(refused && /原图/.test(refused), `原图缺失时原包重跑应明确拒绝：${refused}`);
  const ui = await checkUi(page, call, produced, options.output);
  const pkg = await call("diagnostics_export", { request: { from: started - 60_000, to: Date.now(), historyIds: [produced[0].id], images: "all", maxImageParts: 1 } });
  const entries = zipEntries(pkg.path);
  for (const required of ["manifest.json", "logs/plc.jsonl", "records/parts.csv", "records/parts.jsonl"]) assert(entries.includes(required), `诊断包缺少 ${required}`);
  assert(entries.some(e => /^logs\/gluesight-.*\.jsonl$/.test(e)), "诊断包缺少运行日志");
  assert(entries.some(e => e.startsWith(`records/${produced[0].id}_`) && /\.(pgm|bmp|png)$/.test(e)), "诊断包缺少所选记录原图");
  assert(entries.some(e => e.startsWith("recipes/")), "诊断包缺少配方快照");
  await call("site_leave_offline");
  return { passed: true, elapsedMs: Date.now() - started, engine: engine.version ?? null, recipeId: id, taught, parts, neighbors,
    missingImage: { historyId: missing.id, removed: path.basename(victim), recordImages: images.message, refused },
    diagnostics: { path: pkg.path, bytes: pkg.bytes, logs: pkg.logs, plcLogs: pkg.plcLogs, images: pkg.images, entries: entries.length }, uiDiagnostics: ui,
    boundary: "进程内 S7 服务与虚拟产线回放 2026-10-17 现场单圈原图；像素当量 0.1 mm/px 为模拟标定，ROI 为演示夹具" };
}

async function main() {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) args[process.argv[i].slice(2)] = process.argv[i + 1];
  const cdp = new URL(args.cdp ?? "http://127.0.0.1:9346");
  assert(["127.0.0.1", "localhost"].includes(cdp.hostname), "仅允许本机 CDP");
  const sampleSet = args["sample-set"] ?? process.env.GLUESIGHT_SAMPLE_SET;
  assert(sampleSet && existsSync(path.join(sampleSet, "sample-set.json")), "需要 --sample-set 指向 2026-10-17 单圈样本集");
  const output = path.resolve(args.output ?? "output/playwright/log-replay");
  await mkdir(output, { recursive: true });
  const main = browser => browser.contexts().flatMap(c => c.pages()).find(p => /tauri\.localhost\/(index\.html)?#\//.test(p.url()));
  let browser = await chromium.connectOverCDP(cdp.href);
  for (const deadline = Date.now() + 60000; !main(browser) && Date.now() < deadline;) {
    await browser.close();
    await new Promise(resolve => setTimeout(resolve, 1000));
    browser = await chromium.connectOverCDP(cdp.href);
  }
  try {
    const page = main(browser);
    assert(page, "未找到主界面页面（资料准备页尚未关闭）");
    page.setDefaultTimeout(30000);
    await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
    const result = await runLogReplay(page, { sampleSet, output, parts: Number(args.parts ?? 3) });
    await writeFile(path.join(output, "result.json"), JSON.stringify(result, null, 2) + "\n");
    process.stdout.write(JSON.stringify({ passed: result.passed, parts: result.parts.map(p => [p.id, p.verdict, p.replay.verdict, p.stages.total]) }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
