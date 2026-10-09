async (page) => {
  return await page.evaluate(async () => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    const test = window.__offlineTest;
    if (!test) throw new Error('Run offline-native-setup.js first');
    const group = test.groups[test.groups.length - 1];
    const options = test.options[test.currentGroup];
    const first = test.nextSequence;
    const last = Math.min(options.count, first + 14);
    const summary = values => {
      if (!values.length) return null;
      values.sort((a, b) => a - b);
      return {min: values[0], median: values[Math.floor(values.length / 2)], max: values[values.length - 1]};
    };
    const rows = [];
    for (let sequence = first; sequence <= last; sequence++) {
      for (let cam = 0; cam < 3; cam++) {
        const before = (await api('camera_rig_status'))[cam].frames;
        await api('camera_soft_trigger', {cam});
        let status, attempts = 0;
        do {
          await new Promise(resolve => setTimeout(resolve, 10));
          status = (await api('camera_rig_status'))[cam];
          if (++attempts > 300) throw new Error('Replay timeout: ' + group.name + '/' + cam + '/' + sequence);
        } while (status.frames <= before);
        if (status.frames !== before + 1) throw new Error('Unexpected frame jump');
        const binary = await api('camera_preview', {cam});
        const data = binary instanceof ArrayBuffer ? new Uint8Array(binary) : new Uint8Array(binary);
        const header = new DataView(data.buffer, data.byteOffset, data.byteLength);
        const preview = {width: header.getUint32(0, true), height: header.getUint32(4, true),
          fullWidth: header.getUint32(8, true), fullHeight: header.getUint32(12, true)};
        if (data.length !== 16 + preview.width * preview.height || preview.fullWidth !== 1280 || preview.fullHeight !== 1024)
          throw new Error('Invalid real-image preview dimensions');
        let hash = 2166136261;
        for (let i = 16; i < data.length; i++) hash = Math.imul(hash ^ data[i], 16777619);
        const calib = {nozzle: [688, 646], angleDeg: options.directionDeg + 180,
          mirror: false, mmPerPx: 1, maskPx: 50, imageSize: [1280, 1024]};
        const probe = await api('teach_follow_probe', {request: {
          cam, calib, directionDeg: options.directionDeg, polarity: 'dark',
          beadWidth: 16, searchMm: 25, nearMm: 70, farMm: 280
        }});
        if (probe.imageSize[0] !== 1280 || probe.imageSize[1] !== 1024 || probe.points.length !== 421)
          throw new Error('Invalid native-probe geometry');
        const found = probe.points.filter(p => p.st === 0);
        if (found.some(p => !Number.isFinite(p.offset) || !Number.isFinite(p.width) || p.width <= 0 ||
          !p.px.every(Number.isFinite))) throw new Error('Non-finite native-probe measurement');
        const row = {group: group.name, channel: cam + 1, sequence,
          file: 'cam' + (cam + 1) + '_' + String(sequence).padStart(6, '0') + '.jpg',
          frameCounter: status.frames, preview: {...preview, hash: (hash >>> 0).toString(16)},
          directionDeg: probe.directionDeg, testedPoints: probe.points.length, foundPoints: found.length,
          missingPoints: probe.points.length - found.length, widthPx: summary(found.map(p => p.width)),
          offsetPx: summary(found.map(p => p.offset)), lostPackets: status.lostPackets, droppedFrames: status.droppedFrames};
        rows.push(row);
        group.rows.push(row);
      }
      test.nextSequence = sequence + 1;
    }
    return {group: group.name, first, last, complete: test.nextSequence > options.count, rows};
  });
}
