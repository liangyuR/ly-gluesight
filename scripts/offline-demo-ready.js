async (page) => {
  const data=await page.evaluate(async()=>{
    const api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const cameras=await api('camera_rig_config');
    const group=cameras[0].replayDir.split(/[\\/]/).pop(),recipeId='OFFLINE-'+group.toUpperCase();
    const history=(await api('history_query',{query:{recipeId,limit:1}})).items[0];
    if(!history)throw new Error('No saved demonstration record');
    if(cameras.some(c=>c.follow.angleDeg!==(group==='Glue1'?105:128)))throw new Error('Wrong group calibration');
    return {group,recipeId,history,cameras};
  });
  await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'历史记录',exact:true}).click();
  await page.getByRole('row',{name:'查看 SN '+data.history.sn+' 的记录',exact:true}).click();
  await page.getByRole('button',{name:'使用该配方候选',exact:true}).click();
  const saved=await page.evaluate(async({recipeId,history})=>{
    const api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const comparisons=await api('workspace_comparisons',{id:recipeId,historyId:history.id});
    const raw=await api('workspace_record_images',{historyId:history.id});
    const baseline=comparisons.filter(c=>c.source==='raw'&&c.candidateRecipe.follow.minContrast===18)
      .sort((a,b)=>b.createdAt-a.createdAt)[0];
    if(!baseline||!raw.complete)throw new Error('Saved raw-image evidence unavailable');
    const cam1=raw.frames.filter(f=>f.camera==='cam1');
    return {comparisonId:baseline.id,rawFrames:raw.frames.length,frameKey:String(cam1[Math.floor(cam1.length/2)].k)};
  },data);
  await page.getByRole('combobox',{name:'已保存对照结果',exact:true}).selectOption(saved.comparisonId);
  await page.getByRole('heading',{name:'原图复测结果',exact:true}).waitFor();
  await page.getByRole('combobox',{name:'历史帧选择',exact:true}).selectOption(saved.frameKey);
  await page.locator('.wp-gray-image').waitFor({state:'visible'});
  await page.locator('.wp-gray-image').scrollIntoViewIfNeeded();
  await page.locator('.wp-viewport').screenshot({path:'output/playwright/offline-demo-20261009/'+data.group+'-original-frame.png'});
  await page.getByRole('heading',{name:'原图复测结果',exact:true}).scrollIntoViewIfNeeded();
  await page.screenshot({path:'output/playwright/offline-demo-20261009/'+data.group+'-ready.png',fullPage:true});
  return {group:data.group,historyId:data.history.id,rawFrames:saved.rawFrames,comparisonId:saved.comparisonId,
    cameraAngles:data.cameras.map(c=>c.follow.angleDeg),openedSavedRawComparison:true};
}
