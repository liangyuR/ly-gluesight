async (page) => {
  if (await page.getByRole("dialog").isVisible()) await page.getByRole("button", {name:"取消",exact:true}).click();
  const root = "output/playwright\\ui-regression\\flyshot-samples\\";
  for (const [name, expected, first] of [["离线良品整组", "OK", "k1.pgm"], ["离线断胶整组", "NG_GAP", "k1-gap.pgm"]]) {
    const selected = page.getByRole("checkbox", {name:"选用样本 "+name,exact:true});
    if (!await selected.count()) {
      await page.getByRole("button", { name:"导入原图样本组",exact:true }).click();
      await page.getByRole("textbox", { name:"样本名称",exact:true }).fill(name);
      await page.getByRole("combobox", { name:"人工确认的期望结论",exact:true }).selectOption(expected);
      for (let k = 1; k <= 4; k++) await page.getByRole("dialog").locator('input[type="file"]').nth(k-1).setInputFiles(root+(k===1?first:"k"+k+".pgm"));
      await page.getByRole("button", {name:"保存样本组",exact:true}).click();
      await selected.waitFor();
    }
    await selected.check();
  }
  await page.getByRole("button", {name:"运行规则与图像验证",exact:true}).click();
  await page.getByText("当前候选的验证已完成", {exact:true}).waitFor();
  const state = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("workspace_get", {id:"UI-FLYSHOT"}));
  const validation = state.workspace.validation;
  if (!validation?.passed || validation.samples.length!==2 || validation.samples.some(s=>!s.passed) || !validation.samples.some(s=>s.actual==="NG_GAP")) throw new Error(JSON.stringify(validation));
  await page.getByRole("button", {name:"发布生产配方",exact:true}).click();
  await page.getByRole("button", {name:"取消",exact:true}).click();
  const cancelled = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("recipe_doc", {id:"UI-FLYSHOT"}).then(()=>true,()=>false));
  if (cancelled) throw new Error("Cancelled publish unexpectedly created production recipe");
  await page.getByRole("button", {name:"发布生产配方",exact:true}).click();
  await page.getByRole("button", {name:"确认发布 v1",exact:true}).click();
  await page.getByText("已提交发布，将在工件边界生效", {exact:true}).waitFor();
  const published = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("recipe_doc", {id:"UI-FLYSHOT"}));
  if (published.id!=="UI-FLYSHOT" || published.productCode!==93 || !published.teachingId) throw new Error("Production snapshot incomplete");
  const result = {operation:"良品/断胶整组原图导入、人工期望、真实图像验证、发布取消与确认",passed:true,validation,published:{id:published.id,version:published.version,teachingId:published.teachingId}};
  await page.evaluate(result=>window.__uiOperations.checks.push(result),result);
  await page.screenshot({path:"output/playwright\\ui-regression\\flyshot-validation.png",fullPage:true});
  await page.getByRole("link", {name:"查看在线检测",exact:true}).click();
  return result;
}
