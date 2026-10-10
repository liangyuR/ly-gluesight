async (page) => {
  const id = "P0-TRICAM-UI";
  const evidence = "D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step5-ui";
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const workspace = () => read("workspace_get", { id });
  const guard = { records: await read("records_list"), cameras: await read("camera_rig_config"), plc: await read("plc_get_config"), cycle: await read("cycle_snapshot"), production: await read("cycle_layout", { recipeId: id }) };
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.cameras.some(c => c.source !== "sim") || !guard.cameras.some(c => c.id === "cam2" && c.viewCount === 3 && c.acquisition === "triggered") || guard.plc.connection.protocol !== "simulator" || guard.cycle.phase !== "IDLE") throw new Error(JSON.stringify(guard));
  const initial = await workspace();
  if (initial.workspace.frames.some(f => !f.saved || !f.trial?.passed)) throw new Error("Candidate must be restored before camera continuation");
  const beforeOptions = await page.getByRole("combobox", { name: "拍照点 3 · 相机", exact: true }).locator("option").evaluateAll(options => options.map(option => option.value));
  await page.reload();
  await page.getByRole("combobox", { name: "当前配方", exact: true }).selectOption(id);
  const target = page.getByRole("combobox", { name: "拍照点 3 · 相机", exact: true });
  await target.locator('option[value="cam2"]').waitFor({ state: "attached" });
  const afterOptions = await target.locator("option").evaluateAll(options => options.map(option => option.value));
  await page.screenshot({ path: evidence + "/20-camera-options-after-refresh.png", fullPage: true });
  await target.selectOption("cam2");
  await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
  let invalidated;
  for (let attempt = 0; attempt < 100; attempt++) {
    invalidated = await workspace();
    if (invalidated.workspace.doc.shots[2].camera === "cam2") break;
    await page.waitForTimeout(100);
  }
  const frame = invalidated.workspace.frames[2];
  if (invalidated.workspace.doc.shots[2].camera !== "cam2" || frame.saved || frame.trial || frame.image || frame.views.length) throw new Error("Camera binding retained old teaching");
  for (const k of [0, 1, 3]) if (JSON.stringify(initial.workspace.frames[k]) !== JSON.stringify(invalidated.workspace.frames[k]) || JSON.stringify(initial.workspace.doc.shots[k]) !== JSON.stringify(invalidated.workspace.doc.shots[k])) throw new Error(`Unrelated k${k + 1} changed`);
  if (invalidated.productionVersion !== initial.productionVersion || invalidated.workspace.baseRevision !== initial.workspace.baseRevision) throw new Error("Published production identity changed");
  await page.screenshot({ path: evidence + "/13-camera-invalidated.png", fullPage: true });
  await target.selectOption("cam1");
  await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
  for (let attempt = 0; attempt < 100 && (await workspace()).workspace.doc.shots[2].camera !== "cam1"; attempt++) await page.waitForTimeout(100);
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "单帧示教", exact: true }).click();
  await page.getByRole("button", { name: "选择帧 k3", exact: true }).click();
  let captured;
  for (let attempt = 0; attempt < 8; attempt++) {
    await page.getByRole("button", { name: "取新样本", exact: true }).click();
    for (let poll = 0; poll < 15; poll++) {
      captured = await workspace();
      if (captured.workspace.frames[2].image) break;
      await page.waitForTimeout(100);
    }
    if (captured.workspace.frames[2].image) break;
    if (!await page.getByText("正在处理 PLC 事务，请稍后重试取图", { exact: true }).isVisible()) throw new Error(await page.locator("main").innerText());
  }
  if (!captured.workspace.frames[2].image) throw new Error("Restored camera capture did not complete");
  await page.getByRole("button", { name: "试测当前帧", exact: true }).click();
  let tested;
  for (let attempt = 0; attempt < 200; attempt++) {
    tested = await workspace();
    if (tested.workspace.frames[2].trial) break;
    await page.waitForTimeout(100);
  }
  if (!tested.workspace.frames[2].trial?.passed) throw new Error(JSON.stringify(tested.workspace.frames[2].trial));
  await page.getByRole("button", { name: "保存本帧示教", exact: true }).click();
  let restored;
  for (let attempt = 0; attempt < 100; attempt++) {
    restored = await workspace();
    if (restored.workspace.frames[2].saved) break;
    await page.waitForTimeout(100);
  }
  const production = await read("cycle_layout", { recipeId: id });
  if (restored.workspace.frames.some(f => !f.saved || !f.trial?.passed) || JSON.stringify(restored.workspace.doc) !== JSON.stringify(initial.workspace.doc) || production.revisionId !== guard.production.revisionId || production.teachingId !== guard.production.teachingId || production.version !== 1) throw new Error("Candidate or publication was not restored");
  await page.screenshot({ path: evidence + "/14-teaching-restored.png", fullPage: true });
  return { staleOptionsBeforeRefresh: beforeOptions, optionsAfterRefresh: afterOptions, invalidation: { k: 2, from: "cam1", to: "cam2", saved: invalidated.workspace.frames.map(f => f.saved), otherFramesAndShotsUnchanged: true, image: frame.image, trial: frame.trial }, final: { saved: restored.workspace.frames.map(f => f.saved), views: restored.workspace.doc.shots.map(s => s.view), revision: restored.workspace.revision, productionVersion: production.version, productionRevision: production.revisionId, teachingId: production.teachingId, restoredTrial: { passed: restored.workspace.frames[2].trial.passed, engineTag: restored.workspace.frames[2].trial.engineTag, elapsedMs: restored.workspace.frames[2].trial.elapsedMs } }, cycle: await read("cycle_snapshot") };
}
