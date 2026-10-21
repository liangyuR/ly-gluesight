import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { mkdir, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  identifier, MISSING_INPUT_EXIT, captureRound, client, instanceCdp, missingInputs, prepareSampleSet, produce, teach, uiPage, until, workspaceCalls,
} from "./offline-e2e.mjs";

// 特征定位示教的端到端场景（定稿 §5、§13）：真 lyFlow + 样本集副本，在离线模式里走
// 示教页画框（压胶路被拦、高光框保存被拦、好框通过）→ 定位试测 → 验证统计与两端参数建议 → 确认采用 → 重新试测保存 → 发布 → 投产。
// 中线示教、样本采集、验证与发布走 IPC（offline-e2e 已覆盖其界面）；定位特征步骤与“采用建议”走界面。
// 框是按样本集 S002_cam2（拍照点组 S001 选的图）手工选的演示夹具，不代表现场选框：
//   bead 压在示教中线上；bright 在图中央高光区（≥ 215 的像素约 96%，纹理够）；good 是 G4 应用内检查扫出来的第一个全过的框。
export const SCENARIOS = {
  "gs-20261017-normal": { pose: "S001", camera: "cam2", bead: [700, 150, 80, 80], bright: [464, 416, 96, 96], good: [248, 248, 96, 96] },
};

const outputFiles = [];

async function shot(page, dir, name) {
  if (!dir) return;
  const file = path.join(dir, name);
  await page.screenshot({ path: file, fullPage: true });
  outputFiles.push(file);
}

/** 打开工作台某一页：HashRouter，重载让工作台重新读取 IPC 改过的候选。 */
async function openWorkbench(page, id, route) {
  await page.evaluate(route => { window.location.hash = route; }, route);
  await page.reload();
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
  const picker = page.locator("label.wp-recipe-picker select");
  await until(() => picker.locator("option").evaluateAll(options => options.map(o => o.value)), values => values.includes(id), `当前配方下拉里没有 ${id}`, 30000);
  if (await picker.inputValue() !== id) await picker.selectOption(id);
  await until(() => picker.inputValue(), value => value === id, `没有切换到配方 ${id}`, 15000);
}

/** 在示教图上按原图像素拖出一个框（经 SVG 的屏幕变换换算成窗口坐标）。 */
async function dragBox(page, label, [x, y, w, h]) {
  const corners = await page.evaluate(({ label, box }) => {
    const svg = [...document.querySelectorAll("svg.wp-gray-image")].find(s => s.getAttribute("aria-label") === label);
    const g = svg?.querySelector(":scope > g");
    const m = g?.getScreenCTM();
    if (!m) return null;
    const at = (px, py) => { const p = new DOMPoint(px, py).matrixTransform(m); return [p.x, p.y]; };
    return [at(box[0], box[1]), at(box[0] + box[2], box[1] + box[3])];
  }, { label, box: [x, y, w, h] });
  assert(corners, `找不到示教图 ${label}`);
  const [[x0, y0], [x1, y1]] = corners;
  await page.mouse.move(x0, y0);
  await page.mouse.down();
  await page.mouse.move((x0 + x1) / 2, (y0 + y1) / 2, { steps: 4 });
  await page.mouse.move(x1, y1, { steps: 4 });
  await page.mouse.up();
}

const card = page => page.locator("details.wp-fixture-card");
async function openCard(page) {
  if (!(await card(page).evaluate(d => d.open))) await card(page).locator("summary").click();
}
const cardSummary = async page => (await card(page).locator("summary").innerText()).replace(/\s+/g, " ").trim();
const rectText = rect => `框 ${rect.join(", ")}`;

/** 屏幕拖框会差一两个像素：数值框逐项改到夹具值，每改一项等检查回来（摘要显示新框）再改下一项。 */
async function settleRect(page, target) {
  const labels = ["定位框 x", "定位框 y", "定位框 宽", "定位框 高"];
  for (let i = 0; i < 4; i++) {
    const field = card(page).getByLabel(labels[i], { exact: true });
    if (Number(await field.inputValue()) === target[i]) continue;
    const before = await cardSummary(page);
    await field.fill(String(target[i]));
    await until(() => cardSummary(page), s => s !== before, `定位框 ${labels[i]} 改为 ${target[i]} 后摘要未更新`, 15000);
  }
  await until(() => cardSummary(page), s => s.includes(rectText(target)), `定位框没有落到 ${target.join(",")}`, 15000);
}

async function drawFixture(page, label, rect) {
  const start = card(page).getByRole("button", { name: /在原图上画定位框|重画定位框/ });
  await start.click();
  await dragBox(page, label, rect);
  await until(() => cardSummary(page), s => /框 \d+, \d+, \d+, \d+/.test(s), "画框后定位特征卡片没有显示框", 15000);
  await settleRect(page, rect);
}

const checkList = (page, name) => card(page).getByRole("list", { name, exact: true });
async function checkItems(list) {
  return (await list.locator("li").evaluateAll(items => items.map(li => ({ cls: li.className, text: li.innerText.trim() }))));
}

async function runTrial(page, call, id, k) {
  const before = (await call("workspace_get", { id })).workspace.frames[k].trial;
  await page.getByRole("button", { name: "运行试测", exact: true }).click();
  return until(async () => (await call("workspace_get", { id })).workspace.frames[k].trial, t => t && JSON.stringify(t) !== JSON.stringify(before), "试测没有结果", 90000);
}

function poses(value, out = []) {
  if (Array.isArray(value)) value.forEach(v => poses(v, out));
  else if (value && typeof value === "object") {
    if (value.pose && typeof value.pose === "object" && "accepted" in value.pose) out.push(value.pose);
    Object.values(value).forEach(v => poses(v, out));
  }
  return out;
}

export async function runFixtureE2E(page, options) {
  const startedAt = Date.now();
  const steps = [];
  const step = (name, detail) => { steps.push({ name, atMs: Date.now() - startedAt, ...(detail ? { detail } : {}) }); process.stderr.write(`· ${name}\n`); };
  const call = client(page);
  const shots = options.output ? path.dirname(options.output) : null;
  if (shots) await mkdir(shots, { recursive: true });

  const records = await call("records_list");
  assert(records.root.includes(identifier), `数据目录不属于隔离实例：${records.root}`);
  assert.equal((await call("site_status")).mode, "real", "软件启动必须处于真机模式");
  const settings = await call("cycle_get_settings");
  if (settings.productSource !== "plc") await call("cycle_save_settings", { settings: { ...settings, productSource: "plc" } });
  const engine = await until(() => call("engine_status"), e => e.ready, "lyFlow 引擎未就绪", 30000);

  const prepared = await prepareSampleSet(options.sampleSet, options.workDir);
  const { fixture } = prepared;
  const scenario = SCENARIOS[prepared.manifest.id.replace(/-e2e$/, "")];
  assert(scenario, `样本集 ${prepared.manifest.id} 没有特征定位场景`);
  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: prepared.dir } });
  assert.equal(entered.measurement, "lyFlow");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const cameraIds = (await call("camera_rig_config")).map(c => c.id);
  step("lyFlow 就绪并进入离线·样本集", { version: engine.version ?? null });

  const created = await call("workspace_create", { doc: await call("recipe_template") });
  const id = created.workspace.doc.id;
  const ws = workspaceCalls(call, id);
  const teachRound = await captureRound(call, id, cameraIds, fixture.normalPart);
  const adopted = await ws.edit("workspace_adopt_capture", { roundId: teachRound.roundId, reuseTeaching: false, correspondenceConfirmed: false });
  const taught = await teach(ws, adopted, fixture);
  const target = taught.find(t => t.pose === scenario.pose);
  const k = target.k;
  const shotSpec = (await ws.get()).workspace.doc.shots[k];
  assert.equal(shotSpec.camera, scenario.camera);
  // 验证样本先进样本库：定位试测要在“其他帧”上跑
  const samples = [];
  for (const [partId, expected] of [[fixture.normalPart, "OK"], [fixture.defect.partId, fixture.defect.expected]]) {
    const round = await captureRound(call, id, cameraIds, partId);
    await ws.edit("workspace_capture_sample", { roundId: round.roundId, expected, correspondenceConfirmed: true });
    samples.push({ partId, roundId: round.roundId, expected });
  }
  step("中线示教与样本采集（IPC）", { recipeId: id, k, shotId: shotSpec.id, samples: samples.map(s => s.partId) });

  // —— 示教页：定位特征 ——
  const label = `${scenario.pose} · ${scenario.camera} · 图 ${shotSpec.view}`;
  await openWorkbench(page, id, `#/recipe/teach?frame=${k}`);
  await page.getByRole("img", { name: label, exact: true }).waitFor({ timeout: 30000 });
  await openCard(page);

  // 1. 压在胶路上的框：外圈 / 走廊检查不过，框不进草稿，卡片标“框未采用”并说出原因
  await drawFixture(page, label, scenario.bead);
  await until(() => cardSummary(page), s => s.includes("框未采用"), "压胶路的框应标“框未采用”", 15000);
  const beadAlert = (await card(page).getByRole("alert").filter({ hasText: "这个框没有写进示教" }).innerText()).trim();
  assert.match(beadAlert, /走廊/, `压胶路的框原因应指向走廊：${beadAlert}`);
  assert.equal((await ws.get()).workspace.doc.shots[k].fixture ?? null, null, "被拦的框不得进入候选");
  await page.locator('svg.wp-gray-image g[aria-label="定位框（未通过检查）"]').waitFor({ state: "attached" });
  await shot(page, shots, "fixture-1-bead-rejected.png");
  step("压胶路的框被拦", { rect: scenario.bead, reason: beadAlert });

  // 2. 高光框：几何没问题、进草稿，但亮像素检查阻断；试测无效，保存按钮不可用
  await drawFixture(page, label, scenario.bright);
  const liveBright = await until(async () => (await checkList(page, "定位框检查").count()) ? checkItems(checkList(page, "定位框检查")) : null,
    items => items?.some(i => /block/.test(i.cls)), "高光框的实时检查没有阻断项", 15000);
  const current = (await ws.get()).workspace;
  const { path: centerline, mmPerPx, detect, limits } = current.doc.shots[k];
  const brightCheck = await call("workspace_fixture_check", { id, revision: current.revision, k,
    params: { path: centerline, mmPerPx, detect: detect ?? null, limits: limits ?? null, fixture: { searchRadiusPx: 60, angleRangeDeg: 0, minScore: .6, maxPeakRatio: 1, source: "", rect: scenario.bright } } });
  const brightItem = brightCheck.checks.find(c => c.id === "bright");
  assert(brightItem && !brightItem.passed, `高光框的亮像素检查应不过：${JSON.stringify(brightCheck.checks)}`);
  assert(brightCheck.checks.filter(c => c.level === "block" && !c.passed).every(c => !["inside", "corridor"].includes(c.id)), "高光框应只因纹理 / 亮度被拦，几何应通过");
  const brightTrial = await runTrial(page, call, id, k);
  assert.equal(brightTrial.passed, false, "高光框的试测不应有效");
  assert(brightTrial.fixture && !brightTrial.fixture.passed && brightTrial.fixture.checks.some(c => c.id === "bright" && !c.passed), `试测报告应含亮像素阻断：${JSON.stringify(brightTrial.fixture)}`);
  await checkList(page, "示教图检查（试测）").waitFor();
  assert(await page.getByRole("button", { name: "保存本组示教", exact: true }).isDisabled(), "高光框试测后不应能保存本组示教");
  await shot(page, shots, "fixture-2-bright-blocked.png");
  step("高光框保存被拦", { rect: scenario.bright, brightFraction: brightCheck.brightFraction, live: liveBright.filter(i => /block/.test(i.cls)).map(i => i.text), trialReason: brightTrial.reason });

  // 3. 好框：检查全过，试测有效（示教图自定位恒等）
  await drawFixture(page, label, scenario.good);
  await until(async () => (await checkList(page, "定位框检查").count()) ? checkItems(checkList(page, "定位框检查")) : null,
    items => items?.length && !items.some(i => /block/.test(i.cls)), "好框的实时检查应没有阻断项", 15000);
  const goodTrial = await runTrial(page, call, id, k);
  assert(goodTrial.passed && goodTrial.fixture?.passed, `好框试测应有效：${goodTrial.reason} ${JSON.stringify(goodTrial.fixture?.checks)}`);
  const selfPose = goodTrial.fixture.pose;
  assert(selfPose?.accepted && Math.hypot(...selfPose.offset) <= 0.5, `示教图自定位应恒等：${JSON.stringify(selfPose)}`);
  await until(() => cardSummary(page), s => s.includes("试测已验收"), "好框试测后卡片应显示“试测已验收”", 15000);
  const savedFixture = (await ws.get()).workspace.doc.shots[k];
  const fixtureSpec = savedFixture.fixture ?? savedFixture.views?.find(v => v.view === shotSpec.view)?.fixture;
  assert.deepEqual(fixtureSpec?.rect, scenario.good, "保存的定位框应是好框");
  assert.equal(fixtureSpec.angleRangeDeg, 0, "v1 默认只找平移");

  // 4. 定位试测：示教图 + 样本库同一拍照点同一幅图的其他帧，逐帧显示位姿与 d′
  await card(page).getByRole("button", { name: "定位试测", exact: true }).click();
  const trialTable = page.getByRole("table", { name: "定位试测逐帧结果", exact: true });
  await trialTable.waitFor({ timeout: 60000 });
  const trialRows = await trialTable.locator("tbody tr").evaluateAll(rows => rows.map(r => [...r.cells].map(c => c.innerText.trim().replace(/\s+/g, " "))));
  assert.equal(trialRows.length, 1 + samples.length, `定位试测应有示教图与 ${samples.length} 帧样本：${JSON.stringify(trialRows)}`);
  assert(trialRows.every(r => r[1].startsWith("采用")), `定位试测各帧位姿都应被采用：${JSON.stringify(trialRows)}`);
  assert.equal(await page.getByRole("alert").filter({ hasText: "这次定位试测无效" }).count(), 0);
  await shot(page, shots, "fixture-3-good-box.png");
  await trialTable.scrollIntoViewIfNeeded();
  await shot(page, shots, "fixture-4-locate-trial.png");
  step("好框试测有效、定位试测", { rect: scenario.good, selfPose: { offset: selfPose.offset, score: selfPose.score }, checks: goodTrial.fixture.checks.map(c => `${c.id}:${c.passed ? "过" : c.level}`), locateTrial: trialRows.map(r => ({ frame: r[0], pose: r[1], score: r[2], peakRatio: r[3], offset: r[4], d: r[7] })) });

  // 5. 喷嘴确认后才能保存
  const save = page.getByRole("button", { name: "保存本组示教", exact: true });
  assert(await save.isDisabled(), "未确认喷嘴前不应能保存");
  await card(page).getByRole("checkbox", { name: "已确认定位框内没有胶（含越过示教段后拐弯、其他 Pose 或其他胶路的胶），也没有喷嘴、胶枪等随相机不动的结构" }).check();
  await until(() => save.isEnabled(), Boolean, "确认喷嘴后保存按钮未启用", 10000);
  await save.click();
  await until(async () => (await ws.get()).progress.shots[k].views.find(v => v.view === shotSpec.view)?.state, s => s === "taught", "保存本组示教未生效", 15000);
  step("喷嘴确认后保存示教");

  // —— 验证：统计、建议、确认采用 ——
  const validate = async () => (await ws.edit("workspace_validate", { samples: samples.map(s => ({ historyId: null, sampleId: s.roundId, expected: s.expected })), sourceConfirmed: true })).workspace.validation;
  const first = await validate();
  const report = first.fixture?.find(r => r.k === k && r.view === shotSpec.view);
  assert(report, `验证应给出特征定位统计：${JSON.stringify(first.checks)}`);
  assert(report.passed, `特征定位统计应通过：${JSON.stringify(report.checks.filter(c => !c.passed))}`);
  assert(report.checks.some(c => c.id === "brightnessLock" && c.passed), "亮度 ±40 锁定检查应在验证里跑过并通过");
  assert(first.checks.some(c => c.name === "特征定位" && c.passed), "发布前检查应含“特征定位”");
  assert(report.suggestion?.applicable, `应有可采用的两端参数建议：${JSON.stringify(report.suggestion)}`);
  assert(report.checks.some(c => c.id === "endParams" && !c.passed), "没采用建议时应告警两端参数仍是临时默认");
  await openWorkbench(page, id, "#/recipe/validation");
  const panel = page.getByRole("region", { name: `拍照点 ${shotSpec.id} 图 ${shotSpec.view} 特征定位统计`, exact: true });
  await panel.waitFor({ timeout: 30000 });
  const panelText = (await panel.innerText()).replace(/\s+/g, " ");
  await panel.scrollIntoViewIfNeeded();
  await shot(page, shots, "fixture-5-validation-panel.png");
  step("验证统计", { sigmaMm: report.sigmaMm, meanMm: report.meanMm, searchUse: report.searchUse, samples: report.samples,
    warnings: report.checks.filter(c => !c.passed).map(c => c.detail), suggestion: report.suggestion, rotation: report.rotation, panel: panelText.slice(0, 400) });

  await panel.getByRole("button", { name: "采用建议…", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "采用两端参数建议", exact: true });
  await dialog.waitFor();
  await dialog.getByRole("button", { name: "确认写入配方", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  const appliedDoc = await until(() => ws.get(), v => {
    const s = v.workspace.doc.shots[k], f = s.fixture ?? s.views?.find(x => x.view === shotSpec.view)?.fixture;
    return f?.maxEndShiftMm === report.suggestion.maxEndShiftMm && f?.endMarginMm === report.suggestion.endMarginMm;
  }, "采用建议后配方没有写入两端参数", 15000);
  assert.notEqual(appliedDoc.progress.shots[k].views.find(v => v.view === shotSpec.view)?.state, "taught", "采用建议后该视图应要求重新试测");
  await assert.rejects(ws.edit("workspace_publish", {}), /验证/, "采用建议后旧验证不得发布");
  step("确认采用建议", { maxEndShiftMm: report.suggestion.maxEndShiftMm, endMarginMm: report.suggestion.endMarginMm });

  // 重新试测、确认喷嘴、保存，再验证
  await openWorkbench(page, id, `#/recipe/teach?frame=${k}`);
  await page.getByRole("img", { name: label, exact: true }).waitFor({ timeout: 30000 });
  const retrial = await runTrial(page, call, id, k);
  assert(retrial.passed && retrial.fixture?.passed, `采用建议后重新试测应有效：${retrial.reason}`);
  await openCard(page);
  await card(page).getByRole("checkbox", { name: "已确认定位框内没有胶（含越过示教段后拐弯、其他 Pose 或其他胶路的胶），也没有喷嘴、胶枪等随相机不动的结构" }).check();
  await until(() => save.isEnabled(), Boolean, "重新试测后保存按钮未启用", 10000);
  await save.click();
  await until(async () => (await ws.get()).progress.shots[k].views.find(v => v.view === shotSpec.view)?.state, s => s === "taught", "重新保存示教未生效", 15000);
  const second = await validate();
  assert(second.passed, `采用建议后验证未通过：${JSON.stringify(second.checks.filter(c => !c.passed))}`);
  const secondReport = second.fixture.find(r => r.k === k && r.view === shotSpec.view);
  assert(secondReport.passed);
  assert(secondReport.checks.some(c => c.id === "endParams" && c.passed), `采用建议后两端参数告警应消失：${JSON.stringify(secondReport.checks.filter(c => !c.passed))}`);
  step("重新试测、保存并验证", { passed: second.passed, sigmaMm: secondReport.sigmaMm });

  // —— 发布与投产 ——
  await ws.edit("workspace_publish", {});
  const active = await until(() => ws.get(), v => !v.workspace.pending && v.workspace.publish.state === "active", "发布未生效", 30000);
  await until(async () => (await call("cycle_recipes")).find(r => r.id === id), Boolean, "发布后生产配方未生效", 30000);
  step("发布生效", { version: active.workspace.publish.version });

  const produced = {};
  for (const partId of [fixture.normalPart, fixture.defect.partId]) produced[partId] = await produce(call, id, partId);
  const normal = produced[fixture.normalPart].summary, defective = produced[fixture.defect.partId].summary;
  assert(["OK", "OK_WITH_EXCURSION"].includes(normal.verdict), `投产 ${fixture.normalPart} 应为合格类：${normal.verdict} ${normal.reason}`);
  assert.equal(defective.verdict, fixture.defect.expected, `投产 ${fixture.defect.partId} 应为 ${fixture.defect.expected}：${defective.reason}`);
  const detail = await call("history_detail", { id: normal.id });
  const productionPoses = poses(detail);
  assert(productionPoses.length >= 1 && productionPoses.every(p => p.accepted), `投产记录应带被采用的定位位姿：${JSON.stringify(productionPoses)}`);
  step("离线投产", { [fixture.normalPart]: normal.verdict, [fixture.defect.partId]: defective.verdict, poses: productionPoses.map(p => ({ offset: p.offset, score: p.score, endShiftMm: p.endShiftMm, excludedMm: p.excludedMm })) });

  // —— 生产页与历史页按记录的位姿叠图（G5，定稿 §6“生产/历史页显示测试”）——
  const overlayOf = (pose, where) => {
    const image = page.locator("svg.wp-gray-image");
    const box = pose.accepted ? image.locator('[aria-label="定位框（按位姿）"]') : image.locator('[aria-label^="定位未采用"]');
    return box.first().waitFor({ timeout: 30000 }).catch(() => { throw new Error(`${where}没有按记录的位姿画定位框（位姿 ${JSON.stringify(pose)}）`); });
  };
  const live = (await call("cycle_snapshot")).part;
  assert(live, "投产后检测页没有最近一件");
  const livePose = live.frames[k].viewResults.find(v => v.view === shotSpec.view)?.pose;
  assert(livePose, `生产帧 k${k + 1} 没有位姿记录`);
  await page.evaluate(() => { window.location.hash = "/inspect"; });
  await page.locator(`.qs-camera-thumbnail[data-frame-id="k${k + 1}"]`).click();
  await overlayOf(livePose, "生产页选中帧");
  await page.locator(".wp-runtime-strip .wp-kv").filter({ hasText: "定位" }).getByText(livePose.accepted ? /^采用 \(/ : /^未采用/).waitFor();
  await shot(page, shots, "fixture-6-production-overlay.png");
  const historyPose = detail.frames[k].viewResults.find(v => v.view === shotSpec.view)?.pose;
  assert(historyPose?.accepted, `投产正常件 k${k + 1} 的位姿应被采用：${JSON.stringify(historyPose)}`);
  await page.evaluate(id => { window.location.hash = `/history/${id}`; }, normal.id);
  await page.getByRole("heading", { name: "逐拍照点结果与原图", exact: true }).waitFor();
  await page.locator('select[aria-label="历史帧选择"]').selectOption(String(k));
  await overlayOf(historyPose, "历史原图");
  await page.getByLabel("定位摘要", { exact: true }).getByText(/采用 · 偏移/).waitFor();
  await shot(page, shots, "fixture-7-history-overlay.png");
  step("生产与历史按位姿叠图", { live: { cycleId: live.cycleId, accepted: livePose.accepted, offset: livePose.offset }, history: { id: normal.id, offset: historyPose.offset } });

  const fixtureDirs = await readdir(path.join(records.root, "..", "workspaces", id, "fixtures")).catch(() => null);
  await call("site_leave_offline").catch(() => {});

  const result = {
    passed: true, identifier, elapsedMs: Date.now() - startedAt,
    inputs: { sampleSet: options.sampleSet, offlineSampleSet: prepared.dir, sampleSetId: prepared.manifest.id, mmPerPx: fixture.mmPerPx, lyflow: options.lyflow, lyflowVersion: engine.version ?? null },
    recipe: { id, k, shotId: shotSpec.id, view: shotSpec.view, boxes: { bead: scenario.bead, bright: scenario.bright, good: scenario.good } },
    fixtureDirs, steps, screenshots: outputFiles,
    boundary: "离线样本集副本的验证样本与示教是同一件的同一批图，统计偏乐观；像素当量为模拟标定，框是演示夹具，不代表现场选框或门限调优",
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
  const inputs = { sampleSet: args["sample-set"] ?? process.env.GLUESIGHT_SAMPLE_SET, lyflow: args["lyflow-runtime"] ?? process.env.GLUESIGHT_LYFLOW_RUNTIME };
  const problems = missingInputs(inputs);
  if (problems.length) {
    process.stderr.write(`特征定位端到端输入缺失，未运行（不算通过）：\n${problems.join("\n")}\n`);
    process.exitCode = MISSING_INPUT_EXIT;
    return;
  }
  const instance = args.instance ?? "output/playwright/e2e-regression/instance.json";
  const cdp = await instanceCdp(instance, args.cdp, id => id === identifier);
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = await uiPage(browser);
    const output = args.output ?? "output/playwright/e2e-regression/fixture/result.json";
    let result;
    try { result = await runFixtureE2E(page, { ...inputs, output, workDir: path.dirname(output) }); } catch (error) {
      await mkdir(path.dirname(output), { recursive: true });
      await page.screenshot({ path: path.join(path.dirname(output), "failure.png"), fullPage: true }).catch(() => {});
      await writeFile(path.join(path.dirname(output), "failure.txt"), `${page.url()}
${await page.locator("body").innerText().catch(() => "")}`).catch(() => {});
      throw error;
    }
    process.stdout.write(JSON.stringify({ passed: result.passed, elapsedMs: result.elapsedMs, steps: result.steps.map(s => s.name) }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
