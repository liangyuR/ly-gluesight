async (page) => {
  const originalValue=await page.evaluate(async()=>String((await window.__TAURI_INTERNALS__.invoke('workspace_get',{id:window.__offlineDemo.recipeId})).workspace.doc.follow.minContrast));
  if(originalValue!=='18') throw new Error('Unexpected baseline contrast');
  await page.getByRole('spinbutton',{name:'最小边缘灰度差',exact:true}).fill('80');
  await page.getByRole('button',{name:'保存候选配置',exact:true}).last().click();
  await page.getByRole('button',{name:'保存候选配置',exact:true}).last().waitFor({state:'visible'});
  const target=await page.evaluate(()=>({id:window.__offlineDemo.currentHistoryId,sn:window.__offlineDemo.cycles.at(-1).detail.summary.sn}));
  await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'历史记录',exact:true}).click();
  await page.getByRole('row',{name:'查看 SN '+target.sn+' 的记录',exact:true}).click();
  const baselineIds=await page.evaluate(async()=>{
    const t=window.__offlineDemo;
    return (await window.__TAURI_INTERNALS__.invoke('workspace_comparisons',{id:t.recipeId,historyId:t.currentHistoryId})).map(c=>c.id);
  });
  await page.getByRole('button',{name:'从原图复测整件',exact:true}).click();
  await page.getByRole('heading',{name:'原图复测结果',exact:true}).waitFor({timeout:30000});
  const result=await page.evaluate(async baselineIds=>{
    const t=window.__offlineDemo,run=t.cycles.at(-1),api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const c=(await api('workspace_comparisons',{id:t.recipeId,historyId:t.currentHistoryId})).find(c=>!baselineIds.includes(c.id));
    if(!c||c.candidateRecipe.follow.minContrast!==80) throw new Error('The changed measurement parameter was not used');
    const original=await api('history_detail',{id:t.currentHistoryId});
    if(JSON.stringify(original)!==JSON.stringify(run.detail)) throw new Error('Original record changed');
    const pointMap=Array(run.detail.points.st.length).fill(null);
    for(const m of c.measurements) m.idx.forEach((idx,n)=>{pointMap[idx]=m.st[n];});
    const changed=pointMap.filter((st,j)=>st!==run.detail.points.st[j]).length;
    run.comparisons.push(c);
    const check={operation:'measurement parameter sensitivity through UI',group:t.group,historyId:t.currentHistoryId,
      parameter:'follow.minContrast',before:18,after:80,changedPointStatuses:changed,
      baselineVerdict:run.detail.summary.verdict,changedVerdict:c.judgement.verdict,originalUnchanged:true};
    t.checks.push(check);return check;
  },baselineIds);
  await page.screenshot({path:'output/playwright/offline-demo-20261009/'+result.group+'-contrast-comparison.png',fullPage:true});
  await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'胶路与拍照规划',exact:true}).click();
  await page.getByRole('spinbutton',{name:'最小边缘灰度差',exact:true}).fill(originalValue);
  await page.getByRole('button',{name:'保存候选配置',exact:true}).last().click();
  await page.getByRole('button',{name:'保存候选配置',exact:true}).last().waitFor({state:'visible'});
  await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'历史记录',exact:true}).click();
  await page.getByRole('row',{name:'查看 SN '+target.sn+' 的记录',exact:true}).click();
  return result;
}
