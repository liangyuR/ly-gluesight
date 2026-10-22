import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  buildBurstRecipe, burstFacts, CAMERA_FIXTURES, captureRound, checksOff, client, copySampleSet, instanceCdp, legacyTemplate, produce, produceBurst, uiPage, until, workspaceCalls,
} from "./lib.mjs";

// 离线回放验证有两套流程，由 --model 选择（默认 burst）：
//   burst  段配方（分段连拍）：按区域比对——原包重跑的区域划分、结论、帧数、不完整组与胶宽统计逐区域对照原记录；
//   legacy 旧拍照点配方（segments 为空，只停用不删除）：按拍照点逐点比对，行为不变。

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

// 样本集副本：只把全局当量改为模拟标定 0.1 mm/px，不改 id、不加人工缺陷件（与 lib.mjs 的 FIXTURES 夹具不同）。
async function prepareSampleSet(source, outputDir) {
  return (await copySampleSet(source, outputDir, manifest => ({ ...manifest, mmPerPx: MM_PER_PX }))).dir;
}

// 示教步骤与 lib.mjs 的 teach 不同，保留本地实现：按拍照点 id 的 ROI 表用 workspace_set_views 选图并跳过其余点
// （不走 workspace_select_group_image），检测参数固定为 DETECT，试测结论在全部示教后汇总检查。
async function teach(call, id) {
  const { get, edit } = workspaceCalls(call, id);
  const shots = (await get()).workspace.doc.shots;
  for (const [k, shot] of shots.entries()) await edit("workspace_set_views", { k, views: TEACH[shot.id] ? [1] : [], skip: !TEACH[shot.id] });
  const taught = [];
  for (const [k, shot] of shots.entries()) {
    const roi = TEACH[shot.id];
    if (!roi) continue;
    const imageId = (await get()).workspace.frames[k].image.id;
    const extracted = await edit("workspace_extract_centerline", { k, imageId, roi });
    assert(extracted.path.length >= 3, `${shot.id} 中线提取失败：${extracted.message}`);
    await edit("workspace_save_params", { k, params: { path: extracted.path, mmPerPx: MM_PER_PX, detect: DETECT, fixture: null } });
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
  await page.getByRole("button", { name: `查看 SN ${second.sn} 的记录`, exact: true }).click();
  await page.getByRole("heading", { name: "本件事件时间线", exact: true }).waitFor();
  await page.getByRole("table", { name: "本件事件", exact: true }).getByRole("cell", { name: "partStart↑", exact: true }).first().waitFor();
  await page.getByRole("region", { name: "本件事件时间线" }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: path.join(output, "history-timeline.png") });
  await page.getByRole("button", { name: "按原发布包重现", exact: true }).click();
  await page.getByRole("heading", { name: "原发布包重现结果", exact: true }).waitFor({ timeout: 90000 });
  await page.getByText(/个拍照点全部与原结果一致/).waitFor();
  await page.getByRole("checkbox", { name: "只看差异" }).uncheck();
  const panel = name => page.locator(".wp-panel").filter({ has: page.getByRole("heading", { name, exact: true }) });
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
  await dialog.getByRole("combobox", { name: "诊断包记录范围" }).click();
  await page.getByRole("option", { name: "离线件", exact: true }).click();
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


// ───────────── 段配方（分段连拍，默认）：按区域比对 ─────────────

/** 一个区域的结论、帧数与胶宽统计：原记录与重现结果逐区域对照用。 */
const regionKey = r => ({ kind: r.region.kind, verdict: r.verdict, frameCount: r.frameCount, incompleteGroups: r.incompleteGroups, width: r.width && { min: r.width.min, max: r.width.max, mean: r.width.mean } });

/** 按区域对照两份区域判定：区域划分、结论、帧数、不完整组一致，胶宽统计差 ≤ 0.01 mm。 */
function assertSameRegions(before, after, what) {
  assert.equal(after.length, before.length, `${what}：区域个数不同`);
  for (const [i, original] of before.entries()) {
    const a = regionKey(original), b = regionKey(after[i]);
    assert.deepEqual({ ...b, width: null }, { ...a, width: null }, `${what}：第 ${i + 1} 个区域的划分、结论、帧数或不完整组与原记录不一致`);
    if (a.width) for (const key of ["min", "max", "mean"]) assert(Math.abs(a.width[key] - b.width[key]) <= 0.01, `${what}：第 ${i + 1} 个区域胶宽 ${key} 差 ${Math.abs(a.width[key] - b.width[key])} mm`);
  }
}

async function checkBurstPart(call, summary) {
  const detail = await call("history_detail", { id: summary.id });
  const facts = burstFacts(detail);
  assert(detail.burst.frames.length > 0 && detail.burst.frames.every(f => f.status === "missing" || f.rawFile), "每个到达的帧都应保留原图");
  assert.equal(facts.rawFiles, detail.burst.frames.filter(f => f.status !== "missing").length);
  const timeline = await call("history_timeline", { id: summary.id });
  const marks = Object.fromEntries(STAGES.map(stage => [stage, timeline.entries.filter(e => e.stage === stage).length]));
  for (const stage of STAGES) assert(marks[stage] >= 1, `时间线缺少 ${stage}：${JSON.stringify(marks)}`);
  const frames = timeline.entries.filter(e => e.stage === "frame");
  const seen = new Set(frames.map(e => `${e.camera}#${e.seq}`));
  const expected = detail.burst.frames.filter(f => f.status !== "missing").map(f => `${detail.burst.cameras[f.camera]}#${f.seq}`);
  assert.deepEqual([...seen].sort(), expected.sort(), "时间线的帧到达记录应与每相机每帧一一对应");
  assert(frames.every(e => e.data?.frameCounter != null && e.data?.triggerCounter != null), "帧到达记录应带 SDK 帧计数与触发计数");
  assert(timeline.stages.every(s => s.ms != null && s.ms >= 0), `阶段耗时不完整：${JSON.stringify(timeline.stages)}`);
  assert.equal(timeline.requestSeq != null, true, "时间线应带 requestSeq");
  const plc = timeline.entries.filter(e => e.source === "plc");
  const appLogs = await call("applog_query", { query: { cycleId: summary.cycleId, limit: 1 } });
  assert.equal(appLogs.total, timeline.entries.length - plc.length, "运行日志按 cycleId 查询结果应与时间线一致");
  const comparison = await call("workspace_compare_original", { historyId: summary.id });
  assert.equal(comparison.bundleId, summary.bundleId);
  assert.equal(comparison.candidateRecipe.revisionId, summary.recipeRevision);
  assert(comparison.burst, "原包重现应带段配方的区域判定");
  assert.equal(comparison.judgement.verdict, summary.verdict, `原包重跑结论与原记录不一致：${comparison.judgement.reason}`);
  assertSameRegions(detail.burst.judgement.regions, comparison.burst.judgement.regions, `原包重跑（${summary.sn}）`);
  assert.deepEqual(comparison.burst.judgement.flags, detail.burst.judgement.flags, "原包重跑的整件标记应与原记录一致");
  return { id: summary.id, sn: summary.sn, cycleId: summary.cycleId, verdict: summary.verdict, requestSeq: timeline.requestSeq,
    stages: Object.fromEntries(timeline.stages.map(s => [s.key, s.ms])), timeline: { app: timeline.entries.length - plc.length, plc: plc.length, notes: timeline.notes },
    regions: detail.burst.judgement.regions.map(regionKey), frames: { total: facts.framesRecorded, groups: facts.groupCount, states: facts.frameStates, withGlue: facts.framesWithGlue },
    replay: { verdict: comparison.judgement.verdict, regions: comparison.burst.judgement.regions.length } };
}

async function checkBurstUi(page, call, parts, output) {
  const nav = page.getByRole("navigation", { name: "操作导航" });
  await nav.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  const [first, second] = parts;
  await page.getByRole("button", { name: `查看 SN ${second.sn} 的记录`, exact: true }).click();
  await page.getByRole("heading", { name: "本件事件时间线", exact: true }).waitFor();
  await page.getByRole("table", { name: "本件事件", exact: true }).getByRole("cell", { name: "partStart↑", exact: true }).first().waitFor();
  await page.getByRole("region", { name: "本件事件时间线" }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: path.join(output, "history-timeline.png") });
  await page.getByRole("button", { name: "按原发布包重现", exact: true }).click();
  await page.getByRole("heading", { name: "原发布包重现结果", exact: true }).waitFor({ timeout: 90000 });
  await page.getByText(/个区域全部与原结果一致/).waitFor();
  await page.getByRole("heading", { name: "帧组与原图", exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: path.join(output, "history-replay.png") });
  await page.keyboard.press("[");
  await until(() => page.url(), url => url.endsWith(`/history/${first.id}`), "按 [ 未跳到前一件", 10000);
  await page.getByRole("heading", { name: "本件事件时间线", exact: true }).waitFor();
  await page.keyboard.press("]");
  await until(() => page.url(), url => url.endsWith(`/history/${second.id}`), "按 ] 未跳到后一件", 10000);
  await nav.getByRole("link", { name: "运行日志", exact: true }).click();
  await page.getByRole("table", { name: "运行日志", exact: true }).getByRole("row").nth(1).waitFor();
  await page.getByRole("searchbox", { name: "搜索日志" }).or(page.getByRole("textbox", { name: "搜索日志" })).fill(second.cycleId);
  await page.keyboard.press("Enter");
  await page.getByRole("table", { name: "运行日志", exact: true }).getByRole("cell", { name: "检测结果", exact: true }).first().waitFor();
  await page.screenshot({ path: path.join(output, "app-logs.png") });
  await page.getByRole("button", { name: "导出诊断包", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "导出诊断包", exact: true });
  await dialog.getByRole("combobox", { name: "诊断包记录范围" }).click();
  await page.getByRole("option", { name: "离线件", exact: true }).click();
  await dialog.getByRole("button", { name: "导出", exact: true }).click();
  await dialog.getByText(/^已导出/).waitFor({ timeout: 120000 });
  await page.screenshot({ path: path.join(output, "diagnostics-dialog.png") });
  const text = await dialog.innerText();
  await dialog.getByRole("button", { name: "完成", exact: true }).click();
  return text.match(/已导出\s*(\S+\.zip)/)?.[1] ?? null;
}

/**
 * 段配方的离线回放验证（2026-10-17 单圈样本集，lyFlow 测量；停稳拍数据对不上直线走廊，所以建站时检测项全关，只验证回放链路）：
 * 建站 → 投产 N 件 → 逐件核对时间线与运行日志，原包重跑按区域逐一对照（划分、结论、帧数、不完整组、胶宽统计）→ 删一张原图后
 * 原包重跑明确拒绝 → 界面回放与诊断包。
 */
async function runBurstLogReplay(page, options) {
  assert(Number.isInteger(options.parts) && options.parts >= 3, `--parts 至少为 3：第 2 件用于前后件导航与界面回放，最后一件用于删除原图，需互不重叠；实际 ${options.parts}`);
  const started = Date.now();
  const call = client(page);
  const records = await call("records_list");
  assert(records.root.includes("-tests"), `数据目录不属于隔离实例：${records.root}`);
  const settings = await call("cycle_get_settings");
  // 调试构建下布防实测约 250 ms（发布包核验约 56 ms、审计健康约 28 ms、PLC 布防确认约 160 ms），超过默认 200 ms 预算会判 ERR 98；验证回放时放宽到 1 s。
  await call("cycle_save_settings", { settings: { ...settings, productSource: "plc", timeouts: { ...settings.timeouts, armMs: ARM_MS } } });
  const engine = await until(() => call("engine_status"), e => e.ready, "lyFlow 引擎未就绪", 30000);
  const original = JSON.parse((await readFile(path.join(options.sampleSet, "sample-set.json"), "utf8")).replace(/^﻿/, "").trim());
  assert(CAMERA_FIXTURES[original.id], `样本集 ${original.id} 没有段配方夹具，已有：${Object.keys(CAMERA_FIXTURES).join("、")}`);
  const { dir, manifest } = await copySampleSet(options.sampleSet, options.output, read => ({ ...read, mmPerPx: CAMERA_FIXTURES[read.id].mmPerPx }));
  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir } });
  assert.equal(entered.measurement, "lyFlow");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const built = await buildBurstRecipe(call, { sourceId: manifest.id, partId: PART, patchSegments: checksOff, log: message => process.stderr.write(`· ${message}\n`) });
  assert(built.validation.passed, `验证未通过：${JSON.stringify(built.validation.checks.filter(c => !c.passed))}`);
  assert.equal(built.active.workspace.publish.state, "active");
  const id = built.id;
  const produced = [];
  for (let i = 0; i < options.parts; i++) produced.push((await produceBurst(call, id, { scenario: "normal", partId: PART })).summary);
  const parts = [];
  for (const summary of produced) parts.push(await checkBurstPart(call, summary));
  const middle = produced[1];
  const neighbors = await call("history_neighbors", { id: middle.id, query: null });
  assert.deepEqual(neighbors, { older: produced[0].id, newer: produced[2].id }, "前后件导航与生产顺序不符");
  const missing = produced.at(-1);
  const missingDetail = await call("history_detail", { id: missing.id });
  const root = records.root.replace(/[\\/]records$/, "");
  const frame = missingDetail.burst.frames.find(f => f.rawFile);
  const victim = path.join(root, "records-offline", frame.rawFile);
  assert(existsSync(victim), `找不到原图文件：${victim}`);
  await unlink(victim);
  const readBack = await call("workspace_record_frame_image", { historyId: missing.id, camera: frame.camera, seq: frame.seq }).then(() => null, error => error.message);
  assert(readBack, "原图删除后读该帧应报错");
  // 段配方的录制证据状态不随事后删文件刷新（仍是 complete），原包重跑读不到这一帧时按“测量失败”记检测不完整、不改 OK/NG；
  // 旧拍照点配方是整件拒绝。两种处理都不能悄悄当作原图完整：要么明确拒绝，要么把这一帧所在的帧组标检测不完整
  const lost = await call("workspace_compare_original", { historyId: missing.id }).then(comparison => ({ comparison }), error => ({ refused: error.message }));
  if (lost.refused) assert(/原图/.test(lost.refused), `原图缺失时原包重跑拒绝，原因应指向原图：${lost.refused}`);
  else {
    const regions = lost.comparison.burst.judgement.regions;
    assert(regions.some(r => r.incompleteGroups.includes(frame.seq)) && lost.comparison.burst.judgement.flags.incomplete,
      `原图缺失时原包重跑应把第 ${frame.seq} 帧组标检测不完整：${JSON.stringify(regions.map(r => r.incompleteGroups))}`);
    assert.equal(lost.comparison.judgement.verdict, missing.verdict, "缺一张原图不改 OK/NG");
  }
  const refused = lost.refused ?? `未拒绝：第 ${frame.seq} 帧组标检测不完整，结论 ${lost.comparison.judgement.verdict} 不变`;
  const ui = await checkBurstUi(page, call, produced, options.output);
  const pkg = await call("diagnostics_export", { request: { from: started - 60_000, to: Date.now(), historyIds: [produced[0].id], images: "all", maxImageParts: 1 } });
  const entries = zipEntries(pkg.path);
  for (const required of ["manifest.json", "logs/plc.jsonl", "records/parts.csv", "records/parts.jsonl"]) assert(entries.includes(required), `诊断包缺少 ${required}`);
  assert(entries.some(e => /^logs\/gluesight-.*\.jsonl$/.test(e)), "诊断包缺少运行日志");
  assert(entries.some(e => e.startsWith(`records/${produced[0].id}_`) && /\.(pgm|bmp|png)$/.test(e)), "诊断包缺少所选记录原图");
  assert(entries.some(e => e.startsWith("recipes/")), "诊断包缺少配方快照");
  await call("site_leave_offline");
  return { passed: true, model: "burst", elapsedMs: Date.now() - started, engine: engine.version ?? null, recipeId: id,
    taught: built.taught, calibration: { groupCounts: built.calibration.rounds.map(r => r.groupCount), expectedTotal: built.calibration.expectedTotal, toleranceTotal: built.calibration.toleranceTotal }, parts, neighbors,
    missingImage: { historyId: missing.id, removed: path.basename(victim), refused, readBack },
    diagnostics: { path: pkg.path, bytes: pkg.bytes, logs: pkg.logs, plcLogs: pkg.plcLogs, images: pkg.images, entries: entries.length }, uiDiagnostics: ui,
    boundary: "进程内 S7 服务与虚拟产线回放 2026-10-17 现场单圈原图（停稳拍数据）；像素当量 0.112 mm/px 为模拟标定，走廊笔划为演示夹具，检测项全关，只验证回放链路、不代表算法质量或现场标定" };
}

/** 旧拍照点配方的离线回放验证：按拍照点逐点比对，行为不变（新建起点改为 legacyTemplate）。 */
export async function runLegacyLogReplay(page, options) {
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
  const created = await call("workspace_create", { doc: await legacyTemplate(call) });
  const id = created.workspace.doc.id;
  const ws = workspaceCalls(call, id);
  const round = await captureRound(call, id, cameras.map(c => c.id), PART);
  await ws.edit("workspace_adopt_capture", { roundId: round.roundId, reuseTeaching: false, correspondenceConfirmed: false });
  const taught = await teach(call, id);
  const sample = await captureRound(call, id, cameras.map(c => c.id), PART);
  await ws.edit("workspace_capture_sample", { roundId: sample.roundId, expected: "OK", correspondenceConfirmed: true });
  const validation = (await ws.edit("workspace_validate", { samples: [{ historyId: null, sampleId: sample.roundId, expected: "OK" }], sourceConfirmed: true })).workspace.validation;
  assert(validation.passed, `验证未通过：${JSON.stringify(validation.checks.filter(c => !c.passed))}`);
  await ws.edit("workspace_publish", {});
  await until(async () => (await ws.get()).workspace.publish.state === "active", Boolean, "发布未生效", 60000);
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
  return { passed: true, model: "legacy", elapsedMs: Date.now() - started, engine: engine.version ?? null, recipeId: id, taught, parts, neighbors,
    missingImage: { historyId: missing.id, removed: path.basename(victim), recordImages: images.message, refused },
    diagnostics: { path: pkg.path, bytes: pkg.bytes, logs: pkg.logs, plcLogs: pkg.plcLogs, images: pkg.images, entries: entries.length }, uiDiagnostics: ui,
    boundary: "进程内 S7 服务与虚拟产线回放 2026-10-17 现场单圈原图；像素当量 0.1 mm/px 为模拟标定，ROI 为演示夹具" };
}

export async function runLogReplay(page, options) {
  return (options.model ?? "burst") === "legacy" ? runLegacyLogReplay(page, options) : runBurstLogReplay(page, options);
}

async function main() {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) args[process.argv[i].slice(2)] = process.argv[i + 1];
  const sampleSet = args["sample-set"] ?? process.env.GLUESIGHT_SAMPLE_SET;
  assert(sampleSet && existsSync(path.join(sampleSet, "sample-set.json")), "需要 --sample-set 指向 2026-10-17 单圈样本集");
  const output = path.resolve(args.output ?? "output/playwright/log-replay");
  await mkdir(output, { recursive: true });
  const instance = args.instance ?? "output/playwright/e2e-regression/instance.json";
  const cdp = await instanceCdp(instance, args.cdp, id => /^com\.xyzrobotics\.tujiaovision\.[a-z0-9]+-tests$/.test(id));
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = await uiPage(browser);
    const model = args.model ?? "burst";
    assert(["burst", "legacy"].includes(model), `--model 只能是 burst 或 legacy：${model}`);
    const result = await runLogReplay(page, { sampleSet, output, parts: Number(args.parts ?? 3), model });
    await writeFile(path.join(output, "result.json"), JSON.stringify(result, null, 2) + "\n");
    process.stdout.write(JSON.stringify({ passed: result.passed, model, parts: result.parts.map(p => [p.id, p.verdict, p.replay.verdict, p.stages.total]) }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
