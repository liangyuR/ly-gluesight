import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { readFile, mkdir, realpath, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { buildBurstRecipe, burstFacts, client, instanceCdp, uiPage } from "./lib.mjs";

const identifier = "com.xyzrobotics.tujiaovision.flow-tests";

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

async function runOfflinePart(page, invokeRead, navigation, option, id) {
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
  assert(ctx.engine?.ready, `lyFlow 引擎未就绪：${JSON.stringify(ctx.engine)}`);
  const call = client(page);
  await clearBurstRecipe(invokeRead, burstId);
  const template = await invokeRead("recipe_template");
  const log = message => process.stderr.write(`· 段配方：${message}\n`);
  const built = await buildBurstRecipe(call, { sourceId: "synthetic", doc: { ...template, id: burstId, name: "流程测试 · 段配方" }, log });
  assert(built.validation.passed, `验证未通过：${JSON.stringify(built.validation.checks.filter(c => !c.passed))}`);
  assert.equal(built.active.workspace.publish.state, "active");
  const expected = { expectedTotal: built.calibration.expectedTotal, toleranceTotal: built.calibration.toleranceTotal };
  // 合成图逐像素渲染赶不上 20 fps 的三路采集，末尾的帧会晚于 partEnd；脚本只验证落盘，用 5 Hz 让每件帧数完整
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
  const suite = options.suite ?? "burst";
  assert(suite === "burst", `不支持的生产落盘用例：${suite}`);
  const startedAt = Date.now();
  let result;
  let failure;
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
  const segments = root.split(path.sep);
  assert(segments.includes(identifier) && segments.at(-3) === "projects", `录制目录不属于隔离实例的项目目录：${root}`);
  assert.equal(path.basename(root), "records");
  assert(guard.cameras.length > 0 && guard.cameras.every(camera => camera.source === "mvs"), "真机模式相机组必须全部为海康来源");
  const initialSite = await invokeRead("site_status");
  assert.equal(initialSite.mode, "real", "软件启动必须处于真机模式");
  assert.notEqual((await invokeRead("plc_get_status")).state, "connected", "外部 PLC 必须未连接，才可进入离线");
  const navigation = page.getByRole("navigation", { name: "操作导航" });
  try {
    if (guard.settings.productSource !== "plc") {
      await page.getByRole("link", { name: "系统设置", exact: true }).click();
      await page.getByRole("combobox", { name: "型号来源", exact: true }).click();
      await page.getByRole("option", { name: "PLC 下发型号", exact: true }).click();
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
    const offlineRig = offlineCameras.map(camera => [camera.id, camera.source, camera.viewCount]);
    assert.deepEqual(offlineRig.slice(0, guard.cameras.length), guard.cameras.map(camera => [camera.id, "sim", camera.viewCount]));
    assert(offlineRig.slice(guard.cameras.length).every(([, source, views]) => source === "sim" && views === 1), `补足的相机应是单视角合成图相机：${JSON.stringify(offlineRig)}`);
    assert(offlineCameras.filter(camera => camera.viewCount === 1 && !camera.compositeLayout).length >= 3, "合成图离线环境应至少有三台单视角相机");
    // 离线产线会以 PLC 现场复位（plcState=99）自动复位；自动复位没完成才点软件复位
    const cleared = await until(page, () => invokeRead("cycle_snapshot"), snapshot => snapshot.phase !== "FAULT", "离线自动复位未完成", 15000).then(() => true, () => false);
    if (!cleared) {
      await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
      await page.getByRole("button", { name: "软件复位", exact: true }).click();
      await page.getByRole("dialog").getByRole("button", { name: "确认复位", exact: true }).click();
      await until(page, () => invokeRead("cycle_snapshot"), snapshot => snapshot.phase !== "FAULT", "软件复位后检测仍处于故障", 5000).catch(() => {});
      await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
    }
    const burst = await runBurstStage(page, invokeRead, navigation, { engine: guard.engine });
    await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
    await page.getByRole("button", { name: "退出离线", exact: true }).click();
    await until(page, () => invokeRead("site_status"), status => status.mode === "real", "UI 未退出离线模式");
    await page.getByRole("heading", { name: "当前：真机", exact: true }).waitFor();
    assert.deepEqual(await invokeRead("plc_get_config"), guard.config, "退出离线后 PLC 配置未恢复");
    assert.deepEqual(await invokeRead("camera_rig_config"), guard.cameras, "退出离线后相机组未恢复");
    assert.deepEqual(await invokeRead("cycle_get_settings"), (({ productSource, ...rest }) => ({ ...rest, productSource: "plc" }))(guard.settings), "退出离线后系统设置被改动");
    result = { passed: true, suite, operation: "段配方合成图采集、S7 握手与落盘", identifier, offlinePort: offline.port, startedAt, elapsedMs: Date.now() - startedAt, burst, boundary: "离线合成图验证，不代表现场 PLC 与相机验收" };
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    try {
      const status = await invokeRead("site_status");
      if (status.running) {
        await invokeRead("offline_stop", { now: true });
        await until(page, () => invokeRead("site_status"), value => !value.running, "清理时离线产线未停止");
      }
      if (status.mode === "offline") await invokeRead("site_leave_offline");
      await invokeRead("cycle_save_settings", { settings: guard.settings });
      assert.deepEqual(await invokeRead("plc_get_config"), guard.config, "清理后 PLC 配置未恢复");
      assert.deepEqual(await invokeRead("camera_rig_config"), guard.cameras, "清理后相机组未恢复");
      assert.deepEqual(await invokeRead("cycle_get_settings"), guard.settings, "清理后系统设置未恢复");
    } catch (error) {
      if (!failure) throw error;
      process.stderr.write(`生产流程清理失败：${error.stack ?? error}\n`);
    }
  }
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
    const result = await runProductionFlow(page, { instance, suite: args.suite ?? "burst", lyflow: args.lyflow ?? process.env.GLUESIGHT_LYFLOW_RUNTIME, output: args.output ?? "output/playwright/flow-regression/result.json" });
    process.stdout.write(JSON.stringify({ passed: result.passed,
      burst: { recipeId: result.burst.recipeId, historyId: result.burst.summary.id, verdict: result.burst.summary.verdict, groupCount: result.burst.facts.groupCount, files: result.burst.disk.files.length } }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
