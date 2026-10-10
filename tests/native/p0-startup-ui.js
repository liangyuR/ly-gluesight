async (page) => {
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const initial = { records: await read("records_list"), config: await read("plc_get_config"), cameras: await read("camera_rig_config"), snapshot: await read("cycle_snapshot"), logs: await read("cycle_logs"), values: await read("plc_get_values") };
  if (!initial.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || initial.config.connection.protocol !== "simulator" || initial.cameras.some(c => c.source !== "sim")) throw new Error("P0 offline guard rejected the restarted app");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "PLC 通讯", exact: true }).click();
  if ((await read("plc_get_status")).state !== "connected") await page.getByRole("button", { name: "连接", exact: true }).click();
  await page.getByRole("button", { name: "断开", exact: true }).waitFor();
  let snapshot;
  for (let attempt = 0; attempt < 200; attempt++) {
    snapshot = await read("cycle_snapshot");
    if (snapshot.phase === "IDLE" && !snapshot.alarms.some(a => a.includes("P0-TRICAM-UI"))) break;
    await page.waitForTimeout(100);
  }
  const logs = await read("cycle_logs");
  const prewarm = logs.find(l => l.ev === "生产预热" && l.msg.includes("3b643e189e0f73e5") && l.msg.includes("已就绪"));
  const production = await read("cycle_layout", { recipeId: "P0-TRICAM-UI" });
  if (!prewarm || snapshot.phase !== "IDLE" || snapshot.alarms.some(a => a.includes("P0-TRICAM-UI")) || production.hash !== "c1574d7504b2a8d4" || production.teachingHash !== "3e55860501858d50") throw new Error(JSON.stringify({ snapshot, logs, production }));
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  await page.screenshot({ path: "D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step5-ui/23-pool-original-bundle-prewarm.png", fullPage: true });
  return { initial, prewarm, plc: await read("plc_get_status"), engine: await read("engine_status"), recipeHash: production.hash, teachingHash: production.teachingHash, originalBundleHash: "3b643e189e0f73e5", snapshot, logs, values: await read("plc_get_values") };
}
