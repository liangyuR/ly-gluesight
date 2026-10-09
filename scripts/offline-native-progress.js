async (page) => {
  return await page.evaluate(() => {
    const t = window.__offlineTest;
    return t.groups.map(g => ({name: g.name, passed: g.passed, checks: g.channelChecks,
      triplets: g.identicalChannelTriplets, before: g.statusBefore, after: g.statusAfter,
      dryFirst: g.dryFrames?.slice(0, 3), dryLast: g.dryFrames?.slice(-3),
      rows: g.rows.length, nextSequence: t.nextSequence}));
  });
}
