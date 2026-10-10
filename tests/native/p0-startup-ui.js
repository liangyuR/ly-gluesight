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
  const prewarm = logs.find(l => l.ev === "生产预热" && l.msg.includes("P0-TRICAM-UI") && l.msg.includes("已就绪"));
  const production = await read("cycle_layout", { recipeId: "P0-TRICAM-UI" });
  if (!prewarm || snapshot.phase !== "IDLE" || snapshot.alarms.some(a => a.includes("P0-TRICAM-UI")) || production.revisionId !== "P0-TRICAM-UI-v1" || !production.teachingId) throw new Error(JSON.stringify({ snapshot, logs, production }));
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  await page.screenshot({ path: "output/playwright/p0-step5-ui/23-pool-original-bundle-prewarm.png", fullPage: true });
  return { initial, prewarm, plc: await read("plc_get_status"), engine: await read("engine_status"), recipeRevision: production.revisionId, teachingId: production.teachingId, snapshot, logs, values: await read("plc_get_values") };
}
