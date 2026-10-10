async (page) => {
  const id = "P0-TRICAM-UI";
  const evidence = "output/playwright/p0-step5-ui";
  const read = () => page.evaluate(async id => window.__TAURI_INTERNALS__.invoke("workspace_get", { id }), id);
  const nav = name => page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name, exact: true }).click();
  const until = async (predicate, message, timeout = 10000) => {
    const started = Date.now();
    do {
      const state = await read();
      if (predicate(state)) return state;
      await page.waitForTimeout(100);
    } while (Date.now() - started < timeout);
    throw new Error(message + ": " + await page.locator("main").innerText());
  };
  const guard = await page.evaluate(async () => ({ records: await window.__TAURI_INTERNALS__.invoke("records_list"), cameras: await window.__TAURI_INTERNALS__.invoke("camera_rig_config"), plc: await window.__TAURI_INTERNALS__.invoke("plc_get_config"), cycle: await window.__TAURI_INTERNALS__.invoke("cycle_snapshot"), production: await window.__TAURI_INTERNALS__.invoke("cycle_layout", { recipeId: "P0-TRICAM-UI" }) }));
  if (!guard.records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || guard.cameras.some(c => c.source !== "sim") || guard.plc.connection.protocol !== "simulator" || !["IDLE", "FAULT"].includes(guard.cycle.phase)) throw new Error(JSON.stringify(guard));
  const initial = await read();
  if (initial.workspace.frames.some(f => !f.saved || !f.trial?.passed)) throw new Error("Start invalidation checks with all four saved native trials");
  const assertOnly = (before, after, k) => {
    if (after.workspace.frames[k].saved || after.workspace.frames[k].trial) throw new Error(`k${k + 1} did not become invalid`);
    for (let j = 0; j < 4; j++) if (j !== k && (JSON.stringify(before.workspace.frames[j]) !== JSON.stringify(after.workspace.frames[j]) || JSON.stringify(before.workspace.doc.shots[j]) !== JSON.stringify(after.workspace.doc.shots[j]))) throw new Error(`Unrelated k${j + 1} was changed by k${k + 1}`);
    if (after.productionVersion !== before.productionVersion || after.workspace.baseRevision !== before.workspace.baseRevision) throw new Error("Candidate edits changed the published production identity");
  };
  const draw = async path => {
    const svg = page.locator("svg.wp-gray-image.editable");
    await svg.waitFor();
    const clear = page.getByRole("button", { name: "清空中线", exact: true });
    if (await clear.isEnabled()) await clear.click();
    await svg.scrollIntoViewIfNeeded();
    for (const point of path) {
      const location = await svg.locator("g").first().evaluate((g, p) => {
        const mapped = new DOMPoint(p[0], p[1]).matrixTransform(g.getScreenCTM());
        return { x: mapped.x, y: mapped.y };
      }, point);
      await page.mouse.click(location.x, location.y);
    }
    await page.getByRole("spinbutton", { name: "像素当量", exact: true }).fill("0.112");
    const saveLine = page.getByRole("button", { name: "保存中线", exact: true });
    if (await saveLine.isEnabled()) await saveLine.click();
  };
  const trialAndSave = async k => {
    const previous = (await read()).workspace.frames[k].trial;
    await page.getByRole("button", { name: "试测当前帧", exact: true }).click();
    const tested = await until(v => v.workspace.frames[k].trial && JSON.stringify(v.workspace.frames[k].trial) !== JSON.stringify(previous), "Native trial did not complete", 30000);
    if (!tested.workspace.frames[k].trial.passed) throw new Error(JSON.stringify(tested.workspace.frames[k].trial));
    await page.getByRole("button", { name: "保存本帧示教", exact: true }).click();
    return until(v => v.workspace.frames[k].saved, "The restored frame was not saved");
  };
  const restoreCapture = async k => {
    await nav("单帧示教");
    await page.getByRole("button", { name: `选择帧 k${k + 1}`, exact: true }).click();
    for (let attempt = 0; attempt < 8; attempt++) {
      await page.getByRole("button", { name: "取新样本", exact: true }).click();
      for (let poll = 0; poll < 15; poll++) {
        if ((await read()).workspace.frames[k].image) return trialAndSave(k);
        await page.waitForTimeout(100);
      }
      if (!await page.getByText("正在处理 PLC 事务，请稍后重试取图", { exact: true }).isVisible()) throw new Error("Restored capture failed: " + await page.locator("main").innerText());
    }
    throw new Error("PLC capture gate did not become available");
  };
  const rows = [];
  await nav("单帧示教");
  await page.getByRole("combobox", { name: "当前配方", exact: true }).selectOption(id);
  await page.getByRole("button", { name: "选择帧 k2", exact: true }).click();
  const beforeView = await read();
  const path = beforeView.workspace.doc.shots[1].path;
  await page.getByRole("button", { name: "选择视角 3", exact: true }).click();
  const wrong = await until(v => v.workspace.doc.shots[1].view === 3, "Frozen view did not change");
  assertOnly(beforeView, wrong, 1);
  if (wrong.workspace.doc.shots[1].path.length || wrong.workspace.doc.shots[1].mmPerPx != null || wrong.workspace.frames[1].image.view !== 3) throw new Error("View change retained old teaching or selected the wrong frozen image");
  await draw(path);
  await until(v => v.workspace.doc.shots[1].path.length === 3 && v.workspace.doc.shots[1].mmPerPx === 0.112, "Wrong-view trial line was not saved");
  await page.getByRole("button", { name: "试测当前帧", exact: true }).click();
  const failed = await until(v => !!v.workspace.frames[1].trial, "Wrong-view native trial did not complete", 30000);
  if (failed.workspace.frames[1].trial.passed || failed.workspace.frames[1].saved || await page.getByRole("button", { name: "保存本帧示教", exact: true }).isEnabled()) throw new Error("A background-only frozen view incorrectly passed native teaching");
  await page.screenshot({ path: evidence + "/11-wrong-view-rejected.png", fullPage: true });
  rows.push({ kind: "frozenView", k: 1, from: 2, to: 3, saved: wrong.workspace.frames.map(f => f.saved), nativeTrial: failed.workspace.frames[1].trial, imageId: failed.workspace.frames[1].image.id });
  await page.getByRole("button", { name: "选择视角 2", exact: true }).click();
  await until(v => v.workspace.doc.shots[1].view === 2 && !v.workspace.frames[1].trial, "Original frozen view was not restored");
  await draw(path);
  await until(v => v.workspace.doc.shots[1].path.length === 3 && v.workspace.doc.shots[1].mmPerPx === 0.112, "Restored line was not saved");
  await trialAndSave(1);
  for (const [kind, k, field, changed] of [["pose", 0, "Pose", "P1-UI-CHANGED"], ["calibrationReference", 3, "标定引用", "p0-alt-calib"]]) {
    const before = await read();
    const original = kind === "pose" ? before.workspace.doc.shots[k].poseId : before.workspace.doc.shots[k].calib ?? "";
    await nav("拍照点规划");
    await page.getByRole("textbox", { name: `拍照点 ${k + 1} · ${field}`, exact: true }).fill(changed);
    await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
    const invalidated = await until(v => v.workspace.revision > before.workspace.revision, "Candidate identity edit was not saved");
    assertOnly(before, invalidated, k);
    if (invalidated.workspace.frames[k].image || invalidated.workspace.frames[k].views.length) throw new Error("The changed identity retained incompatible frozen images");
    await page.screenshot({ path: `${evidence}/12-${kind}-invalidated.png`, fullPage: true });
    rows.push({ kind, k, from: original, to: changed, saved: invalidated.workspace.frames.map(f => f.saved), image: invalidated.workspace.frames[k].image, productionVersion: invalidated.productionVersion, baseRevision: invalidated.workspace.baseRevision });
    await page.getByRole("textbox", { name: `拍照点 ${k + 1} · ${field}`, exact: true }).fill(original);
    await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
    await until(v => v.workspace.revision > invalidated.workspace.revision, "Original identity was not restored");
    await restoreCapture(k);
  }
  await nav("设备与采集");
  let configs = await page.evaluate(async () => window.__TAURI_INTERNALS__.invoke("camera_rig_config"));
  const known = new Set(configs.map(c => c.id));
  for (let attempt = 0; attempt < 8; attempt++) {
    await page.getByRole("button", { name: "添加相机", exact: true }).click();
    for (let poll = 0; poll < 15; poll++) {
      configs = await page.evaluate(async () => window.__TAURI_INTERNALS__.invoke("camera_rig_config"));
      if (configs.some(c => !known.has(c.id))) break;
      await page.waitForTimeout(100);
    }
    if (configs.some(c => !known.has(c.id))) break;
  }
  const extra = configs.find(c => !known.has(c.id));
  if (!extra || extra.source !== "sim" || extra.viewCount !== 3 || extra.acquisition !== "triggered") throw new Error(JSON.stringify(configs));
  const beforeCamera = await read();
  if (beforeCamera.workspace.frames.some(f => !f.saved)) throw new Error("Adding an unrelated simulator camera invalidated existing teaching");
  await nav("拍照点规划");
  await page.getByRole("combobox", { name: "拍照点 3 · 相机", exact: true }).selectOption(extra.id);
  await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
  const cameraChanged = await until(v => v.workspace.doc.shots[2].camera === extra.id, "The single camera binding was not saved");
  assertOnly(beforeCamera, cameraChanged, 2);
  if (cameraChanged.workspace.frames[2].image || cameraChanged.workspace.frames[2].views.length) throw new Error("The changed camera retained incompatible frozen images");
  await page.screenshot({ path: evidence + "/13-camera-invalidated.png", fullPage: true });
  rows.push({ kind: "cameraBinding", k: 2, from: "cam1", to: extra.id, saved: cameraChanged.workspace.frames.map(f => f.saved), image: cameraChanged.workspace.frames[2].image, productionVersion: cameraChanged.productionVersion, baseRevision: cameraChanged.workspace.baseRevision });
  await page.getByRole("combobox", { name: "拍照点 3 · 相机", exact: true }).selectOption("cam1");
  await page.getByRole("button", { name: "保存候选配置", exact: true }).last().click();
  await until(v => v.workspace.doc.shots[2].camera === "cam1", "The original camera binding was not restored");
  const restored = await restoreCapture(2);
  const finalProduction = await page.evaluate(async id => window.__TAURI_INTERNALS__.invoke("cycle_layout", { recipeId: id }), id);
  if (restored.workspace.frames.some(f => !f.saved || !f.trial?.passed) || finalProduction.revisionId !== guard.production.revisionId || finalProduction.teachingId !== guard.production.teachingId) throw new Error("The candidate did not recover or published identity changed");
  await page.screenshot({ path: evidence + "/14-teaching-restored.png", fullPage: true });
  return { rows, final: { saved: restored.workspace.frames.map(f => f.saved), views: restored.workspace.doc.shots.map(s => s.view), version: restored.workspace.doc.version, revision: restored.workspace.revision, productionVersion: restored.productionVersion, productionRevision: finalProduction.revisionId, teachingId: finalProduction.teachingId }, extraSimulatorCamera: extra };
}
