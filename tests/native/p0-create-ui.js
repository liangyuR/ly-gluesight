async (page) => {
  const guard = await page.evaluate(async () => ({ records: await window.__TAURI_INTERNALS__.invoke("records_list"), cameras: await window.__TAURI_INTERNALS__.invoke("camera_rig_config"), plc: await window.__TAURI_INTERNALS__.invoke("plc_get_config") }));
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.cameras.some(c => c.source === "mvs") || guard.plc.connection.protocol !== "simulator") throw new Error("The isolated offline P0 app is required");
  await page.getByRole("button", { name: "复制配方 MTR-HSG-B", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "建立候选配方", exact: true });
  await dialog.getByRole("textbox", { name: "配方编号", exact: true }).fill("P0-TRICAM-UI");
  await dialog.getByRole("textbox", { name: "配方名称", exact: true }).fill("P0 三目 UI 验收");
  await dialog.getByRole("spinbutton", { name: "产品代码", exact: true }).fill("701");
  await dialog.getByRole("button", { name: "创建候选", exact: true }).click();
  await page.waitForURL("**/#/recipe/geometry");
  await page.getByRole("combobox", { name: "触发方式", exact: true }).selectOption("fly");
  for (let k = 0; k < 4; k++) {
    await page.getByRole("combobox", { name: `拍照点 ${k + 1} · 相机`, exact: true }).selectOption("cam1");
    const view = page.getByRole("combobox", { name: `拍照点 ${k + 1} · 视角`, exact: true });
    const target = String([1, 2, 3, 1][k]);
    if (await view.inputValue() !== target) await view.selectOption(target);
  }
  await page.getByRole("spinbutton", { name: "搜索半宽（mm）", exact: true }).fill("8");
  await page.getByRole("spinbutton", { name: "胶宽下限（mm）", exact: true }).fill("1.5");
  await page.getByRole("spinbutton", { name: "胶宽上限（mm）", exact: true }).fill("6.5");
  await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
  let result;
  for (let attempt = 0; attempt < 100; attempt++) {
    result = await page.evaluate(async () => window.__TAURI_INTERNALS__.invoke("workspace_get", { id: "P0-TRICAM-UI" }));
    if (result.workspace.doc.triggerMode === "fly" && result.workspace.doc.shots.length === 4 && result.workspace.doc.shots.every((s, k) => s.camera === "cam1" && s.view === [1, 2, 3, 1][k]) && result.workspace.doc.detect.searchMm === 8) break;
    await page.waitForTimeout(100);
  }
  if (result.workspace.doc.triggerMode !== "fly" || result.workspace.doc.shots.length !== 4 || result.workspace.doc.shots.some((s, k) => s.camera !== "cam1" || s.view !== [1, 2, 3, 1][k]) || result.workspace.doc.detect.searchMm !== 8) throw new Error(JSON.stringify(result.workspace.doc));
  await page.screenshot({ path: "D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step5-ui/02-four-shot-geometry.png", fullPage: true });
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "单帧示教", exact: true }).click();
  return { id: result.workspace.doc.id, productCode: result.workspace.doc.productCode, shots: result.workspace.doc.shots, frames: result.workspace.frames };
}
