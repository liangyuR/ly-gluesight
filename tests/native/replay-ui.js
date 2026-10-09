async (page) => {
  if (!await page.evaluate(() => !!window.__uiOperations)) throw new Error("Run guard.js first");
  await page.getByRole("button", { name: "回放目录", exact: true }).click();
  await page.getByRole("button", { name: "触发（飞拍）", exact: true }).click();
  await page.getByPlaceholder("D:\\现场图\\Glue1").fill("D:/project/ly-gluesight/output/offline/hikvision-three-camera/replay/Glue1");
  await page.getByRole("spinbutton").fill("1");
  await page.getByRole("button", { name: "连续（仅预览）", exact: true }).click();
  await page.getByRole("button", { name: "保存并应用", exact: true }).click();
  await page.getByText("已保存并应用", { exact: true }).waitFor();
  await page.getByRole("button", { name: "下一张", exact: true }).click();
  await page.locator(".preview-box canvas").waitFor({ state: "visible" });
  const preview = await page.locator(".preview-box canvas").evaluate(canvas => ({ width: canvas.width, height: canvas.height }));
  const config = await page.evaluate(async () => (await window.__TAURI_INTERNALS__.invoke("camera_rig_config"))[0]);
  if (config.source !== "replay" || config.replayChannel !== 1 || config.acquisition !== "freeRun" || !preview.width) throw new Error("Replay UI settings did not reach backend or preview");
  const result = { operation: "回放目录、通道、连续采集、保存应用与下一张", passed: true, preview };
  await page.evaluate(result => window.__uiOperations.checks.push(result), result);
  await page.screenshot({ path: "output/playwright/ui-regression/replay-camera.png", fullPage: true });
  return result;
}
