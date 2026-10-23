import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  buildBurstRecipe, burstFacts, CAMERA_FIXTURES, captureRound, checksOff, client, copySampleSet, instanceCdp, produceBurst, uiPage, until, workspaceCalls,
} from "./lib.mjs";

const PART = "p01";
const ARM_MS = 1000;
const STAGES = ["partStart", "armed", "partEnd", "result", "released"];

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

export async function runLogReplay(page, options) {
  return runBurstLogReplay(page, options);
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
    assert.equal(model, "burst", "仅支持段配方");
    const result = await runLogReplay(page, { sampleSet, output, parts: Number(args.parts ?? 3), model });
    await writeFile(path.join(output, "result.json"), JSON.stringify(result, null, 2) + "\n");
    process.stdout.write(JSON.stringify({ passed: result.passed, model, parts: result.parts.map(p => [p.id, p.verdict, p.replay.verdict, p.stages.total]) }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
