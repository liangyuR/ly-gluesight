async (page) => {
  const id = "P0-TRICAM-UI";
  const root = "D:/project/ly-gluesight/tmp/p0-step7-regression/tmp/p0-step5-ui";
  const read = () => page.evaluate(async id => window.__TAURI_INTERNALS__.invoke("workspace_get", { id }), id);
  const initial = await read();
  const firstGood = initial.workspace.validation?.samples.find(s => s.name === "P0 UI 合格原图");
  if (!firstGood || firstGood.actual !== "OK_WITH_EXCURSION" || firstGood.expected !== "OK" || firstGood.passed) throw new Error("The initial strict-OK mismatch must remain documented before the reviewed label is used");
  await page.getByRole("combobox", { name: "P0 UI 合格原图期望结论", exact: true }).selectOption("OK_WITH_EXCURSION");
  await page.getByRole("button", { name: "运行规则与图像验证", exact: true }).click();
  let validated;
  for (let attempt = 0; attempt < 200; attempt++) {
    validated = await read();
    if (validated.workspace.validation?.checkedAt !== initial.workspace.validation.checkedAt) break;
    await page.waitForTimeout(150);
  }
  await page.screenshot({ path: root + "/06-reviewed-validation.png", fullPage: true });
  if (!validated.workspace.validation?.passed || validated.workspace.validation.checks.some(c => !c.passed) || validated.workspace.validation.samples.some(s => !s.passed || s.actual !== s.expected)) throw new Error(JSON.stringify(validated.workspace.validation));
  await page.getByRole("button", { name: "发布生产配方", exact: true }).click();
  await page.getByRole("dialog", { name: "发布生产配方", exact: true }).getByRole("button", { name: `确认发布 v${validated.workspace.doc.version}`, exact: true }).click();
  let published;
  for (let attempt = 0; attempt < 200; attempt++) {
    published = await read();
    if (published.productionVersion === validated.workspace.doc.version && !published.workspace.pending) break;
    if (published.workspace.publishError) throw new Error(published.workspace.publishError);
    await page.waitForTimeout(150);
  }
  if (published.productionVersion !== validated.workspace.doc.version || published.workspace.pending || published.workspace.publishError) throw new Error(JSON.stringify(published));
  await page.screenshot({ path: root + "/07-published.png", fullPage: true });
  const production = await page.evaluate(async id => window.__TAURI_INTERNALS__.invoke("cycle_layout", { recipeId: id }), id);
  if (!production.teachingHash || production.shots.some((s, k) => s.view !== [1, 2, 3, 1][k])) throw new Error("The production recipe must contain the teaching identity and four planned views");
  await page.getByRole("link", { name: "查看在线检测", exact: true }).click();
  return { id, originalStrictOkMismatch: firstGood, reviewedValidation: validated.workspace.validation, publication: { productionVersion: published.productionVersion, candidateVersion: published.workspace.doc.version, baseHash: published.workspace.baseHash, revision: published.workspace.revision, recipeHash: production.hash, teachingHash: production.teachingHash } };
}
