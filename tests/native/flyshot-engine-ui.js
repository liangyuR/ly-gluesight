async (page) => {
  await page.getByRole("textbox", { name: "核心库路径（lyflow_core.dll）" }).fill("D:\\project\\LyFlow\\build\\core\\bin\\lyflow_core.dll");
  await page.getByRole("combobox", { name: "帧录制", exact: true }).selectOption("all");
  await page.getByRole("heading", { name: "测量与帧录制", exact: true }).locator("..").getByRole("button", { name: "保存", exact: true }).click();
  await page.getByText("已保存", { exact: true }).waitFor();
  const state = await page.evaluate(async () => ({
    settings: await window.__TAURI_INTERNALS__.invoke("cycle_get_settings"),
    engine: await window.__TAURI_INTERNALS__.invoke("engine_status")
  }));
  if (!state.settings.vision || state.settings.record !== "all" || !state.engine.ready) throw new Error(JSON.stringify(state));
  await page.evaluate(state => window.__uiOperations.checks.push({ operation: "飞拍图像引擎与全帧录制的分区保存", passed: true, engine: state.engine }), state);
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "飞拍工位标定", exact: true }).click();
  return state;
}
