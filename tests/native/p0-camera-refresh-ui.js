async (page) => {
  const id = "P0-TRICAM-UI";
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const guard = { records: await read("records_list"), cameras: await read("camera_rig_config"), plc: await read("plc_get_config"), cycle: await read("cycle_snapshot"), workspace: await read("workspace_get", { id }) };
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.cameras.some(c => c.source !== "sim") || guard.plc.connection.protocol !== "simulator" || !["IDLE", "FAULT"].includes(guard.cycle.phase) || guard.workspace.workspace.frames.some(f => !f.saved || !f.trial?.passed)) throw new Error(JSON.stringify(guard));
  const known = new Set(guard.cameras.map(c => c.id));
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "设备与采集", exact: true }).click();
  let cameras;
  for (let attempt = 0; attempt < 8; attempt++) {
    await page.getByRole("button", { name: "添加相机", exact: true }).click();
    for (let poll = 0; poll < 15; poll++) {
      cameras = await read("camera_rig_config");
      if (cameras.some(c => !known.has(c.id))) break;
      await page.waitForTimeout(100);
    }
    if (cameras.some(c => !known.has(c.id))) break;
  }
  const added = cameras.find(c => !known.has(c.id));
  if (!added || added.source !== "sim" || added.viewCount !== 3 || added.acquisition !== "triggered") throw new Error(JSON.stringify(cameras));
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "拍照点规划", exact: true }).click();
  await page.getByRole("combobox", { name: "当前配方", exact: true }).selectOption(id);
  const target = page.getByRole("combobox", { name: "拍照点 3 · 相机", exact: true });
  await target.locator(`option[value="${added.id}"]`).waitFor({ state: "attached", timeout: 5000 });
  const options = await target.locator("option").evaluateAll(options => options.map(option => option.value));
  const after = await read("workspace_get", { id });
  if (JSON.stringify(after.workspace.doc) !== JSON.stringify(guard.workspace.workspace.doc) || JSON.stringify(after.workspace.frames) !== JSON.stringify(guard.workspace.workspace.frames) || after.productionVersion !== 1 || after.workspace.baseRevision !== guard.workspace.workspace.baseRevision) throw new Error("Adding an unrelated camera changed existing candidate teaching or production");
  await page.screenshot({ path: "output/playwright/p0-step5-ui/22-camera-refresh-fixed.png", fullPage: true });
  return { addedCamera: added, options, appearedWithoutPageRefresh: true, existingCandidateAndProductionUnchanged: true, revision: after.workspace.revision };
}
