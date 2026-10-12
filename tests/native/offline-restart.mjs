import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { client, gateRetries, identifier, produce, until } from "./offline-e2e.mjs";

export async function runOfflineRestart(page, options) {
  const startedAt = Date.now();
  const previous = JSON.parse(await readFile(options.previous, "utf8"));
  assert(previous.passed, "重启续测需要先通过的离线端到端结果");
  const recipeId = previous.recipe.id;
  const before = previous.production.p02;
  assert(before?.bundleId && before.recipeRevision, "离线端到端结果缺少 p02 的发布包与配方修订");
  const call = client(page);
  assert((await call("records_list")).root.includes(identifier), "数据目录不属于隔离实例");
  assert.equal((await call("site_status")).mode, "real", "重启后必须回到真机模式");
  assert.equal((await call("history_query", { query: { recipeId, limit: 1 } })).total, Object.keys(previous.production).length, "重启后历史记录与重启前不一致");
  const layout = await call("cycle_layout", { recipeId });
  assert.equal(layout.revisionId, previous.recipe.revisionId, "重启后生产配方修订改变");
  assert.equal(layout.teachingId, previous.recipe.teachingId, "重启后生产配方的示教身份改变");

  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: previous.inputs.offlineSampleSet } });
  assert.equal(entered.source, "sampleSet");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const prewarm = await until(async () => (await call("cycle_logs")).find(l => l.ev === "生产预热" && l.msg.includes(`发布包 ${before.bundleId} 已就绪`)),
    Boolean, `重启后发布包 ${before.bundleId} 未预热就绪`, 60000);
  const { summary } = await produce(call, recipeId, "p02");
  assert.equal(summary.bundleId, before.bundleId, "重启后投产未使用原发布包");
  assert.equal(summary.recipeRevision, before.recipeRevision);
  assert.equal(summary.verdict, before.verdict, `重启后同一样本件结论改变：${summary.reason}`);
  assert.equal(summary.sample?.partId, "p02");
  assert.equal((await call("site_leave_offline")).mode, "real", "未退出离线模式");

  const result = {
    passed: true, identifier, recipeId, elapsedMs: Date.now() - startedAt,
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
  for (const key of ["cdp", "instance", "previous", "output"]) assert(args[key], `缺少 --${key}`);
  const url = new URL(args.cdp);
  assert(["127.0.0.1", "localhost", "[::1]"].includes(url.hostname), "仅允许本机 CDP");
  const manifest = JSON.parse(await readFile(args.instance, "utf8"));
  assert.equal(manifest.identifier, identifier, "仅允许隔离端到端实例");
  assert.equal(Number(url.port), manifest.debugPort);
  const browser = await chromium.connectOverCDP(args.cdp);
  try {
    const page = browser.contexts().flatMap(context => context.pages()).find(p => /tauri\.localhost|localhost:1420|^tauri:\/\//.test(p.url()));
    assert(page, "未找到 Tauri UI 页面");
    page.setDefaultTimeout(30000);
    await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
    const result = await runOfflineRestart(page, { previous: args.previous, output: args.output });
    process.stdout.write(JSON.stringify({ passed: result.passed, elapsedMs: result.elapsedMs, verdict: result.after.verdict, bundleId: result.after.bundleId }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
