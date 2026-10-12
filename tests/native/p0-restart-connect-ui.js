async (page) => {
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const records = await read("records_list");
  const plc = await read("plc_get_status"), offline = await read("site_status");
  const cameras = await read("camera_rig_config");
  if (!records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || (plc.state === "connected" && !(offline.mode === "offline")) || cameras.some(c => c.source !== (offline.mode === "offline" ? "sim" : "mvs"))) throw new Error("P0 offline guard rejected the restarted app");
  if (!((await read("site_status")).mode === "offline")) {
    await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "设备与采集", exact: true }).click();
    await page.getByRole("link", { name: "离线控制台", exact: true }).click();
    await page.getByRole("combobox", { name: "离线来源", exact: true }).selectOption("synthetic");
    await page.getByRole("button", { name: "进入离线", exact: true }).click();
  }
  for (let attempt = 0; attempt < 100 && (await read("plc_get_status")).state !== "connected"; attempt++) await page.waitForTimeout(100);
  if (!((await read("site_status")).mode === "offline") || (await read("plc_get_status")).state !== "connected") throw new Error("Offline S7 line did not connect");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  if ((await read("cycle_snapshot")).phase === "FAULT") await page.getByRole("button", { name: "复位故障", exact: true }).click();
  const status = await read("plc_get_status");
  if (status.state !== "connected") throw new Error(JSON.stringify(status));
  let snapshot;
  for (let attempt = 0; attempt < 200; attempt++) {
    snapshot = await read("cycle_snapshot");
    if (!snapshot.alarms.some(a => a.includes("P0-TRICAM-UI"))) break;
    await page.waitForTimeout(100);
  }
  const logs = await read("cycle_logs");
  const prewarm = logs.find(l => l.ev === "生产预热" && l.msg.includes("P0-TRICAM-UI") && l.msg.includes("已就绪"));
  if (!prewarm || snapshot.alarms.some(a => a.includes("P0-TRICAM-UI"))) throw new Error(JSON.stringify({ snapshot, logs }));
  const production = await read("cycle_layout", { recipeId: "P0-TRICAM-UI" });
  if (production.revisionId !== "P0-TRICAM-UI-v1" || !production.teachingId) throw new Error("Original production identity changed");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  await page.screenshot({ path: "output/playwright/p0-step5-ui/16-original-bundle-prewarm.png", fullPage: true });
  return { prewarm, plc: status, engine: await read("engine_status"), recipeRevision: production.revisionId, teachingId: production.teachingId, snapshot };
}
