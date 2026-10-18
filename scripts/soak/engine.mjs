import path from "node:path";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import { connect, parseArgs, until, writeJson } from "./lib.mjs";

/** lyFlow 运行库缺失（外层脚本指向空目录启动）：引擎未就绪、样本集离线被拒绝，提示能看懂，不产生记录。 */
async function missing({ call }, out) {
  const engine = await until(() => call("engine_status"), e => e && !e.ready, "运行库缺失时引擎仍报告就绪", 30000, 500);
  const setup = JSON.parse(await readFile(path.join(out, "setup.json"), "utf8"));
  const last = (await call("site_status")).last;
  const dir = last?.kind === "sampleSet" && last.sampleSetDir ? last.sampleSetDir : setup.sampleSetCopy;
  let entered;
  try { entered = { ok: await call("site_enter_offline", { source: { kind: "sampleSet", dir } }) }; }
  catch (error) { entered = { error: error.message }; }
  const readiness = await call("cycle_readiness");
  const result = {
    engine: { ready: engine.ready, measuring: engine.measuring, message: engine.message },
    enterOffline: entered.error ?? `意外进入：${entered.ok?.mode}`,
    engineItem: readiness.items.find(i => i.key === "engine"),
    passed: !engine.ready && Boolean(entered.error),
  };
  if (entered.ok) await call("site_leave_offline").catch(() => {});
  await writeJson(path.join(out, "engine-missing.json"), result);
  process.stdout.write(JSON.stringify(result) + "\n");
  if (!result.passed) process.exitCode = 1;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const out = path.resolve(args.out ?? "output/soak");
  const session = await connect(args.cdp ?? "http://[::1]:9347", args.instance ?? "output/playwright/soak-regression/instance.json");
  try {
    if (args.command === "missing") await missing(session, out);
    else throw new Error(`未知命令 ${args.command}`);
  } finally { await session.browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
