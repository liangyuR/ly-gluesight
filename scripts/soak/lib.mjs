import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { mkdir, readdir, readFile, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import { buildBurstRecipe, checksOff, client, copySampleSet, until } from "../../tests/native/lib.mjs";

export { until };
export const identifier = "com.xyzrobotics.tujiaovision.soak-tests";
export const RECIPE_ID = "SOAK-1";
export const SAMPLE_PART = "p01";
// 模拟标定：样本集原件 mmPerPx=null。取 docs/architecture/p0-plan.md 的假设当量 0.112 mm/px，
// 在该当量下 S019 胶宽实测约 3.9 mm，与模板胶宽名义 4 mm 吻合；理由见 docs/decisions/soak.md。
export const SIMULATED_MM_PER_PX = 0.112;
const run = promisify(execFile);

export function parseArgs(argv) {
  const [command, ...rest] = argv;
  const args = { command };
  for (let i = 0; i < rest.length; i += 2) {
    assert(rest[i].startsWith("--") && rest[i + 1] !== undefined, "参数必须为 --name value");
    args[rest[i].slice(2)] = rest[i + 1];
  }
  return args;
}

export async function connect(cdp, instancePath) {
  const url = new URL(cdp);
  assert(["127.0.0.1", "localhost", "[::1]"].includes(url.hostname), "仅允许本机 CDP");
  const manifest = JSON.parse((await readFile(instancePath, "utf8")).replace(/^\uFEFF/, ""));
  assert.equal(manifest.identifier, identifier, "仅允许长稳隔离实例");
  assert.equal(Number(url.port), manifest.debugPort);
  let browser;
  for (const host of [url.hostname, "[::1]", "127.0.0.1"]) {
    try { browser = await chromium.connectOverCDP(`http://${host}:${url.port}`); break; } catch (error) { if (host === "127.0.0.1") throw error; }
  }
  const main = () => browser.contexts().flatMap(c => c.pages()).find(p => /tauri\.localhost|localhost:1420|^tauri:\/\//.test(p.url()) && !p.url().includes("storage-startup"));
  const page = await until(async () => main() ?? null, Boolean, "未找到 Tauri 主窗口页面（资料准备未完成？）", 90000, 500);
  page.setDefaultTimeout(30000);
  await page.waitForFunction(() => !!window.__TAURI_INTERNALS__);
  const call = client(page);
  const records = await call("records_list");
  assert(records.root.includes(identifier), `数据目录不属于长稳隔离实例：${records.root}`);
  return { browser, page, call, manifest };
}

export const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

/** setup 写下的 setup.json：建站用的模型（burst）与样本集副本；没有时按段配方。 */
export async function readSetup(out) {
  try { return JSON.parse(await readFile(path.join(out, "setup.json"), "utf8")); } catch { return { model: "burst" }; }
}

export async function prepareSampleSet(source, outputDir) {
  return (await copySampleSet(source, outputDir, manifest => ({ ...manifest, mmPerPx: SIMULATED_MM_PER_PX }))).dir;
}

/**
 * 段配方长稳建站：进入离线·样本集，走廊示教 → 10 轮标定 → 独立样本验证 → 发布（检测项全关，见文件头）。
 * 样本集 id 取夹具（tests/native/lib.mjs 的 CAMERA_FIXTURES）；已发布时直接复用。
 */
export async function setupRecipe(call, sampleSetDir, log = console.log) {
  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir: sampleSetDir } });
  assert.equal(entered.mode, "offline");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const cameraIds = (await call("camera_rig_config")).map(c => c.id);
  const existing = (await call("cycle_recipes")).find(r => r.id === RECIPE_ID);
  if (existing) { log(`配方 ${RECIPE_ID} 已发布，跳过建配方`); return { model: "burst", recipeId: RECIPE_ID, cameraIds, reused: true }; }
  const manifest = JSON.parse((await readFile(path.join(sampleSetDir, "sample-set.json"), "utf8")).replace(/^\uFEFF/, "").trim());
  const template = await call("recipe_template");
  const built = await buildBurstRecipe(call, { sourceId: manifest.id, partId: SAMPLE_PART, doc: { ...template, id: RECIPE_ID, name: "长稳测试" }, patchSegments: checksOff, log });
  assert(built.validation.passed, `验证未通过：${JSON.stringify(built.validation.checks.filter(c => !c.passed))} ${JSON.stringify(built.validation.samples)}`);
  await until(async () => (await call("cycle_readiness")).recipes.find(r => r.id === built.id)?.runnable === true, Boolean, `配方 ${built.id} 预热后仍不可运行`, 90000, 500);
  log(`配方 ${built.id} 已发布：ΣN_exp ${built.calibration.expectedTotal} ± ${built.calibration.toleranceTotal}，示教 ${built.taught.length} 台相机的走廊`);
  return { model: "burst", recipeId: built.id, cameraIds, taught: built.taught, expectedTotal: built.calibration.expectedTotal,
    toleranceTotal: built.calibration.toleranceTotal, validation: built.validation.samples };
}

/** 回到离线·样本集并等长稳配方可运行：重启后站点模式不持久化。PC 主动断开 PLC 后不会自动重连，
 *  有未结事务时也退不出离线，离线 PLC 页又是只读，这里经接口补一次连接（相当于真机在 PLC 页点“连接”），并如实返回。 */
export async function ensureOffline(call, out) {
  let manualReconnect = false;
  if ((await call("site_status")).mode === "offline") {
    const settled = await until(() => call("plc_get_status"), s => s.state === "connected" || s.state === "disconnected", "PLC 连接状态未稳定", 30000, 500).catch(() => null);
    if (settled?.state === "disconnected") {
      await sleep(10000);
      if ((await call("plc_get_status")).state === "disconnected") { await call("plc_connect"); manualReconnect = true; }
    }
  }
  const site = await call("site_status");
  if (site.mode !== "offline") {
    const setup = JSON.parse(await readFile(path.join(out, "setup.json"), "utf8"));
    const dir = site.last?.kind === "sampleSet" && site.last.sampleSetDir ? site.last.sampleSetDir : setup.sampleSetCopy;
    try { await call("site_enter_offline", { source: { kind: "sampleSet", dir } }); } catch (error) {
      if (!/在途事务/.test(error.message)) throw error;
      throw new Error(`已知问题（见 docs/testing/soak-report.md 剩余风险 1）：重启前留下的离线未结事务阻止回到离线：${error.message}`);
    }
  }
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  await until(async () => (await call("cycle_readiness")).recipes.find(r => r.id === RECIPE_ID)?.runnable === true, Boolean, `配方 ${RECIPE_ID} 不可运行`, 90000, 500);
  return { manualReconnect };
}

/** 主进程及其子进程（WebView2）的内存、句柄、线程。 */
export async function processMetrics(pid) {
  const script = `$all = @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId); $ids = @(${Number(pid)});
do { $new = @($all | Where-Object { $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId } | ForEach-Object ProcessId); $ids += $new } while ($new.Count)
$rows = @(Get-Process -Id $ids -ErrorAction SilentlyContinue | ForEach-Object { [pscustomobject]@{ id = $_.Id; name = $_.ProcessName; ws = $_.WorkingSet64; priv = $_.PrivateMemorySize64; handles = $_.HandleCount; threads = $_.Threads.Count; cpu = $_.TotalProcessorTime.TotalSeconds } })
[pscustomobject]@{ load = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average; rows = $rows } | ConvertTo-Json -Compress -Depth 3`;
  const { stdout } = await run("pwsh", ["-NoProfile", "-NonInteractive", "-Command", script], { windowsHide: true, maxBuffer: 1 << 20 });
  const parsed = JSON.parse(stdout.trim() || "{}");
  const rows = [parsed.rows ?? []].flat();
  const main = rows.find(r => r.id === Number(pid));
  const children = rows.filter(r => r.id !== Number(pid));
  const sum = key => children.reduce((total, r) => total + (r[key] ?? 0), 0);
  return {
    systemLoad: parsed.load ?? null,
    main: main && { ws: main.ws, priv: main.priv, handles: main.handles, threads: main.threads, cpu: main.cpu },
    children: { count: children.length, ws: sum("ws"), priv: sum("priv"), handles: sum("handles"), threads: sum("threads") },
  };
}

async function treeSize(dir) {
  let bytes = 0, files = 0;
  let entries;
  try { entries = await readdir(dir, { withFileTypes: true }); } catch { return { bytes, files }; }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) { const sub = await treeSize(full); bytes += sub.bytes; files += sub.files; }
    else if (entry.isFile()) { try { bytes += (await stat(full)).size; files += 1; } catch { } }
  }
  return { bytes, files };
}

/** 当前项目目录：资料目录下 projects/<config/current-project.json 记的编号>。 */
export function projectRoot(dataRoot) {
  const pointer = JSON.parse(readFileSync(path.join(dataRoot, "config", "current-project.json"), "utf8"));
  return path.join(dataRoot, "projects", pointer.current);
}

/** 资料目录各部分占用：原图、记录库、日志、握手状态等。项目资料取当前项目。 */
export async function diskUsage(dataRoot) {
  const result = {};
  const project = projectRoot(dataRoot);
  for (const name of ["records-offline", "records", "audit-spool", "recipe-capture", "workspaces", "vision"]) {
    result[name] = await treeSize(path.join(project, name));
  }
  for (const name of ["logs", "state", "samples", "cache"]) {
    result[name] = await treeSize(path.join(dataRoot, name));
  }
  let db = 0;
  for (const suffix of ["", "-wal", "-shm"]) {
    const file = path.join(project, `inspection.db${suffix}`);
    if (existsSync(file)) db += (await stat(file)).size;
  }
  result["inspection.db"] = { bytes: db, files: 3 };
  result.total = { bytes: Object.values(result).reduce((t, v) => t + v.bytes, 0) };
  return result;
}

/** 分页读出离线范围的全部检测记录。 */
export async function offlineHistory(call, recipeId) {
  const items = [];
  for (let offset = 0; ; offset += 200) {
    const page = await call("history_query", { query: { recipeId, scope: "offline", offset, limit: 200 } });
    items.push(...page.items);
    if (page.items.length < 200 || items.length >= page.total) return { total: page.total, counts: page.counts, items };
  }
}

export function percentiles(values) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const at = q => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return { n: sorted.length, min: sorted[0], p50: at(0.5), p90: at(0.9), p99: at(0.99), max: sorted.at(-1),
    mean: Math.round(sorted.reduce((a, b) => a + b, 0) / sorted.length) };
}

/** 持续收集节拍日志（内存只保留最近 300 行），按时间戳与内容去重。 */
export class LogCollector {
  constructor(call) { this.call = call; this.seen = new Set(); this.lines = []; }
  async poll() {
    for (const line of await this.call("cycle_logs")) {
      const key = `${line.ts}|${line.ev}|${line.msg}`;
      if (this.seen.has(key)) continue;
      this.seen.add(key);
      this.lines.push(line);
    }
  }
  since(ts) { return this.lines.filter(l => l.ts >= ts); }
  counts(filter = () => true) {
    const counts = {};
    for (const line of this.lines.filter(filter)) counts[`${line.level} ${line.ev}`] = (counts[`${line.level} ${line.ev}`] ?? 0) + 1;
    return counts;
  }
}


export async function writeJson(file, value) {
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, JSON.stringify(value, null, 2) + "\n");
}
