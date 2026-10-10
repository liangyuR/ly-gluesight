async (page) => {
  const id = "P0-TRICAM-UI";
  const evidence = "D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step5-ui";
  const invokeRead = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const guard = { records: await invokeRead("records_list"), cameras: await invokeRead("camera_rig_config"), config: await invokeRead("plc_get_config"), status: await invokeRead("plc_get_status"), settings: await invokeRead("cycle_get_settings"), engine: await invokeRead("engine_status") };
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.cameras.some(c => c.source !== "sim") || guard.config.connection.protocol !== "simulator" || guard.status.state !== "connected" || guard.settings.timeouts.armMs !== 200 || !guard.settings.vision || guard.settings.record !== "all" || !guard.engine.ready || !guard.engine.measuring) throw new Error(JSON.stringify(guard));
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  const initial = await invokeRead("cycle_snapshot");
  if (initial.alarms.some(a => a.includes(id))) throw new Error("The published P0 recipe is not runnable: " + initial.alarms.join("; "));
  if (initial.phase === "FAULT") {
    await page.getByRole("button", { name: "复位故障", exact: true }).click();
    for (let attempt = 0; attempt < 100 && (await invokeRead("cycle_snapshot")).phase === "FAULT"; attempt++) await page.waitForTimeout(100);
    if ((await invokeRead("cycle_snapshot")).phase === "FAULT") throw new Error("UI reset did not clear the disconnected-S7 startup fault");
  }
  await page.getByRole("combobox", { name: "模拟配方", exact: true }).selectOption(id);
  const production = await invokeRead("cycle_layout", { recipeId: id });
  const rows = [];
  for (const [scenario, imageName] of [["normal", "09-online-normal"], ["gap", "10-online-gap"]]) {
    const previous = await invokeRead("history_query", { query: { recipeId: id, limit: 1 } });
    const previousId = previous.items[0]?.id ?? -1;
    await page.getByRole("combobox", { name: "模拟工况", exact: true }).selectOption(scenario);
    const runStartedAt = Date.now();
    await page.getByRole("button", { name: "运行一件", exact: true }).click();
    const observations = [];
    let summary;
    for (let attempt = 0; attempt < 200; attempt++) {
      const snapshot = await invokeRead("cycle_snapshot");
      const last = observations.at(-1);
      if (!last || last.phase !== snapshot.phase || last.received !== snapshot.part?.received || last.filled !== snapshot.part?.filled || last.queue !== snapshot.part?.queue) observations.push({ observedAt: Date.now(), phase: snapshot.phase, cycleId: snapshot.part?.cycleId, sn: snapshot.part?.sn, received: snapshot.part?.received, filled: snapshot.part?.filled, queue: snapshot.part?.queue, result: snapshot.result });
      const history = await invokeRead("history_query", { query: { recipeId: id, limit: 1 } });
      if (history.items[0]?.id > previousId) { summary = history.items[0]; break; }
      if (!((await invokeRead("sim_status")).running) && attempt > 5) throw new Error("Simulation stopped before a new recorded result: " + await page.locator("main").innerText());
      await page.waitForTimeout(100);
    }
    if (!summary) throw new Error("No new production result within 20 seconds");
    const detail = await invokeRead("history_detail", { id: summary.id });
    const snapshot = await invokeRead("cycle_snapshot");
    const measured = await invokeRead("cycle_part_data");
    const cycleLogs = (await invokeRead("cycle_logs")).filter(line => line.ts >= runStartedAt);
    const armingLog = cycleLogs.find(line => line.ev === "armed↑ busy↑");
    const armingMs = Number(armingLog?.msg.match(/布防耗时 (\d+) ms/)?.[1]);
    if (!Number.isFinite(armingMs) || armingMs > guard.settings.timeouts.armMs) throw new Error(JSON.stringify({ configuredArmMs: guard.settings.timeouts.armMs, armingMs, cycleLogs }));
    const recordingWaitStarted = Date.now();
    let recorded;
    for (let attempt = 0; attempt < 100; attempt++) {
      recorded = await invokeRead("workspace_record_images", { historyId: summary.id });
      if (recorded.complete) break;
      await page.waitForTimeout(100);
    }
    const recordingSettledMs = Date.now() - recordingWaitStarted;
    if (detail.summary.recipeHash !== production.hash || detail.summary.framesExpected !== 4 || detail.summary.framesReceived !== 4 || detail.triggers !== 4 || detail.frames.some((f, k) => f.camera !== "cam1" || f.view !== [1, 2, 3, 1][k] || f.status !== "done" || f.ordinal !== k + 1) || detail.frames.length !== 4 || detail.points?.d.length !== production.points.k.length || detail.points?.w.length !== production.points.k.length || detail.points?.st.length !== production.points.k.length || !recorded.complete || recorded.frames.length !== 4 || recorded.frames.some((f, k) => f.k !== k || f.camera !== "cam1" || f.view !== [1, 2, 3, 1][k] || !f.available)) throw new Error(JSON.stringify({ detail, recorded }));
    if (scenario === "normal" ? !["OK", "OK_WITH_EXCURSION"].includes(summary.verdict) : summary.verdict !== "NG_GAP" || summary.plcCode !== 13) throw new Error(JSON.stringify(summary));
    if (measured.length !== 4 || measured.some(m => m.error || !m.located || m.bundleHash !== snapshot.part?.bundleHash || m.idx.length < 70)) throw new Error(JSON.stringify(measured));
    for (let k = 0; k < 4; k++) {
      await page.getByRole("button", { name: `查看帧 k${k + 1}`, exact: true }).click();
      await page.locator("svg.wp-gray-image").waitFor();
      await page.screenshot({ path: `${evidence}/${imageName}-k${k + 1}.png`, fullPage: true });
    }
    rows.push({ scenario, summary, arming: { configuredArmMs: guard.settings.timeouts.armMs, elapsedMs: armingMs, log: armingLog }, judgement: detail.judgement, frames: detail.frames, triggers: detail.triggers, softwareVersion: detail.softwareVersion, measurements: measured.map(m => ({ k: m.k, cycleId: m.cycleId, shotId: m.shotId, camera: m.camera, bundleHash: m.bundleHash, located: m.located, score: m.score, ms: m.ms, points: m.idx.length, error: m.error })), recorded, recordingSettledMs, observations, cycleLogs });
    for (let attempt = 0; attempt < 100 && (await invokeRead("sim_status")).running; attempt++) await page.waitForTimeout(100);
    if ((await invokeRead("sim_status")).running) throw new Error("The simulator did not finish the one-part handshake");
  }
  return { engine: guard.engine, recipeHash: production.hash, teachingHash: production.teachingHash, rows, final: await invokeRead("cycle_snapshot") };
}
