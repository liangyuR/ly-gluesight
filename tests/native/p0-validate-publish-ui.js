async (page) => {
  const id = "P0-TRICAM-UI";
  const root = "D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step5-ui";
  const read = () => page.evaluate(async id => window.__TAURI_INTERNALS__.invoke("workspace_get", { id }), id);
  const until = async (predicate, description, timeout = 10000) => {
    const started = Date.now();
    do {
      const state = await read();
      if (predicate(state)) return state;
      await page.waitForTimeout(150);
    } while (Date.now() - started < timeout);
    throw new Error(description + ": " + await page.locator("main").innerText());
  };
  const initial = await read();
  if (initial.workspace.frames.some(f => !f.saved || !f.trial?.passed)) throw new Error("All four real trials must be saved before representative validation");
  await page.getByRole("button", { name: "总览选择帧 k2", exact: true }).press("ArrowDown");
  await page.getByRole("button", { name: "保存总览", exact: true }).click();
  await until(v => v.workspace.overview.saved, "Overview was not saved");
  await page.screenshot({ path: root + "/05-overview.png", fullPage: true });
  await page.getByRole("navigation", { name: "操作导航" }).getByRole("link", { name: "验证与发布", exact: true }).click();
  for (const [folder, name, expected] of [["good", "P0 UI 合格原图", "OK"], ["gap", "P0 UI 8mm 断胶原图", "NG_GAP"]]) {
    await page.getByRole("button", { name: "导入原图样本组", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "导入代表性原图样本组", exact: true });
    await dialog.getByRole("textbox", { name: "样本名称", exact: true }).fill(name);
    await dialog.getByRole("combobox", { name: "人工确认的期望结论", exact: true }).selectOption(expected);
    for (let k = 1; k <= 4; k++) await dialog.locator(`input[aria-label="k${k} 原图"]`).setInputFiles(`${root}/samples/${folder}/k${k}.pgm`);
    await dialog.getByRole("button", { name: "保存样本组", exact: true }).click();
    await until(v => v.workspace.sampleBank.some(b => b.name === name && b.expected === expected), "Original image group was not imported", 30000);
    await page.getByRole("checkbox", { name: "选用样本 " + name, exact: true }).check();
  }
  await page.getByRole("button", { name: "运行规则与图像验证", exact: true }).click();
  const validated = await until(v => v.workspace.validation && v.workspace.validation.samples.length === 2, "Real image validation did not complete", 30000);
  await page.screenshot({ path: root + "/06-validation.png", fullPage: true });
  if (!validated.workspace.validation.passed || validated.workspace.validation.checks.some(c => !c.passed) || validated.workspace.validation.samples.some(s => !s.passed || s.actual !== s.expected)) throw new Error(JSON.stringify(validated.workspace.validation));
  await page.getByRole("button", { name: "发布生产配方", exact: true }).click();
  await page.getByRole("dialog", { name: "发布生产配方", exact: true }).getByRole("button", { name: `确认发布 v${validated.workspace.doc.version}`, exact: true }).click();
  const published = await until(v => v.productionVersion === validated.workspace.doc.version && !v.workspace.pending, "Validated release did not become active", 30000);
  await page.screenshot({ path: root + "/07-published.png", fullPage: true });
  await page.getByRole("link", { name: "查看在线检测", exact: true }).click();
  return { id, validation: validated.workspace.validation, publication: { productionVersion: published.productionVersion, candidateVersion: published.workspace.doc.version, baseRevision: published.workspace.baseRevision, revision: published.workspace.revision, error: published.workspace.publishError } };
}
