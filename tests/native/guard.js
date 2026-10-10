async (page) => {
  return await page.evaluate(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const records = await invoke("records_list");
    if (!["com.xyzrobotics.tujiaovision.ui-tests", "com.xyzrobotics.tujiaovision.p0-tests"].some(id => records.root.includes(id))) throw new Error("Non-isolated data directory");
    const cameras = await invoke("camera_rig_config");
    const plc = await invoke("plc_get_config");
    if (plc.connection.protocol !== "simulator" || cameras.some(c => c.source === "mvs")) throw new Error("UI testing requires offline sources");
    const phase = (await invoke("cycle_snapshot")).phase;
    if (!["IDLE", "NOT_READY", "FAULT"].includes(phase)) throw new Error("UI configuration testing requires a stopped cycle");
    window.__uiOperations = { startedAt: new Date().toISOString(), checks: [], initial: { cameras, plc, phase }, dataRoot: records.root };
    return { dataRoot: records.root, cameraCount: cameras.length, plcProtocol: plc.connection.protocol, phase };
  });
}
