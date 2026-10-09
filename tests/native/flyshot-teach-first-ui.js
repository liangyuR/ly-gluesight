async (page) => {
  await page.getByRole("combobox", { name: "触发方式", exact: true }).selectOption("fly");
  await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
  await page.getByText("候选配置已保存，生产版本保持不变", { exact: true }).first().waitFor();
  await page.getByRole("link", { name: "进入单帧示教", exact: true }).click();
  await page.getByRole("button", { name: "取新样本", exact: true }).click();
  const svg = page.locator("svg.wp-gray-image.editable");
  await svg.waitFor();
  await svg.scrollIntoViewIfNeeded();
  const pointer = await svg.locator("g").first().evaluate(el => {
    const matrix = el.getScreenCTM();
    return [[1225, 75], [1575, 425]].map(([x,y]) => { const p = new DOMPoint(x,y).matrixTransform(matrix); return { x:p.x, y:p.y }; });
  });
  await page.mouse.move(pointer[0].x, pointer[0].y);
  await page.mouse.down();
  await page.mouse.move(pointer[1].x, pointer[1].y, { steps: 10 });
  await page.mouse.up();
  await page.getByRole("button", { name: "试测当前帧", exact: true }).click();
  await page.getByText("当前冻结图像的试测已完成", { exact: true }).waitFor();
  const view = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("workspace_get", { id: "UI-FLYSHOT" }));
  const frame = view.workspace.frames[0];
  if (!frame.trial?.passed) throw new Error(JSON.stringify(frame));
  await page.getByRole("button", { name: "保存并示教下一帧", exact: true }).click();
  await page.getByRole("button", { name: "选择帧 k2", exact: true }).waitFor();
  return { operation: "飞拍候选规划保存、取实际合成图、真实指针拖模板、LyFlow 试测、保存并下一帧", frame };
}
