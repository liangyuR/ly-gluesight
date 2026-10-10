async (page) => {
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const guard = { records: await read("records_list"), plc: await read("plc_get_config"), status: await read("plc_get_status"), cycle: await read("cycle_snapshot"), sim: await read("sim_status") };
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.plc.connection.protocol !== "simulator" || guard.status.state !== "connected" || !["IDLE", "FAULT"].includes(guard.cycle.phase) || guard.sim.running) throw new Error(JSON.stringify(guard));
  const reset = guard.plc.points.find(p => p.tags.includes("faultReset"));
  if (reset?.id !== "p_fault_reset" || reset.name !== "故障复位" || reset.address !== "C13") throw new Error("Unexpected isolated simulator reset point");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "PLC 通讯", exact: true }).click();
  const row = page.getByRole("row").filter({ has: page.getByText("故障复位", { exact: true }) });
  for (const value of [false, true, false]) {
    await row.getByRole("button", { name: "写入", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "写入 · 故障复位", exact: true });
    await dialog.getByRole("button", { name: value ? "置 1 (ON)" : "置 0 (OFF)", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    await page.waitForTimeout(150);
  }
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  const after = await read("cycle_snapshot");
  if (after.phase !== "IDLE" || after.fault) throw new Error(JSON.stringify(after));
  return { before: guard.cycle, after, logs: (await read("cycle_logs")).slice(-8) };
}
