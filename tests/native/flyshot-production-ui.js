async (page) => {
  await page.getByRole("combobox", {name:"模拟配方",exact:true}).selectOption("UI-FLYSHOT");
  const records = [];
  for (const [scenario, expected] of [["normal","OK"],["gap","NG_GAP"]]) {
    await page.getByRole("combobox", {name:"模拟工况",exact:true}).selectOption(scenario);
    const before = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("history_query", {query:{recipeId:"UI-FLYSHOT",limit:1}}));
    await page.getByRole("button", {name:"运行一件",exact:true}).click();
    await page.getByText(new RegExp("^SN (?!"+(before.items[0]?.sn??"none")+"\\b)\\d+ 完成")).waitFor();
    const state = await page.evaluate(async () => ({
      history:await window.__TAURI_INTERNALS__.invoke("history_query", {query:{recipeId:"UI-FLYSHOT",limit:1}}),
      sim:await window.__TAURI_INTERNALS__.invoke("sim_status")
    }));
    const part = state.history.items[0];
    if (!part || state.history.total<=before.total) throw new Error(JSON.stringify(state));
    if (part.verdict!==expected || part.framesReceived!==4 || part.framesExpected!==4) throw new Error(JSON.stringify(part));
    records.push({scenario,part});
  }
  await page.getByRole("button", {name:"整件",exact:true}).click();
  await page.getByRole("button", {name:"总览选择帧 k2",exact:true}).click();
  await page.getByRole("button", {name:"逐拍照点",exact:true}).click();
  await page.getByRole("button", {name:"拍照点 P2",exact:true}).waitFor();
  await page.getByRole("heading", {name:"选中帧 k2",exact:true}).waitFor();
  const result={operation:"发布配方在线模拟工件触发、真实图像良品与断胶判定、整件/逐拍照点与选帧",passed:true,records};
  await page.evaluate(result=>window.__uiOperations.checks.push(result),result);
  await page.screenshot({path:"output/playwright\\ui-regression\\flyshot-production.png",fullPage:true});
  await page.getByRole("navigation",{name:"操作导航"}).getByRole("link",{name:"历史记录",exact:true}).click();
  return result;
}
