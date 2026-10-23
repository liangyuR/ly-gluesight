import { chromium } from "playwright-core";
import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";
import {
  buildBurstRecipe, captureRound, CAMERA_FIXTURES, checksOff, client, copySampleSet, gateRetries, identifier, instanceCdp, MISSING_INPUT_EXIT, missingInputs, OK_CLASS,
  produceBurst, uiPage, until, workspaceCalls,
} from "./lib.mjs";

const verdictLabel = {
  OK: "OK", OK_WITH_EXCURSION: "OK · 局部超差", NG_POSITION: "NG · 位置超差", NG_WIDTH: "NG · 胶宽超差",
  NG_ABSOLUTE: "NG · 超绝对限", NG_GAP: "NG · 断胶", ERR_INSPECT: "ERR · 未测成",
};

/** 导出历史 CSV 并核对行数；burst 传入段配方记录的事实时，还核对末尾 ΣN_exp、Σtol、各相机帧数、deviceTriggers、标记五列。 */
async function exportHistory(page, call, recipeId, { burst = [] } = {}) {
  await page.getByRole("button", { name: "导出 CSV", exact: true }).click();
  const notice = page.getByText(/^已导出：/);
  await notice.waitFor();
  const file = (await notice.locator(".mono").innerText()).trim();
  const lines = (await readFile(file, "utf8")).trim().split(/\r?\n/);
  const { total } = await call("history_query", { query: { recipeId, scope: "offline", limit: 1 } });
  assert.equal(lines.length - 1, total, `导出 CSV 行数与历史记录不符：${file}`);
  if (burst.length) {
    assert(lines[0].endsWith("预期帧组数(ΣN_exp),帧组容差(Σtol),各相机帧数,deviceTriggers,标记"), `导出 CSV 表头缺段配方列：${lines[0]}`);
    for (const f of burst) {
      const tail = [f.expectedTotal, f.toleranceTotal, f.cameraFrames.join("/"), f.deviceTriggers].join(",");
      const labels = Object.entries({ incomplete: "检测不完整", frameCountAnomaly: "帧数异常", offsetUnjudged: "胶偏未判", unsegmented: "含未分段区域", shortPart: "疑似提前终止" })
        .filter(([flag]) => f.flags[flag]).map(([, label]) => label).join("、");
      assert(lines.slice(1).some(line => line.endsWith(`,${tail},"${labels}"`)), `导出 CSV 没有找到段配方记录行（…,${tail},"${labels}"）`);
    }
  }
  return { file, rows: lines.length - 1 };
}

const flagLabel = { incomplete: "检测不完整", frameCountAnomaly: "帧数异常", offsetUnjudged: "胶偏未判", unsegmented: "含未分段", shortPart: "疑似提前终止" };
const progress = message => process.stderr.write(`· ${message}\n`);

/**
 * 段配方记录的整件事实对照预期：帧组数、缺帧、标记（缺帧与帧数越界只告警，不改 OK/NG）。
 * expected.lost = 工况注入的缺帧 [计划相机下标, seq]；除此之外只允许最后两个帧组里的帧缺失——收尾在每台相机 max_seq ≥ deviceTriggers − 1
 * 时就判（容许一帧在途，规格 §3 丢帧 ≤ 1 视为正常），合成图逐像素渲染赶不上脉冲节拍时，末尾的帧晚于判定，记为缺帧、该组检测不完整、帧组数比脉冲数少。
 * expected.jittered = 本件脉冲数有波动（pulseJitter）：首尾各 ±1，总数最多偏离名义 2 个，是否越出 ± Σtol 取决于标定轮次碰巧观察到的范围，
 * 所以不要求落在容差内，只核对标记与帧组数一致（越界只告警、不改 OK/NG）；没有波动的件仍要求落在容差内、不告警。
 */
function assertBurstPart(name, facts, { expectedTotal, toleranceTotal }, expected) {
  const lost = expected.lost ?? [];
  const where = `${name}：${JSON.stringify({ verdict: facts.verdict, reason: facts.reason, groupCount: facts.groupCount, cameraFrames: facts.cameraFrames, missing: facts.missing, flags: facts.flags })}`;
  assert.equal(facts.expectedTotal, expectedTotal, `${name} 记录的 ΣN_exp 与发布的不一致`);
  assert.equal(facts.toleranceTotal, toleranceTotal, `${name} 记录的 Σtol 与发布的不一致`);
  assert.equal(facts.framesRecorded, facts.groupCount * facts.cameraFrames.length, `${name} 每相机每帧都应有一条记录（缺帧也有）：${where}`);
  const lower = expectedTotal - toleranceTotal, outside = facts.groupCount < lower || facts.groupCount > expectedTotal + toleranceTotal;
  if (expected.jittered) {
    assert.equal(facts.flags.frameCountAnomaly, outside, `${name} 帧数异常标记应与帧组数是否越出 ${expectedTotal} ± ${toleranceTotal} 一致：${where}`);
    assert.equal(facts.flags.shortPart, facts.groupCount < lower, `${name} 疑似提前终止标记应与帧组数是否低于 ${lower} 一致：${where}`);
  } else {
    assert(!outside, `${name} 帧组数 ${facts.groupCount} 超出 ${expectedTotal} ± ${toleranceTotal}：${where}`);
    assert.equal(facts.flags.frameCountAnomaly, false, `${name} 帧数异常标记不符：${where}`);
    assert.equal(facts.flags.shortPart, false, `${name} 不应标疑似提前终止：${where}`);
  }
  const same = (a, b) => a[0] === b[0] && a[1] === b[1];
  for (const frame of lost) assert(facts.missing.some(m => same(m, frame)), `${name} 注入的缺帧 ${frame} 没有记为缺帧：${where}`);
  const extra = facts.missing.filter(m => !lost.some(frame => same(frame, m)));
  assert(extra.every(([, seq]) => seq >= facts.groupCount - 1), `${name} 除注入的缺帧外只允许最后两个帧组的帧缺失：${where}`);
  assert.equal(facts.flags.incomplete, facts.missing.length > 0, `${name} 检测不完整标记应与缺帧一致：${where}`);
  assert.deepEqual(facts.incompleteGroups, [...new Set(facts.missing.map(([, seq]) => seq))].sort((a, b) => a - b), `${name} 不完整帧组应恰为有缺帧的帧组：${where}`);
  assert(facts.flags.unsegmented && facts.flags.offsetUnjudged, `${name} 本轮没有分段算法，整条胶路应按未分段处理并标胶偏未判：${where}`);
  assert.deepEqual(facts.unchecked, ["bubble", "stringing"], `${name} 气泡、拉丝本轮不检，应标未检项`);
  assert([0, 1].includes(facts.deviceTriggers - facts.groupCount), `${name} deviceTriggers 是 PLC 发出的脉冲数，帧组数至多比它少 1（末帧晚于判定）：${where}`);
}

/** 合成图主流程：三台相机 → 走廊 → 10 轮标定 → 验证 → 发布 → 多个工况投产 → 历史界面与导出。 */
async function syntheticFlow(page, call, navigation, options, step) {
  const entered = await call("site_enter_offline", { source: { kind: "synthetic" } });
  assert.equal(entered.mode, "offline");
  assert.equal(entered.source, "synthetic");
  // 功能验收用 5 Hz 脉冲：合成图每帧逐像素渲染，实测本机三台相机合计只能出约 20 帧/s；脉冲再快，帧会晚于 partEnd 才画完，
  // 末尾几帧记缺帧、帧组数比脉冲数少。吞吐门槛（3 × 25 Hz）另见 scripts/soak/throughput.mjs，不在功能验收里测
  await call("offline_pace", { pace: { shotIntervalMs: 200, pauseMs: 200 } });
  assert.equal(entered.measurement, "simulated", "合成图来源用模拟测量；标定与验证里的每帧测量才用 lyFlow");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("heading", { name: "当前：离线 · 合成图", exact: true }).waitFor();
  step("进入离线·合成图", { port: entered.port });

  const built = await buildBurstRecipe(call, { sourceId: "synthetic", log: progress });
  const { id, calibration, validation } = built;
  assert(validation.passed, `验证未通过：${JSON.stringify(validation.checks.filter(c => !c.passed))} ${JSON.stringify(validation.samples)}`);
  const plan = (await built.ws.get()).plan;
  const counts = calibration.rounds.filter(r => !r.standard).map(r => r.groupCount);
  const [min, max] = [Math.min(...counts), Math.max(...counts)];
  assert.equal(counts.length, 10, "多轮标定应加入 10 轮（不含标准件轮次）");
  assert(min < max, `合成图每轮首尾与弯道帧数有波动，10 轮标定应看到 min / max 差异：${counts}`);
  const [segment] = plan.segments;
  assert.equal(plan.segments.length, 1, "合成直线单段");
  assert.equal(segment.kind, "straight");
  assert.equal(segment.expectedFrames, calibration.expectedTotal, "单段计划的 N_exp 应等于整件标定的 ΣN_exp");
  assert.equal(calibration.expectedTotal, Math.round(counts.reduce((a, b) => a + b, 0) / counts.length), "ΣN_exp 应为各轮帧组数均值四舍五入");
  assert.deepEqual([segment.calib.rounds, segment.calib.min, segment.calib.max], [10, min, max], "段标定统计应只含 10 轮标定轮次");
  const suggested = calibration.segments[0].suggestedTolerance;
  assert.equal(suggested, Math.max(1, calibration.expectedTotal - min, max - calibration.expectedTotal), "建议容差 = max(默认容差, N − min, max − N)");
  assert(suggested >= 1 && segment.tolerance === suggested, `操作员确认建议容差后应写入配方：建议 ${suggested}，配方 ${segment.tolerance}`);
  assert.equal(plan.toleranceTotal, suggested);
  assert(segment.offsetRef?.xRef !== null && segment.offsetRef?.xRef !== undefined, "标准件轮次应得出胶偏参考 x_ref");
  assert(plan.calibrated && plan.expectedTotal === calibration.expectedTotal);
  assert.equal(validation.samples.length, 1);
  assert(OK_CLASS.includes(validation.samples[0].actual) && validation.samples[0].passed, `独立正常样本应符合：${JSON.stringify(validation.samples[0])}`);
  step("建站、10 轮标定、验证、发布", {
    recipeId: id, taught: built.taught, groupCounts: counts, min, max, expectedTotal: calibration.expectedTotal, toleranceTotal: plan.toleranceTotal,
    suggestedTolerance: suggested, xRef: segment.offsetRef, validation: validation.checks.map(c => `${c.name}${c.passed ? "√" : "×"}`),
  });

  const active = built.active;
  assert.equal(active.workspace.publish.state, "active");
  assert.equal(active.workspace.publish.validationSite, "offline", "离线发布的版本必须标记离线验证");
  const layout = await call("cycle_layout", { recipeId: id });
  const exported = JSON.parse(await call("recipe_plan_export", { id }));
  assert.equal(exported.expectedTotal, calibration.expectedTotal, "导出给 PLC 的 shotCount 应等于 ΣN_exp");
  assert.equal(exported.protocolVersion, 6);
  assert.equal(exported.toleranceTotal, plan.toleranceTotal);
  const runSelect = page.locator('select[aria-label="离线配方"]');
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  // 合成图下首启样例配方（旧拍照点配方）也能运行，所以新配方不会被默认选中：只核对它出现在可选配方里，再手动选上
  await until(() => runSelect.locator("option").evaluateAll(options => options.map(o => o.value)), values => values.includes(id), "发布后离线配方下拉里没有新配方", 15000);
  await runSelect.selectOption(id);
  await until(() => runSelect.inputValue(), value => value === id, "没有选中新配方", 15000);
  step("发布生效", { recipeId: id, planVersion: exported.planVersion, publish: active.workspace.publish });

  const totals = { expectedTotal: calibration.expectedTotal, toleranceTotal: plan.toleranceTotal };
  const produced = {};
  produced.normal = await produceBurst(call, id, { scenario: "normal" });
  assert(OK_CLASS.includes(produced.normal.summary.verdict), `正常件应为合格类：${produced.normal.summary.verdict} ${produced.normal.summary.reason}`);
  assertBurstPart("正常件", produced.normal.facts, totals, {});
  produced.cameraLost = await produceBurst(call, id, { scenario: "cameraLost" });
  assert(OK_CLASS.includes(produced.cameraLost.summary.verdict), `单台相机缺帧不影响 OK/NG：${produced.cameraLost.summary.verdict} ${produced.cameraLost.summary.reason}`);
  assertBurstPart("单台相机缺帧", produced.cameraLost.facts, totals, { lost: [[1, Math.min(3, produced.cameraLost.facts.deviceTriggers)]] });
  produced.pulseJitter = await produceBurst(call, id, { scenario: "pulseJitter" });
  assert(OK_CLASS.includes(produced.pulseJitter.summary.verdict), `首尾 ±1 个脉冲的波动只改帧数、不改 OK/NG：${produced.pulseJitter.summary.verdict} ${produced.pulseJitter.summary.reason}`);
  assertBurstPart("脉冲数波动", produced.pulseJitter.facts, totals, { jittered: true });
  produced.gap = await produceBurst(call, id, { scenario: "gap" });
  assert.equal(produced.gap.summary.verdict, "NG_GAP", `断胶工况应判 NG_GAP：${produced.gap.summary.reason}`);
  assert.equal(produced.gap.summary.plcCode, 13);
  assertBurstPart("断胶", produced.gap.facts, totals, {});
  assert.equal((await call("history_query", { query: { recipeId: id, scope: "offline", limit: 1 } })).total, Object.keys(produced).length,
    "标定、示教与验证的整圈采集不得生成检测记录");
  assert.equal((await call("history_query", { query: { limit: 1 } })).total, 0, "离线件不得进入真机历史与统计");
  step("投产", Object.fromEntries(Object.entries(produced).map(([name, { facts }]) => [name, { verdict: facts.verdict, groupCount: facts.groupCount, cameraFrames: facts.cameraFrames, incompleteGroups: facts.incompleteGroups, flags: facts.flags }])));

  await navigation.getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("group", { name: "记录来源", exact: true }).getByRole("button", { name: "离线件", exact: true }).click();
  await page.getByRole("combobox", { name: "历史配方", exact: true }).click();
  await page.getByRole("option", { name: id, exact: true }).click();
  for (const { summary, facts } of Object.values(produced)) {
    const row = page.getByRole("row").filter({ has: page.getByRole("button", { name: `查看 SN ${summary.sn} 的记录`, exact: true }) });
    await row.getByText(verdictLabel[summary.verdict], { exact: true }).waitFor();
    await row.getByText(`${facts.groupCount}/${facts.expectedTotal}`, { exact: true }).waitFor();
    for (const [flag, on] of Object.entries(facts.flags)) {
      if (on) await row.getByText(flagLabel[flag], { exact: true }).waitFor();
      else assert.equal(await row.getByText(flagLabel[flag], { exact: true }).count(), 0, `${summary.sn} 不应标“${flagLabel[flag]}”`);
    }
  }
  const exportedCsv = await exportHistory(page, call, id, { burst: Object.values(produced).map(p => p.facts) });
  const lost = produced.cameraLost;
  await page.getByRole("button", { name: `查看 SN ${lost.summary.sn} 的记录`, exact: true }).click();
  await page.getByRole("heading", { name: "帧组与原图", exact: true }).waitFor();
  await page.getByText(`cam1 ${lost.facts.cameraFrames[0]} · cam2 ${lost.facts.cameraFrames[1]} · cam3 ${lost.facts.cameraFrames[2]}`, { exact: true }).first().waitFor();
  const detailText = await page.locator("main").innerText();
  assert(detailText.includes("检测不完整") && detailText.includes("未检项（本轮没有算法）：气泡、拉丝"), "历史详情应标检测不完整与未检项");
  step("历史界面与 CSV 导出", { exported: exportedCsv.rows });

  if (options.output) {
    await mkdir(path.dirname(options.output), { recursive: true });
    await page.screenshot({ path: path.join(path.dirname(options.output), "offline-e2e-history.png"), fullPage: true });
  }
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("button", { name: "退出离线", exact: true }).click();
  await until(() => call("site_status"), s => s.mode === "real", "未退出离线模式");
  return {
    recipe: { id, revisionId: layout.revisionId, planVersion: exported.planVersion },
    taught: built.taught,
    calibration: { groupCounts: counts, min, max, expectedTotal: calibration.expectedTotal, toleranceTotal: plan.toleranceTotal, suggestedTolerance: suggested,
      xRef: segment.offsetRef, planVersionChange: calibration.planVersionChange, rounds: calibration.rounds.length },
    validation: { checks: validation.checks.map(c => ({ name: c.name, passed: c.passed })), samples: validation.samples.map(s => ({ expected: s.expected, actual: s.actual, passed: s.passed, reason: s.reason })) },
    production: Object.fromEntries(Object.entries(produced).map(([name, { summary, facts }]) => [name, { historyId: summary.id, sn: summary.sn, cycleId: summary.cycleId, bundleId: summary.bundleId, ...facts }])),
    exported: exportedCsv,
  };
}

/**
 * 样本集冒烟（2026-10-17 停稳拍数据，不是飞拍）：同一套流程走一遍。每台相机一条走廊对不上其他点位，帧里多半找不到胶；
 * 检测项开着只会判 NG_GAP、验证过不了发布，所以建站时把各段检测项全关（checksOff），只验证真 lyFlow 测量下流程不崩、记录自洽、
 * 原包重现与原结论一致，并把帧状态比例（测成、找到胶、缺帧）写进报告；不作门槛。
 */
async function sampleSetSmoke(page, call, navigation, options, step) {
  const { dir, manifest } = await copySampleSet(options.sampleSet, options.workDir, read => ({ ...read, id: `${read.id}-e2e`, mmPerPx: CAMERA_FIXTURES[read.id]?.mmPerPx }));
  const sourceId = manifest.id.replace(/-e2e$/, "");
  assert(CAMERA_FIXTURES[sourceId], `样本集 ${sourceId} 没有段配方夹具，已有：${Object.keys(CAMERA_FIXTURES).join("、")}`);
  const partId = manifest.parts[0].partId;
  const entered = await call("site_enter_offline", { source: { kind: "sampleSet", dir } });
  assert.equal(entered.source, "sampleSet");
  assert.equal(entered.measurement, "lyFlow");
  await until(() => call("plc_get_status"), s => s.state === "connected", "离线 S7 服务未连接");
  const cameras = await call("camera_rig_config");
  assert.deepEqual(cameras.map(c => [c.id, c.source]), manifest.cameras.map(c => [c.id, "sampleSet"]), "离线相机组应按样本集相机生成");
  const built = await buildBurstRecipe(call, { sourceId, partId, patchSegments: checksOff, log: progress });
  const smoke = {
    recipeId: built.id, sampleSetId: manifest.id, partId, taught: built.taught,
    calibration: { groupCounts: built.calibration.rounds.map(r => r.groupCount), expectedTotal: built.calibration.expectedTotal, toleranceTotal: built.calibration.toleranceTotal },
    validation: { passed: built.validation.passed, checks: built.validation.checks.map(c => ({ name: c.name, passed: c.passed, detail: c.passed ? undefined : c.detail })),
      samples: built.validation.samples.map(s => ({ actual: s.actual, passed: s.passed, reason: s.reason })) },
    published: Boolean(built.active),
  };
  if (built.active) {
    const part = await produceBurst(call, built.id, { scenario: "normal", partId });
    const { facts } = part;
    const total = facts.framesRecorded;
    const bad = total - (facts.frameStates.measured ?? 0);
    assert.equal(facts.framesRecorded, facts.groupCount * facts.cameraFrames.length, `每相机每帧都应有一条记录：${JSON.stringify(facts)}`);
    assert.equal(part.summary.sample?.partId, partId, "历史记录应带样本件引用");
    assert.equal(part.summary.sample.setId, manifest.id);
    smoke.production = { historyId: part.summary.id, bundleId: part.summary.bundleId, ...facts,
      incompleteRatio: { frames: total, notMeasured: bad, notMeasuredRatio: total ? bad / total : 0, withoutGlue: total - facts.framesWithGlue,
        withoutGlueRatio: total ? (total - facts.framesWithGlue) / total : 0, groups: facts.groupCount, incompleteGroups: facts.incompleteGroups.length } };
    // 原包重现与规则重判都读本件原图：同一份发布包、同一批原图，结论应与原记录一致
    assert(part.summary.bundleId, "样本集（lyFlow 测量）的生产件应带发布包");
    const replay = {};
    for (const [source, run] of [["original", () => call("workspace_compare_original", { historyId: part.summary.id })]]) {
      const comparison = await run();
      assert(comparison.burst, `${source} 重现结果应带段配方区域判定`);
      assert.equal(comparison.judgement.verdict, part.summary.verdict, `${source} 重现与原结论不一致：${comparison.judgement.reason}`);
      replay[source] = { verdict: comparison.judgement.verdict, regions: comparison.burst.judgement.regions.map(r => r.verdict), reason: comparison.judgement.reason };
    }
    smoke.replay = replay;
    smoke.recipe = { id: built.id, revisionId: (await call("cycle_layout", { recipeId: built.id })).revisionId };
  }
  step("样本集冒烟", smoke);
  await navigation.getByRole("link", { name: "离线模式", exact: true }).click();
  await page.getByRole("button", { name: "退出离线", exact: true }).click();
  await until(() => call("site_status"), s => s.mode === "real", "未退出离线模式");
  return smoke;
}

/** 段配方离线端到端（默认）：先合成图主流程，再样本集冒烟。 */
async function runBurstOfflineE2E(page, options) {
  const startedAt = Date.now();
  const steps = [];
  const step = (name, detail) => steps.push({ name, atMs: Date.now() - startedAt, ...(detail ? { detail } : {}) });
  const call = client(page);
  const navigation = page.getByRole("navigation", { name: "操作导航" });

  const records = await call("records_list");
  assert(records.root.includes(identifier), `数据目录不属于隔离实例：${records.root}`);
  const initialSite = await call("site_status");
  assert.equal(initialSite.mode, "real", "软件启动必须处于真机模式");
  assert.equal((await call("history_query", { query: { limit: 1 } })).total, 0, "隔离实例应从全新站点开始");
  assert((await call("workspace_list")).every(w => !w.captureId && !w.doc.segments.length), "全新站点不应已有采集建立的候选或段配方候选");
  const before = { plc: await call("plc_get_config"), cameras: await call("camera_rig_config") };
  const settings = await call("cycle_get_settings");
  if (settings.productSource !== "plc" || options.armMs) {
    const timeouts = options.armMs ? { ...settings.timeouts, armMs: options.armMs } : settings.timeouts;
    await call("cycle_save_settings", { settings: { ...settings, productSource: "plc", timeouts } });
  }
  const engine = await until(() => call("engine_status"), e => e.ready, "lyFlow 引擎未就绪", 30000);
  step("lyFlow 就绪", { version: engine.version ?? null });

  const main = await syntheticFlow(page, call, navigation, options, step);
  assert.deepEqual(await call("plc_get_config"), before.plc, "退出离线后 PLC 配置未恢复");
  assert.deepEqual(await call("camera_rig_config"), before.cameras, "退出离线后相机组未恢复");
  step("退出离线并恢复");

  const smoke = options.sampleSet ? await sampleSetSmoke(page, call, navigation, options, step) : null;
  if (smoke) {
    assert.deepEqual(await call("plc_get_config"), before.plc, "退出离线后 PLC 配置未恢复");
    assert.deepEqual(await call("camera_rig_config"), before.cameras, "退出离线后相机组未恢复");
  }

  const result = {
    passed: true,
    model: "burst",
    identifier,
    inputs: { sampleSet: options.sampleSet ?? null, lyflow: options.lyflow, lyflowVersion: engine.version ?? null, armBudgetMs: options.armMs ?? null },
    elapsedMs: Date.now() - startedAt,
    recipe: main.recipe,
    main,
    smoke,
    gateRetries: Object.entries(gateRetries.reduce((counts, r) => ({ ...counts, [`${r.command}：${r.error}`]: (counts[`${r.command}：${r.error}`] ?? 0) + 1 }), {})).map(([what, count]) => ({ what, count })),
    steps,
    boundary: "进程内 S7 服务与虚拟产线；主流程是合成图三相机（标定、验证的每帧测量用真实 lyFlow，投产用模拟测量，所以投产件没有发布包、不能原包重现），"
      + "样本集冒烟是 2026-10-17 停稳拍数据（lyFlow 测量，一条走廊对不上其他点位，大量帧检测不完整）；像素当量与笔划是演示夹具，结论不代表算法质量、现场标定或现场节拍",
  };
  if (options.output) await writeFile(options.output, JSON.stringify(result, null, 2) + "\n");
  return result;
}

export async function runOfflineE2E(page, options) {
  return runBurstOfflineE2E(page, options);
}

async function main() {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) {
    assert(process.argv[i].startsWith("--") && process.argv[i + 1], "参数必须为 --name value");
    args[process.argv[i].slice(2)] = process.argv[i + 1];
  }
  const inputs = {
    sampleSet: args["sample-set"] ?? process.env.GLUESIGHT_SAMPLE_SET,
    lyflow: args["lyflow-runtime"] ?? process.env.GLUESIGHT_LYFLOW_RUNTIME,
  };
  const problems = missingInputs(inputs);
  if (problems.length) {
    process.stderr.write(`离线端到端输入缺失，未运行（不算通过）：\n${problems.join("\n")}\n`);
    process.exitCode = MISSING_INPUT_EXIT;
    return;
  }
  const instance = args.instance ?? "output/playwright/e2e-regression/instance.json";
  const cdp = await instanceCdp(instance, args.cdp, id => id === identifier);
  const browser = await chromium.connectOverCDP(cdp);
  try {
    const page = await uiPage(browser);
    const output = args.output ?? "output/playwright/e2e-regression/result.json";
    const armMs = args["arm-ms"] === undefined ? undefined : Number(args["arm-ms"]);
    assert(armMs === undefined || (Number.isInteger(armMs) && armMs > 0), `--arm-ms 必须是正整数：${args["arm-ms"]}`);
    const model = args.model ?? "burst";
    assert.equal(model, "burst", "仅支持段配方");
    const result = await runOfflineE2E(page, { ...inputs, armMs, output, workDir: path.dirname(output), model });
    const verdicts = Object.fromEntries(Object.entries(result.main?.production ?? result.production).map(([k, v]) => [k, v.verdict]));
    process.stdout.write(JSON.stringify({ passed: result.passed, model, elapsedMs: result.elapsedMs, expectedTotal: result.main.calibration.expectedTotal, toleranceTotal: result.main.calibration.toleranceTotal,
        production: verdicts, smoke: result.smoke && { validationPassed: result.smoke.validation.passed, published: result.smoke.published, incompleteRatio: result.smoke.production?.incompleteRatio ?? null } }) + "\n");
  } finally { await browser.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { process.stderr.write(`${error.stack ?? error}\n`); process.exitCode = 1; });
}
