async (page) => {
  const id = "P0-TRICAM-UI", hash = "3b643e189e0f73e5";
  const read = (command, args) => page.evaluate(async ({command,args}) => window.__TAURI_INTERNALS__.invoke(command,args), {command,args});
  const records = await read("records_list"), plc = await read("plc_get_config"), cameras = await read("camera_rig_config");
  if (!records.root.includes("com.xyzrobotics.tujiaovision.p0-tests") || plc.connection.protocol !== "simulator" || cameras.some(c => c.source !== "sim")) throw new Error("Isolated native test guard rejected");
  const saved = await read("workspace_get", {id}), production = await read("cycle_layout", {recipeId:id});
  const recent = await read("history_query", {query:{recipeId:id,limit:20}});
  const row = recent.items.find(r => r.cycleId && r.bundleId === hash && r.verdict === "NG_GAP");
  if (!row) throw new Error("Create a schema-2 recorded NG_GAP part first");
  const original = await read("history_detail", {id:row.id});
  const nav = name => page.getByRole("navigation",{name:"操作导航"}).getByRole("link",{name,exact:true}).click();
  const openRecord = async () => {
    await nav("历史记录");
    await page.getByRole("button",{name:"全部",exact:true}).click();
    await page.getByRole("combobox",{name:"历史配方",exact:true}).selectOption(id);
    await page.getByRole("row",{name:`查看 SN ${row.sn} 的记录`,exact:true}).click();
    await page.getByRole("heading",{name:"逐拍照点追溯",exact:true}).waitFor();
    await page.getByRole("button",{name:"按原发布包重现",exact:true}).waitFor({state:"visible"});
  };
  const reproduce = async scope => {
    const prior = await read("workspace_comparisons",{id,historyId:row.id});
    if (!await page.getByRole("button",{name:"按原发布包重现",exact:true}).isEnabled() || await page.getByRole("button",{name:"按候选规则重判",exact:true}).isEnabled() || await page.getByRole("button",{name:"从原图复测整件",exact:true}).isEnabled()) throw new Error("Original replay or candidate action isolation failed: " + scope);
    await page.getByRole("button",{name:"按原发布包重现",exact:true}).click();
    let result;
    for (let i=0;i<150;i++) {
      const current = await read("workspace_comparisons",{id,historyId:row.id});
      result = current.find(c => c.source === "original" && !prior.some(old => old.id === c.id));
      if (result) break;
      if (await page.getByRole("heading",{name:"操作未完成",exact:true}).isVisible()) throw new Error(await page.locator("main").innerText());
      await page.waitForTimeout(100);
    }
    if (!result || result.candidateRevision !== 0 || result.candidateRecipe.revisionId !== row.recipeRevision || result.bundleId !== hash || result.cycleId !== row.cycleId || result.judgement.verdict !== row.verdict || result.measurements.length !== 4 || result.measurements.some(m => m.error || !m.located || m.bundleId !== hash || m.cycleId !== row.cycleId)) throw new Error(JSON.stringify(result));
    await page.getByRole("heading",{name:"原发布包重现结果",exact:true}).waitFor();
    await page.screenshot({path:`output/playwright/p0-history/03-original-independent-${scope}.png`,fullPage:true});
    return {scope,id:result.id,candidateRevision:result.candidateRevision,candidateHash:result.candidateRecipe.revisionId,bundleId:result.bundleId,cycleId:result.cycleId,verdict:result.judgement.verdict,measurements:result.measurements.map(m=>({k:m.k,ms:m.ms,points:m.idx.length,error:m.error}))};
  };
  await nav("拍照点规划");
  const picker = page.getByRole("combobox",{name:"当前配方",exact:true});
  await picker.selectOption("MTR-HSG-A");
  await page.getByRole("textbox",{name:"拍照点 1 · Pose",exact:true}).waitFor();
  await openRecord();
  const otherCandidate = await reproduce("other-candidate");
  await nav("拍照点规划");
  await page.getByRole("combobox",{name:"当前配方",exact:true}).selectOption(id);
  const pose = page.getByRole("textbox",{name:"拍照点 1 · Pose",exact:true});
  await pose.waitFor();
  const originalPose = await pose.inputValue();
  await pose.fill(originalPose + "-UNSAVED-REPLAY");
  await page.getByText("候选配置尚未保存",{exact:true}).waitFor();
  await openRecord();
  await page.getByText("先保存候选配置和示教中线，再执行对照。",{exact:true}).waitFor();
  const dirtyCandidate = await reproduce("unsaved-pose");
  await nav("拍照点规划");
  if (await pose.inputValue() !== originalPose + "-UNSAVED-REPLAY") throw new Error("Original replay discarded the unsaved candidate Pose");
  await pose.fill(originalPose);
  await page.getByText("候选配置尚未保存",{exact:true}).waitFor({state:"hidden"});
  const after = await read("history_detail",{id:row.id}), afterSaved = await read("workspace_get",{id}), afterProduction = await read("cycle_layout",{recipeId:id});
  if (JSON.stringify(after) !== JSON.stringify(original) || JSON.stringify(afterSaved.workspace) !== JSON.stringify(saved.workspace) || JSON.stringify(afterProduction) !== JSON.stringify(production)) throw new Error("Original replay changed production record, stored candidate, or production layout");
  return {historyId:row.id,sn:row.sn,cycleId:row.cycleId,bundleId:hash,otherCandidate,dirtyCandidate,unsavedCandidatePreserved:true,storedCandidateAndProductionUnchanged:true,originalRecordUnchanged:true,accuracyQualified:false};
}
