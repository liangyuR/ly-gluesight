async (page) => {
  const before = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("workspace_get", { id:"UI-FLYSHOT" }));
  await page.getByRole("button", { name: "选择帧 k4", exact: true }).click();
  await page.locator('input[type="file"]').setInputFiles("output/playwright\\ui-regression\\flyshot-samples\\k4.pgm");
  await page.getByText("离线原图已绑定本帧，请重新试测", { exact: true }).waitFor();
  const imported = await page.evaluate(async () => (await window.__TAURI_INTERNALS__.invoke("workspace_get", { id:"UI-FLYSHOT" })).workspace.frames[3]);
  if (imported.image.source !== "import" || imported.image.size.join("x") !== "1280x1024" || imported.saved || imported.trial || !imported.backup) throw new Error("Offline image was not bound and invalidated through UI");
  await page.getByRole("button", { name:"试测当前帧", exact:true }).click();
  await page.getByText("当前冻结图像的试测已完成", { exact:true }).waitFor();
  await page.getByRole("button", { name:"保存本帧示教", exact:true }).click();
  await page.getByText("本帧示教已保存，发布前仍需整体验证", { exact:true }).waitFor();
  await page.getByRole("button", { name:"恢复原始示教", exact:true }).click();
  await page.getByText("原始图像已恢复，请重新试测", { exact:true }).waitFor();
  await page.getByRole("button", { name:"试测当前帧", exact:true }).click();
  await page.getByText("当前冻结图像的试测已完成", { exact:true }).waitFor();
  await page.getByRole("button", { name:"保存本帧示教", exact:true }).click();
  await page.getByText("本帧示教已保存，发布前仍需整体验证", { exact:true }).waitFor();
  await page.getByRole("link", { name:"布置总览", exact:true }).click();
  await page.getByRole("button", { name:"自动布置", exact:true }).click();
  const svg = page.locator("svg.wp-overview.editable");
  const box = page.getByRole("button", { name:"总览选择帧 k1", exact:true });
  await box.scrollIntoViewIfNeeded();
  const pointer = await svg.evaluate(el => [[270,131.5],[301.2,140.7]].map(([x,y]) => {
    const p = new DOMPoint(x,y).matrixTransform(el.getScreenCTM()); return {x:p.x,y:p.y};
  }));
  await page.mouse.move(pointer[0].x,pointer[0].y); await page.mouse.down();
  await page.mouse.move(pointer[1].x,pointer[1].y,{steps:8}); await page.mouse.up();
  await box.focus(); await page.keyboard.press("ArrowRight");
  await page.getByRole("button", { name:"保存总览", exact:true }).click();
  await page.getByText("总览布置已保存，拍照与测量不受影响", { exact:true }).waitFor();
  const state = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("workspace_get", {id:"UI-FLYSHOT"}));
  if (!state.workspace.overview.saved || Math.abs(state.workspace.overview.positions[0][0]-.32)>.01 || JSON.stringify(state.layout.shots.map(s=>s.path))!==JSON.stringify(before.layout.shots.map(s=>s.path))) throw new Error(JSON.stringify(state.workspace.overview));
  const result={ operation:"完整离线原图导入、试测保存、原始示教恢复、总览真实拖动与方向键、独立保存（中线不变）",passed:true, imported:imported.image, positions:state.workspace.overview.positions, shots:state.layout.shots };
  await page.evaluate(result=>window.__uiOperations.checks.push(result),result);
  await page.getByRole("link", { name:"验证与发布",exact:true }).last().click();
  return result;
}
