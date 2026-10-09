async (page) => {
  await page.getByRole('button',{name:'使用该配方候选',exact:true}).click();
  const before = await page.evaluate(async () => {
    const test=window.__offlineDemo, api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const [detail,raw,comparisons]=await Promise.all([
      api('history_detail',{id:test.currentHistoryId}),api('workspace_record_images',{historyId:test.currentHistoryId}),
      api('workspace_comparisons',{id:test.recipeId,historyId:test.currentHistoryId})]);
    if(!raw.complete) throw new Error(raw.message);
    return {detail,raw,comparisonIds:comparisons.map(c=>c.id)};
  });
  await page.getByRole('button',{name:'从原图复测整件',exact:true}).click();
  await page.getByRole('heading',{name:'原图复测结果',exact:true}).waitFor({timeout:30000});
  const result = await page.evaluate(async before => {
    const test=window.__offlineDemo, api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const comparison=(await api('workspace_comparisons',{id:test.recipeId,historyId:test.currentHistoryId}))
      .find(c=>!before.comparisonIds.includes(c.id));
    if(!comparison || comparison.source!=='raw' || !comparison.measurements.length) throw new Error('No new real-image comparison');
    const after=await api('history_detail',{id:test.currentHistoryId});
    const unchanged=JSON.stringify(before.detail)===JSON.stringify(after);
    if(!unchanged) throw new Error('Original inspection record changed');
    const original=before.detail.points;
    const reconstructed=Array(original.st.length).fill(null);
    for(const m of comparison.measurements) for(let n=0;n<m.idx.length;n++)
      reconstructed[m.idx[n]]={st:m.st[n],d:m.d[n],w:m.w[n]};
    let statusMismatches=0, valueMismatches=0, maxDifference=0;
    for(let j=0;j<original.st.length;j++) {
      const point=reconstructed[j];
      if(!point || point.st!==original.st[j]){statusMismatches++;continue;}
      if(point.st===0) for(const key of ['d','w']) {
        const a=point[key],b=original[key]?.[j];
        if(a==null || b==null) {if(a!==b)valueMismatches++;continue;}
        const diff=Math.abs(a-b);maxDifference=Math.max(maxDifference,diff);if(diff>0.0001)valueMismatches++;
      }
    }
    const check={operation:'same-parameter raw-image remeasurement through UI',group:test.group,historyId:test.currentHistoryId,
      rawFrames:before.raw.frames.length,rawComplete:before.raw.complete,originalUnchanged:unchanged,
      originalVerdict:before.detail.summary.verdict,retestVerdict:comparison.judgement.verdict,
      remeasuredFrames:comparison.measurements.length,points:original.st.length,statusMismatches,valueMismatches,maxDifference,
      passed:!statusMismatches&&!valueMismatches&&comparison.judgement.verdict===before.detail.summary.verdict};
    const cycle=test.cycles.at(-1);cycle.raw=before.raw;cycle.comparisons.push(comparison);cycle.retestCheck=check;
    test.checks.push(check);
    return check;
  },before);
  const selected=await page.evaluate(() => {
    const raw=window.__offlineDemo.cycles.at(-1).raw.frames;
    const perCam=raw.filter(f=>f.camera==='cam1');return String(perCam[Math.floor(perCam.length/2)].k);
  });
  await page.getByRole('combobox',{name:'历史帧选择',exact:true}).selectOption(selected);
  await page.screenshot({path:'output/playwright/offline-demo-20261009/'+result.group+'-retest.png',fullPage:true});
  await page.locator('.wp-gray-image').waitFor({state:'visible'});
  await page.locator('.wp-gray-image').scrollIntoViewIfNeeded();
  await page.locator('.wp-viewport').screenshot({path:'output/playwright/offline-demo-20261009/'+result.group+'-original-frame.png'});
  return result;
}
