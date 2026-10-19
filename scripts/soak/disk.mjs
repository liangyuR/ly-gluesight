import assert from "node:assert/strict";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { connect, ensureOffline, parseArgs, RECIPE_ID, until, writeJson } from "./lib.mjs";

const OK_CLASS = new Set(["OK", "OK_WITH_EXCURSION"]);

async function enterOffline(call, out) {
  await ensureOffline(call, out);
  await call("offline_pace", { pace: { shotIntervalMs: 150, pauseMs: 200 } });
}

/** 资料盘低于下限（以开发构建的 GLUESIGHT_DISK_MIN_FREE_MB 模拟）：不接件、提示看得懂、不产生记录。 */
async function blocked({ call }, out) {
  await enterOffline(call, out);
  const readiness = await until(() => call("cycle_readiness"), r => r.items.find(i => i.key === "audit")?.state === "waiting", "资料盘不足未体现在就绪检查", 30000, 500);
  const before = (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).total;
  await call("offline_run", { recipeId: RECIPE_ID, scenario: "normal", continuous: false, partId: null });
  const status = await until(() => call("site_status"), s => !s.running, "资料盘不足时离线产线未结束", 60000, 500);
  const after = (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).total;
  const snapshot = await call("cycle_snapshot");
  const result = { audit: readiness.items.find(i => i.key === "audit"), plc: readiness.items.find(i => i.key === "plc"), lineMessage: status.message,
    alarms: snapshot.alarms, phase: snapshot.phase, newRecords: after - before };
  result.passed = result.newRecords === 0 && /剩余空间/.test(result.audit.detail) && snapshot.alarms.some(a => a.includes("剩余空间"));
  await writeJson(path.join(out, "disk-blocked.json"), result);
  process.stdout.write(JSON.stringify(result) + "\n");
  if (!result.passed) process.exitCode = 1;
}

async function recovered({ call }, out) {
  await enterOffline(call, out);
  const attempts = [];
  let record;
  do {
    const last = (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0];
    await call("offline_run", { recipeId: RECIPE_ID, scenario: "normal", continuous: false, partId: null });
    record = await until(async () => (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0],
      r => r && r.id !== last?.id, "空间恢复后投产未生成记录", 120000, 300);
    await until(() => call("site_status"), s => !s.running, "离线产线未停止", 60000, 300);
    record = (await until(() => call("history_detail", { id: record.id }), d => ["acknowledged", "failed"].includes(d.summary.delivery.state), "交付状态未落定", 30000, 300)).summary;
    attempts.push({ verdict: record.verdict, reason: record.reason });
  } while (!OK_CLASS.has(record.verdict) && attempts.length < 3);
  const result = { verdict: record.verdict, delivery: record.delivery.state, attempts, passed: OK_CLASS.has(record.verdict) && record.delivery.state === "acknowledged" };
  await writeJson(path.join(out, "disk-recovered.json"), result);
  process.stdout.write(JSON.stringify(result) + "\n");
  assert(result.passed, "空间恢复后未恢复生产");
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const out = path.resolve(args.out ?? "output/soak");
  const session = await connect(args.cdp ?? "http://[::1]:9347", args.instance ?? "output/playwright/soak-regression/instance.json");
  try {
    if (args.command === "blocked") await blocked(session, out);
    else if (args.command === "recovered") await recovered(session, out);
    else throw new Error(`未知命令 ${args.command}`);
  } finally { await session.browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
