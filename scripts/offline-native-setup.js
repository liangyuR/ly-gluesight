async (page, options = {}) => {
  const root = typeof options.datasetRoot === 'string' ? options.datasetRoot.replaceAll('\\', '/').replace(/\/+$/, '') : '';
  if (!/^(?:[A-Za-z]:\/|\/)/.test(root)) throw new Error('Pass an absolute datasetRoot through Run-OfflineSampleTest.ps1');
  const result = await page.evaluate(async root => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    const records = await api('records_list');
    if (!records.root.replaceAll('\\', '/').split('/').includes('com.xyzrobotics.tujiaovision.ui-tests'))
      throw new Error('Offline runner requires the isolated UI test data directory');
    const initial = {
      cameras: await api('camera_rig_config'),
      settings: await api('cycle_get_settings'),
      plc: await api('plc_get_config')
    };
    if (initial.plc.connection.protocol !== 'simulator') throw new Error('Offline runner requires simulator PLC');
    if ((await api('cycle_snapshot')).phase !== 'IDLE') throw new Error('Offline runner is not idle');
    if (initial.cameras.length !== 1 || initial.cameras[0].source !== 'sim') throw new Error('Unexpected isolated test configuration');
    const test = window.__offlineTest = {
      started: new Date().toISOString(), root, initial,
      units: 'px; mmPerPx=1 is a test coordinate scale without physical calibration',
      backend: 'native replay decoder and native follow caliper, not simulated measurements',
      groups: [], cycles: [], nextSequence: 1, currentGroup: 'Glue1',
      options: {Glue1: {count: 80, directionDeg: -75}, Glue2: {count: 71, directionDeg: -52}}
    };
    const base = {...initial.cameras[0], source: 'replay', acquisition: 'triggered',
      fps: 10, triggerSource: 'Software', replayDir: root + '/replay/Glue1', follow: null};
    await api('cycle_save_settings', {settings: {...initial.settings, record: 'off', followVision: true}});
    for (let cam = 0; cam < 3; cam++) {
      const config = {...base, name: '离线样本 通道 ' + (cam + 1), replayChannel: cam + 1};
      if (cam === 0) await api('camera_save_config', {cam, config});
      else await api('camera_add', {config});
    }
    test.cameraIds = (await api('camera_rig_config')).map(c => c.id);
    const status = await api('camera_rig_status');
    if (status.some(s => !s.ready || s.source !== 'replay')) throw new Error('Replay cameras are not ready');
    await api('camera_dry_run_start');
    test.groups.push({name: 'Glue1', rows: [], cameraIds: test.cameraIds, statusBefore: status});
    return {started: test.started, cameraIds: test.cameraIds, status, settings: await api('cycle_get_settings')};
  }, root);
  await page.evaluate(() => { location.hash = '/camera'; });
  await page.waitForTimeout(600);
  return result;
}
