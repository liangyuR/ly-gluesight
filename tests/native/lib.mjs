import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { cp, mkdir, readFile, realpath, rm, writeFile } from "node:fs/promises";
import path from "node:path";

export const identifier = "com.xyzrobotics.tujiaovision.e2e-tests";
export const MISSING_INPUT_EXIT = 3;
export const OK_CLASS = ["OK", "OK_WITH_EXCURSION"];

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

const GATE_BUSY = /正在处理采集或设备事务|正在处理 PLC 事务|检测进行中，工件结束后再操作相机|检测仍有在途事务|设备配置正在变化|尚未回到空闲|正在预热|PLC 事务 3 秒内仍未结束|离线 S7 服务尚未连接/;

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

/** 开始一轮整圈采集并等它完成：partId 只对样本集来源有意义，合成图来源传 null。不核对采集的触发数或帧数。 */
export async function captureRound(call, id, cameraIds, partId = null) {
  const round = await call("recipe_capture_start", { recipeId: id, cameraIds, drainTimeoutMs: 1000 });
  assert.equal(round.state, "receiving");
  await call("offline_capture", { partId });
  const done = await until(() => call("recipe_capture_get", { roundId: round.roundId }), r => ["complete", "failed"].includes(r?.state), `整圈采集 ${partId} 未结束`, 90000);
  assert.equal(done.state, "complete", `整圈采集 ${partId} 失败：${done.error}`);
  await until(() => call("site_status"), s => !s.running, "离线产线采集后未停止", 15000);
  await until(() => call("cycle_snapshot"), s => s.phase === "IDLE", "采集完成后 PLC 握手未回到空闲", 15000);
  return done;
}

function strokeRoi(points, [width, height], margin = 40) {
  const xs = points.map(p => p[0]), ys = points.map(p => p[1]);
  const x = Math.max(0, Math.floor(Math.min(...xs) - margin)), y = Math.max(0, Math.floor(Math.min(...ys) - margin));
  return [x, y, Math.min(width, Math.ceil(Math.max(...xs) + margin)) - x, Math.min(height, Math.ceil(Math.max(...ys) + margin)) - y];
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
 * 段配方的演示夹具（按相机）：每台计划相机在示教采集轮次里选一帧（seq）并给刷选笔划（原图像素）。
 * 走廊按直线段几何，所有帧共用一条；像素当量与检测参数只为让流程跑通而选定，不代表算法调优结果或现场标定。
 *   - synthetic：合成图，胶条沿缺省位置，当量用模拟当量（mmPerPx = null 由后端取默认）。
 *   - gs-20261017-normal：2026-10-17 停稳拍样本集，每台相机 12 帧（第 n 帧是第 n 个点位）；一条走廊对不上其他点位，
 *     多数帧测成了但沿走廊找不到胶（有胶站占比 0），检测项开着会判 NG_GAP，只作冒烟（建站时检测项全关）。笔划选取各相机胶条清晰的点位。
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
      return {
        cam1: { seq: 3, guide: [[40, 472], [65, 470], [90, 467], [115, 458], [140, 450], [165, 442], [190, 429], [215, 420], [240, 401], [265, 397], [290, 383], [315, 370], [340, 349], [365, 336], [390, 329], [415, 316], [440, 303], [465, 284], [490, 278], [515, 265], [540, 258]] },
        cam2: { seq: 1, guide: [[745, 60], [740, 85], [735, 110], [734, 135], [740, 160], [746, 185], [743, 210], [736, 235], [735, 260], [734, 285], [740, 310], [739, 335], [738, 360], [731, 385], [724, 410], [723, 435]] },
        cam3: { seq: 4, guide: [[715, 380], [722, 405], [723, 430], [732, 455], [738, 480], [749, 505], [746, 530], [743, 555], [739, 580], [739, 605], [738, 630], [735, 655], [733, 680]], detect: FRAME_NARROW },
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
  assert(template.segments?.length > 0, "recipe_template 应给段配方");
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
