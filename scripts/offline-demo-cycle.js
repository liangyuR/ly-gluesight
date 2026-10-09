async (page) => {
  const initial = await page.evaluate(async () => {
    const api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    return {recipeId:window.__offlineDemo.recipeId,group:window.__offlineDemo.group,
      snapshot:await api('cycle_snapshot'),cameras:await api('camera_rig_status')};
  });
  await page.evaluate(initial => { window.__offlineDemo.pendingInitial = initial; },initial);
  await page.getByRole('combobox',{name:'模拟配方',exact:true}).selectOption(initial.recipeId);
  await page.getByRole('combobox',{name:'模拟工况',exact:true}).selectOption('normal');
  await page.getByRole('button',{name:'运行一件',exact:true}).click();
  const poll = async (predicate, arg, timeout) => {
    const end=Date.now()+timeout;
    while(Date.now()<end) { if(await page.evaluate(predicate,arg)) return; await page.waitForTimeout(150); }
    throw new Error('Timed out waiting for native cycle state');
  };
  await poll(async () => {
    const s=await window.__TAURI_INTERNALS__.invoke('cycle_snapshot');
    return (s.part?.measuredFrames??0)>5 || s.phase==='FAULT';
  },null,20000);
  await page.screenshot({path:'output/playwright/offline-demo-20261009/'+initial.group+'-running.png',fullPage:true});
  await poll(async total => {
    const s=await window.__TAURI_INTERNALS__.invoke('cycle_snapshot');
    const sim=await window.__TAURI_INTERNALS__.invoke('sim_status');
    return s.phase==='IDLE' && s.stats.total>total && !sim.running;
  },initial.snapshot.stats.total,40000);
  const result = await page.evaluate(async initial => {
    const api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const history=await api('history_query',{query:{recipeId:initial.recipeId,limit:1}});
    if(!history.items.length) throw new Error('No recorded workpiece');
    const detail=await api('history_detail',{id:history.items[0].id});
    const cameras=await api('camera_rig_status');
    const measurements=await api('cycle_part_data');
    const cycle={group:initial.group,recipeId:initial.recipeId,historyId:detail.summary.id,
      detail,measurements,cameras,capturedPerChannel:cameras.map((c,i)=>c.frames-initial.cameras[i].frames),
      droppedPerChannel:cameras.map((c,i)=>(c.droppedFrames??0)-(initial.cameras[i].droppedFrames??0)),
      settings:await api('cycle_get_settings'),cameraConfig:await api('camera_rig_config'),logs:await api('cycle_logs'),
      snapshot:await api('cycle_snapshot'),raw:null,comparisons:[]};
    window.__offlineDemo.cycles.push(cycle);
    window.__offlineDemo.currentHistoryId=detail.summary.id;
    return {group:cycle.group,historyId:cycle.historyId,summary:detail.summary,
      capturedPerChannel:cycle.capturedPerChannel,droppedPerChannel:cycle.droppedPerChannel,
      measurements:measurements.length,measurementErrors:measurements.filter(m=>m.error).map(m=>m.error),
      pendingPoints:detail.points?.st.filter(s=>s>=2).length};
  },initial);
  await page.screenshot({path:'output/playwright/offline-demo-20261009/'+initial.group+'-result.png',fullPage:true});
  await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'历史记录',exact:true}).click();
  return result;
}
