async (page) => {
  const demo = await page.evaluate(() => ({group:window.__offlineDemo.group, directory:window.__offlineDemo.cameras[0].replayDir}));
  const input = demo.directory + '/cam1_' + (demo.group==='Glue1'?'000040':'000035') + '.png';
  await page.getByLabel('导入离线原图',{exact:true}).setInputFiles(input);
  await page.getByText('cam1 · 离线原图',{exact:true}).waitFor();
  for(const [name,value] of [['名义胶宽（mm）',16],['搜索半宽（mm）',25],['窗口近端（mm）',70],['窗口远端（mm）',280]])
    await page.getByRole('spinbutton',{name,exact:true}).fill(String(value));
  await page.getByRole('button',{name:'在当前帧试测',exact:true}).click();
  const summary = page.getByText(/测到 \d+\/421 点/);
  await summary.waitFor();
  const text = await summary.textContent();
  const measured = Number(text.match(/测到 (\d+)/)[1]);
  if(!measured) throw new Error('No real image measurements from the imported PNG');
  await page.getByRole('button',{name:'保存标定',exact:true}).click();
  const end=Date.now()+10000;
  while(Date.now()<end) {
    if(await page.evaluate(async () => (await window.__TAURI_INTERNALS__.invoke('camera_rig_config'))[0].follow?.mmPerPx===1)) break;
    await page.waitForTimeout(100);
  }
  await page.screenshot({path:'output/playwright/offline-demo-20261009/'+demo.group+'-probe.png',fullPage:true});
  const check = {operation:'import normalized PNG and run native image caliper through UI',group:demo.group,input,summary:text,measured,units:'px; demo scale=1'};
  await page.evaluate(check => window.__offlineDemo.checks.push(check),check);
  await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'在线检测',exact:true}).click();
  return check;
}
