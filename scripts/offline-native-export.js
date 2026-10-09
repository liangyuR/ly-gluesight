async (page) => {
  return await page.evaluate(() => {
    const t = window.__offlineTest;
    if (!t || t.cycles.length < 2) throw new Error('Complete both offline cycles first');
    return {passedInfrastructure: t.groups.every(g => g.passed) && t.cycles.every(c => c.passedPipeline),
      productAcceptanceEstablished: false, ...t, exported: new Date().toISOString()};
  });
}
