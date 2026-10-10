async (page) => {
  const initial = await page.evaluate(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    return { records: await invoke("records_list"), cameras: await invoke("camera_rig_config"), plc: await invoke("plc_get_config"), status: await invoke("plc_get_status") };
  });
  if (!initial.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || initial.cameras.some(c => c.source === "mvs") || (initial.status.state === "connected" && initial.plc.connection.protocol !== "simulator")) throw new Error("Only the isolated offline P0 app may be configured");
  const specs = [
    ["part_start", "工件开始", "C10", "bool", "rising", "partStart"],
    ["part_end", "运动结束", "C11", "bool", "rising", "partEnd"],
    ["result_ack", "结果确认", "C12", "bool", "rising", "resultAck"],
    ["fault_reset", "故障复位", "C13", "bool", "rising", "faultReset"],
    ["part_sn", "工件序列号", "HR100", "u32", "none", "partSn"],
    ["product_code", "产品代码", "HR102", "u16", "none", "productCode"],
    ["shot_count", "计划拍照点数", "HR103", "u16", "none", "shotCount"],
    ["vision_ready", "视觉就绪", "C20", "bool", "none", "visionReady"],
    ["armed", "已布防", "C21", "bool", "none", "armed"],
    ["busy", "检测中", "C22", "bool", "none", "busy"],
    ["done", "结果有效", "C23", "bool", "none", "done"],
    ["result_code", "结果码", "HR110", "u16", "none", "resultCode"],
    ["fault_code", "异常码", "HR111", "u16", "none", "faultCode"],
    ["result_sn", "结果 SN", "HR112", "u32", "none", "resultSn"],
    ["heartbeat", "上位机心跳", "C0", "bool", "none", ""],
  ];
  const points = specs.map(([id, name, address, dataType, edge, tag]) => ({ id: "p_" + id, name, address, dataType, edge, access: "readWrite", logChanges: !!tag, tags: tag ? [tag] : [], wordOrder: dataType === "bool" ? null : "ABCD", description: "P0 专用离线模拟器" }));
  await page.getByRole("combobox", { name: "协议", exact: true }).selectOption("simulator");
  await page.getByRole("button", { name: "导入/导出", exact: true }).click();
  await page.getByRole("textbox", { name: "地址表 JSON", exact: true }).fill(JSON.stringify(points, null, 2));
  await page.getByRole("button", { name: "应用到地址表", exact: true }).click();
  await page.getByRole("combobox", { name: "心跳点位", exact: true }).selectOption("p_heartbeat");
  await page.getByRole("button", { name: "保存配置", exact: true }).click();
  await page.getByText("配置已保存", { exact: true }).waitFor();
  await page.getByRole("button", { name: "连接", exact: true }).click();
  await page.getByRole("button", { name: "断开", exact: true }).waitFor();
  const result = await page.evaluate(async () => ({ config: await window.__TAURI_INTERNALS__.invoke("plc_get_config"), status: await window.__TAURI_INTERNALS__.invoke("plc_get_status") }));
  if (result.config.connection.protocol !== "simulator" || result.config.points.length !== 15 || result.config.points.some(p => !/^(C|HR)\d+$/.test(p.address)) || result.status.state !== "connected") throw new Error(JSON.stringify(result));
  await page.screenshot({ path: "output/playwright/p0-step5-ui/01-plc-simulator.png", fullPage: true });
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "设备与采集", exact: true }).click();
  return { operation: "UI replaces legacy S7 point table with 15 simulator C/HR points, saves and connects", protocol: result.config.connection.protocol, points: result.config.points.length, state: result.status.state, dataRoot: initial.records.root };
}
