import assert from "node:assert/strict";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  connect, diskUsage, LogCollector, offlineHistory, parseArgs, percentiles, prepareSampleSet, processMetrics,
  RECIPE_ID, setupRecipe, sleep, until, writeJson,
} from "./lib.mjs";

const OK_CLASS = new Set(["OK", "OK_WITH_EXCURSION"]);

/** 界面 JS 堆：先强制回收再量，只看回收不掉的部分；DOM 节点数一并记下。 */
async function uiHeap(page, cdp) {
  await cdp.send("HeapProfiler.collectGarbage");
  const { usedSize } = await cdp.send("Runtime.getHeapUsage");
  return { jsHeapBytes: usedSize, domNodes: await page.evaluate(() => document.getElementsByTagName("*").length) };
}

async function metricsSample({ manifest, call, page }, cdp, startedAt, parts) {
  const [proc, disk, snapshot, ui] = await Promise.all([processMetrics(manifest.pid), diskUsage(manifest.dataRoot), call("cycle_snapshot"), uiHeap(page, cdp)]);
  return { atMs: Date.now() - startedAt, parts, proc, ui, disk: Object.fromEntries(Object.entries(disk).map(([k, v]) => [k, v.bytes])),
    files: { recordsOffline: disk["records-offline"].files }, phase: snapshot?.phase ?? null };
}

/** 长稳判定：正常样本判 NG、cycleId 重复、判定与交付不一致是硬失败；布防预算等时序原因的 ERR 只拒绝本件，按比例阈值判定。 */
export function assessSoak({ parts, produced, items, maxErrRatio }) {
  const failures = [];
  const count = test => items.filter(test).length;
  const ng = count(i => i.verdict.startsWith("NG"));
  const err = count(i => i.verdict === "ERR_INSPECT");
  const errRatio = items.length ? err / items.length : 0;
  const deliveryMismatch = items.filter(i => OK_CLASS.has(i.verdict) ? i.delivery.state !== "acknowledged" : !["acknowledged", "failed"].includes(i.delivery.state))
    .map(i => ({ id: i.id, cycleId: i.cycleId, verdict: i.verdict, delivery: i.delivery.state }));
  const cycleIds = new Set(items.map(i => i.cycleId)).size;
  if (produced < parts) failures.push(`只生产了 ${produced}/${parts} 件`);
  if (ng) failures.push(`正常样本判 NG ${ng} 件`);
  if (cycleIds !== items.length) failures.push(`${items.length} 条记录只有 ${cycleIds} 个不同的 cycleId`);
  if (deliveryMismatch.length) failures.push(`判定与 PLC 交付不一致 ${deliveryMismatch.length} 件（合格件未确认，或交付未落定）`);
  if (errRatio > maxErrRatio) failures.push(`ERR ${err} 件，占 ${(errRatio * 100).toFixed(1)}%，超过阈值 ${(maxErrRatio * 100).toFixed(1)}%`);
  return { failures, errRatio, maxErrRatio, uniqueCycleIds: cycleIds, deliveryMismatch: deliveryMismatch.slice(0, 20) };
}

export async function soak(session, options) {
  const { call, page } = session;
  if (options.page) {
    await page.evaluate(route => { location.hash = `#${route}`; }, options.page);
    await page.waitForTimeout(2000);
  }
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("HeapProfiler.enable");
  const parts = Number(options.parts ?? 1000);
  const pace = { shotIntervalMs: Number(options.interval ?? 150), pauseMs: Number(options.pause ?? 200) };
  const metricsEveryMs = Number(options["metrics-every"] ?? 60000);
  const maxRestarts = Number(options["max-restarts"] ?? 200);
  const maxErrRatio = Number(options["max-err-ratio"] ?? 0.2);
  assert(maxErrRatio >= 0 && maxErrRatio <= 1, "--max-err-ratio 需在 0–1 之间");
  const settings = await call("cycle_get_settings");
  const before = await offlineHistory(call, RECIPE_ID);
  const logs = new LogCollector(call);
  await logs.poll();
  const startedAt = Date.now();
  const runs = new Map();
  const incidents = [];
  const samples = [await metricsSample(session, cdp, startedAt, 0)];
  let lastSample = Date.now();
  let produced = 0;
  let restarts = 0;
  await call("offline_pace", { pace });
  const start = async () => call("offline_run", { recipeId: RECIPE_ID, scenario: "normal", continuous: true, partId: null });
  await start();
  for (;;) {
    await sleep(1000);
    const status = await call("site_status");
    for (const r of status.recent) if (r.purpose === "production") runs.set(r.partSn, { ...runs.get(r.partSn), ...r });
    await logs.poll();
    produced = (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).total - before.total;
    if (Date.now() - lastSample >= metricsEveryMs) {
      samples.push(await metricsSample(session, cdp, startedAt, produced));
      lastSample = Date.now();
      process.stdout.write(`[soak] ${produced}/${parts} 件，主进程私有内存 ${(samples.at(-1).proc.main?.priv / 2 ** 20).toFixed(0)} MiB，句柄 ${samples.at(-1).proc.main?.handles}\n`);
    }
    if (produced >= parts) {
      await call("offline_stop", { now: false });
      await until(() => call("site_status"), s => !s.running, "离线产线未停止", 60000, 500);
      break;
    }
    if (!status.running) {
      const snapshot = await call("cycle_snapshot");
      incidents.push({ atMs: Date.now() - startedAt, parts: produced, message: status.message, phase: snapshot?.phase, fault: snapshot?.fault ?? null });
      assert(restarts++ < maxRestarts, `离线产线中止次数超过 ${maxRestarts}：${status.message}`);
      await sleep(2000);
      await start();
    }
  }
  await logs.poll();
  samples.push(await metricsSample(session, cdp, startedAt, produced));
  const history = await offlineHistory(call, RECIPE_ID);
  const known = new Set(before.items.map(i => i.id));
  const items = history.items.filter(i => !known.has(i.id)).sort((a, b) => a.ts - b.ts);
  const byCycle = new Map([...runs.values()].filter(r => r.cycleId).map(r => [r.cycleId, r]));
  const latency = items.map(i => byCycle.get(i.cycleId)).map((r, n) => r && items[n].ts - r.ts).filter(v => Number.isFinite(v));
  const intervals = items.slice(1).map((item, n) => item.ts - items[n].ts);
  const verdicts = items.reduce((acc, i) => ({ ...acc, [i.verdict]: (acc[i.verdict] ?? 0) + 1 }), {});
  const deliveries = items.reduce((acc, i) => ({ ...acc, [i.delivery.state]: (acc[i.delivery.state] ?? 0) + 1 }), {});
  const recent = items.slice(-Math.min(20, items.length));
  const recordings = [];
  for (const item of [items[0], ...recent].filter(Boolean)) {
    const detail = await call("history_detail", { id: item.id });
    recordings.push({ id: item.id, ts: item.ts, state: detail.recording.state, available: detail.recording.available, directory: detail.recording.directory });
  }
  const first = samples[1] ?? samples[0], last = samples.at(-1);
  const growth = key => last.proc.main && first.proc.main ? last.proc.main[key] - first.proc.main[key] : null;
  const handshakeTimeouts = logs.lines.filter(l => l.level === "err" && /超时|未等到|T_ack|心跳/.test(`${l.ev}${l.msg}`));
  const arms = logs.lines.filter(l => l.ev === "布防阶段").map(l => { try { return JSON.parse(l.msg); } catch { return null; } }).filter(Boolean);
  const stage = name => arms.map(a => a.trace?.stages?.find(s => s.stage === name)?.intervalMs).filter(Number.isFinite).map(Math.round);
  const arming = {
    statuses: arms.reduce((acc, a) => ({ ...acc, [a.status]: (acc[a.status] ?? 0) + 1 }), {}),
    totalMs: percentiles(arms.filter(a => a.status === "armed").map(a => Math.round(a.trace.elapsedMs))),
    auditHealthMs: percentiles(stage("audit-health")), plcArmMs: percentiles(stage("plc-arm-confirmed")),
    overBudget: arms.filter(a => a.status !== "armed").map(a => ({ status: a.status, elapsedMs: Math.round(a.trace?.elapsedMs ?? 0), last: a.trace?.stages?.at(-1)?.stage })),
  };
  const check = assessSoak({ parts, produced, items, maxErrRatio });
  const result = {
    passed: check.failures.length === 0, failures: check.failures, errRatio: check.errRatio, maxErrRatio: check.maxErrRatio, deliveryMismatch: check.deliveryMismatch,
    parts: produced, pace, page: options.page ?? null, settings: { offlineRecordKeep: settings.offlineRecordKeep, offlineRecordMaxGb: settings.offlineRecordMaxGb, historyDays: settings.historyDays },
    elapsedMs: Date.now() - startedAt,
    verdicts, deliveries, uniqueCycleIds: check.uniqueCycleIds,
    latencyMs: percentiles(latency), intervalMs: percentiles(intervals),
    throughputPerHour: items.length > 1 ? Math.round(3600000 * (items.length - 1) / (items.at(-1).ts - items[0].ts)) : null,
    growth: { mainPrivBytes: growth("priv"), mainWsBytes: growth("ws"), mainHandles: growth("handles"), mainThreads: growth("threads"),
      childrenPrivBytes: last.proc.children.priv - first.proc.children.priv, childrenHandles: last.proc.children.handles - first.proc.children.handles,
      uiJsHeapBytes: last.ui.jsHeapBytes - first.ui.jsHeapBytes, uiDomNodes: last.ui.domNodes - first.ui.domNodes,
      diskBytes: Object.fromEntries(Object.keys(last.disk).map(k => [k, last.disk[k] - samples[0].disk[k]])) },
    incidents, restarts, arming,
    handshakeTimeouts: { count: handshakeTimeouts.length, lines: handshakeTimeouts.slice(0, 50) },
    logCounts: logs.counts(l => l.level === "err" || l.level === "warn"),
    recordings,
    samples,
  };
  return result;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const out = path.resolve(args.out ?? "output/soak");
  const session = await connect(args.cdp ?? "http://[::1]:9347", args.instance ?? "output/playwright/soak-regression/instance.json");
  try {
    if (args.command === "setup") {
      assert(args["sample-set"], "缺少 --sample-set");
      const copy = await prepareSampleSet(args["sample-set"], out);
      const model = args.model ?? "burst";
      assert.equal(model, "burst", "仅支持段配方");
      const setup = await setupRecipe(session.call, copy);
      await writeJson(path.join(out, "setup.json"), { sampleSetCopy: copy, ...setup });
      process.stdout.write(JSON.stringify({ model, recipeId: setup.recipeId, taught: setup.taught?.length ?? "reused", expectedTotal: setup.expectedTotal ?? null }) + "\n");
    } else if (args.command === "run") {
      const result = await soak(session, args);
      await writeJson(path.join(out, `soak-${result.parts}.json`), result);
      const { samples, ...summary } = result;
      process.stdout.write(JSON.stringify({ ...summary, recordings: undefined, handshakeTimeouts: summary.handshakeTimeouts.count, samples: samples.length }) + "\n");
      if (!result.passed) process.exitCode = 1;
    } else {
      throw new Error(`未知命令 ${args.command}`);
    }
  } finally { await session.browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
