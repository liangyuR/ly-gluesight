import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { client, gateRetries, identifier, instanceCdp, produce, uiPage, until } from "./offline-e2e.mjs";

export async function runOfflineRestart(page, options) {
  const startedAt = Date.now();
  const previous = JSON.parse(await readFile(options.previous, "utf8"));
  assert(previous.passed, "重启续测需要先通过的离线端到端结果");
  const recipeId = previous.recipe.id;
  const partId = previous.replayPart;
  const before = previous.production[partId];
  assert(before?.bundleId && before.recipeRevision, `离线端到端结果缺少 ${partId} 的发布包与配方修订`);
  const call = client(page);
  assert((await call("records_list")).root.includes(identifier), "数据目录不属于隔离实例");
  const site = await call("site_status");
  assert.equal(site.mode, "real", "重启后必须回到真机模式");
  assert.equal(site.last?.kind, "sampleSet", "重启后应记住上次的离线样本集");
  assert.equal((await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } })).total, Object.keys(previous.production).length, "重启后历史记录与重启前不一致");
  const layout = await call("cycle_layout", { recipeId });
  assert.equal(layout.revisionId, previous.recipe.revisionId, "重启后生产配方修订改变");
  assert.equal(layout.teachingId, previous.recipe.teachingId, "重启后生产配方的示教身份改变");
  const recipeOption = async () => (await call("cycle_readiness")).recipes.find(r => r.id === recipeId);
  const realOption = await recipeOption();
  assert(realOption, `真机就绪检查未列出配方 ${recipeId}`);
  assert.equal(realOption.runnable, false, "离线环境验证发布的配方不应能在真机开工");
  assert(realOption.reason?.includes("离线环境验证发布"), `真机拒绝原因不符：${realOption.reason}`);
  assert.equal(realOption.target, "workspace", "真机拒绝应引导到工作台重新验证发布");

  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: site.last.sampleSetDir } });
  assert.equal(entered.source, "sampleSet");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const prewarm = await until(async () => (await call("cycle_logs")).find(l => l.ev === "生产预热" && l.msg.includes(`发布包 ${before.bundleId} 已就绪`)),
    Boolean, `重启后发布包 ${before.bundleId} 未预热就绪`, 60000);
  await until(recipeOption, r => r?.runnable === true, `离线后配方 ${recipeId} 仍不可运行`, 60000);
  const { summary } = await produce(call, recipeId, partId);
  assert.equal(summary.bundleId, before.bundleId, "重启后投产未使用原发布包");
  assert.equal(summary.recipeRevision, before.recipeRevision);
  assert.equal(summary.verdict, before.verdict, `重启后同一样本件结论改变：${summary.reason}`);
  assert.equal(summary.sample?.partId, partId);
  assert.equal((await call("site_leave_offline")).mode, "real", "未退出离线模式");

  const result = {
    passed: true, identifier, recipeId, elapsedMs: Date.now() - startedAt,
    realSiteRejection: { runnable: realOption.runnable, reason: realOption.reason, target: realOption.target },
    offlineRunnable: true,
    prewarm: { ts: prewarm.ts, msg: prewarm.msg },
    before: { historyId: before.historyId, verdict: before.verdict, bundleId: before.bundleId, recipeRevision: before.recipeRevision },
    after: { historyId: summary.id, verdict: summary.verdict, bundleId: summary.bundleId, recipeRevision: summary.recipeRevision, reason: summary.reason },
    gateRetries: gateRetries.length,
    boundary: "同一隔离数据目录重启进程后，已发布配方与发布包从磁盘恢复并预热，离线样本件结论不变；不代表现场重启或断电恢复验收",
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
  for (const key of ["instance", "previous", "output"]) assert(args[key], `缺少 --${key}`);
  const cdp = await instanceCdp(args.instance, args.cdp, id => id === identifier);
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = await uiPage(browser);
    const result = await runOfflineRestart(page, { previous: args.previous, output: args.output });
    process.stdout.write(JSON.stringify({ passed: result.passed, elapsedMs: result.elapsedMs, verdict: result.after.verdict, bundleId: result.after.bundleId }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
