async (page) => {
  return await page.evaluate(async () => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    const test = window.__offlineTest;
    const group = test.groups[test.groups.length - 1];
    const count = test.options[group.name].count;
    if (test.nextSequence !== count + 1) throw new Error('Group is incomplete');
    if (!group.dryFrames) group.dryFrames = await api('camera_dry_run_stop');
    group.statusAfter = await api('camera_rig_status');
    if (group.rows.length !== count * 3 || group.dryFrames.length !== count * 3) throw new Error('Frame-count mismatch');
    group.channelChecks = [0, 1, 2].map(cam => {
      const received = group.dryFrames.filter(f => f.cam === cam);
      const baseline = group.statusBefore[cam].frames;
      return {channel: cam + 1, count: received.length, counterBaseline: baseline,
        consecutive: received.every((f, i) => f.frameCounter === baseline + i + 1),
        lostPackets: received.reduce((n, f) => n + f.lostPackets, 0),
        uniquePreviews: new Set(group.rows.filter(r => r.channel === cam + 1).map(r => r.preview.hash)).size};
    });
    group.identicalChannelTriplets = Array.from({length: count}, (_, i) =>
      new Set(group.rows.filter(r => r.sequence === i + 1).map(r => r.preview.hash)).size === 1).filter(Boolean).length;
    group.passed = group.channelChecks.every(c => c.count === count && c.consecutive && c.lostPackets === 0) &&
      group.statusAfter.every(c => c.droppedFrames === 0) && group.identicalChannelTriplets === count;
    if (!group.passed) throw new Error('Replay sequence or channel consistency failed');
    if (group.name === 'Glue1') {
      const configs = await api('camera_rig_config');
      for (let cam = 0; cam < 3; cam++)
        await api('camera_save_config', {cam, config: {...configs[cam], replayDir: test.root + '/replay/Glue2'}});
      test.currentGroup = 'Glue2';
      test.nextSequence = 1;
      await api('camera_dry_run_start');
      test.groups.push({name: 'Glue2', rows: [], cameraIds: test.cameraIds, statusBefore: await api('camera_rig_status')});
    }
    return {name: group.name, passed: group.passed, frames: group.rows.length,
      channelChecks: group.channelChecks, identicalChannelTriplets: group.identicalChannelTriplets,
      nextGroup: test.currentGroup};
  });
}
