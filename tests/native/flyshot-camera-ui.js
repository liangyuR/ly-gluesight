async (page) => {
  await page.getByRole("button", { name: "模拟相机", exact: true }).click();
  await page.getByRole("button", { name: "触发（飞拍）", exact: true }).click();
  await page.getByRole("button", { name: "保存并应用", exact: true }).click();
  await page.waitForFunction(async () => {
    const cfg = await window.__TAURI_INTERNALS__.invoke("camera_rig_config");
    return cfg[0].source === "sim" && cfg[0].acquisition === "triggered";
  });
  const cfg = await page.evaluate(async () => (await window.__TAURI_INTERNALS__.invoke("camera_rig_config"))[0]);
  await page.evaluate(cfg => window.__uiOperations.checks.push({ operation: "模拟相机源、飞拍触发采集、保存应用", passed: true, camera: cfg.id }), cfg);
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "系统设置", exact: true }).click();
  return { source: cfg.source, acquisition: cfg.acquisition };
}
