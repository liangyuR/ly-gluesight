import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { connect, ensureOffline, identifier, parseArgs, RECIPE_ID, until, writeJson } from "./lib.mjs";
import { isolatedStorage } from "../../tests/native/storage-paths.mjs";

const OK_CLASS = new Set(["OK", "OK_WITH_EXCURSION"]);

/** 事务进行中：放慢节拍跑一件，等收到几帧后记下工件身份，交给外层脚本强制结束进程。 */
async function arm({ call }, out) {
  await ensureOffline(call, out);
  const site = await call("site_status");
  const latest = (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0] ?? null;
  await call("offline_pace", { pace: { shotIntervalMs: 1000, pauseMs: 200 } });
  await call("offline_run", { recipeId: RECIPE_ID, scenario: "normal", continuous: false, partId: null });
  const snapshot = await until(() => call("cycle_snapshot"), s => s?.phase === "ACQUIRE" && (s.part?.received ?? 0) >= 3, "未进入采集", 30000, 50);
  const state = { killedAtPhase: snapshot.phase, cycleId: snapshot.part.cycleId, sn: snapshot.part.sn, received: snapshot.part.received,
    latestHistoryId: latest?.id ?? null, sampleSetDir: site.sampleSet.dir, armedAt: Date.now() };
  await writeJson(path.join(out, "restart-armed.json"), state);
  process.stdout.write(JSON.stringify(state) + "\n");
}

/** 重启后：核对站点、在途事务、记录与原图目录，再进入离线复位并投产一件。 */
async function verify({ call, manifest }, out) {
  const armed = JSON.parse(await readFile(path.join(out, "restart-armed.json"), "utf8"));
  const site = await call("site_status");
  const snapshot = await until(() => call("cycle_snapshot"), Boolean, "重启后没有节拍快照", 30000, 200);
  const logs = await call("cycle_logs");
  const interrupted = (await call("history_query", { query: { sn: armed.sn, scope: "offline", limit: 5 } })).items;
  const before = { mode: site.mode, phase: snapshot.phase, fault: snapshot.fault, handshake: snapshot.plcHandshake,
    interruptedRecords: interrupted.map(i => ({ id: i.id, cycleId: i.cycleId, verdict: i.verdict, reason: i.reason, delivery: i.delivery.state })),
    startupLogs: logs.filter(l => ["err", "warn"].includes(l.level)).map(l => `${l.level} ${l.ev}：${l.msg}`).slice(0, 30) };
  const { vision } = await isolatedStorage(call, manifest, identifier);
  const pending = async () => (await readdir(path.join(vision, "records-offline", "_pending")).catch(() => [])).length;
  before.pendingDirs = await pending();
  try { await ensureOffline(call, out); } catch (error) {
    // 已知问题：离线事务中途崩溃后，重启回到真机模式仍保留该未结事务；真机 PLC 不在线时无法复位，又因“在途事务”进不了离线。
    const result = { armed, before, blockedByOfflinePending: error.message, passed: false, dataRoot: manifest.dataRoot };
    await writeJson(path.join(out, "restart.json"), result);
    throw new Error(`重启后无法回到离线：${error.message}`);
  }
  await call("offline_pace", { pace: { shotIntervalMs: 150, pauseMs: 200 } });
  const attempts = [];
  let record, status, detail;
  do {
    const last = (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0];
    await call("offline_run", { recipeId: RECIPE_ID, scenario: "normal", continuous: false, partId: null });
    record = await until(async () => (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0],
      r => r && r.id !== last?.id, "重启后投产未生成记录", 120000, 300);
    status = await until(() => call("site_status"), s => !s.running, "重启后离线产线未停止", 60000, 300);
    detail = await until(() => call("history_detail", { id: record.id }), d => d.recording.state !== "pending" && ["acknowledged", "failed"].includes(d.summary.delivery.state),
      "重启后投产记录的录制或交付未落定", 30000, 300);
    record = detail.summary;
    attempts.push({ verdict: record.verdict, reason: record.reason });
  } while (!OK_CLASS.has(record.verdict) && attempts.length < 3);
  const afterLogs = (await call("cycle_logs")).filter(l => ["err", "warn", "ok"].includes(l.level) && l.ts >= armed.armedAt).map(l => `${l.level} ${l.ev}：${l.msg}`).slice(-30);
  const result = {
    armed, before,
    after: { verdict: record.verdict, delivery: record.delivery.state, recording: detail.recording.state, recordingAvailable: detail.recording.available,
      message: status.message, attempts, pendingDirs: await pending(), logs: afterLogs },
    passed: before.mode === "real" && OK_CLASS.has(record.verdict) && record.delivery.state === "acknowledged" && detail.recording.available
      && before.interruptedRecords.every(r => !OK_CLASS.has(r.verdict)),
    dataRoot: manifest.dataRoot,
  };
  await writeJson(path.join(out, "restart.json"), result);
  process.stdout.write(JSON.stringify({ passed: result.passed, before: { mode: before.mode, phase: before.phase, interrupted: before.interruptedRecords }, after: result.after.verdict }) + "\n");
  if (!result.passed) process.exitCode = 1;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const out = path.resolve(args.out ?? "output/soak");
  const session = await connect(args.cdp ?? "http://[::1]:9347", args.instance ?? "output/playwright/soak-regression/instance.json");
  try {
    if (args.command === "arm") await arm(session, out);
    else if (args.command === "verify") await verify(session, out);
    else throw new Error(`未知命令 ${args.command}`);
  } finally { await session.browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
