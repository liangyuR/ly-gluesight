async (page) => {
  await page.locator('input[type="file"]').setInputFiles("output/engine-tests\\board\\board.pgm");
  await page.getByRole("spinbutton", { name: "内角点（列）", exact: true }).waitFor();
  await page.getByRole("spinbutton", { name: "内角点（列）", exact: true }).fill("9");
  await page.getByRole("spinbutton", { name: "内角点（行）", exact: true }).fill("6");
  await page.getByRole("spinbutton", { name: "格长（mm）", exact: true }).fill("3.2");
  await page.getByRole("button", { name: "用冻结样本标定", exact: true }).click();
  await page.getByText(/标定完成：残差 RMS/).waitFor();
  const info = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("vision_calib_info", { cam: 0 }));
  if (!info || Math.abs(info.mmPerPx - 0.08) > 0.001 || info.rms > 0.1 || info.pattern.join("x") !== "9x6") throw new Error(JSON.stringify(info));
  await page.evaluate(info => window.__uiOperations.checks.push({ operation: "离线棋盘原图导入、内角点与格长设置、真实工位标定", passed: true, info }), info);
  await page.screenshot({ path: "output/playwright\\ui-regression\\flyshot-calibration.png", fullPage: true });
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "配方库", exact: true }).click();
  return info;
}
