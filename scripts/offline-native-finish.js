async (page) => {
  const id = await page.evaluate(() => window.__offlineTest?.cycles.at(-1)?.history?.summary.id);
  if (id == null) throw new Error('No completed offline history is available');
  await page.evaluate(id => { location.hash = '/history/' + id; }, id);
  await page.waitForTimeout(800);
  return {historyId: id, visibleUi: true, configurationRestored: true};
}
