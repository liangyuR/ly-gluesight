import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { cp, mkdir, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export const identifier = "com.xyzrobotics.tujiaovision.e2e-tests";
export const MISSING_INPUT_EXIT = 3;

const NARROW = { searchMm: 3, polarity: "dark", widthRange: [1, 5] };

// 演示夹具：按样本集 id 选取。每个拍照点组（Pose）选一台相机的图示教；guide 是刷选笔划（原图像素），roi 是矩形框选。
// 像素当量、笔划与检测参数只为让流程跑通而手工选定，不代表算法调优结果或现场标定。
export const FIXTURES = {
  "gs-20261017-normal": {
    // 样本集未标定（mmPerPx=null）。模拟标定沿用 docs/architecture/p0-plan.md 记录的同分辨率现场相机假设当量 0.112 mm/px，写进副本清单。
    mmPerPx: 0.112,
    normalPart: "p01",
    // 样本集只有正常件：在副本里由 p01 复制出 p02，把 S011 组所选图（样本 S032）的一段胶路逐行抹成两侧板面像素的渐变，作为人工断胶件。
    defect: { partId: "p02", from: "p01", sampleShot: "S032", camera: "cam2", box: [880, 140, 80, 100], pose: "S011", expected: "NG_GAP" },
    groups: {
      S001: { camera: "cam2", guide: [[745, 60], [740, 85], [735, 110], [734, 135], [740, 160], [746, 185], [743, 210], [736, 235], [735, 260], [734, 285], [740, 310], [739, 335], [738, 360], [731, 385], [724, 410], [723, 435]] },
      S002: { camera: "cam2", guide: [[615, 250], [633, 275], [649, 300], [662, 325], [676, 350], [690, 375], [701, 400], [708, 425], [712, 450], [714, 475], [710, 500], [705, 525], [700, 550], [707, 575], [706, 600], [705, 625], [703, 650], [708, 675], [714, 700], [708, 725], [696, 750]], detect: NARROW },
      S003: { camera: "cam1", guide: [[40, 472], [65, 470], [90, 467], [115, 458], [140, 450], [165, 442], [190, 429], [215, 420], [240, 401], [265, 397], [290, 383], [315, 370], [340, 349], [365, 336], [390, 329], [415, 316], [440, 303], [465, 284], [490, 278], [515, 265], [540, 258]] },
      S004: { camera: "cam3", guide: [[715, 380], [722, 405], [723, 430], [732, 455], [738, 480], [749, 505], [746, 530], [743, 555], [739, 580], [739, 605], [738, 630], [735, 655], [733, 680]], detect: NARROW },
      S005: { camera: "cam2", guide: [[1000, 639], [1025, 636], [1050, 626], [1075, 617], [1100, 609], [1125, 610], [1150, 612], [1175, 622], [1200, 625], [1225, 630], [1250, 631]], detect: NARROW },
      S006: { camera: "cam2", roi: [620, 380, 140, 380] },
      S007: { camera: "cam2", guide: [[684, 190], [682, 215], [670, 240], [656, 265], [645, 290], [648, 315], [646, 340], [652, 365], [654, 390], [655, 415], [658, 440], [660, 465], [662, 490], [657, 515], [659, 540], [668, 565], [677, 590], [680, 615], [682, 640], [684, 665], [686, 690]] },
      S008: { camera: "cam3", guide: [[610, 60], [614, 85], [622, 110], [625, 135], [630, 160], [626, 185], [626, 210], [622, 235], [630, 260], [632, 285], [638, 310], [639, 335], [646, 360], [646, 385], [645, 410], [643, 435], [641, 460], [632, 485], [616, 510]], detect: NARROW },
      S009: { camera: "cam2", guide: [[486, 30], [482, 55], [480, 80], [480, 105], [475, 130], [472, 155], [476, 180], [474, 205], [477, 230], [473, 255], [470, 280], [468, 305], [473, 330]] },
      S010: { camera: "cam1", guide: [[820, 246], [845, 256], [870, 266], [895, 275], [920, 285], [945, 296], [970, 306], [995, 312], [1020, 322], [1045, 332], [1070, 345], [1095, 353], [1120, 355], [1145, 355], [1170, 354], [1195, 349], [1220, 338]] },
      S011: { camera: "cam2", guide: [[935, 40], [933, 65], [925, 90], [920, 115], [912, 140], [918, 165], [922, 190], [921, 215], [915, 240], [912, 265], [916, 290], [921, 315], [923, 340]] },
      S012: { camera: "cam3", guide: [[496, 100], [502, 125], [510, 150], [517, 175], [518, 200], [515, 225], [519, 250], [526, 275], [531, 300], [535, 325], [535, 350], [538, 375], [541, 400], [544, 425], [548, 450], [546, 475], [549, 500], [558, 525], [568, 550], [577, 575], [581, 600]] },
    },
  },
};

const OK_CLASS = ["OK", "OK_WITH_EXCURSION"];
const verdictLabel = {
  OK: "OK", OK_WITH_EXCURSION: "OK · 局部超差", NG_POSITION: "NG · 位置超差", NG_WIDTH: "NG · 胶宽超差",
  NG_ABSOLUTE: "NG · 超绝对限", NG_GAP: "NG · 断胶", ERR_INSPECT: "ERR · 未测成",
};
const sampleLabelText = { ok: "合格", ng: "不合格", unknown: "未标注" };

export function missingInputs(inputs) {
  const problems = [];
  if (!inputs.sampleSet || !existsSync(path.join(inputs.sampleSet, "sample-set.json"))) problems.push(`样本集目录缺少 sample-set.json：${inputs.sampleSet || "（未通过 --sample-set 或 GLUESIGHT_SAMPLE_SET 提供）"}`);
  if (!inputs.lyflow || !existsSync(path.join(inputs.lyflow, "lyflow_core.dll"))) problems.push(`lyFlow 运行库目录缺少 lyflow_core.dll：${inputs.lyflow || "（未通过 --lyflow-runtime 或 GLUESIGHT_LYFLOW_RUNTIME 提供）"}`);
  return problems;
}

function within(parent, child) {
  const rel = path.relative(parent, child);
  return rel === "" || (rel !== ".." && !rel.startsWith(`..${path.sep}`) && !path.isAbsolute(rel));
}

/** 把 24/8 位未压缩 BMP 里 box 区域逐行换成左右紧邻像素之间的线性渐变，抹掉区域内的胶条。 */
export function eraseBmp(bytes, [x0, y0, w, h]) {
  assert.equal(bytes.toString("ascii", 0, 2), "BM", "人工断胶只支持 BMP 原图");
  const offset = bytes.readUInt32LE(10), width = bytes.readInt32LE(18), height = bytes.readInt32LE(22);
  const bpp = bytes.readUInt16LE(28);
  assert(bytes.readUInt32LE(30) === 0 && [8, 24].includes(bpp), "人工断胶只支持未压缩的 8/24 位 BMP");
  const pixel = bpp / 8, stride = Math.ceil((width * pixel) / 4) * 4, rows = Math.abs(height);
  const at = (x, y) => offset + (height > 0 ? rows - 1 - y : y) * stride + x * pixel;
  assert(x0 >= 1 && x0 + w < width && y0 >= 0 && y0 + h <= rows, "人工断胶区域越界");
  const out = Buffer.from(bytes);
  for (let y = y0; y < y0 + h; y++) {
    const left = at(x0 - 1, y), right = at(x0 + w, y);
    for (let x = x0; x < x0 + w; x++) {
      const t = (x - x0 + 1) / (w + 1);
      for (let c = 0; c < pixel; c++) out[at(x, y) + c] = Math.round(bytes[left + c] * (1 - t) + bytes[right + c] * t);
    }
  }
  return out;
}

/** 复制样本集到输出目录：原件只读；副本写入模拟标定当量，并按夹具生成人工断胶件。返回副本目录与夹具。 */
export async function prepareSampleSet(source, outputDir) {
  const original = JSON.parse((await readFile(path.join(source, "sample-set.json"), "utf8")).replace(/^﻿/, "").trim());
  const fixture = FIXTURES[original.id];
  assert(fixture, `样本集 ${original.id} 没有端到端夹具，已有：${Object.keys(FIXTURES).join("、")}`);
  await mkdir(outputDir, { recursive: true });
  const root = await realpath(outputDir);
  const target = path.join(root, "sample-set");
  assert.equal(path.dirname(target), root);
  const realSource = await realpath(source);
  const realTarget = existsSync(target) ? await realpath(target) : target;
  for (const candidate of new Set([target, realTarget])) {
    if (within(realSource, candidate) || within(candidate, realSource)) {
      throw new Error(`样本集副本目录 ${candidate} 与样本集原件 ${realSource} 相同或互相包含，拒绝复制`);
    }
  }
  await rm(target, { recursive: true, force: true });
  await cp(source, target, { recursive: true });
  const manifest = { ...original, id: `${original.id}-e2e`, mmPerPx: fixture.mmPerPx };
  const defect = fixture.defect;
  if (defect) {
    const from = manifest.parts.find(p => p.partId === defect.from);
    assert(from && !manifest.parts.some(p => p.partId === defect.partId), `样本集无法生成人工断胶件 ${defect.partId}`);
    const dir = path.join(target, "parts", defect.partId);
    await cp(path.join(target, "parts", defect.from), dir, { recursive: true });
    const file = (await readdir(dir)).find(name => name.toLowerCase().startsWith(`${defect.sampleShot}_${defect.camera}_v1.`.toLowerCase()));
    assert(file, `样本件 ${defect.from} 缺少 ${defect.sampleShot}_${defect.camera}_v1`);
    await writeFile(path.join(dir, file), eraseBmp(await readFile(path.join(dir, file)), defect.box));
    manifest.parts = [...manifest.parts, { ...from, partId: defect.partId, label: "ng", shotLabels: { [defect.sampleShot]: "gap" },
      origin: `${from.origin ?? from.partId}（端到端人工断胶：${defect.sampleShot} 区域 ${defect.box.join(",")}）` }];
  }
  await writeFile(path.join(target, "sample-set.json"), JSON.stringify(manifest, null, 2) + "\n");
  return { dir: target, fixture, manifest };
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

const GATE_BUSY = /正在处理 PLC 事务|检测仍有在途事务|设备配置正在变化|尚未回到空闲|正在预热|PLC 事务 3 秒内仍未结束/;
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

async function captureRound(call, id, cameraIds, partId) {
  const round = await call("recipe_capture_start", { recipeId: id, cameraIds, drainTimeoutMs: 1000 });
  assert.equal(round.state, "waitingStart");
  await call("offline_capture", { partId });
  const done = await until(() => call("recipe_capture_get", { roundId: round.roundId }), r => ["complete", "failed"].includes(r?.state), `整圈采集 ${partId} 未结束`, 90000);
  assert.equal(done.state, "complete", `整圈采集 ${partId} 失败：${done.error}`);
  await until(() => call("site_status"), s => !s.running, "离线产线采集后未停止", 15000);
  return done;
}

function strokeRoi(points, [width, height], margin = 40) {
  const xs = points.map(p => p[0]), ys = points.map(p => p[1]);
  const x = Math.max(0, Math.floor(Math.min(...xs) - margin)), y = Math.max(0, Math.floor(Math.min(...ys) - margin));
  return [x, y, Math.min(width, Math.ceil(Math.max(...xs) + margin)) - x, Math.min(height, Math.ceil(Math.max(...ys) + margin)) - y];
}

async function teach(ws, view, fixture) {
  const shots = view.workspace.doc.shots;
  const poses = [...new Set(shots.map(s => s.poseId))];
  assert.deepEqual(poses.filter(p => !fixture.groups[p]), [], "夹具未覆盖全部拍照点组");
  const taught = [];
  for (const pose of poses) {
    const choice = fixture.groups[pose];
    const k = shots.findIndex(s => s.poseId === pose && s.camera === choice.camera);
    assert(k >= 0, `拍照点组 ${pose} 没有相机 ${choice.camera} 的图`);
    const selected = await ws.edit("workspace_select_group_image", { k, view: 1 });
    const { id: imageId, size } = selected.workspace.frames[k].image;
    const declared = selected.workspace.doc.shots[k].mmPerPx;
    assert(Math.abs(declared - fixture.mmPerPx) < 1e-6, `${shots[k].id} 未采用样本集声明当量：${declared}`);
    const extracted = choice.guide
      ? await ws.edit("workspace_extract_centerline", { k, imageId, roi: strokeRoi(choice.guide, size), guide: { points: choice.guide, radiusPx: 30 } })
      : await ws.edit("workspace_extract_centerline", { k, imageId, roi: choice.roi });
    assert(extracted.path.length >= 3, `${shots[k].id} 中线提取失败：${extracted.message}`);
    await ws.edit("workspace_save_params", { k, params: { path: extracted.path, mmPerPx: declared, ...(choice.detect ? { detect: choice.detect } : {}) } });
    const trial = (await ws.edit("workspace_trial", { k, imageId })).workspace.frames[k].trial;
    assert(trial?.passed, `${shots[k].id} 试测未通过：${trial?.reason}`);
    assert(OK_CLASS.includes(trial.verdict), `${shots[k].id} 正常示教图试测结论不是合格类：${trial.verdict} ${trial.reason}`);
    await ws.edit("workspace_save_teach", { k, imageId });
    taught.push({ k, pose, shotId: shots[k].id, method: choice.guide ? "brush" : "roi", points: extracted.path.length, trialVerdict: trial.verdict, coverage: trial.coverage });
  }
  return taught;
}

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

export async function produce(call, recipeId, partId) {
  const previous = await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } });
  await call("offline_run", { recipeId, scenario: "normal", continuous: false, partId });
  const summary = await until(async () => (await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } })).items.find(i => i.id !== previous.items[0]?.id),
    Boolean, `样本件 ${partId} 未生成检测记录`, 90000);
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
  if (settings.productSource !== "plc") {
    await call("cycle_save_settings", { settings: { ...settings, productSource: "plc" } });
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
  const runSelect = page.getByRole("combobox", { name: "离线配方", exact: true });
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
  await page.getByRole("combobox", { name: "历史配方", exact: true }).selectOption(id);
  for (const { summary } of Object.values(produced)) {
    const row = page.getByRole("row", { name: `查看 SN ${summary.sn} 的记录`, exact: true });
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
    inputs: { sampleSet: options.sampleSet, offlineSampleSet: prepared.dir, sampleSetId: prepared.manifest.id, mmPerPx: fixture.mmPerPx, lyflow: options.lyflow, lyflowVersion: engine.version ?? null },
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

/** 实例刚启动时先显示资料准备窗口，等到主窗口的 Tauri UI 页面出现。 */
export async function uiPage(browser) {
  const page = await until(() => browser.contexts().flatMap(context => context.pages())
    .find(p => /tauri\.localhost|localhost:1420|^tauri:\/\//.test(p.url()) && !p.url().includes("storage-startup")),
    Boolean, "未找到 Tauri UI 页面", 120000, 250);
  page.setDefaultTimeout(30000);
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
  return page;
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
  const cdp = args.cdp ?? "http://localhost:9341";
  const url = new URL(cdp);
  assert(["127.0.0.1", "localhost", "[::1]"].includes(url.hostname), "仅允许本机 CDP");
  const instance = args.instance ?? "output/playwright/e2e-regression/instance.json";
  const manifest = JSON.parse(await readFile(instance, "utf8"));
  assert.equal(manifest.identifier, identifier, "仅允许隔离端到端实例");
  assert.equal(Number(url.port), manifest.debugPort);
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = await uiPage(browser);
    const output = args.output ?? "output/playwright/e2e-regression/result.json";
    const result = await runOfflineE2E(page, { ...inputs, output, workDir: path.dirname(output) });
    process.stdout.write(JSON.stringify({ passed: result.passed, elapsedMs: result.elapsedMs, branches: result.branches,
      production: Object.fromEntries(Object.entries(result.production).map(([k, v]) => [k, v.verdict])), replay: Object.fromEntries(["original", "rules", "raw"].map(k => [k, `${result.replay[k].verdict} via ${result.replay[k].via}`])) }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
