import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { readFile, mkdir, realpath, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { instanceCdp, uiPage } from "./offline-e2e.mjs";

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

async function verifyDisk(root, detail, recipe) {
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
    assert.equal(points.format, 2);
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

export async function runProductionFlow(page, options = {}) {
  const instancePath = options.instance ?? "output/playwright/flow-regression/instance.json";
  const manifest = JSON.parse(await readFile(instancePath, "utf8"));
  assert.equal(manifest.identifier, identifier, "仅允许隔离流程测试实例");
  const invokeRead = (command, args) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const guard = {
    records: await invokeRead("records_list"), cameras: await invokeRead("camera_rig_config"),
    config: await invokeRead("plc_get_config"), settings: await invokeRead("cycle_get_settings"),
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
    await page.getByRole("combobox", { name: "型号来源", exact: true }).selectOption("plc");
    const cyclePanel = page.locator(".panel").filter({ has: page.getByRole("heading", { name: "检测节拍", exact: true }) });
    await cyclePanel.getByRole("button", { name: "保存", exact: true }).click();
    await until(page, () => invokeRead("cycle_get_settings"), settings => settings.productSource === "plc", "UI 保存型号来源失败");
  }
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("heading", { name: "当前：真机", exact: true }).waitFor();
  await page.getByRole("combobox", { name: "离线来源", exact: true }).selectOption("synthetic");
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
  assert.deepEqual(offlineCameras.map(camera => [camera.id, camera.source, camera.viewCount]), guard.cameras.map(camera => [camera.id, "sim", camera.viewCount]));
  if ((await invokeRead("cycle_snapshot")).phase === "FAULT") {
    await navigation.getByRole("link", { name: "在线检测", exact: true }).click();
    await page.getByRole("button", { name: "复位故障", exact: true }).click();
    await until(page, () => invokeRead("cycle_snapshot"), snapshot => snapshot.phase !== "FAULT", "复位故障后检测仍处于故障", 5000).catch(() => {});
    await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  }
  await page.getByRole("combobox", { name: "离线配方", exact: true }).selectOption(recipeId);
  await page.getByRole("combobox", { name: "离线工况", exact: true }).selectOption("normal");
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
  const disk = await verifyDisk(path.join(path.dirname(root), "records-offline"), detail, recipe);
  await navigation.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  await page.getByRole("combobox", { name: "历史配方", exact: true }).selectOption(recipeId);
  await page.getByRole("row", { name: `查看 SN ${summary.sn} 的记录`, exact: true }).click();
  await page.getByRole("heading", { name: "逐拍照点结果与原图", exact: true }).waitFor();
  for (let k = 0; k < recipe.shots.length; k++) {
    await page.getByRole("combobox", { name: "历史帧选择", exact: true }).selectOption(String(k));
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
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("button", { name: "退出离线", exact: true }).click();
  await until(page, () => invokeRead("site_status"), status => status.mode === "real", "UI 未退出离线模式");
  await page.getByRole("heading", { name: "当前：真机", exact: true }).waitFor();
  assert.deepEqual(await invokeRead("plc_get_config"), guard.config, "退出离线后 PLC 配置未恢复");
  assert.deepEqual(await invokeRead("camera_rig_config"), guard.cameras, "退出离线后相机组未恢复");
  assert.deepEqual(await invokeRead("cycle_get_settings"), (({ productSource, ...rest }) => ({ ...rest, productSource: "plc" }))(guard.settings), "退出离线后系统设置被改动");
  const result = { passed: true, operation: "UI 通过站点模式进入离线·合成图、工件启动、模拟测量、S7 握手结束、SQLite 与原图真实落盘并重开读取、退出离线恢复真机配置", identifier, recipeId, offlinePort: offline.port, startedAt, elapsedMs: Date.now() - startedAt, summary, observations, recorded, disk, boundary: "进程内 S7 服务、虚拟产线、模拟相机与模拟测量；不代表现场 PLC、SDK 或 lyFlow 引擎验收" };
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
    process.stdout.write(JSON.stringify({ passed: result.passed, cycleId: result.summary.cycleId, historyId: result.summary.id, files: result.disk.files.length }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
