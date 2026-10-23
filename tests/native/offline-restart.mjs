import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { client, gateRetries, identifier, instanceCdp, OK_CLASS, produceBurst, uiPage, until } from "./lib.mjs";

/**
 * 段配方的重启续测（offline-e2e 的 burst 结果）：同一隔离数据目录重启进程后，
 *   - 合成图主流程的配方从磁盘恢复，真机仍拒绝离线验证发布的版本，离线·合成图下能再跑一件，配方修订不变；
 *   - 样本集冒烟的配方（有发布包）：发布包按原身份预热，离线·样本集下同一样本件的结论、发布包与修订都不变。
 */
async function runBurstRestart(page, options, previous) {
  const startedAt = Date.now();
  const call = client(page);
  assert((await call("records_list")).root.includes(identifier), "数据目录不属于隔离实例");
  const site = await call("site_status");
  assert.equal(site.mode, "real", "重启后必须回到真机模式");
  const sampleSetDir = site.last?.sampleSetDir;
  const recipeOption = async id => (await call("cycle_readiness")).recipes.find(r => r.id === id);
  const result = { passed: true, model: "burst", identifier, boundary: "同一隔离数据目录重启进程后，已发布的段配方从磁盘恢复；不代表现场重启或断电恢复验收" };

  const main = previous.main;
  const id = main.recipe.id;
  const mainRuns = Object.keys(main.production).length;
  assert.equal((await call("history_query", { query: { recipeId: id, scope: "offline", limit: 1 } })).total, mainRuns, "重启后历史记录与重启前不一致");
  assert.equal((await call("cycle_layout", { recipeId: id })).revisionId, main.recipe.revisionId, "重启后生产配方修订改变");
  const realOption = await recipeOption(id);
  assert(realOption, `真机就绪检查未列出配方 ${id}`);
  assert.equal(realOption.runnable, false, "离线环境验证发布的配方不应能在真机开工");
  assert(realOption.reason?.includes("离线环境验证发布"), `真机拒绝原因不符：${realOption.reason}`);
  assert.equal(realOption.target, "workspace", "真机拒绝应引导到工作台重新验证发布");
  const entered = await call("site_enter_offline", { source: { kind: "synthetic" } });
  assert.equal(entered.source, "synthetic");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  await call("offline_pace", { pace: { shotIntervalMs: 200, pauseMs: 200 } });
  await until(() => recipeOption(id), r => r?.runnable === true, `离线后配方 ${id} 仍不可运行`, 60000);
  const synthetic = await produceBurst(call, id, { scenario: "normal" });
  assert(OK_CLASS.includes(synthetic.summary.verdict), `重启后合成图正常件应为合格类：${synthetic.summary.verdict} ${synthetic.summary.reason}`);
  assert.equal(synthetic.summary.recipeRevision, main.recipe.revisionId);
  assert.equal(synthetic.facts.expectedTotal, main.calibration.expectedTotal, "重启后 ΣN_exp 改变");
  assert.equal((await call("site_leave_offline")).mode, "real", "未退出离线模式");
  result.synthetic = {
    recipeId: id, realRejection: { runnable: realOption.runnable, reason: realOption.reason, target: realOption.target },
    after: { historyId: synthetic.summary.id, verdict: synthetic.summary.verdict, groupCount: synthetic.facts.groupCount, recipeRevision: synthetic.summary.recipeRevision },
  };

  const smoke = previous.smoke;
  if (smoke?.published && smoke.production) {
    const before = smoke.production;
    assert(sampleSetDir, "站点没有记住样本集目录，无法回到离线·样本集");
    const smokeOption = await recipeOption(smoke.recipeId);
    assert(smokeOption && smokeOption.runnable === false && smokeOption.reason?.includes("离线环境验证发布"), `样本集冒烟配方在真机应被拒绝：${JSON.stringify(smokeOption)}`);
    const reentered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: sampleSetDir } });
    assert.equal(reentered.source, "sampleSet");
    await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
    const prewarm = await until(async () => (await call("cycle_logs")).find(l => l.ev === "生产预热" && l.msg.includes(`发布包 ${before.bundleId} 已就绪`)),
      Boolean, `重启后发布包 ${before.bundleId} 未预热就绪`, 60000);
    await until(() => recipeOption(smoke.recipeId), r => r?.runnable === true, `离线后配方 ${smoke.recipeId} 仍不可运行`, 60000);
    const part = await produceBurst(call, smoke.recipeId, { scenario: "normal", partId: smoke.partId });
    assert.equal(part.summary.bundleId, before.bundleId, "重启后投产未使用原发布包");
    assert.equal(part.summary.recipeRevision, smoke.recipe.revisionId);
    assert.equal(part.summary.verdict, before.verdict, `重启后同一样本件结论改变：${part.summary.reason}`);
    assert.deepEqual(part.facts.frameStates, before.frameStates, "重启后同一样本件的帧状态分布改变");
    assert.equal((await call("site_leave_offline")).mode, "real", "未退出离线模式");
    result.sampleSet = {
      recipeId: smoke.recipeId, prewarm: { ts: prewarm.ts, msg: prewarm.msg },
      before: { historyId: before.historyId, verdict: before.verdict, bundleId: before.bundleId },
      after: { historyId: part.summary.id, verdict: part.summary.verdict, bundleId: part.summary.bundleId, frameStates: part.facts.frameStates },
    };
  }
  result.elapsedMs = Date.now() - startedAt;
  result.gateRetries = gateRetries.length;
  if (options.output) await writeFile(options.output, JSON.stringify(result, null, 2) + "\n");
  return result;
}

export async function runOfflineRestart(page, options) {
  const previous = JSON.parse(await readFile(options.previous, "utf8"));
  assert(previous.passed, "重启续测需要先通过的离线端到端结果");
  assert.equal(previous.model, "burst", "仅支持段配方续测");
  return runBurstRestart(page, options, previous);
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
    process.stdout.write(JSON.stringify({ passed: result.passed, model: result.model, elapsedMs: result.elapsedMs, synthetic: result.synthetic.after.verdict, sampleSet: result.sampleSet?.after.verdict ?? null }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
