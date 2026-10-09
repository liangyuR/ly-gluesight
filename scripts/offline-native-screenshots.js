async (page) => {
  const dataset = await page.evaluate(() => window.__offlineTest?.root);
  if (typeof dataset !== 'string' || !/^(?:[A-Za-z]:\/|\/)/.test(dataset))
    throw new Error('Run offline-native-setup.js with an absolute datasetRoot first');
  const root = dataset.replace(/\/+$/, '') + '/preview/';
  await page.evaluate(() => { location.hash = '/inspect'; });
  await page.waitForTimeout(1400);
  await page.screenshot({path: root + 'native-inspect.png', fullPage: true});
  const records = await page.evaluate(() => window.__offlineTest.cycles.map(c =>
    ({id: c.history.summary.id, name: c.name, step: c.doc.follow.stepMm})));
  const ids = records.map(r => r.id);
  for (let i = 0; i < ids.length; i++) {
    await page.evaluate(id => { location.hash = '/history/' + id; }, ids[i]);
    await page.waitForTimeout(1100);
    await page.screenshot({path: root + records[i].name + '-step' + records[i].step + '-native-history.png', fullPage: true});
  }
  await page.evaluate(() => { location.hash = '/camera'; });
  await page.waitForTimeout(1100);
  await page.screenshot({path: root + 'native-replay-camera.png', fullPage: true});
  return {historyIds: ids, screenshots: ['native-inspect.png', ...records.map(r =>
    r.name + '-step' + r.step + '-native-history.png'), 'native-replay-camera.png']};
}
