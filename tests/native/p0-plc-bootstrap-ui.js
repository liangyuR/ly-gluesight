async (page) => {
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const initial = { records: await read("records_list"), cameras: await read("camera_rig_config"), status: await read("plc_get_status"), offline: await read("site_status") };
  if (!initial.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || initial.cameras.some(c => c.source !== (initial.offline.mode === "offline" ? "sim" : "mvs")) || (initial.status.state === "connected" && !(initial.offline.mode === "offline"))) throw new Error("Only the isolated offline P0 app may be configured");
  if (!(initial.offline.mode === "offline")) {
    await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "设备与采集", exact: true }).click();
    await page.getByRole("link", { name: "离线控制台", exact: true }).click();
    await page.getByRole("combobox", { name: "离线来源", exact: true }).selectOption("synthetic");
    await page.getByRole("button", { name: "进入离线", exact: true }).click();
  }
  let result;
  for (let attempt = 0; attempt < 100; attempt++) {
    result = { offline: await read("site_status"), config: await read("plc_get_config"), status: await read("plc_get_status") };
    if ((result.offline.mode === "offline") && result.status.state === "connected") break;
    await page.waitForTimeout(100);
  }
  const { offline, config, status } = result;
  if (!(offline.mode === "offline") || status.state !== "connected" || config.connection.protocol !== "s7" || config.connection.host !== "127.0.0.1" || config.connection.port !== offline.port || config.points.length !== 28 || config.points.some(p => !p.address.startsWith("DB100."))) throw new Error(JSON.stringify(result));
  await page.screenshot({ path: "output/playwright/p0-step5-ui/01-offline-s7.png", fullPage: true });
  return { operation: "UI enables the in-process offline S7 line with the phase-1 DB100 point table and connects", protocol: config.connection.protocol, port: offline.port, points: config.points.length, state: status.state, dataRoot: initial.records.root };
}
