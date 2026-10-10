async (page) => {
  const initial = await page.evaluate(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    return { records: await invoke("records_list"), plc: await invoke("plc_get_config") };
  });
  if (!initial.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || initial.plc.connection.protocol !== "simulator") throw new Error("P0 setup requires its isolated offline profile");
  await page.getByRole("button", { name: "模拟相机", exact: true }).click();
  await page.getByRole("combobox", { name: "设备视角", exact: true }).selectOption("3");
  await page.getByRole("button", { name: "触发（飞拍）", exact: true }).click();
  await page.getByRole("button", { name: "保存并应用", exact: true }).click();
  await page.waitForFunction(async () => (await window.__TAURI_INTERNALS__.invoke("camera_rig_config"))[0].viewCount === 3);
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "系统设置", exact: true }).click();
  await page.getByRole("combobox", { name: "飞拍配方", exact: true }).selectOption("lyFlow");
  await page.getByRole("textbox", { name: "核心库路径（lyflow_core.dll）", exact: true }).fill("C:/Users/11601/AppData/Local/Temp/gluesight-p0-recovery-20261010/native/lyflow_core.dll");
  await page.getByRole("combobox", { name: "帧录制", exact: true }).selectOption("all");
  await page.getByRole("heading", { name: "测量与帧录制", exact: true }).locator("..").getByRole("button", { name: "保存", exact: true }).click();
  await page.waitForFunction(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const settings = await invoke("cycle_get_settings");
    return settings.vision && settings.record === "all" && (await invoke("engine_status")).ready;
  });
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "配方库", exact: true }).click();
  return await page.evaluate(async () => ({ cameras: await window.__TAURI_INTERNALS__.invoke("camera_rig_config"), engine: await window.__TAURI_INTERNALS__.invoke("engine_status") }));
}
