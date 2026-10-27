import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { copyFile, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";
import { burstFacts } from "../../tests/native/lib.mjs";
import { connect, ensureOffline, identifier, LogCollector, parseArgs, readSetup, RECIPE_ID, sleep, until, writeJson } from "./lib.mjs";
import { isolatedDirectory, isolatedStorage } from "../../tests/native/storage-paths.mjs";

const OK_CLASS = new Set(["OK", "OK_WITH_EXCURSION"]);
// 这些注入不损坏本件图像与计数，本件判合格是正确结果；其余注入下本件判合格即为“错误的 OK”。
const MAY_PASS = new Set(["plcDelayMidPart", "plcRejectWriteMidPart", "recordsDirReadOnly"]);
// 段配方（规格 §0）：缺帧、坏帧只把所在帧组标“检测不完整”，不影响 OK/NG，所以缺帧判 OK 不算错误的 OK；
// 长稳的段配方检测项全关（见 lib.mjs），空白图 / 噪声图本身也判 OK。这类场景改为核对“检测不完整”标记（EXPECT_INCOMPLETE）。
const MAY_PASS_BURST = new Set([...MAY_PASS, "cameraLostFrame", "pulseJitter", "replayMissingTaughtImage", "replayMissingUntaughtImage", "cameraCorruptImage", "lyflowBlankImage", "lyflowNoiseImage"]);
// 段配方里这些注入让某帧缺失或读不出，本件必须标检测不完整
const EXPECT_INCOMPLETE = new Set(["cameraLostFrame", "replayMissingTaughtImage", "replayMissingUntaughtImage", "cameraCorruptImage"]);
const RECOVER_ENTRIES = new Set(["baseline", "afterRestart"]);
const OUTAGE_MS = 8000;
const run = promisify(execFile);
// 段配方：样本集按“每台相机的帧序列”解释。cam1 的第 3 帧（S007）是它走廊的示教帧，cam2 的第 2 帧（S005）不是任何相机的示教帧。
const BURST_TAUGHT_IMAGE = "S007_cam1_v1.bmp";
const BURST_UNTAUGHT_IMAGE = "S005_cam2_v1.bmp";

/** 记录摘要：结论加整件标记（段配方记录有 flags，旧记录为 null）。 */
const view = record => ({ verdict: record.verdict, plcCode: record.plcCode, faultCode: record.faultCode, reason: record.reason, flags: record.flags ?? null });

function context(session, out, model) {
  const { call, manifest } = session;
  const taughtImage = BURST_TAUGHT_IMAGE, untaughtImage = BURST_UNTAUGHT_IMAGE;
  const logs = new LogCollector(call);
  const latest = async () => (await call("history_query", { query: { recipeId: RECIPE_ID, scope: "offline", limit: 1 } })).items[0] ?? null;
  const snapshot = () => call("cycle_snapshot");
  const site = () => call("site_status");
  const stopLine = async () => {
    await call("offline_stop", { now: true });
    await until(site, s => !s.running, "离线产线未停止", 90000, 300);
  };
  const start = (scenario = "normal") => call("offline_run", { recipeId: RECIPE_ID, scenario, continuous: false, partId: null });
  /** 跑一件并等到出记录、产线停止；返回本件记录。 */
  const runOne = async (scenario = "normal", timeout = 90000) => {
    const before = await latest();
    await start(scenario);
    const record = await until(latest, r => r && r.id !== before?.id, `工况 ${scenario} 未生成检测记录`, timeout, 300);
    const status = await until(site, s => !s.running, "离线产线未停止", timeout, 300);
    return { record, message: status.message };
  };
  const waitPhase = async phases => until(snapshot, s => phases.includes(s?.phase), `节拍未进入 ${phases.join("/")}`, 30000, 50);
  /** 故障后恢复：虚拟 PLC 像操作员一样复位，再跑正常件直到合格（最多 3 件），记下每次结果；需要人工补连接时单独记下。 */
  const recover = async () => {
    const startedAt = Date.now();
    const attempts = [];
    const stuck = (await ensureOffline(call, out)).manualReconnect;
    if (stuck) {
      attempts.push({ manualReconnect: "PC 已主动断开 PLC 且未自动重连，经接口补连接" });
      await call("offline_pace", { pace: { shotIntervalMs: 400, pauseMs: 200 } });
    }
    for (;;) {
      try {
        const result = await runOne("normal");
        const detail = await until(() => call("history_detail", { id: result.record.id }), d => d.recording.state !== "pending" && d.summary.delivery.state !== "pending" && d.summary.delivery.state !== "submitted",
          "恢复件的录制或交付未落定", 30000, 300).catch(() => call("history_detail", { id: result.record.id }));
        const outcome = { verdict: result.record.verdict, reason: result.record.reason, delivery: detail.summary.delivery.state, recording: detail.recording.state,
          recordingAvailable: detail.recording.available, message: result.message };
        attempts.push(outcome);
        if (OK_CLASS.has(outcome.verdict) || attempts.length >= 3) return { ...outcome, recoverMs: Date.now() - startedAt, manualReconnect: Boolean(stuck), attempts };
      } catch (error) {
        attempts.push({ error: error.message });
        if (attempts.length >= 3) throw new Error(`${error.message}；${JSON.stringify(attempts)}`);
        await stopLine().catch(() => {});
        await sleep(3000);
      }
    }
  };
  const faultView = async () => {
    const [s, st] = await Promise.all([snapshot(), site()]);
    return { phase: s?.phase, fault: s?.fault ?? null, alarms: s?.alarms ?? [], handshake: s?.plcHandshake ?? null, running: st.running, siteMessage: st.message };
  };
  const sampleDir = async () => path.join((await site()).sampleSet.dir, "parts", "p01");
  return { call, logs, latest, snapshot, site, stopLine, start, runOne, waitPhase, recover, faultView, sampleDir, manifest, model, taughtImage, untaughtImage };
}

async function icacls(target, ...args) {
  await run("icacls", [target, ...args], { windowsHide: true });
}

async function whoami() {
  const { stdout } = await run("whoami", [], { windowsHide: true });
  return stdout.trim();
}

/** 在途件中途注入：等节拍进入采集再动手，结束后停掉虚拟 PLC。不该进故障的注入等到本件走完、产线自行停下或进故障为止。 */
async function midPart(c, inject, { scenario = "normal", wait = 6000, expectFault = true } = {}) {
  const before = await c.latest();
  await c.start(scenario);
  await c.waitPhase(["ACQUIRE", "DRAIN"]);
  await inject();
  const seen = await until(c.faultView, v => v.phase === "FAULT" || (!expectFault && !v.running), expectFault ? "注入后未进入故障" : "注入后本件未走完", wait + 30000, 200)
    .catch(error => ({ timeout: error.message }));
  await c.stopLine();
  const record = await until(c.latest, r => r && r.id !== before?.id, "在途件未生成记录", 30000, 300).catch(() => null);
  return { expectFault, seen, record: record && { verdict: record.verdict, plcCode: record.plcCode, faultCode: record.faultCode, reason: record.reason, delivery: record.delivery.state } };
}

/** PLC 停机期满后等 PC 自动重连再返回，避免外层 clear 提前结束停机；PC 主动断开（已知问题）时交给恢复步骤补连接。 */
async function outageReconnect(c, injectedAt) {
  const status = await until(() => c.call("plc_get_status"), s => s.state === "connected" || s.state === "disconnected", "PLC 停机结束后未自动重连", OUTAGE_MS + 60000, 300);
  return { outageMs: OUTAGE_MS, reconnectAfterMs: status.state === "connected" ? Date.now() - injectedAt : null, pcDisconnected: status.state === "disconnected" };
}

const scenarios = {
  async plcDisconnectIdle(c) {
    await c.call("offline_inject", { fault: "disconnect" });
    const seen = await until(c.faultView, v => v.phase === "FAULT", "断线后未进入故障", 15000, 100);
    await until(() => c.call("plc_get_status"), s => s.state === "connected", "断线后未自动重连", 30000, 200);
    return { seen };
  },
  async plcDisconnectMidPart(c) {
    return midPart(c, () => c.call("offline_inject", { fault: "disconnect" }));
  },
  async plcOutageIdle(c) {
    const injectedAt = Date.now();
    await c.call("offline_inject", { fault: "outage", durationMs: OUTAGE_MS });
    const seen = await until(c.faultView, v => v.phase === "FAULT", "PLC 停机后未进入故障", 15000, 100);
    const during = await c.call("plc_get_status");
    await until(() => c.call("plc_get_status"), s => s.state === "connected", "PLC 恢复后未自动重连", OUTAGE_MS + 60000, 300);
    return { seen, plcDuringOutage: { state: during.state, message: during.message }, outageMs: OUTAGE_MS, reconnectAfterMs: Date.now() - injectedAt };
  },
  async plcOutageMidPart(c) {
    let injectedAt;
    const result = await midPart(c, () => { injectedAt = Date.now(); return c.call("offline_inject", { fault: "outage", durationMs: OUTAGE_MS }); });
    return { ...result, ...(await outageReconnect(c, injectedAt)) };
  },
  async plcDelayMidPart(c) {
    return midPart(c, () => c.call("offline_inject", { fault: "delay" }));
  },
  async plcRejectWriteMidPart(c) {
    const result = await midPart(c, async () => { await c.call("offline_inject", { fault: "rejectWrite" }); await c.call("offline_inject", { fault: "rejectWrite" }); },
      { expectFault: false });
    await c.call("offline_inject", { fault: "clear" });
    return result;
  },
  async heartbeatLossIdle(c) {
    await c.call("offline_inject", { fault: "heartbeatStop" });
    try {
      return { seen: await until(c.faultView, v => v.phase === "FAULT", "心跳停止后未进入故障", 20000, 200) };
    } finally { await c.call("offline_inject", { fault: "heartbeatResume" }); }
  },
  async heartbeatLossMidPart(c) {
    try { return await midPart(c, () => c.call("offline_inject", { fault: "heartbeatStop" })); }
    finally { await c.call("offline_inject", { fault: "heartbeatResume" }); }
  },
  async cameraLostFrame(c) {
    const { record, message } = await c.runOne("lostFrame");
    return { record: view(record), message };
  },
  /** 段配方：首尾各 ±1 个脉冲（样本集没有的帧按缺帧），照常判，帧数异常只告警。带回本件的帧组数与 ΣN_exp、Σtol，供 assess 核对标记。 */
  async pulseJitter(c) {
    const { record, message } = await c.runOne("pulseJitter");
    const { groupCount, expectedTotal, toleranceTotal } = burstFacts(await c.call("history_detail", { id: record.id }));
    return { record: view(record), count: { groupCount, expectedTotal, toleranceTotal }, message };
  },
  async replayMissingTaughtImage(c) {
    const file = path.join(await c.sampleDir(), c.taughtImage);
    await rename(file, `${file}.hidden`);
    try {
      const { record, message } = await c.runOne("normal");
      return { record: view(record), message };
    } finally { await rename(`${file}.hidden`, file); }
  },
  async replayMissingUntaughtImage(c) {
    const file = path.join(await c.sampleDir(), c.untaughtImage);
    await rename(file, `${file}.hidden`);
    try {
      const { record, message } = await c.runOne("normal");
      return { record: view(record), message };
    } finally { await rename(`${file}.hidden`, file); }
  },
  async cameraCorruptImage(c) {
    const file = path.join(await c.sampleDir(), c.taughtImage);
    await copyFile(file, `${file}.orig`);
    const bytes = await readFile(file);
    await writeFile(file, bytes.subarray(0, 4096));
    try {
      const { record, message } = await c.runOne("normal");
      return { record: view(record), message };
    } finally { await rename(`${file}.orig`, file); }
  },
  async lyflowBlankImage(c) {
    const file = path.join(await c.sampleDir(), c.taughtImage);
    await copyFile(file, `${file}.orig`);
    const bytes = Buffer.from(await readFile(file));
    const offset = bytes.readUInt32LE(10);
    bytes.fill(128, offset);
    await writeFile(file, bytes);
    try {
      const { record, message } = await c.runOne("normal");
      return { record: view(record), message };
    } finally { await rename(`${file}.orig`, file); }
  },
  async lyflowNoiseImage(c) {
    const file = path.join(await c.sampleDir(), c.taughtImage);
    await copyFile(file, `${file}.orig`);
    const bytes = Buffer.from(await readFile(file));
    const offset = bytes.readUInt32LE(10);
    let seed = 12345;
    for (let i = offset; i < bytes.length; i++) { seed = (seed * 1103515245 + 12345) >>> 0; bytes[i] = seed >>> 24; }
    await writeFile(file, bytes);
    try {
      const { record, message } = await c.runOne("normal");
      return { record: view(record), message };
    } finally { await rename(`${file}.orig`, file); }
  },
  async recordsDirReadOnly(c) {
    const { vision } = await isolatedStorage(c.call, c.manifest, identifier);
    const dir = await isolatedDirectory(vision, "records-offline");
    const user = await whoami();
    await icacls(dir, "/deny", `${user}:(OI)(CI)(W)`);
    try {
      const { record, message } = await c.runOne("normal");
      const detail = await c.call("history_detail", { id: record.id });
      const view = await c.faultView();
      return { record: { verdict: record.verdict, delivery: record.delivery.state, recording: detail.recording.state, errors: detail.recording.errors.slice(0, 3) }, message, after: view };
    } finally { await icacls(dir, "/remove:d", user); }
  },
};

/** 追溯目录不可写会进入只能重启恢复的故障，单独运行，之后由外层脚本重启实例再验证恢复。 */
const auditScenarios = {
  async auditSpoolReadOnly(c) {
    const { project } = await isolatedStorage(c.call, c.manifest, identifier);
    const dir = await isolatedDirectory(project, "audit-spool");
    const user = await whoami();
    await icacls(dir, "/deny", `${user}:(OI)(CI)(W)`);
    let result;
    try {
      const before = await c.latest();
      await c.start("normal");
      const seen = await until(c.faultView, v => v.phase === "FAULT", "追溯目录不可写后未进入故障", 60000, 200).catch(error => ({ timeout: error.message }));
      await c.stopLine();
      const record = await c.latest();
      result = { seen, newRecord: record?.id !== before?.id ? { verdict: record.verdict, delivery: record.delivery.state } : null };
    } finally { await icacls(dir, "/remove:d", user); }
    await sleep(2000);
    const afterRestore = await c.faultView();
    const readiness = await c.call("cycle_readiness");
    return { ...result, afterRestore, auditReadiness: readiness.items?.find(i => i.key === "audit") ?? null, needsRestart: true };
  },
};

const lateScenarios = {
  async resetWhileInFlight(c) {
    return midPart(c, () => c.call("cycle_reset"));
  },
  async plcResetRequest(c) {
    await until(c.snapshot, s => s?.phase === "IDLE" && s.plcHandshake?.phase === "idle", "复位请求验证前节拍未回到空闲", 30000, 100);
    await c.call("offline_inject", { fault: "disconnect" });
    await until(() => c.call("plc_get_status"), s => s.state === "connected", "断线后未重连", 30000, 200);
    await sleep(4000);
    const beforeRequest = await c.faultView();
    await c.call("offline_fault_reset", { level: true });
    const afterRequest = await until(c.faultView, v => v.phase === "IDLE", "PLC 复位请求未让视觉回到就绪", 15000, 200).catch(error => ({ timeout: error.message }));
    await c.call("offline_fault_reset", { level: false });
    return { beforeRequestPhase: beforeRequest.phase, beforeRequest, afterRequest };
  },
};

async function inject(c, name, scenario, recover) {
  process.stdout.write(`[faults] ${name}\n`);
  const startedAt = Date.now();
  let outcome;
  try { outcome = await scenario(c); } catch (error) { outcome = { error: error.message }; }
  await c.call("offline_inject", { fault: "clear" }).catch(() => {});
  await c.call("offline_inject", { fault: "heartbeatResume" }).catch(() => {});
  await c.call("offline_fault_reset", { level: false }).catch(() => {});
  await c.stopLine().catch(() => {});
  let recovery = null;
  if (recover) {
    try { recovery = await c.recover(); } catch (error) { recovery = { error: error.message, view: await c.faultView() }; }
  }
  await c.logs.poll();
  const window = c.logs.since(startedAt);
  const logs = window.filter(l => ["err", "warn", "ng"].includes(l.level)).map(l => `${l.level} ${l.ev}：${l.msg}`).slice(0, 25);
  const falseOk = Boolean(outcome?.record) && !MAY_PASS_BURST.has(name) && OK_CLASS.has(outcome.record.verdict);
  return { ...outcome, recovery, falseOk, recovered: recovery ? OK_CLASS.has(recovery.verdict) : null, logLines: window.length, logs };
}

/** 依次注入：每类注入后复位并跑正常件，确认恢复生产。`audit` 组最后单独跑，进入需要重启的故障。 */
export async function runFaults(session, out, group, only) {
  const setup = await readSetup(out);
  const c = context(session, out, setup.model ?? "burst");
  const entry = await ensureOffline(c.call, out);
  await c.call("offline_pace", { pace: { shotIntervalMs: 400, pauseMs: 200 } });
  if (group === "recover") return { afterRestart: await c.recover() };
  const results = entry.manualReconnect ? { entryManualReconnect: true } : {};
  if (group === "run") {
    results.baseline = await c.recover();
    assert(OK_CLASS.has(results.baseline.verdict), `故障注入前正常件不是合格：${JSON.stringify(results.baseline)}`);
  }
  const selected = group === "audit" ? auditScenarios : { ...scenarios, ...lateScenarios };
  for (const [name, scenario] of Object.entries(selected)) {
    if (only && !only.includes(name)) continue;
    results[name] = await inject(c, name, scenario, group !== "audit");
  }
  return results;
}

/** 逐项判定：错误的 OK、场景出错或超时、该进故障没进（或反之）、复位沿失效、恢复不合格都是失败；
 *  PC 主动断开后经接口补连接（报告剩余风险 2）单列为已知问题，不算通过。 */
export function assess(results, model = "burst") {
  const failures = [], knownIssues = [];
  const manual = (name, entry) => entry?.manualReconnect && knownIssues.push(`${name}：PC 已主动断开 PLC 且未自动重连，经接口补连接（剩余风险 2）`);
  if (results.entryManualReconnect) knownIssues.push("进入注入前：PC 已主动断开 PLC 且未自动重连，经接口补连接（剩余风险 2）");
  for (const [name, r] of Object.entries(results)) {
    if (!r || typeof r !== "object") continue;
    const fail = what => failures.push(`${name}：${what}`);
    if (RECOVER_ENTRIES.has(name)) {
      if (!OK_CLASS.has(r.verdict)) fail(`投产未恢复合格（${r.verdict}）`);
      manual(name, r);
      continue;
    }
    if (r.error) fail(`场景出错：${r.error}`);
    if (r.seen?.timeout) fail(r.seen.timeout);
    else if (r.seen && (r.seen.phase === "FAULT") !== (r.expectFault ?? true)) fail(r.expectFault === false ? "不该进入故障却进入了故障" : `未进入故障（${r.seen.phase}）`);
    if ("record" in r && !r.record) fail("在途件未生成记录");
    if (r.heldHigh && r.heldHigh.phase !== "FAULT") fail(`复位输入一直为高电平时已离开故障（${r.heldHigh.phase}），复位沿判定回退`);
    if (r.afterEdge?.timeout) fail(r.afterEdge.timeout);
    if (r.reconnectAfterMs != null && r.reconnectAfterMs < r.outageMs) fail(`停机 ${r.outageMs} ms 未到就已重连（${r.reconnectAfterMs} ms）`);
    if (r.falseOk) fail(`错误的 OK（${r.record.verdict}）`);
    if (EXPECT_INCOMPLETE.has(name) && r.record && !r.record.flags?.incomplete) fail("缺帧或坏帧的件应标“检测不完整”（规格 §0），没有标");
    if (name === "pulseJitter" && r.record) {
      // 首尾各 ±1 个脉冲，总数最多偏离名义 2 个；长稳用样本集标定，Σtol 只有 1，帧组数可以落到 ΣN_exp − Σtol 之下，
      // 此时标疑似提前终止是规格要求的告警（E1），不是缺陷。所以只核对两件事：脉冲数只改帧数、不改 OK/NG（不能出 ERR 或 NG）；
      // 帧组数没有低于下限时，不该标疑似提前终止。
      if (!OK_CLASS.has(r.record.verdict)) fail(`脉冲数波动不该改变结论（${r.record.verdict}）`);
      const count = r.count;
      if (!count) fail("没有读到本件的帧组数，无法核对疑似提前终止标记");
      else if (count.groupCount >= count.expectedTotal - count.toleranceTotal && r.record.flags?.shortPart) {
        fail(`帧组数 ${count.groupCount} 没有低于下限 ${count.expectedTotal - count.toleranceTotal}，不应标疑似提前终止`);
      }
    }
    if (r.recovery?.error) fail(`恢复出错：${r.recovery.error}`);
    else if (r.recovered === false) fail(`复位后 3 件内未恢复合格（${r.recovery.verdict}）`);
    manual(name, r.recovery);
  }
  return { failures, knownIssues };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  assert(["run", "audit", "recover"].includes(args.command), "命令为 run、audit 或 recover");
  const out = path.resolve(args.out ?? "output/soak");
  const session = await connect(args.cdp ?? "http://[::1]:9347", args.instance ?? "output/playwright/soak-regression/instance.json");
  try {
    const results = await runFaults(session, out, args.command, args.only?.split(","));
    const { failures, knownIssues } = assess(results, (await readSetup(out)).model ?? "burst");
    await writeJson(path.join(out, args.file ?? `faults-${args.command}.json`), { failures, knownIssues, results });
    const summary = Object.fromEntries(Object.entries(results).map(([k, v]) => [k, v.error ? `ERROR ${v.error}` : { record: v.record?.verdict, seen: v.seen?.phase, falseOk: v.falseOk, recovered: v.recovered ?? OK_CLASS.has(v.verdict) }]));
    process.stdout.write(JSON.stringify({ summary, failures, knownIssues }, null, 1) + "\n");
    if (failures.length) process.exitCode = 1;
    else if (knownIssues.length) process.exitCode = 2;
  } finally { await session.browser.close(); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
