async (page) => {
  const id = "P0-TRICAM-UI";
  const paths = [[[980, 480], [300, 460]], [[980, 480], [700, 420], [320, 330]], [[980, 480], [640, 560], [330, 690]], [[980, 480], [300, 460]]];
  const read = () => page.evaluate(async id => window.__TAURI_INTERNALS__.invoke("workspace_get", { id }), id);
  const initial = await read();
  if (initial.workspace.doc.shots.some((s, k) => s.camera !== "cam1" || s.view !== [1, 2, 3, 1][k])) throw new Error("Four cam1 shots with views 1231 are required");
  const until = async (predicate, description, timeout = 10000) => {
    const started = Date.now();
    do {
      const state = await read();
      if (predicate(state)) return state;
      await page.waitForTimeout(150);
    } while (Date.now() - started < timeout);
    throw new Error(description + ": " + await page.locator("main").innerText());
  };
  const captures = [];
  const capture = async k => {
    const old = (await read()).workspace.frames[k].image?.id;
    for (let attempt = 1; attempt <= 8; attempt++) {
      await page.getByRole("button", { name: "取新样本", exact: true }).click();
      const started = Date.now();
      do {
        const state = await read();
        const frame = state.workspace.frames[k];
        if (frame.image && frame.image.id !== old && frame.views.length === 3) {
          captures.push({ k, attempt, id: frame.image.id });
          return state;
        }
        await page.waitForTimeout(150);
      } while (Date.now() - started < 2000);
      if (!await page.getByText("正在处理 PLC 事务，请稍后重试取图", { exact: true }).isVisible()) throw new Error("Capture failed: " + await page.locator("main").innerText());
    }
    throw new Error("Capture repeatedly collided with PLC transactions");
  };
  if (initial.workspace.doc.triggerMode !== "fly") {
    await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "拍照点规划", exact: true }).click();
    await page.getByRole("combobox", { name: "触发方式", exact: true }).selectOption("fly");
    await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
    await until(v => v.workspace.doc.triggerMode === "fly", "Fly mode was not saved");
    await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "单帧示教", exact: true }).click();
  }
  const results = [];
  for (let k = 0; k < 4; k++) {
    await page.getByRole("button", { name: `选择帧 k${k + 1}`, exact: true }).click();
    const before = (await read()).workspace.frames[k];
    if (before.saved) throw new Error(`k${k + 1} was already saved; do not replace evidence`);
    await capture(k);
    const svg = page.locator("svg.wp-gray-image.editable");
    await svg.waitFor();
    await svg.scrollIntoViewIfNeeded();
    const clear = page.getByRole("button", { name: "清空中线", exact: true });
    if (await clear.isEnabled()) await clear.click();
    await svg.scrollIntoViewIfNeeded();
    for (const point of paths[k]) {
      const location = await svg.locator("g").first().evaluate((g, p) => {
        const mapped = new DOMPoint(p[0], p[1]).matrixTransform(g.getScreenCTM());
        return { x: mapped.x, y: mapped.y };
      }, point);
      await page.mouse.click(location.x, location.y);
    }
    await page.getByRole("spinbutton", { name: "像素当量", exact: true }).fill("0.112");
    const saveLine = page.getByRole("button", { name: "保存中线", exact: true });
    if (await saveLine.isEnabled()) await saveLine.click();
    await until(v => v.workspace.doc.shots[k].mmPerPx === 0.112 && JSON.stringify(v.workspace.doc.shots[k].path) === JSON.stringify(paths[k]), `k${k + 1} line was not saved`);
    let recapturedForSimulationPath = false;
    if (k === 1 || k === 2) {
      await capture(k);
      recapturedForSimulationPath = true;
    }
    await page.getByRole("button", { name: "试测当前帧", exact: true }).click();
    await until(v => !!v.workspace.frames[k].trial, `k${k + 1} trial did not complete`, 30000);
    const tested = await read();
    const frame = tested.workspace.frames[k];
    if (!frame.trial.passed || frame.trial.coverage < 0.8 || !frame.trial.measurement) throw new Error(JSON.stringify({ k, trial: frame.trial, shot: tested.workspace.doc.shots[k] }));
    await page.getByRole("button", { name: "保存本帧示教", exact: true }).click();
    await until(v => v.workspace.frames[k].saved, `k${k + 1} teaching was not saved`);
    await page.screenshot({ path: `D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step5-ui/03-teaching-k${k + 1}.png`, fullPage: true });
    results.push({ k, recapturedForSimulationPath, shot: tested.workspace.doc.shots[k], image: frame.image, views: frame.views, trial: { ...frame.trial, measurement: { stations: frame.trial.measurement?.width?.length, runId: frame.trial.measurement?.runId } } });
  }
  const final = await read();
  if (final.workspace.frames.some(f => !f.saved || !f.trial?.passed || f.image?.size?.[0] !== 1280 || f.image?.size?.[1] !== 1024)) throw new Error("All four full-resolution frames must be saved after a real successful trial");
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "工件总览", exact: true }).click();
  return { id, saved: final.workspace.frames.map(f => f.saved), captures, results };
}
