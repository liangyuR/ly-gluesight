async (page) => {
  const id = "P0-TRICAM-UI", bundleHash = "3b643e189e0f73e5";
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const guard = { records: await read("records_list"), cameras: await read("camera_rig_config"), plc: await read("plc_get_config"), status: await read("plc_get_status"), settings: await read("cycle_get_settings"), cycle: await read("cycle_snapshot"), engine: await read("engine_status") };
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.cameras.some(c => c.source !== "sim") || guard.plc.connection.protocol !== "simulator" || guard.status.state !== "connected" || guard.cycle.phase !== "IDLE" || guard.settings.timeouts.armMs !== 200 || !guard.settings.vision || guard.settings.record !== "all" || !guard.engine.ready || !guard.engine.measuring) throw new Error(JSON.stringify(guard));
  const production = await read("cycle_layout", { recipeId: id });
  if (production.hash !== "c1574d7504b2a8d4") throw new Error("Original production recipe changed");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  await page.getByRole("combobox", { name: "模拟配方", exact: true }).selectOption(id);
  const results = [];
  for (const scenario of ["normal", "gap"]) {
    const prior = await read("history_query", { query: { recipeId: id, limit: 1 } });
    const previousId = prior.items[0]?.id ?? -1;
    await page.getByRole("combobox", { name: "模拟工况", exact: true }).selectOption(scenario);
    const started = Date.now();
    await page.getByRole("button", { name: "运行一件", exact: true }).click();
    let detail, raw;
    for (let attempt = 0; attempt < 200; attempt++) {
      const recent = await read("history_query", { query: { recipeId: id, limit: 1 } });
      if (recent.items[0]?.id > previousId) {
        detail = await read("history_detail", { id: recent.items[0].id });
        raw = await read("workspace_record_images", { historyId: detail.summary.id });
        if (detail.recording?.state === "complete" && detail.summary.delivery?.state === "acknowledged" && raw.complete && raw.frames.length === 12) break;
      }
      await page.waitForTimeout(100);
    }
    if (!detail?.summary.cycleId || detail.summary.bundleHash !== bundleHash || detail.summary.recipeHash !== production.hash || detail.summary.delivery.state !== "acknowledged" || detail.recording.state !== "complete" || !raw?.complete || raw.frames.length !== 12 || raw.frames.some(f => !f.available || f.error)) throw new Error(JSON.stringify({ detail, raw }));
    const s = detail.summary;
    if (scenario === "normal" ? !["OK", "OK_WITH_EXCURSION"].includes(s.verdict) : s.verdict !== "NG_GAP" || s.plcCode !== 13) throw new Error(JSON.stringify(s));
    if (s.framesExpected !== 4 || s.framesReceived !== 4 || detail.triggers !== 4 || detail.shots.length !== 4 || detail.points.st.length !== 307) throw new Error(JSON.stringify(detail));
    for (let k = 0; k < 4; k++) {
      const shot = detail.shots[k];
      if (shot.k !== k || shot.shotId !== `P${k + 1}` || shot.camera !== "cam1" || shot.view !== [1, 2, 3, 1][k] || shot.ordinal !== k + 1 || shot.status !== "done" || shot.error || shot.rawFiles.length !== 3 || JSON.stringify(shot.rawFiles.map(f => f.view).sort()) !== "[1,2,3]" || shot.rawFiles.some(f => !f.hash)) throw new Error(JSON.stringify(shot));
      if (JSON.stringify(raw.frames.filter(f => f.k === k).map(f => f.view).sort()) !== "[1,2,3]") throw new Error("Missing recorded physical view");
    }
    const logs = (await read("cycle_logs")).filter(line => line.ts >= started);
    const arming = logs.find(line => line.ev === "armed↑ busy↑");
    const armMs = Number(arming?.msg.match(/布防耗时 (\d+) ms/)?.[1]);
    if (!Number.isFinite(armMs) || armMs > 200) throw new Error(JSON.stringify(logs));
    const measured = await read("cycle_part_data");
    if (measured.length !== 4 || measured.some(m => m.cycleId !== s.cycleId || m.bundleHash !== bundleHash || m.error || !m.located)) throw new Error(JSON.stringify(measured));
    await page.screenshot({ path: `D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step6-ui/01-online-${scenario}.png`, fullPage: true });
    results.push({ scenario, summary: s, arming: { configuredArmMs: 200, elapsedMs: armMs, log: arming }, shots: detail.shots, recording: detail.recording, originals: raw, measurements: measured.map(m => ({ k: m.k, cycleId: m.cycleId, bundleHash: m.bundleHash, camera: m.camera, ms: m.ms, points: m.idx.length })), logs });
    for (let attempt = 0; attempt < 100 && (await read("sim_status")).running; attempt++) await page.waitForTimeout(100);
    if ((await read("sim_status")).running || (await read("cycle_snapshot")).phase !== "IDLE") throw new Error("Simulator did not finish the result handshake");
  }
  return { results, engine: guard.engine, settings: guard.settings, productionHash: production.hash, bundleHash };
}
