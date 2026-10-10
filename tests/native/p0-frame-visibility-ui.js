async (page) => {
  const id = "P0-TRICAM-UI", recipeRevision = `${id}-v1`;
  const evidence = "output/playwright/p0-frame-visibility";
  const read = (command, args) => page.evaluate(async ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
  const before = { records: await read("records_list"), plc: await read("plc_get_config"), cameras: await read("camera_rig_config"), settings: await read("cycle_get_settings"), workspace: await read("workspace_get", { id }) };
  if (!before.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || before.plc.connection.protocol !== "simulator" || before.cameras.some(c => c.source !== "sim") || before.settings.timeouts.armMs !== 200 || !before.settings.vision || before.settings.record !== "all") throw new Error("Isolated P0 preview guard rejected the app");
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "PLC 通讯", exact: true }).click();
  if ((await read("plc_get_status")).state !== "connected") await page.getByRole("button", { name: "连接", exact: true }).click();
  await page.getByRole("button", { name: "断开", exact: true }).waitFor();
  let snapshot;
  for (let attempt = 0; attempt < 200; attempt++) {
    snapshot = await read("cycle_snapshot");
    if (snapshot.phase === "IDLE" && !snapshot.alarms.some(a => a.includes(id))) break;
    await page.waitForTimeout(100);
  }
  const production = await read("cycle_layout", { recipeId: id });
  if (snapshot.phase !== "IDLE" || snapshot.alarms.some(a => a.includes(id)) || production.revisionId !== recipeRevision || !production.teachingId) throw new Error(JSON.stringify({ snapshot, production }));
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "在线检测", exact: true }).click();
  let addedPart = false, runStarted = 0, detail, recorded;
  if (!snapshot.part) {
    const previous = await read("history_query", { query: { recipeId: id, limit: 1 } });
    await page.getByRole("combobox", { name: "模拟配方", exact: true }).selectOption(id);
    await page.getByRole("combobox", { name: "模拟工况", exact: true }).selectOption("normal");
    runStarted = Date.now();
    await page.getByRole("button", { name: "运行一件", exact: true }).click();
    addedPart = true;
    for (let attempt = 0; attempt < 200; attempt++) {
      const recent = await read("history_query", { query: { recipeId: id, limit: 1 } });
      if (recent.items[0]?.id > (previous.items[0]?.id ?? -1)) {
        detail = await read("history_detail", { id: recent.items[0].id });
        recorded = await read("workspace_record_images", { historyId: detail.summary.id });
        if (recorded.complete && recorded.frames.length === 4 && recorded.frames.every(f => f.available) && !(await read("sim_status")).running) break;
      }
      await page.waitForTimeout(100);
    }
    snapshot = await read("cycle_snapshot");
  } else {
    const recent = await read("history_query", { query: { recipeId: id, sn: String(snapshot.part.sn), limit: 1 } });
    if (recent.items[0]?.sn !== snapshot.part.sn) throw new Error("Completed part has no matching history");
    detail = await read("history_detail", { id: recent.items[0].id });
    recorded = await read("workspace_record_images", { historyId: detail.summary.id });
  }
  const bundleId = snapshot.part?.bundleId;
  const measured = await read("cycle_part_data"), allLogs = await read("cycle_logs"), logs = allLogs.filter(line => line.ts >= runStarted);
  const arming = logs.findLast(line => line.ev === "armed↑ busy↑");
  const armMs = Number(arming?.msg.match(/布防耗时 (\d+) ms/)?.[1]);
  if (!detail || !recorded?.complete || recorded.frames.length !== 4 || recorded.frames.some(f => !f.available || f.error) || snapshot.phase !== "IDLE" || snapshot.part?.bundleId !== bundleId || detail.summary.recipeRevision !== recipeRevision || detail.summary.framesReceived !== 4 || detail.summary.faultCode !== 0 || !["OK", "OK_WITH_EXCURSION"].includes(detail.summary.verdict) || detail.frames.some(f => f.error) || detail.points.st.length !== 307 || measured.length !== 4 || measured.some(m => m.error || !m.located || m.bundleId !== bundleId) || !Number.isFinite(armMs) || armMs > 200) throw new Error(JSON.stringify({ detail, recorded, snapshot, measured, logs }));
  const scrollState = () => page.locator(".main-body").evaluate(element => ({ scrollTop: element.scrollTop, clientHeight: element.clientHeight, scrollHeight: element.scrollHeight, overflowY: getComputedStyle(element).overflowY }));
  const inspect = async k => {
    await page.getByRole("button", { name: "查看帧 k" + (k + 1), exact: true }).click();
    const svg = page.getByRole("img", { name: "SN " + detail.summary.sn + " · k" + (k + 1), exact: true });
    await svg.waitFor();
    await svg.scrollIntoViewIfNeeded();
    const geometry = await svg.evaluate(svg => {
      const box = element => { const r = element.getBoundingClientRect(); return { x: r.x, y: r.y, right: r.right, bottom: r.bottom, width: r.width, height: r.height }; };
      const viewport = svg.closest(".wp-viewport"), image = svg.querySelector("image"), path = svg.querySelector("polyline");
      if (!viewport || !image || !path) throw new Error("Missing runtime image, viewport, or centerline");
      const project = (x, y, element) => { const p = new DOMPoint(x, y).matrixTransform(element.getScreenCTM()); return { x: p.x, y: p.y }; };
      const width = Number(image.getAttribute("width")), height = Number(image.getAttribute("height"));
      const corners = [[0, 0], [width, 0], [width, height], [0, height]].map(([x, y]) => project(x, y, image));
      const vertices = Array.from(path.points).map(point => project(point.x, point.y, path));
      const centerline = vertices.flatMap((point, i) => i ? [point, { x: (point.x + vertices[i - 1].x) / 2, y: (point.y + vertices[i - 1].y) / 2 }] : [point]);
      const viewportBox = box(viewport), svgBox = box(svg);
      const inside = (point, r) => point.x >= r.x - 1 && point.x <= r.right + 1 && point.y >= r.y - 1 && point.y <= r.bottom + 1;
      const points = [...corners, ...centerline];
      const selectors = ["main", ".main-body", ".fly-main", ".insp-top", ".wp-runtime-frame", ".wp-runtime-frame .wp-panel-head", ".wp-runtime-frame .wp-viewport", ".wp-runtime-frame .wp-image-tools", ".wp-runtime-frame .wp-gray-image", ".wp-runtime-frame .wp-image-footer", ".wp-runtime-frame .wp-runtime-strip"];
      return { browser: { width: innerWidth, height: innerHeight }, image: { width, height }, viewport: viewportBox, svg: svgBox, corners, centerline, actualImageWidth: corners[1].x - corners[0].x, actualImageHeight: corners[2].y - corners[1].y, allCornersAndCenterlineInside: points.every(point => inside(point, viewportBox) && inside(point, svgBox)), allPointsVisibleInBrowser: points.every(point => inside(point, { x: 0, y: 0, right: innerWidth, bottom: innerHeight })), rects: selectors.flatMap(selector => Array.from(document.querySelectorAll(selector)).map(element => ({ selector, ...box(element) }))) };
    });
    if (geometry.image.width !== 1280 || geometry.image.height !== 1024 || !geometry.allCornersAndCenterlineInside || !geometry.allPointsVisibleInBrowser || geometry.actualImageWidth < 200) throw new Error(JSON.stringify(geometry));
    return { k, view: [1, 2, 3, 1][k], ...geometry };
  };
  const matrices = [];
  for (const size of [{ width: 1440, height: 900 }, { width: 1280, height: 800 }]) {
    await page.setViewportSize(size);
    const frames = [];
    for (let k = 0; k < 4; k++) {
      frames.push(await inspect(k));
      await page.screenshot({ path: evidence + "/29-final-preview-" + size.width + "-k" + (k + 1) + ".png", fullPage: true });
    }
    matrices.push({ size, frames });
  }
  await page.mouse.move(1100, 400);
  await page.mouse.wheel(0, -2500);
  await page.waitForTimeout(150);
  const scrollBefore = await scrollState();
  await page.mouse.wheel(0, 600);
  await page.waitForTimeout(150);
  const scrollAfter = await scrollState();
  const curveBox = await page.getByText("展开曲线", { exact: true }).evaluate(element => { const r = element.getBoundingClientRect(); return { top: r.top, bottom: r.bottom }; });
  if (scrollBefore.scrollHeight <= scrollBefore.clientHeight || scrollAfter.scrollTop <= scrollBefore.scrollTop || curveBox.top < 60 || curveBox.bottom > 800) throw new Error(JSON.stringify({ scrollBefore, scrollAfter, curveBox }));
  await page.screenshot({ path: evidence + "/29-final-preview-1280-scrolled.png", fullPage: true });
  await page.setViewportSize({ width: 1440, height: 900 });
  const restored = await inspect(1);
  const after = { settings: await read("cycle_get_settings"), workspace: await read("workspace_get", { id }), production: await read("cycle_layout", { recipeId: id }), cycle: await read("cycle_snapshot") };
  if (JSON.stringify(after.settings) !== JSON.stringify(before.settings) || JSON.stringify(after.workspace.workspace) !== JSON.stringify(before.workspace.workspace) || JSON.stringify(after.production) !== JSON.stringify(production) || after.cycle.part?.cycleId !== snapshot.part.cycleId) throw new Error("Preview verification changed settings, candidate teaching, production layout, or the completed part");
  return { nativeExeOnly: true, noInjectedStyles: true, addedPart, completedPart: { summary: detail.summary, cycleId: snapshot.part.cycleId, bundleId, frames: detail.frames, points: detail.points.st.length, measurements: measured.map(m => ({ k: m.k, ms: m.ms, points: m.idx.length, error: m.error })), arming: { configuredArmMs: 200, elapsedMs: armMs, log: arming }, recorded }, engine: await read("engine_status"), prewarm: allLogs.find(line => line.ev === "生产预热" && line.msg.includes(bundleId)), matrices, scrollBefore, scrollAfter, curveBox, restored, settingsCandidateProductionUnchanged: true, accuracyQualified: false, logs };
}

