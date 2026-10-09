async (page) => {
  const state = await page.evaluate(async () => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    const [records, cameras, settings, plc, snapshot] = await Promise.all([
      api('records_list'), api('camera_rig_config'), api('cycle_get_settings'), api('plc_get_config'), api('cycle_snapshot')]);
    if (!records.root.includes('com.xyzrobotics.gluesight.offline-demo-20261009')) throw new Error('Unexpected application profile');
    if (plc.connection.protocol !== 'simulator' || cameras.some(c => c.source !== 'replay')) throw new Error('Expected offline devices');
    if (!settings.followVision || settings.record !== 'all') throw new Error('Real image measurement and full recording are required');
    if (snapshot.phase !== 'IDLE') throw new Error('Demo must start idle');
    const group = cameras[0].replayDir.split(/[\\/]/).pop();
    window.__offlineDemo = {group, recipeId:'OFFLINE-'+group.toUpperCase(), startedAt:new Date().toISOString(),
      recordsRoot:records.root, cameras, settings, baseline:snapshot, checks:[], cycles:[], consoleErrors:[]};
    return {group, recordsRoot:records.root, cameras:cameras.map(c=>({id:c.id,channel:c.replayChannel,dir:c.replayDir})),
      actualImageMeasurement:settings.followVision, recording:settings.record, plc:plc.connection.protocol};
  });
  page.on('pageerror', error => { void page.evaluate(message => window.__offlineDemo?.consoleErrors.push(message), String(error)); });
  await page.getByRole('navigation', {name:'操作导航'}).getByRole('link',{name:'设备与采集',exact:true}).click();
  await page.getByRole('button',{name:'下一张',exact:true}).click();
  await page.locator('.preview-box canvas').waitFor({state:'visible'});
  await page.screenshot({path:'output/playwright/offline-demo-20261009/'+state.group+'-camera.png',fullPage:true});
  return state;
}
