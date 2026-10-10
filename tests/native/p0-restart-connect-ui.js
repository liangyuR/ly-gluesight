async (page) => {
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const records = await read("records_list");
  const config = await read("plc_get_config");
  const cameras = await read("camera_rig_config");
  if (!records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || config.connection.protocol !== "simulator" || cameras.some(c => c.source !== "sim")) throw new Error("P0 offline guard rejected the restarted app");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "PLC 通讯", exact: true }).click();
  if ((await read("plc_get_status")).state !== "connected") await page.getByRole("button", { name: "连接", exact: true }).click();
  await page.getByRole("button", { name: "断开", exact: true }).waitFor();
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
