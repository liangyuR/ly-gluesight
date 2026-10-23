import { readFile, mkdir, writeFile, copyFile } from "node:fs/promises";
import path from "node:path";
import { test, options } from "./fixtures.mjs";
import { runProductionFlow } from "../native/production-flow-ui.mjs";

async function mergeResult(output, result) {
  const destination = path.join(output, "result.json");
  let previous = {};
  try {
    previous = JSON.parse(await readFile(destination, "utf8"));
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const { legacy, burst, ...metadata } = result;
  const suite = result.failedSuite ?? result.suite;
  const merged = {
    ...previous,
    ...metadata,
    passed: !previous.failedSuite && result.passed,
    stages: {
      ...previous.stages,
      ...(suite ? { [suite]: { passed: result.passed, ...(result.error ? { error: result.error } : {}) } } : {}),
    },
    ...(legacy ? { legacy, summary: legacy.summary, disk: legacy.disk } : {}),
    ...(burst ? { burst } : {}),
    ...(options.productionSuite === "legacy" ? { burst: { skipped: "仅选择旧配方" } }
      : !options.lyflow ? { burst: { skipped: "未提供 lyFlow 运行库" } } : {}),
  };
  await mkdir(output, { recursive: true });
  await writeFile(destination, JSON.stringify(merged, null, 2) + "\n");
}

async function runStage(desktop, suite) {
  try {
    const result = await test.step(`${suite}：离线生产、落盘读回与配置恢复`, () => runProductionFlow(desktop.page, {
      instance: desktop.manifest,
      suite,
      lyflow: options.lyflow,
      output: path.join(desktop.output, "stages", suite, "result.json"),
    }));
    await mergeResult(desktop.output, result);
    await copyFile(path.join(desktop.output, "stages", suite, "production-flow.png"), path.join(desktop.output, "production-flow.png")).catch(error => {
      process.stderr.write(`生产截图复制失败：${error}\n`);
    });
  } catch (error) {
    await mergeResult(desktop.output, { passed: false, failedSuite: suite, error: String(error) }).catch(reportError => {
      process.stderr.write(`生产结果写入失败：${reportError}\n`);
    });
    throw error;
  }
}

test.describe("生产落盘 legacy", () => {
  test.skip(options.productionSuite === "burst", "仅选择段配方");
  test("旧配方生产 → SQLite、原图、历史读回及定位告警一致", async ({ desktop }) => {
    await runStage(desktop, "legacy");
  });
});

test.describe("生产落盘 burst", () => {
  test.skip(options.productionSuite === "legacy", "仅选择旧配方");
  test.skip(!options.lyflow, "未提供 lyFlow 运行库：段配方建站与标定需要真实引擎");
  test("段配方生产 → 各相机帧、原图、元数据与历史结果一致", async ({ desktop }) => {
    await runStage(desktop, "burst");
  });
});
