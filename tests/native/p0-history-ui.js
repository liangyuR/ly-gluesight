async (page) => {
  const id = "P0-TRICAM-UI", bundleId = "3b643e189e0f73e5";
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const guard = { records: await read("records_list"), plc: await read("plc_get_config"), cameras: await read("camera_rig_config"), cycle: await read("cycle_snapshot"), workspace: await read("workspace_get", { id }) };
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.plc.connection.protocol !== "simulator" || guard.cameras.some(c => c.source !== "sim") || guard.cycle.phase !== "IDLE" || guard.workspace.workspace.frames.some(f => !f.saved || !f.trial?.passed)) throw new Error(JSON.stringify(guard));
  const recent = await read("history_query", { query: { recipeId: id, limit: 20 } });
  const selected = [recent.items.find(item => item.verdict.startsWith("OK")), recent.items.find(item => item.verdict === "NG_GAP")];
  if (selected.some(item => !item?.cycleId || item.bundleId !== bundleId)) throw new Error("Create new schema-2 normal/gap records first");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "历史记录", exact: true }).click();
  await page.getByRole("button", { name: "全部", exact: true }).click();
  await page.getByRole("combobox", { name: "历史配方", exact: true }).selectOption(id);
  await page.getByRole("button", { name: "导出 CSV", exact: true }).click();
  await page.getByText(/已导出：/).waitFor();
  const exported = await page.getByText(/已导出：/).innerText();
  const results = [];
  for (const row of selected) {
    if (results.length) {
      await page.getByRole("button", { name: "返回历史列表", exact: true }).click();
      await page.getByRole("button", { name: "全部", exact: true }).click();
      await page.getByRole("combobox", { name: "历史配方", exact: true }).selectOption(id);
    }
    await page.getByRole("row", { name: `查看 SN ${row.sn} 的记录`, exact: true }).click();
    await page.getByRole("heading", { name: "逐拍照点追溯", exact: true }).waitFor();
    const original = await read("history_detail", { id: row.id });
    const raw = await read("workspace_record_images", { historyId: row.id });
    if (original.summary.delivery.state !== "acknowledged" || original.recording.state !== "complete" || !raw.complete || raw.frames.length !== 12 || raw.frames.some(f => !f.available || f.error)) throw new Error(JSON.stringify({ original, raw }));
    const added = [];
    const compare = async (buttonName, source, title) => {
      const before = await read("workspace_comparisons", { id, historyId: row.id });
      await page.getByRole("button", { name: buttonName, exact: true }).click();
      let result;
      for (let attempt = 0; attempt < 150; attempt++) {
        const current = await read("workspace_comparisons", { id, historyId: row.id });
        result = current.find(c => c.source === source && !before.some(old => old.id === c.id));
        if (result) break;
        if (await page.getByRole("heading", { name: "操作未完成", exact: true }).isVisible()) throw new Error(await page.locator("main").innerText());
        await page.waitForTimeout(100);
      }
      if (!result || result.originalVerdict !== row.verdict || result.judgement.verdict !== row.verdict || result.cycleId !== row.cycleId || result.bundleId !== bundleId) throw new Error(JSON.stringify(result));
      await page.getByRole("heading", { name: title, exact: true }).waitFor();
      if (source !== "rules" && (result.measurements.length !== 4 || result.measurements.some(m => m.error || !m.located || m.idx.length < 70))) throw new Error(JSON.stringify(result));
      if (source === "original" && (result.candidateRevision !== 0 || result.candidateRecipe.revisionId !== row.recipeRevision || result.measurements.some(m => m.bundleId !== bundleId || m.cycleId !== row.cycleId))) throw new Error("Original reproduction did not use the frozen production identity");
      added.push(result);
      return result;
    };
    await compare("按原发布包重现", "original", "原发布包重现结果");
    await page.getByRole("button", { name: "使用该配方候选", exact: true }).click();
    await compare("按候选规则重判", "rules", "规则重判结果");
    await compare("从原图复测整件", "raw", "候选原图复测结果");
    const views = [];
    for (let k = 0; k < 4; k++) {
      for (const view of [1, 2, 3]) {
        await page.getByRole("button", { name: `查看 k${k + 1} 视角 ${view}`, exact: true }).click();
        const image = page.getByRole("img", { name: `原始 SN ${row.sn} · k${k + 1} · 视角 ${view}`, exact: true });
        await image.waitFor();
        await image.scrollIntoViewIfNeeded();
        await image.evaluate(element => element.scrollIntoView({ block: "center", behavior: "instant" }));
        const imageBox = await image.boundingBox();
        if (!imageBox || imageBox.width < 200 || imageBox.height < 150 || imageBox.y < 60 || imageBox.y + imageBox.height > 900) throw new Error(JSON.stringify(imageBox));
        if (await image.getAttribute("viewBox") !== "0 0 1280 1024") throw new Error("Unexpected recorded image resolution");
        const selectedView = view === [1, 2, 3, 1][k];
        const teachingEnabled = await page.getByRole("button", { name: "将此帧用于示教", exact: true }).isEnabled();
        if (teachingEnabled !== selectedView || !selectedView && await image.locator("polyline").count()) throw new Error("A non-detection view was exposed as a valid teaching image");
        await page.screenshot({ path: `output/playwright/p0-history/02-history-${row.id}-k${k + 1}-v${view}.png`, fullPage: true });
        views.push({ k, view, selectedView, teachingEnabled, imageBox, file: raw.frames.find(f => f.k === k && f.view === view)?.file });
      }
    }
    for (const comparison of added) {
      await page.getByRole("combobox", { name: "已保存对照结果", exact: true }).selectOption(comparison.id);
      await page.getByRole("heading", { name: { original: "原发布包重现结果", rules: "规则重判结果", raw: "候选原图复测结果" }[comparison.source], exact: true }).waitFor();
    }
    await page.getByRole("button", { name: "放大原图", exact: true }).click();
    await page.getByRole("button", { name: "适应窗口", exact: true }).click();
    const after = await read("history_detail", { id: row.id });
    if (JSON.stringify(after) !== JSON.stringify(original)) throw new Error("Comparisons modified the original production record");
    results.push({ historyId: row.id, sn: row.sn, cycleId: row.cycleId, bundleId, verdict: row.verdict, views, comparisons: added.map(c => ({ id: c.id, source: c.source, candidateRevision: c.candidateRevision, candidateVersion: c.candidateRecipe.version, candidateHash: c.candidateRecipe.revisionId, bundleId: c.bundleId, originalVerdict: c.originalVerdict, judgement: c.judgement, measurements: c.measurements.map(m => ({ k: m.k, cycleId: m.cycleId, bundleId: m.bundleId, error: m.error, ms: m.ms, points: m.idx.length })) })), originalRecordUnchanged: true });
  }
  const afterWorkspace = await read("workspace_get", { id });
  if (JSON.stringify(afterWorkspace.workspace) !== JSON.stringify(guard.workspace.workspace)) throw new Error("Read-only history viewing or comparisons changed candidate teaching");
  return { exported, results, candidateUnchanged: true, viewsShownPerPart: 12, accuracyQualified: false };
}
