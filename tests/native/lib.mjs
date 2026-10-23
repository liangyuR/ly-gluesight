// 原生端到端脚本共用：IPC 客户端与重试、隔离实例连接、样本集副本、整圈采集、离线投产，以及两套配方流程的建站函数：
//   - 段配方（分段连拍，RecipeDoc.segments 非空，本文件“段配方”一节）：每台相机一条走廊示教 → 多轮标定 → 独立样本验证 → 发布 → 投产；
//   - 旧拍照点配方（segments 为空，只停用不删除）：采用整圈采集 → 按拍照点组示教 → 验证 → 发布。FIXTURES、teach、addErasedPart、
//     prepareSampleSet、legacyTemplate 与 produce 只服务旧配方，fixture-teach-e2e 与各脚本的 --model legacy 使用。
// 使用方：offline-e2e、offline-restart、fixture-teach-e2e、log-replay-offline、production-flow-ui、scripts/soak 与 scripts/soak/throughput。
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { cp, mkdir, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import path from "node:path";

export const identifier = "com.xyzrobotics.tujiaovision.e2e-tests";
export const MISSING_INPUT_EXIT = 3;
export const OK_CLASS = ["OK", "OK_WITH_EXCURSION"];

const NARROW = { searchMm: 3, polarity: "dark", widthRange: [1, 5] };

// 旧拍照点配方的演示夹具（段配方的按相机夹具见 CAMERA_FIXTURES）：按样本集 id 选取。每个拍照点组（Pose）选一台相机的图示教；guide 是刷选笔划（原图像素），roi 是矩形框选。
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

/**
 * 把样本集原件复制到 outputDir/sample-set（原件只读），清单经 patch(original) 改写后写回副本。
 * 复制前解析真实路径，副本与原件相同或互相包含时直接报错，不删除任何内容。返回副本目录与改写后的清单。
 */
export async function copySampleSet(source, outputDir, patch) {
  const original = JSON.parse((await readFile(path.join(source, "sample-set.json"), "utf8")).replace(/^﻿/, "").trim());
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
  const manifest = await patch(original, target);
  await writeFile(path.join(target, "sample-set.json"), JSON.stringify(manifest, null, 2) + "\n");
  return { dir: target, manifest };
}

/** 复制样本集到输出目录：原件只读；副本写入模拟标定当量，并按夹具生成人工断胶件。返回副本目录与夹具。 */
export async function prepareSampleSet(source, outputDir) {
  const original = JSON.parse((await readFile(path.join(source, "sample-set.json"), "utf8")).replace(/^﻿/, "").trim());
  const fixture = FIXTURES[original.id];
  assert(fixture, `样本集 ${original.id} 没有端到端夹具，已有：${Object.keys(FIXTURES).join("、")}`);
  const { dir, manifest } = await copySampleSet(source, outputDir, async (read, target) => {
    const patched = { ...read, id: `${read.id}-e2e`, mmPerPx: fixture.mmPerPx };
    const defect = fixture.defect;
    if (defect) await addErasedPart(target, patched, defect, { label: "ng", shotLabels: { [defect.sampleShot]: "gap" }, note: "人工断胶" });
    return patched;
  });
  return { dir, fixture, manifest };
}

/**
 * 样本集副本里由 spec.from 复制出 spec.partId，把样本 spec.sampleShot / spec.camera 原图的 spec.box 逐行抹平（eraseBmp），追加进 manifest.parts。
 * 只改副本（target 是 prepareSampleSet 给出的副本目录）；调用方负责之后重写副本的 sample-set.json。
 */
export async function addErasedPart(target, manifest, spec, { label, shotLabels = {}, note }) {
  const from = manifest.parts.find(p => p.partId === spec.from);
  assert(from && !manifest.parts.some(p => p.partId === spec.partId), `样本集无法生成${note}件 ${spec.partId}`);
  const dir = path.join(target, "parts", spec.partId);
  await cp(path.join(target, "parts", spec.from), dir, { recursive: true });
  const file = (await readdir(dir)).find(name => name.toLowerCase().startsWith(`${spec.sampleShot}_${spec.camera}_v1.`.toLowerCase()));
  assert(file, `样本件 ${spec.from} 缺少 ${spec.sampleShot}_${spec.camera}_v1`);
  await writeFile(path.join(dir, file), eraseBmp(await readFile(path.join(dir, file)), spec.box));
  manifest.parts = [...manifest.parts, { ...from, partId: spec.partId, label, shotLabels,
    origin: `${from.origin ?? from.partId}（端到端${note}：${spec.sampleShot} 区域 ${spec.box.join(",")}）` }];
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

const GATE_BUSY = /正在处理 PLC 事务|检测仍有在途事务|设备配置正在变化|尚未回到空闲|正在预热|PLC 事务 3 秒内仍未结束|离线 S7 服务尚未连接/;
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

export function workspaceCalls(call, id) {
  const current = async () => (await call("workspace_get", { id })).workspace.revision;
  return {
    get: () => call("workspace_get", { id }),
    edit: async (command, args) => call(command, { id, revision: await current(), ...args }),
  };
}

/**
 * 旧拍照点配方的新建起点：recipe_template 现在给段配方（三台相机、一个直线段），旧配方由它去掉段与相机检测设置得到
 * （segments 为空即旧拍照点配方，拍照点由随后的“采用整圈采集”写入）。只服务旧配方流程。
 */
export async function legacyTemplate(call) {
  const template = await call("recipe_template");
  return { ...template, segments: [], inspect: [], shots: [], cameras: {}, partCalib: null };
}

/** 开始一轮整圈采集并等它完成（旧拍照点配方与段配方通用）：partId 只对样本集来源有意义，合成图来源传 null。不核对采集的触发数或帧数。 */
export async function captureRound(call, id, cameraIds, partId = null) {
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

/** 按夹具给每个拍照点组选一张检测图并示教：刷选笔划或框选提取中线 → 保存参数 → 试测为合格类 → 保存示教。 */
export async function teach(ws, view, fixture) {
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
    await ws.edit("workspace_save_params", { k, params: { path: extracted.path, mmPerPx: declared, fixture: null, ...(choice.detect ? { detect: choice.detect } : {}) } });
    const trial = (await ws.edit("workspace_trial", { k, imageId })).workspace.frames[k].trial;
    assert(trial?.passed, `${shots[k].id} 试测未通过：${trial?.reason}`);
    assert(OK_CLASS.includes(trial.verdict), `${shots[k].id} 正常示教图试测结论不是合格类：${trial.verdict} ${trial.reason}`);
    await ws.edit("workspace_save_teach", { k, imageId });
    taught.push({ k, pose, shotId: shots[k].id, method: choice.guide ? "brush" : "roi", points: extracted.path.length, trialVerdict: trial.verdict, coverage: trial.coverage });
  }
  return taught;
}

export async function produce(call, recipeId, partId) {
  const previous = await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } });
  await call("offline_run", { recipeId, scenario: "normal", continuous: false, partId });
  const summary = await until(async () => (await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } })).items.find(i => i.id !== previous.items[0]?.id),
    Boolean, `样本件 ${partId} 未生成检测记录`, 90000);
  const site = await until(() => call("site_status"), s => !s.running, `样本件 ${partId} 握手未结束`, 30000);
  const run = site.recent.find(r => r.purpose === "production" && r.partId === partId && r.cycleId === summary.cycleId);
  assert(run, `最近样本件里没有 ${partId}（cycleId ${summary.cycleId}）`);
  await until(() => call("workspace_record_images", { historyId: summary.id }), v => v.complete,
    `样本件 ${partId} 原图录制未完成（本件结论 ${summary.verdict}：${summary.reason}）`, 30000);
  return { summary, run };
}

// ───────────── 段配方（分段连拍，RecipeDoc.segments 非空）─────────────
// 流程（规格 §6 顺序，与 docs/decisions/segment-burst.md 一致）：
//   新建（recipe_template 即段配方：三台相机、一个直线段，待标定）→ 一轮整圈采集选示教帧 → 每台相机一条走廊 →
//   ≥ 10 轮多轮标定（另加标准件轮次给 x_ref）→ 应用并确认建议容差 → 独立整圈样本验证 → 发布 → 投产。
// 运行时整条胶路按“未分段”处理（分段算法未做）：只跑每帧独立检测，核对整件帧组数 ∈ ΣN_exp ± Σtol。

/** 多轮标定的轮数下限：配方 calibrationRounds 默认 10（规格 §2 同一工件程序连续采集 ≥ 10 轮）。 */
export const CALIBRATION_ROUNDS = 10;
/** 标准件轮次：只用于胶偏参考 x_ref（均值与极差），不计入 N_exp 的轮数；至少 1 轮，取 2 轮才有极差。 */
export const STANDARD_ROUNDS = 2;

const FRAME_NARROW = { searchMm: 3, polarity: "dark", widthRange: [1, 5] };

/** 合成图缺省胶条：三台相机从不同方位看胶嘴，胶条在画面里上下错开（与 src-tauri/src/simimage.rs 的 burst_default_path 同口径），按相机下标给刷选笔划。 */
function syntheticBead(index) {
  const dy = 80 * (Math.min(index, 2) - 1);
  const points = [];
  for (let x = 940; x >= 340; x -= 40) points.push([x, Math.round(480 + dy - (20 * (980 - x)) / 680)]);
  return points;
}

/**
 * 段配方的演示夹具（按相机；旧配方的按拍照点组夹具见 FIXTURES）：每台计划相机在示教采集轮次里选一帧（seq）并给刷选笔划（原图像素）。
 * 走廊按直线段几何，所有帧共用一条；像素当量与检测参数只为让流程跑通而选定，不代表算法调优结果或现场标定。
 *   - synthetic：合成图，胶条沿缺省位置，当量用模拟当量（mmPerPx = null 由后端取默认）。
 *   - gs-20261017-normal：2026-10-17 停稳拍样本集，每台相机 12 帧（第 n 帧是第 n 个点位）；一条走廊对不上其他点位，
 *     多数帧测成了但沿走廊找不到胶（有胶站占比 0），检测项开着会判 NG_GAP，只作冒烟（建站时检测项全关）。笔划取旧夹具里各相机胶条清晰的点位。
 */
export const CAMERA_FIXTURES = {
  synthetic: {
    mmPerPx: null,
    // 合成图按走廊逐像素画胶条，每帧开销随走廊折点数增长；折点间距取允许的最大值 80 px（4–80）；太密会让 20–25 Hz 的三路帧晚于 partEnd 才画完
    spacingPx: 80,
    pick: (_camera, index) => ({ seq: 2, guide: syntheticBead(index) }),
  },
  "gs-20261017-normal": {
    mmPerPx: 0.112,
    pick: camera => {
      const groups = FIXTURES["gs-20261017-normal"].groups;
      return {
        cam1: { seq: 3, guide: groups.S003.guide },
        cam2: { seq: 1, guide: groups.S001.guide },
        cam3: { seq: 4, guide: groups.S004.guide, detect: FRAME_NARROW },
      }[camera];
    },
  },
};

/** 把各段的检测项全部关掉：停稳拍数据对不上直线走廊，开着只会判 NG_GAP，冒烟与压测只看流程与吞吐、不看判胶。 */
export function checksOff(segments) {
  return segments.map(segment => ({
    ...segment,
    checks: {
      width: { ...segment.checks.width, enabled: false },
      break: { ...segment.checks.break, enabled: false },
      overflow: { ...segment.checks.overflow, enabled: false },
      offset: segment.checks.offset && { ...segment.checks.offset, enabled: false },
    },
  }));
}

/** 段配方候选的计划部分；旧配方没有，调用方传错了配方类型时在这里就报错。 */
function planOf(view) {
  assert(view.plan, "该候选不是段配方（WorkspaceView.plan 为空）");
  return view.plan;
}

/**
 * 每台计划相机一条走廊：先在示教采集轮次里冻结这台相机的一帧，再按刷选笔划提取走廊，最后连同当量与检测参数保存。
 * 保存示教用 workspace_teach_camera（冻结帧 + 写入 inspect），提取用 workspace_extract_corridor（走廊示教，不复用旧中线提取）。
 */
export async function teachCorridors(ws, round, fixture) {
  const cameras = planOf(await ws.get()).cameras;
  const taught = [];
  for (const [index, camera] of cameras.entries()) {
    const choice = fixture.pick(camera, index);
    assert(choice, `夹具没有覆盖相机 ${camera}`);
    const draft = (corridor, detect) => ({ corridor, mmPerPx: fixture.mmPerPx ?? null, detect: detect ?? null });
    const frozen = planOf(await ws.edit("workspace_teach_camera", { camera, round: round.roundId, seq: choice.seq, draft: draft([], choice.detect) }));
    const frame = frozen.cameraTeaching.find(t => t.camera === camera)?.frame;
    assert(frame, `相机 ${camera} 没有冻结示教帧`);
    const extracted = await ws.edit("workspace_extract_corridor", { camera, imageId: frame.id, roi: strokeRoi(choice.guide, frame.size), guide: { points: choice.guide, radiusPx: 30 }, pointSpacingPx: fixture.spacingPx ?? null });
    assert(extracted.path.length >= 2, `相机 ${camera} 走廊提取失败：${extracted.message}`);
    const saved = planOf(await ws.edit("workspace_teach_camera", { camera, round: round.roundId, seq: choice.seq, draft: draft(extracted.path, choice.detect) }));
    const view = saved.cameraTeaching.find(t => t.camera === camera);
    assert(view.saved && view.mmPerPx > 0, `相机 ${camera} 走廊未保存或缺像素当量：${JSON.stringify(view)}`);
    taught.push({ camera, seq: choice.seq, points: extracted.path.length, mmPerPx: view.mmPerPx });
  }
  return taught;
}

/**
 * 多轮标定：采集 rounds 轮（合成图来源每轮帧数有首尾 ±1、弯道 ±2 的波动，所以能看到 min / max 差异）加入标定，
 * 另采 standard 轮标准件（只给 x_ref），应用标定并确认建议容差。返回各轮帧组数、整件标定结果与确认的容差。
 * 标定是整件帧组数的均值四舍五入；N_exp 与 ΣN_exp 不能手填，容差建议由 min / max 给出、操作员确认后才写入。
 */
export async function calibrate(call, ws, id, cameraIds, { rounds = CALIBRATION_ROUNDS, standard = STANDARD_ROUNDS, partId = null, log = () => {} } = {}) {
  const added = [];
  for (let i = 0; i < rounds + standard; i++) {
    const isStandard = i >= rounds;
    const round = await captureRound(call, id, cameraIds, partId);
    await ws.edit("workspace_calibration_add_round", { round: round.roundId, standard: isStandard });
    added.push({ roundId: round.roundId, standard: isStandard, groupCount: round.groupCount, cameraFrames: round.cameraFrames, incompleteGroups: round.incompleteGroups.length });
    log(`标定轮 ${i + 1}/${rounds + standard}${isStandard ? "（标准件）" : ""}：${round.groupCount} 个帧组，各相机 ${round.cameraFrames.join("/")} 帧`);
  }
  const applied = planOf(await ws.edit("workspace_calibration_apply", {}));
  assert(applied.calibrated && applied.expectedTotal > 0, `应用标定后没有 ΣN_exp：${JSON.stringify(applied.calibration)}`);
  const confirmed = [];
  for (const segment of applied.calibration.segments) {
    if (!segment.outcome) continue;
    const suggested = segment.outcome.suggestedTolerance;
    await ws.edit("workspace_confirm_tolerance", { segment: segment.index, tolerance: suggested });
    confirmed.push({ segment: segment.index, nExp: segment.outcome.nExp, calib: segment.outcome.calib, suggestedTolerance: suggested, xRef: segment.xRef });
  }
  const final = planOf(await ws.get());
  return {
    rounds: added, expectedTotal: final.expectedTotal, toleranceTotal: final.toleranceTotal, partCalib: final.partCalib,
    segments: confirmed, planVersionChange: final.calibration.planVersionChange, calibration: final.calibration,
  };
}

/** 独立正常样本验证：新采一轮（不能与示教或标定轮次相同）进样本库，用候选走廊逐帧重测后按未分段判定。 */
export async function validateCandidate(call, ws, id, cameraIds, { partId = null } = {}) {
  const round = await captureRound(call, id, cameraIds, partId);
  await ws.edit("workspace_capture_sample", { roundId: round.roundId, expected: "OK", correspondenceConfirmed: true });
  const validation = (await ws.edit("workspace_validate", { samples: [{ historyId: null, sampleId: round.roundId, expected: "OK" }], sourceConfirmed: true })).workspace.validation;
  return { round, validation };
}

/** 发布候选并等它生效。 */
export async function publishCandidate(call, ws, id) {
  await ws.edit("workspace_publish", {});
  await until(async () => (await call("cycle_recipes")).find(r => r.id === id), Boolean, "发布后生产配方未生效", 30000);
  return until(() => ws.get(), v => !v.workspace.pending && v.workspace.publish.state === "active", "待生效版本未提交", 30000);
}

/**
 * 新建段配方并走到发布：模板 → 示教采集 → 三条走廊 →（patchSegments：可选，改各段检测项）→ 10 轮标定 + 标准件 → 验证 → 发布；采集用计划相机（模板取相机组前三台）。
 * 返回 { id, ws, taught, calibration, validation, active }；验证没通过时 active 为 null 且不发布（调用方决定是否算失败）。
 */
export async function buildBurstRecipe(call, { sourceId, partId = null, rounds = CALIBRATION_ROUNDS, standard = STANDARD_ROUNDS, doc = null, patchSegments = null, log = () => {} }) {
  const fixture = CAMERA_FIXTURES[sourceId];
  assert(fixture, `没有段配方夹具 ${sourceId}，已有：${Object.keys(CAMERA_FIXTURES).join("、")}`);
  const template = doc ?? await call("recipe_template");
  assert(template.segments?.length > 0 && template.shots?.length === 0, "recipe_template 应给段配方");
  const created = await call("workspace_create", { doc: template });
  const id = created.workspace.doc.id;
  assert(created.plan, "新建的候选应带 plan（段配方）");
  const cameraIds = created.plan.cameras;
  assert.equal(cameraIds.length, 3, `段配方需要三台计划相机：${cameraIds}`);
  const ws = workspaceCalls(call, id);
  const teachRound = await captureRound(call, id, cameraIds, partId);
  log(`示教采集 ${teachRound.roundId}：${teachRound.groupCount} 个帧组，各相机 ${teachRound.cameraFrames.join("/")} 帧`);
  const taught = await teachCorridors(ws, teachRound, fixture);
  log(`走廊示教完成：${taught.map(t => `${t.camera}:${t.points}点`).join("、")}`);
  if (patchSegments) {
    // 改检测项限值不影响已有标定（类型序列不变）；用于冒烟与压测，把对不上走廊的数据放行
    await ws.edit("workspace_update_segments", { segments: patchSegments(planOf(await ws.get()).segments) });
  }
  const calibration = await calibrate(call, ws, id, cameraIds, { rounds, standard, partId, log });
  log(`标定完成：ΣN_exp=${calibration.expectedTotal} ± ${calibration.toleranceTotal}`);
  const { round: sampleRound, validation } = await validateCandidate(call, ws, id, cameraIds, { partId });
  log(`验证${validation.passed ? "通过" : "未通过"}：${validation.checks.map(c => `${c.name}${c.passed ? "√" : "×"}`).join(" ")}`);
  const active = validation.passed ? await publishCandidate(call, ws, id) : null;
  return { id, ws, teachRound, taught, calibration, sampleRound, validation, active };
}

/** 段配方记录的事实：历史详情 burst 里的帧组数、各相机帧数、整件标记与区域结论（脚本断言与写进报告共用）。 */
export function burstFacts(detail) {
  const burst = detail.burst;
  assert(burst, `历史记录 ${detail.summary.id} 不是段配方记录（burst 为空）`);
  const j = burst.judgement;
  return {
    verdict: detail.summary.verdict, plcCode: detail.summary.plcCode, faultCode: detail.summary.faultCode, reason: detail.summary.reason,
    expectedTotal: burst.expectedTotal, toleranceTotal: burst.toleranceTotal, deviceTriggers: burst.deviceTriggers,
    groupCount: j.groupCount, countStatus: j.countStatus, cameraFrames: burst.cameraFrames, flags: j.flags,
    regions: j.regions.map(r => ({ kind: r.region.kind.type, verdict: r.verdict, frameCount: r.frameCount, expected: r.expected, countStatus: r.countStatus, offsetUnjudged: r.offsetUnjudged })),
    incompleteGroups: [...new Set(j.regions.flatMap(r => r.incompleteGroups))].sort((a, b) => a - b),
    alarms: j.alarms.length, uninspected: j.uninspected.length, unchecked: j.unchecked,
    frameStates: burst.frames.reduce((counts, f) => ({ ...counts, [f.status]: (counts[f.status] ?? 0) + 1 }), {}),
    /** 缺帧（相机没出这一帧）：[计划相机下标, seq] */
    missing: burst.frames.filter(f => f.status === "missing").map(f => [f.camera, f.seq]),
    framesRecorded: burst.frames.length, rawFiles: burst.frames.filter(f => f.rawFile).length,
    /** 测成且找到胶（有胶站占比 > 0）的帧数：停稳拍数据对不上走廊时偏低 */
    framesWithGlue: burst.frames.filter(f => f.status === "measured" && f.metrics?.presentRatio > 0).length,
  };
}

/** 离线跑一件段配方（工况 scenario）：等出记录、握手结束、录制落定。notInspected 工况 PC 没有记录，不能用本函数。 */
export async function produceBurst(call, recipeId, { scenario = "normal", partId = null, timeout = 90000 } = {}) {
  const query = { recipeId, scope: "offline", limit: 1 };
  const previous = await call("history_query", { query });
  await call("offline_run", { recipeId, scenario, continuous: false, partId });
  const summary = await until(async () => (await call("history_query", { query })).items.find(i => i.id !== previous.items[0]?.id),
    Boolean, `工况 ${scenario} 未生成检测记录`, timeout);
  const site = await until(() => call("site_status"), s => !s.running, `工况 ${scenario} 握手未结束`, 30000);
  const detail = await until(() => call("history_detail", { id: summary.id }), d => d.recording.state !== "pending",
    `工况 ${scenario} 的原图录制未落定（本件结论 ${summary.verdict}：${summary.reason}）`, 30000);
  return { summary: detail.summary, detail, facts: burstFacts(detail), message: site.message };
}

/** 读取隔离实例清单，返回主窗口所在的 CDP 地址（显式 --cdp 优先）。 */
export async function instanceCdp(instancePath, override, acceptsIdentifier) {
  const manifest = JSON.parse(await readFile(instancePath, "utf8"));
  assert(acceptsIdentifier(manifest.identifier), `实例 ${manifest.identifier} 不是允许的隔离测试实例`);
  const cdp = override ?? manifest.cdp;
  assert(cdp, `实例清单缺少 cdp：${instancePath}，请用 scripts/Start-UiTestInstance.ps1 重新启动实例`);
  const url = new URL(cdp);
  assert(["127.0.0.1", "localhost", "[::1]"].includes(url.hostname), "仅允许本机 CDP");
  assert.equal(Number(url.port), manifest.debugPort);
  return cdp;
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
