import assert from "node:assert/strict";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { connect, ensureOffline, parseArgs, RECIPE_ID, until, writeJson } from "./lib.mjs";

const OK_CLASS = new Set(["OK", "OK_WITH_EXCURSION"]);

async function enterOffline(call, out) {
  await ensureOffline(call, out);
  await call("offline_pace", { pace: { shotIntervalMs: 150, pauseMs: 200 } });
}

/** 跑一件，等到出记录、产线停止、录制与交付落定；返回历史详情。 */
async function runPart(call, what) {
  const last = (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0];
  await call("offline_run", { recipeId: RECIPE_ID, scenario: "normal", continuous: false, partId: null });
  const record = await until(async () => (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0],
    r => r && r.id !== last?.id, `${what}未生成记录`, 120000, 300);
  const status = await until(() => call("site_status"), s => !s.running, "离线产线未停止", 60000, 300);
  const detail = await until(() => call("history_detail", { id: record.id }),
    d => ["acknowledged", "failed"].includes(d.summary.delivery.state) && d.recording.state !== "pending", "交付或录制状态未落定", 30000, 300);
  return { detail, lineMessage: status.message };
}

/** 资料盘低于下限（以开发构建的 GLUESIGHT_DISK_MIN_FREE_MB 模拟）：照常接件检测并入历史，报警写明剩余空间，新件不录原图。 */
async function lowSpace({ call }, out) {
  await enterOffline(call, out);
  const readiness = await until(() => call("cycle_readiness"), r => /剩余空间/.test(r.items.find(i => i.key === "audit")?.detail ?? ""), "资料盘不足未体现在就绪检查", 30000, 500);
  const attempts = [];
  let part;
  do {
    part = await runPart(call, "资料盘不足时投产");
    const { summary, recording } = part.detail;
    attempts.push({ verdict: summary.verdict, reason: summary.reason, delivery: summary.delivery.state, recording: recording.state, recordingAvailable: recording.available });
  } while (!OK_CLASS.has(part.detail.summary.verdict) && attempts.length < 3);
  const snapshot = await call("cycle_snapshot");
  const last = attempts.at(-1);
  const result = { audit: readiness.items.find(i => i.key === "audit"), lineMessage: part.lineMessage, alarms: snapshot.alarms, phase: snapshot.phase, attempts };
  result.passed = snapshot.phase !== "FAULT" && OK_CLASS.has(last.verdict) && last.delivery === "acknowledged"
    && snapshot.alarms.some(a => a.includes("剩余空间")) && attempts.every(a => !a.recordingAvailable && ["off", "notRetained"].includes(a.recording));
  await writeJson(path.join(out, "disk-blocked.json"), result);
  process.stdout.write(JSON.stringify(result) + "\n");
  if (!result.passed) process.exitCode = 1;
}

/** 空间恢复后（重启实例、不再调高下限）：投产合格并恢复录制原图。 */
async function recovered({ call }, out) {
  await enterOffline(call, out);
  const attempts = [];
  let detail;
  do {
    ({ detail } = await runPart(call, "空间恢复后投产"));
    attempts.push({ verdict: detail.summary.verdict, reason: detail.summary.reason, recording: detail.recording.state });
  } while (!OK_CLASS.has(detail.summary.verdict) && attempts.length < 3);
  const record = detail.summary;
  const result = { verdict: record.verdict, delivery: record.delivery.state, recording: detail.recording.state, recordingAvailable: detail.recording.available, attempts,
    passed: OK_CLASS.has(record.verdict) && record.delivery.state === "acknowledged" && detail.recording.available };
  await writeJson(path.join(out, "disk-recovered.json"), result);
  process.stdout.write(JSON.stringify(result) + "\n");
  assert(result.passed, "空间恢复后未恢复生产或录制");
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const out = path.resolve(args.out ?? "output/soak");
  const session = await connect(args.cdp ?? "http://[::1]:9347", args.instance ?? "output/playwright/soak-regression/instance.json");
  try {
    if (args.command === "blocked") await lowSpace(session, out);
    else if (args.command === "recovered") await recovered(session, out);
    else throw new Error(`未知命令 ${args.command}`);
  } finally { await session.browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
