async (page) => {
  const state = await page.evaluate(async () => {
    const query = await window.__TAURI_INTERNALS__.invoke("history_query",{query:{recipeId:"UI-FLYSHOT",limit:3}});
    const detail = await window.__TAURI_INTERNALS__.invoke("history_detail",{id:query.items[0].id});
    const layout = await window.__TAURI_INTERNALS__.invoke("history_recipe",{revisionId:detail.summary.recipeRevision,recipeId:"UI-FLYSHOT"});
    const boundary = layout.points.k.findIndex((k,j)=>k===1&&layout.points.k[j+1]===2);
    return {summary:detail.summary,frames:detail.frames,boundary,points:Array.from({length:11},(_,i)=>boundary-5+i).map(j=>({j,x:layout.points.x[j],y:layout.points.y[j],k:layout.points.k[j],st:detail.points.st[j],d:detail.points.d[j],w:detail.points.w[j]}))};
  });
  return state;
}
