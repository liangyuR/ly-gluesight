async (page) => {
  if (!await page.evaluate(() => !!window.__uiOperations)) throw new Error("Run guard.js first");
  await page.getByText("cam1 · 离线原图", { exact: true }).waitFor();
  const svg = page.getByRole("img", { name: "随动标定图像，点击设置胶嘴、方向或比例尺", exact: true });
  const clickPoint = async (x, y) => {
    await svg.scrollIntoViewIfNeeded();
    const point = await svg.evaluate((el, [x, y]) => {
      const p = new DOMPoint(x, y).matrixTransform(el.getScreenCTM()); return { x: p.x, y: p.y };
    }, [x, y]);
    await page.mouse.click(point.x, point.y);
  };
  await page.getByRole("button", { name: "点胶嘴", exact: true }).click();
  await clickPoint(688, 646);
  const clicked = [Number(await page.getByRole("spinbutton", { name: "胶嘴 x（px）", exact: true }).inputValue()),
    Number(await page.getByRole("spinbutton", { name: "胶嘴 y（px）", exact: true }).inputValue())];
  if (Math.abs(clicked[0]-688)>1 || Math.abs(clicked[1]-646)>1) throw new Error("SVG click did not match image coordinates within pointer rounding");
  await page.getByRole("button", { name: "量比例", exact: true }).click();
  await clickPoint(100, 100); await clickPoint(200, 100);
  const before = Number(await page.getByRole("spinbutton", { name: "像素当量（mm/px）", exact: true }).inputValue());
  await page.getByRole("spinbutton", { name: "量比例：实际距离（mm）", exact: true }).fill("20");
  const after = Number(await page.getByRole("spinbutton", { name: "像素当量（mm/px）", exact: true }).inputValue());
  if (Math.abs(after-before*2)>.00002) throw new Error("Scale length did not update mm/px");
  // 这里只验证样本像素坐标，不把未知物理尺度作为现场标定。
  await page.getByRole("spinbutton", { name: "量比例：实际距离（mm）", exact: true }).fill("100");
  for (const [name, value] of [["胶嘴 x（px）", 688], ["胶嘴 y（px）", 646], ["像素当量（mm/px）", 1], ["遮挡半径（px）", 50], ["图像方位（°）", 105], ["名义胶宽（mm）", 16],
    ["搜索半宽（mm）", 25], ["窗口近端（mm）", 70], ["窗口远端（mm）", 280]]) {
    await page.getByRole("spinbutton", { name, exact: true }).fill(String(value));
  }
  await page.getByRole("button", { name: "在当前帧试测", exact: true }).click();
  const stats = page.getByText(/测到 \d+\/421 点/);
  await stats.waitFor(); const manual = await stats.textContent();
  if (!Number(manual.match(/测到 (\d+)/)[1])) throw new Error("Imported image produced no measured bead points");
  await page.getByRole("button", { name: "自动找方向", exact: true }).click();
  await page.waitForFunction(() => [...document.querySelectorAll("button")].find(b => b.textContent.trim() === "自动找方向")?.disabled === false);
  await stats.waitFor();
  const auto = await stats.textContent();
  await page.screenshot({ path: "output/playwright/ui-regression/follow-import-probe.png", fullPage: true });
  await page.getByRole("button", { name: "保存标定", exact: true }).click();
  await page.waitForFunction(async () => !!(await window.__TAURI_INTERNALS__.invoke("camera_rig_config"))[0].follow);
  const saved = await page.evaluate(async () => (await window.__TAURI_INTERNALS__.invoke("camera_rig_config"))[0].follow);
  if (saved.nozzle[0] !== 688 || saved.nozzle[1] !== 646 || saved.mmPerPx !== 1) throw new Error("Calibration was not saved through UI");
  const result = { operation: "离线原图导入、真实 SVG 坐标、两点比例尺联动、手动/自动原图卡尺试测、保存标定", passed: true, manual, auto, saved,
    units: "像素测试尺度 mmPerPx=1，未经物理标定" };
  await page.evaluate(result => window.__uiOperations.checks.push(result), result);
  return result;
}
