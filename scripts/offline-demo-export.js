async (page) => {
  return await page.evaluate(async () => {
    const test=window.__offlineDemo,api=(name,args={})=>window.__TAURI_INTERNALS__.invoke(name,args);
    const workspace=await api('workspace_get',{id:test.recipeId});
    const snapshot=await api('cycle_snapshot');
    if(snapshot.phase!=='IDLE') throw new Error('Cycle must be idle before exporting');
    if(workspace.workspace.doc.follow.minContrast!==18) throw new Error('Demo measurement parameter was not restored');
    if(!test.cycles.length || test.cycles.some(c=>!c.retestCheck?.passed)) throw new Error('Raw-image consistency check did not pass');
    return {...test,finishedAt:new Date().toISOString(),finalCandidate:workspace.workspace.doc,
      finalRevision:workspace.workspace.revision,finalSettings:await api('cycle_get_settings'),finalSnapshot:snapshot,
      history:await api('history_query',{query:{limit:100}}),records:await api('records_list')};
  });
}
