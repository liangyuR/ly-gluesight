async (page) => {
  if (!await page.evaluate(() => !!window.__uiOperations)) throw new Error("Run guard.js first");
  const retention = page.getByRole("spinbutton", { name: "日志保留天数（0 为永久）", exact: true });
  const original = await retention.inputValue();
  await retention.fill("7");
  await page.getByRole("button", { name: "还原", exact: true }).click();
  if (await retention.inputValue() !== original) throw new Error("PLC restore did not recover saved configuration");
  await retention.fill("7");
  await page.getByRole("button", { name: "保存配置", exact: true }).click();
  await page.getByText("配置已保存", { exact: true }).waitFor();
  await page.getByRole("button", { name: "断开", exact: true }).click();
  await page.getByRole("button", { name: "连接", exact: true }).waitFor();
  await page.getByRole("button", { name: "连接", exact: true }).click();
  await page.getByRole("button", { name: "断开", exact: true }).waitFor();
  const row = page.getByRole("row").filter({ has: page.getByRole("cell", { name: "计划拍照点数", exact: true }) });
  await row.getByRole("button", { name: "写入", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox").fill("8");
  await dialog.getByRole("button", { name: "写入", exact: true }).click();
  await dialog.waitFor({ state: "hidden" });
  await page.waitForFunction(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const config = await invoke("plc_get_config"); const point = config.points.find(p => p.name === "计划拍照点数");
    return (await invoke("plc_get_values"))[point.id]?.value === 8;
  });
  await retention.fill(original); await page.getByRole("button", { name: "保存配置", exact: true }).click();
  const result = { operation: "PLC 草稿还原、保存、断开/连接、数值写入", passed: true, writtenValue: 8, protocol: "simulator" };
  await page.evaluate(result => window.__uiOperations.checks.push(result), result);
  await page.screenshot({ path: "output/playwright/ui-regression/plc-write.png", fullPage: true });
  return result;
}
