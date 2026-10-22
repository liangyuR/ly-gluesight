import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { readFile, mkdir, realpath, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { buildBurstRecipe, burstFacts, client, instanceCdp, uiPage } from "./lib.mjs";

// 生产落盘流程有两段，都在离线·合成图下经 UI 与 S7 握手跑一件并核对落盘：
//   旧拍照点配方（首启样例配方 MTR-HSG-A，segments 为空，只停用不删除，行为不变）：定位失败工况、连续定位失败告警、按拍照点读回；
//   段配方（分段连拍，合成图三相机）：真实 lyFlow 建站与标定后投产，核对 part_frames 每相机每帧、按帧命名的原图与 part.json。
// 段配方一段需要 lyFlow 运行库（走廊测量、标定与验证的每帧测量）；没加载时跳过并写明原因。

const identifier = "com.xyzrobotics.tujiaovision.flow-tests";
const recipeId = "MTR-HSG-A";

async function until(page, read, accepts, message, timeout = 30000) {
  const deadline = Date.now() + timeout;
  let value;
  while (Date.now() < deadline) {
    value = await read();
    if (accepts(value)) return value;
    await page.waitForTimeout(100);
  }
  throw new Error(`${message}: ${JSON.stringify(value)}`);
}

async function verifiedPath(root, target) {
  const actual = await realpath(target);
  const relative = path.relative(root, actual);
  assert(relative && !relative.startsWith("..") && !path.isAbsolute(relative), `路径超出隔离目录：${actual}`);
  return actual;
}

async function verifyLegacyDisk(root, detail, recipe) {
  const { DatabaseSync } = await import("node:sqlite");
  const dataRoot = await realpath(path.dirname(root));
  const database = await verifiedPath(dataRoot, path.join(dataRoot, "inspection.db"));
  const db = new DatabaseSync(database, { readOnly: true });
  let part, shots, points;
  try {
    part = db.prepare("SELECT * FROM parts WHERE cycle_id=?").get(detail.summary.cycleId);
    assert(part, "独立 SQLite 连接未找到本件");
    assert.equal(part.id, detail.summary.id);
    assert.equal(part.recipe_revision, recipe.revisionId);
    assert.equal(part.verdict, detail.summary.verdict);
    assert.equal(part.frames_received, recipe.shots.length);
    assert.equal(part.frames_expected, recipe.shots.length);
    assert.equal(part.triggers, recipe.shots.length);
    assert.equal(part.source, "offlineSynthetic");
    assert.equal(part.recording_state, "complete");
    assert.equal(part.recording_available, 1);
    assert.deepEqual(JSON.parse(part.recording_errors), []);
    shots = db.prepare("SELECT * FROM part_shots WHERE part_id=? ORDER BY k").all(part.id);
    assert.equal(shots.length, recipe.shots.length);
    points = db.prepare("SELECT format,data FROM part_points WHERE part_id=?").get(part.id);
    assert.equal(points.format, 3);
    assert.equal(points.data.byteLength, recipe.points.k.length * 9);
    const bytes = Buffer.from(points.data);
    for (let j = 0; j < recipe.points.k.length; j++) {
      assert.equal(bytes[j * 9 + 8], 0, `点 ${j} 未测量`);
      assert(Number.isFinite(bytes.readFloatLE(j * 9)));
      assert(Number.isFinite(bytes.readFloatLE(j * 9 + 4)));
    }
    const snapshot = db.prepare("SELECT recipe_id,json FROM recipe_snapshots WHERE revision_id=?").get(recipe.revisionId);
    assert.equal(snapshot.recipe_id, recipeId);
    assert.equal(JSON.parse(snapshot.json).revisionId, recipe.revisionId);
  } finally { db.close(); }
  const directory = await verifiedPath(root, part.recording_directory);
  const metadataPath = await verifiedPath(directory, path.join(directory, "part.json"));
  const metadata = JSON.parse(await readFile(metadataPath, "utf8"));
  assert.equal(metadata.cycleId, detail.summary.cycleId);
  assert.equal(metadata.recipeRevision, recipe.revisionId);
  assert.equal(metadata.bundleId, part.bundle_id);
  assert.equal(metadata.sn, detail.summary.sn);
  assert.equal(metadata.available, true);
  assert.deepEqual(metadata.errors, []);
  assert.deepEqual(metadata.missingShots, []);
  assert.deepEqual(metadata.plannedShots, recipe.shots.map((shot, k) => ({
    k, shotId: shot.id, camera: shot.camera, selectedView: shot.view,
  })));
  const files = [];
  const matchedFrames = new Set();
  for (const [k, shot] of shots.entries()) {
    assert.equal(shot.k, k);
    assert.equal(shot.shot_id, recipe.shots[k].id);
    assert.equal(shot.camera, recipe.shots[k].camera);
    assert.equal(shot.view, recipe.shots[k].view);
    assert.equal(shot.status, "done");
    assert.equal(Number(shot.ordinal), k + 1);
    assert(shot.session !== null && shot.frame_counter !== null && shot.trigger_counter !== null);
    const refs = JSON.parse(shot.raw_files);
    assert(refs.some(raw => raw.view === shot.view), `k${k + 1} 缺少检测视角原图`);
    for (const raw of refs) {
      const file = await verifiedPath(directory, path.resolve(root, raw.file));
      const relative = path.relative(directory, file).replaceAll("\\", "/");
      const matches = metadata.frames.filter(frame => frame.k === k && frame.view === raw.view && frame.file === relative);
      assert.equal(matches.length, 1, `本件元数据无法唯一关联 k${k + 1}/view${raw.view}/${relative}`);
      const frame = matches[0];
      assert(!matchedFrames.has(frame), `原图重复关联：${relative}`);
      matchedFrames.add(frame);
      assert.equal(await verifiedPath(directory, path.resolve(directory, frame.file)), file);
      assert.equal(frame.cycleId, detail.summary.cycleId);
      assert.equal(frame.recipeRevision, recipe.revisionId);
      assert.equal(frame.bundleId, part.bundle_id);
      assert.equal(frame.shotId, shot.shot_id);
      assert.equal(frame.camera, shot.camera);
      assert.equal(frame.selectedView, shot.view);
      for (const [metadataField, sqliteField] of [["session", "session"], ["ordinal", "ordinal"], ["frameCounter", "frame_counter"], ["triggerCounter", "trigger_counter"]]) {
        assert(Number.isSafeInteger(frame[metadataField]) && frame[metadataField] >= 0, `元数据 ${metadataField} 无效`);
        assert.equal(String(frame[metadataField]), shot[sqliteField], `本件 ${metadataField} 与 SQLite 不一致`);
      }
      assert.equal(frame.counter, "synthetic");
      assert.equal(frame.manual, false);
      assert.equal(frame.lostPackets, 0);
      assert.equal(frame.queued, true);
      assert.equal(frame.available, true);
      assert.equal(frame.error, null);
      assert.equal(frame.width, raw.width);
      assert.equal(frame.height, raw.height);
      const bytes = await readFile(file);
      const header = /^P5\n(\d+) (\d+)\n255\n/.exec(bytes.subarray(0, 100).toString("ascii"));
      assert(header, `原图不是有效 PGM：${file}`);
      const width = Number(header[1]), height = Number(header[2]);
      assert.equal(width, raw.width);
      assert.equal(height, raw.height);
      assert.equal(bytes.length, Buffer.byteLength(header[0]) + width * height);
      files.push({ k, view: raw.view, file, width, height, bytes: bytes.length,
        session: frame.session, ordinal: frame.ordinal, frameCounter: frame.frameCounter, triggerCounter: frame.triggerCounter });
    }
  }
  assert.equal(metadata.frames.length, matchedFrames.size, "本件元数据含有未关联的原图");
  return { database, independentReadOnlyConnection: true, measurementPoints: recipe.points.k.length, directory, files, metadata: metadataPath };
}

/** 离线模式下按某个工况跑一件，等到出记录、握手结束；id 缺省为首启样例配方（旧拍照点配方）。 */
async function runOfflinePart(page, invokeRead, navigation, option, id = recipeId) {
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.locator('select[aria-label="离线配方"]').selectOption(id);
  await page.getByRole("combobox", { name: "离线工况", exact: true }).click();
  await page.getByRole("option", { name: option, exact: true }).click();
  const previous = await invokeRead("history_query", { query: { recipeId: id, scope: "offline", limit: 1 } });
  await page.getByRole("button", { name: "运行一件", exact: true }).click();
  const summary = await until(page, async () => (await invokeRead("history_query", { query: { recipeId: id, scope: "offline", limit: 1 } })).items.find(item => item.id !== previous.items[0]?.id),
    Boolean, `工况“${option}”未生成新的工件记录`, 60000);
  await until(page, () => invokeRead("site_status"), status => !status.running, `工况“${option}”握手未结束`);
  return summary;
}

/**
 * 连续定位失败告警（定稿 §3、§14）：前面两个定位失败工况已让同一拍照点同一幅图连续 2 件没有位置基准（中间不能插正常件），
 * 第 3 件告警——检测页出现告警、应用日志有“连续定位失败”；再跑一件正常件，告警消失、日志有“定位恢复”。
 */
async function runLocateAlarm(page, invokeRead, navigation, locateFail) {
  const appLog = (cycleId, ev) => until(page, async () => (await invokeRead("applog_query", { query: { cycleId, limit: 200 } })).items.find(item => item.ev === ev),
    Boolean, `应用日志里没有本件（${cycleId}）的“${ev}”`, 15000);
  const failed = await runOfflinePart(page, invokeRead, navigation, "定位失败（胶正常）");
  assert.equal(failed.faultCode, 92, failed.reason);
  await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
  const banner = page.getByLabel("连续定位失败告警", { exact: true });
  await banner.waitFor();
  const raised = (await invokeRead("cycle_snapshot")).locateAlarms;
  assert.equal(raised.length, 1, `应恰有一条连续定位失败告警：${JSON.stringify(raised)}`);
  assert.deepEqual([raised[0].k, raised[0].consecutive, raised[0].cycleId], [locateFail[0].frame, 3, failed.cycleId]);
  await appLog(failed.cycleId, "连续定位失败");
  const recovered = await runOfflinePart(page, invokeRead, navigation, "正常件");
  assert(["OK", "OK_WITH_EXCURSION"].includes(recovered.verdict), recovered.reason);
  await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
  await page.locator(".fly-verdict").getByText(new RegExp(`判定结果 · SN ${recovered.sn}`)).waitFor();
  await banner.waitFor({ state: "detached" });
  assert.deepEqual((await invokeRead("cycle_snapshot")).locateAlarms, []);
  await appLog(recovered.cycleId, "定位恢复");
  return { raisedBy: failed.cycleId, clearedBy: recovered.cycleId, alarm: raised[0] };
}

/** 定位失败工况：胶正常 → ERR 90 / 92；同一帧另有断胶 → NG 优先（断胶）。两种都要在判定卡和该帧缩略图上挂“定位失败”徽标。 */
async function runLocateFailScenarios(page, invokeRead, navigation) {
  const outcomes = [];
  for (const { option, verdict, plcCode, faultCode } of [
    { option: "定位失败（胶正常）", verdict: "ERR_INSPECT", plcCode: 90, faultCode: 92 },
    { option: "定位失败 + 断胶", verdict: "NG_GAP", plcCode: 13, faultCode: 0 },
  ]) {
    const summary = await runOfflinePart(page, invokeRead, navigation, option);
    assert.equal(summary.verdict, verdict, `工况“${option}”：${summary.reason}`);
    if (plcCode != null) assert.equal(summary.plcCode, plcCode);
    assert.equal(summary.faultCode, faultCode, `工况“${option}”：${summary.reason}`);
    const detail = await invokeRead("history_detail", { id: summary.id });
    const failed = detail.frames.flatMap((frame, k) => frame.referenceFailed.length ? [k] : []);
    assert.equal(failed.length, 1, `工况“${option}”应恰有一帧没有位置基准：${JSON.stringify(failed)}`);
    assert(detail.frames.every(frame => frame.status === "done"), "定位失败不再是帧状态，帧应照常测完");
    assert(detail.points.st.some(state => state === 4), "定位失败帧的有胶站应为无基准（4）");
    await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
    const card = page.locator(".fly-verdict");
    await card.getByText(new RegExp(`判定结果 · SN ${summary.sn}`)).waitFor();
    await card.getByLabel("位置基准：定位失败", { exact: true }).waitFor();
    // 只看定位失败那一帧的缩略图状态，不能被判定卡上同字的徽标顶替
    const thumbnailState = verdict === "NG_GAP" ? /^NG · 定位失败$/ : /^定位失败$/;
    await page.locator(`.qs-camera-thumbnail[data-frame-id="k${failed[0] + 1}"] .qs-frame-state`).filter({ hasText: thumbnailState }).waitFor();
    outcomes.push({ option, id: summary.id, sn: summary.sn, verdict: summary.verdict, plcCode: summary.plcCode, faultCode: summary.faultCode, reason: summary.reason, frame: failed[0] });
  }
  return outcomes;
}


// ───────────── 段配方（分段连拍）─────────────

/** 段配方一件的落盘核对：parts 行的段配方列、part_frames 每相机每帧一行、没有旧的逐拍照点表、录制目录里按帧命名的原图与 part.json。 */
async function verifyBurstDisk(root, detail, facts) {
  const { DatabaseSync } = await import("node:sqlite");
  const dataRoot = await realpath(path.dirname(root));
  const database = await verifiedPath(dataRoot, path.join(dataRoot, "inspection.db"));
  const db = new DatabaseSync(database, { readOnly: true });
  const summary = detail.summary;
  const cameras = detail.burst.cameras.length;
  let part, frames;
  try {
    part = db.prepare("SELECT * FROM parts WHERE cycle_id=?").get(summary.cycleId);
    assert(part, "独立 SQLite 连接未找到本件");
    assert.equal(part.id, summary.id);
    assert.equal(part.verdict, summary.verdict);
    assert.equal(part.source, "offlineSynthetic");
    assert.equal(part.frames_received, facts.groupCount, "收到帧 = 帧组数");
    assert.equal(part.frames_expected, facts.expectedTotal, "计划帧 = ΣN_exp");
    assert.equal(part.expected_total, facts.expectedTotal);
    assert.equal(part.tolerance_total, facts.toleranceTotal);
    assert.equal(part.device_triggers, facts.deviceTriggers);
    assert.deepEqual(JSON.parse(part.camera_frames), facts.cameraFrames);
    assert.deepEqual(JSON.parse(part.flags), facts.flags);
    assert.equal(part.recording_state, "complete");
    assert.equal(part.recording_available, 1);
    assert.deepEqual(JSON.parse(part.recording_errors), []);
    assert.equal(JSON.parse(part.burst_judgement).regions.length, facts.regions.length);
    frames = db.prepare("SELECT * FROM part_frames WHERE part_id=? ORDER BY camera, seq").all(part.id);
    assert.equal(frames.length, facts.groupCount * cameras, "每台相机每个帧组序号都应有一行（缺帧也有）");
    assert.equal(db.prepare("SELECT COUNT(*) AS n FROM part_shots WHERE part_id=?").get(part.id).n, 0, "段配方不写旧的逐拍照点表");
    assert.equal(db.prepare("SELECT COUNT(*) AS n FROM part_points WHERE part_id=?").get(part.id).n, 0, "段配方不写旧的逐站测点表");
    const snapshot = db.prepare("SELECT recipe_id,json FROM recipe_snapshots WHERE revision_id=?").get(part.recipe_revision);
    assert.equal(snapshot.recipe_id, summary.recipeId);
    assert(JSON.parse(snapshot.json).pathSegments.length > 0, "配方快照应是段配方（编译后的配方带 pathSegments）");
  } finally { db.close(); }
  const directory = await verifiedPath(root, part.recording_directory);
  const metadata = JSON.parse(await readFile(await verifiedPath(directory, path.join(directory, "part.json")), "utf8"));
  assert.equal(metadata.cycleId, summary.cycleId);
  assert.equal(metadata.groupCount, facts.groupCount);
  assert.equal(metadata.available, true);
  assert.deepEqual(metadata.errors, []);
  assert.deepEqual(metadata.missingFrames, facts.missing.map(([camera, seq]) => ({ camera, seq })), "part.json 的缺帧应与记录一致");
  assert.deepEqual(metadata.rawDropped, [], "录制队未满，不应有丢弃的原图");
  assert.equal(metadata.frames.length, frames.filter(f => f.raw_file).length);
  const files = [];
  for (const row of frames) {
    assert.equal(row.status, "measured", `(${row.camera}, ${row.seq}) 应已测`);
    assert(row.frame_counter !== null && row.trigger_counter !== null, "每帧应记 SDK 帧计数与触发计数");
    assert(row.raw_file, `(${row.camera}, ${row.seq}) 缺少原图引用`);
    const cameraId = detail.burst.cameras[row.camera];
    const name = `${cameraId}_s${String(row.seq).padStart(6, "0")}.pgm`;
    assert.equal(path.basename(row.raw_file), name, "原图文件名应为 {相机}_s{seq:06}.pgm");
    const file = await verifiedPath(directory, path.resolve(root, row.raw_file));
    const meta = metadata.frames.find(f => f.camera === row.camera && f.seq === row.seq);
    assert(meta, `part.json 缺少 (${row.camera}, ${row.seq})`);
    assert.equal(meta.cycleId, summary.cycleId);
    assert.equal(meta.file, name);
    assert.equal(String(meta.frameCounter), row.frame_counter, "part.json 与 SQLite 的帧计数一致");
    assert.equal(String(meta.triggerCounter), row.trigger_counter);
    assert.equal(meta.counter, "synthetic");
    assert.equal(meta.available, true);
    assert.equal(meta.error, null);
    const bytes = await readFile(file);
    const header = /^P5\n(\d+) (\d+)\n255\n/.exec(bytes.subarray(0, 100).toString("ascii"));
    assert(header, `原图不是有效 PGM：${file}`);
    assert.equal(bytes.length, Buffer.byteLength(header[0]) + Number(header[1]) * Number(header[2]));
    assert.equal(meta.width, Number(header[1]));
    assert.equal(meta.height, Number(header[2]));
    files.push({ camera: row.camera, seq: row.seq, file, width: meta.width, height: meta.height, bytes: bytes.length });
  }
  return { database, independentReadOnlyConnection: true, frames: frames.length, directory, files };
}

/** 清掉上一次流程测试留下的配方与候选（flow 实例保留资料，重跑要能重建同名配方）。 */
async function clearBurstRecipe(invokeRead, id) {
  if ((await invokeRead("cycle_recipes")).some(r => r.id === id)) await invokeRead("recipe_delete", { id });
  if ((await invokeRead("workspace_list")).some(w => w.doc.id === id)) await invokeRead("workspace_delete", { id });
}

/**
 * 段配方一件的生产落盘（合成图三相机）：建站、10 轮标定、验证、发布用真实 lyFlow 测量合成图（走廊示教、标定、验证的每帧测量必须有引擎），
 * 投产在离线·合成图下用模拟测量。没加载 lyFlow 时整段跳过并写明原因，不算通过也不算失败。
 */
async function runBurstStage(page, invokeRead, navigation, ctx) {
  const burstId = "FLOW-BURST";
  if (!ctx.engine?.ready) {
    return { skipped: "lyFlow 未加载：段配方建站（走廊提取后的标定与验证每帧测量）需要 lyFlow 运行库；用 pnpm test:ui:production -LyFlowRuntime <运行库目录>（或环境变量 GLUESIGHT_LYFLOW_RUNTIME）重跑", engine: ctx.engine ?? null };
  }
  const call = client(page);
  await clearBurstRecipe(invokeRead, burstId);
  const template = await invokeRead("recipe_template");
  const log = message => process.stderr.write(`· 段配方：${message}\n`);
  const built = await buildBurstRecipe(call, { sourceId: "synthetic", doc: { ...template, id: burstId, name: "流程测试 · 段配方" }, log });
  assert(built.validation.passed, `验证未通过：${JSON.stringify(built.validation.checks.filter(c => !c.passed))}`);
  assert.equal(built.active.workspace.publish.state, "active");
  const expected = { expectedTotal: built.calibration.expectedTotal, toleranceTotal: built.calibration.toleranceTotal };
  // 合成图逐像素渲染赶不上 20 Hz 的三路脉冲，末尾的帧会晚于 partEnd；脚本只验证落盘，用 5 Hz 让每件帧数完整
  await invokeRead("offline_pace", { pace: { shotIntervalMs: 200, pauseMs: 200 } });
  const summary = await runOfflinePart(page, invokeRead, navigation, "正常件", burstId);
  assert(["OK", "OK_WITH_EXCURSION"].includes(summary.verdict), JSON.stringify(summary));
  assert.equal(summary.framesExpected, expected.expectedTotal, "计划帧应为 ΣN_exp");
  assert(summary.flags?.unsegmented, "本轮没有分段算法：整条胶路按未分段处理");
  await until(page, () => invokeRead("site_status"), status => !status.running && status.message.includes("完成"), "离线产线单件 S7 握手未完成");
  const detail = await until(page, () => invokeRead("history_detail", { id: summary.id }), d => d.recording.state === "complete", "原图录制未完成");
  const facts = burstFacts(detail);
  assert.equal(facts.groupCount, summary.framesReceived);
  assert(Math.abs(facts.groupCount - expected.expectedTotal) <= expected.toleranceTotal, `帧组数 ${facts.groupCount} 应在 ${expected.expectedTotal} ± ${expected.toleranceTotal} 内`);
  assert.deepEqual(facts.missing, [], "5 Hz 下不应缺帧");
  assert.equal(facts.framesRecorded, facts.groupCount * 3);
  const root = path.join(path.dirname((await invokeRead("records_list")).root), "records-offline");
  const disk = await verifyBurstDisk(root, detail, { ...facts, ...expected });
  await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
  await page.getByText(`SN ${summary.sn}`).first().waitFor();
  await navigation.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  await page.getByRole("combobox", { name: "历史配方", exact: true }).click();
  await page.getByRole("option", { name: burstId, exact: true }).click();
  await page.getByRole("button", { name: `查看 SN ${summary.sn} 的记录`, exact: true }).click();
  await page.getByRole("heading", { name: "帧组与原图", exact: true }).waitFor();
  await page.getByText(`cam1 ${facts.cameraFrames[0]} · cam2 ${facts.cameraFrames[1]} · cam3 ${facts.cameraFrames[2]}`, { exact: true }).first().waitFor();
  for (let camera = 0; camera < 3; camera++) {
    const seq = Math.min(2, facts.groupCount);
    const image = await page.evaluate(async ({ id, camera, seq }) => {
      const response = await window.__TAURI_INTERNALS__.invoke("workspace_record_frame_image", { historyId: id, camera, seq });
      const bytes = response instanceof ArrayBuffer ? new Uint8Array(response) : new Uint8Array(response);
      const header = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      return { bytes: bytes.length, width: header.getUint32(0, true), height: header.getUint32(4, true), sourceWidth: header.getUint32(8, true), sourceHeight: header.getUint32(12, true) };
    }, { id: summary.id, camera, seq });
    assert.equal(image.bytes, 16 + image.width * image.height);
    assert(image.sourceWidth > 0 && image.sourceHeight > 0);
  }
  return { recipeId: burstId, expected, calibration: { rounds: built.calibration.rounds.length, groupCounts: built.calibration.rounds.filter(r => !r.standard).map(r => r.groupCount) }, summary, facts, disk };
}

export async function runProductionFlow(page, options = {}) {
  const instancePath = options.instance ?? "output/playwright/flow-regression/instance.json";
  const manifest = JSON.parse(await readFile(instancePath, "utf8"));
  assert.equal(manifest.identifier, identifier, "仅允许隔离流程测试实例");
  const invokeRead = (command, args) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const guard = {
    records: await invokeRead("records_list"), cameras: await invokeRead("camera_rig_config"),
    config: await invokeRead("plc_get_config"), settings: await invokeRead("cycle_get_settings"),
    // 真机模式下报告 lyFlow 是否加载；进入离线·合成图后这个接口只说“模拟测量”
    engine: await invokeRead("engine_status").catch(() => null),
  };
  const root = path.join(await realpath(path.dirname(guard.records.root)), path.basename(guard.records.root));
  assert.equal(path.basename(path.dirname(root)), identifier, "录制目录不属于隔离实例");
  assert.equal(path.basename(root), "records");
  assert(guard.cameras.length > 0 && guard.cameras.every(camera => camera.source === "mvs"), "真机模式相机组必须全部为海康来源");
  const initialSite = await invokeRead("site_status");
  assert.equal(initialSite.mode, "real", "软件启动必须处于真机模式");
  assert.notEqual((await invokeRead("plc_get_status")).state, "connected", "外部 PLC 必须未连接，才可进入离线");
  const navigation = page.getByRole("navigation", { name: "操作导航" });
  if (guard.settings.productSource !== "plc") {
    await page.getByRole("link", { name: "系统设置", exact: true }).click();
    await page.getByRole("combobox", { name: "型号来源", exact: true }).click();
    await page.getByRole("option", { name: "PLC 下发产品代码", exact: true }).click();
    const cyclePanel = page.getByRole("region", { name: "检测节拍", exact: true });
    await cyclePanel.getByRole("button", { name: "保存", exact: true }).click();
    await until(page, () => invokeRead("cycle_get_settings"), settings => settings.productSource === "plc", "UI 保存型号来源失败");
  }
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("heading", { name: "当前：真机", exact: true }).waitFor();
  await page.getByRole("combobox", { name: "离线来源", exact: true }).click();
  await page.getByRole("option", { name: "合成图", exact: true }).click();
  await page.getByRole("button", { name: "进入离线", exact: true }).click();
  const offline = await until(page, () => invokeRead("site_status"), status => status.mode === "offline" && Number.isInteger(status.port), "UI 未进入离线模式");
  assert.equal(offline.source, "synthetic");
  assert.equal(offline.measurement, "simulated");
  await page.getByRole("heading", { name: "当前：离线 · 合成图", exact: true }).waitFor();
  await until(page, () => invokeRead("plc_get_status"), status => status.state === "connected", "离线 S7 服务未连接");
  const offlineConfig = await invokeRead("plc_get_config");
  assert.equal(offlineConfig.connection.protocol, "s7");
  assert.equal(offlineConfig.connection.host, "127.0.0.1");
  assert.equal(offlineConfig.connection.port, offline.port);
  const offlineCameras = await invokeRead("camera_rig_config");
  // 合成图沿用真机相机组的编号与视角数（旧拍照点配方照常引用），单视角相机不足 3 台时补足到 3 台（段配方要三台独立单视角相机共用触发）
  const offlineRig = offlineCameras.map(camera => [camera.id, camera.source, camera.viewCount]);
  assert.deepEqual(offlineRig.slice(0, guard.cameras.length), guard.cameras.map(camera => [camera.id, "sim", camera.viewCount]));
  assert(offlineRig.slice(guard.cameras.length).every(([, source, views]) => source === "sim" && views === 1), `补足的相机应是单视角合成图相机：${JSON.stringify(offlineRig)}`);
  assert(offlineCameras.filter(camera => camera.viewCount === 1 && !camera.compositeLayout).length >= 3, "合成图离线环境应至少有三台单视角相机");
  if ((await invokeRead("cycle_snapshot")).phase === "FAULT") {
    await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
    await page.getByRole("button", { name: "复位故障", exact: true }).click();
    await until(page, () => invokeRead("cycle_snapshot"), snapshot => snapshot.phase !== "FAULT", "复位故障后检测仍处于故障", 5000).catch(() => {});
    await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  }
  await page.locator('select[aria-label="离线配方"]').selectOption(recipeId);
  await page.getByRole("combobox", { name: "离线工况", exact: true }).click();
  await page.getByRole("option", { name: "正常件", exact: true }).click();
  const recipe = await invokeRead("cycle_layout", { recipeId });
  const previous = await invokeRead("history_query", { query: { recipeId, scope: "offline", limit: 1 } });
  const observations = [];
  const startedAt = Date.now();
  await page.getByRole("button", { name: "运行一件", exact: true }).click();
  const summary = await until(page, async () => {
    const snapshot = await invokeRead("cycle_snapshot");
    if (observations.at(-1)?.phase !== snapshot.phase || observations.at(-1)?.received !== snapshot.part?.received) {
      observations.push({ phase: snapshot.phase, received: snapshot.part?.received, filled: snapshot.part?.filled, cycleId: snapshot.part?.cycleId });
    }
    const history = await invokeRead("history_query", { query: { recipeId, scope: "offline", limit: 1 } });
    return history.items.find(item => item.id !== previous.items[0]?.id);
  }, Boolean, "未生成新的工件记录");
  assert(["OK", "OK_WITH_EXCURSION"].includes(summary.verdict), JSON.stringify(summary));
  assert.equal(summary.framesExpected, recipe.shots.length);
  assert.equal(summary.framesReceived, recipe.shots.length);
  await until(page, () => invokeRead("site_status"), status => !status.running && status.message.includes("完成"), "离线产线单件 S7 握手未完成");
  await page.getByText(new RegExp(`^SN ${summary.sn} 完成`)).waitFor();
  await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
  await page.locator(".fly-verdict").getByText(new RegExp(`判定结果 · SN ${summary.sn}`)).waitFor();
  const recorded = await until(page, () => invokeRead("workspace_record_images", { historyId: summary.id }), value => value.complete, "原图录制未完成");
  const detail = await invokeRead("history_detail", { id: summary.id });
  assert.equal(detail.recording.state, "complete");
  assert.equal(detail.triggers, recipe.shots.length);
  assert.equal(detail.points.st.length, recipe.points.k.length);
  assert(detail.points.st.every(state => state === 0));
  assert(detail.frames.every(frame => frame.status === "done"));
  const disk = await verifyLegacyDisk(path.join(path.dirname(root), "records-offline"), detail, recipe);
  await navigation.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  await page.getByRole("combobox", { name: "历史配方", exact: true }).click();
  await page.getByRole("option", { name: recipeId, exact: true }).click();
  await page.getByRole("button", { name: `查看 SN ${summary.sn} 的记录`, exact: true }).click();
  await page.getByRole("heading", { name: "逐拍照点结果与原图", exact: true }).waitFor();
  for (let k = 0; k < recipe.shots.length; k++) {
    await page.locator('select[aria-label="历史帧选择"]').selectOption(String(k));
    await page.getByText(`原始 SN ${summary.sn} · k${k + 1} · 视角 ${recipe.shots[k].view}`, { exact: true }).waitFor();
    await page.locator("svg.wp-gray-image").waitFor();
    const preview = await page.evaluate(async ({ id, k, view }) => {
      const response = await window.__TAURI_INTERNALS__.invoke("workspace_record_image", { historyId: id, k, view });
      const bytes = response instanceof ArrayBuffer ? new Uint8Array(response) : new Uint8Array(response);
      const header = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      return { bytes: bytes.length, width: header.getUint32(0, true), height: header.getUint32(4, true), sourceWidth: header.getUint32(8, true), sourceHeight: header.getUint32(12, true) };
    }, { id: summary.id, k, view: recipe.shots[k].view });
    assert.equal(preview.bytes, 16 + preview.width * preview.height);
    assert(preview.sourceWidth > 0 && preview.sourceHeight > 0);
  }
  const locateFail = await runLocateFailScenarios(page, invokeRead, navigation);
  const locateAlarm = await runLocateAlarm(page, invokeRead, navigation, locateFail);
  const burst = await runBurstStage(page, invokeRead, navigation, { engine: guard.engine });
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("button", { name: "退出离线", exact: true }).click();
  await until(page, () => invokeRead("site_status"), status => status.mode === "real", "UI 未退出离线模式");
  await page.getByRole("heading", { name: "当前：真机", exact: true }).waitFor();
  assert.deepEqual(await invokeRead("plc_get_config"), guard.config, "退出离线后 PLC 配置未恢复");
  assert.deepEqual(await invokeRead("camera_rig_config"), guard.cameras, "退出离线后相机组未恢复");
  assert.deepEqual(await invokeRead("cycle_get_settings"), (({ productSource, ...rest }) => ({ ...rest, productSource: "plc" }))(guard.settings), "退出离线后系统设置被改动");
  const result = { passed: true, operation: "UI 通过站点模式进入离线·合成图、工件启动、模拟测量、S7 握手结束、SQLite 与原图真实落盘并重开读取、退出离线恢复真机配置；先旧拍照点配方（首启样例），再段配方（需 lyFlow）", identifier, recipeId, offlinePort: offline.port, startedAt, elapsedMs: Date.now() - startedAt, legacy: { summary, observations, recorded, disk, locateFail, locateAlarm }, burst, summary, disk, boundary: "进程内 S7 服务、虚拟产线、模拟相机与模拟测量；不代表现场 PLC、SDK 或 lyFlow 引擎验收（段配方的走廊测量、标定与验证用真实 lyFlow 测量合成图，投产用模拟测量）" };
  if (options.output) {
    await mkdir(path.dirname(options.output), { recursive: true });
    await page.screenshot({ path: path.join(path.dirname(options.output), "production-flow.png"), fullPage: true });
    await writeFile(options.output, JSON.stringify(result, null, 2) + "\n");
  }
  return result;
}

async function main() {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) {
    assert(process.argv[i].startsWith("--") && process.argv[i + 1], "参数必须为 --name value");
    args[process.argv[i].slice(2)] = process.argv[i + 1];
  }
  const instance = args.instance ?? "output/playwright/flow-regression/instance.json";
  const cdp = await instanceCdp(instance, args.cdp, id => id === identifier);
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = await uiPage(browser);
    const result = await runProductionFlow(page, { instance, output: args.output ?? "output/playwright/flow-regression/result.json" });
    process.stdout.write(JSON.stringify({ passed: result.passed, legacy: { cycleId: result.summary.cycleId, historyId: result.summary.id, files: result.disk.files.length },
      burst: result.burst.skipped ? { skipped: result.burst.skipped } : { recipeId: result.burst.recipeId, historyId: result.burst.summary.id, verdict: result.burst.summary.verdict, groupCount: result.burst.facts.groupCount, files: result.burst.disk.files.length } }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
