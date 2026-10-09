async (page) => {
  const setup = await page.evaluate(async () => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    const test = window.__offlineTest;
    if (!test || test.groups.length !== 2 || test.groups.some(g => !g.passed)) throw new Error('Complete both replay groups first');
    const name = test.cycles.length % 2 === 0 ? 'Glue1' : 'Glue2';
    if (test.cycles.length >= 4) throw new Error('All four cycle experiments already completed');
    const step = test.cycles.length < 2 ? 8 : 5;
    const options = test.options[name];
    const length = (options.count - 1) * 8 - 280;
    const calib = {nozzle: [688, 646], angleDeg: options.directionDeg + 180, mirror: false,
      mmPerPx: 1, maskPx: 50, imageSize: [1280, 1024]};
    const configs = await api('camera_rig_config');
    for (let cam = 0; cam < 3; cam++)
      await api('camera_save_config', {cam, config: {...configs[cam], acquisition: 'freeRun', fps: 10,
        replayDir: test.root + '/timed-replay/' + name, follow: calib}});
    const settings = {...test.initial.settings, productSource: 'plc', manualRecipeId: null,
      followVision: true, record: 'off', timeouts: {...test.initial.settings.timeouts, motionMs: 60000}};
    await api('cycle_save_settings', {settings});
    const doc = await api('recipe_template', {mode: 'follow'});
    const position = {nominal: 0, tolUpper: 10, tolLower: 10, absMin: -25, absMax: 25, maxExcursionLen: 20};
    const width = {nominal: 16, tolUpper: 8, tolLower: 8, absMin: 4, absMax: 35, maxExcursionLen: 20};
    Object.assign(doc, {id: 'OFFLINE-' + name.toUpperCase(), name: name + ' 离线实验（像素单位/合成时序）',
      productCode: name === 'Glue1' ? 4401 : 4402, camera: configs[0].id,
      path: {kind: 'polyline', points: [[0, 0], [length, 0]], closed: false, radius: 0, bulges: []},
      spacing: 5, filterWindow: 1, maxGapLen: 15, segmentOverrides: {}, shots: [],
      line: {position, width}, corner: {position, width},
      follow: {cameras: configs.map(c => c.id), timing: {kind: 'timed', speedMmS: 80, delayMs: 200},
        nearMm: 70, farMm: 280, stepMm: step, overrunMm: 280, searchMm: 25, beadWidth: 16,
        polarity: 'dark', minContrast: 18, autoSync: false, startZoneMm: 0}});
    const saved = await api('recipe_save', {doc, originalId: null});
    const baseline = await api('cycle_snapshot');
    test.pendingCycle = {name, experiment: step === 8 ? 'step-8-baseline' : 'step-5-tail-coverage',
      doc, saved, settings, baseline, calibration: calib, cameraStatusBefore: await api('camera_rig_status'),
      syntheticTimingMs: 100, syntheticPath: true, physicalCalibration: false,
      started: new Date().toISOString(), startWall: Date.now(), maximumMeasuredFrames: 0};
    return {name, saved};
  });
  await page.evaluate(() => { location.hash = '/inspect'; });
  await page.waitForTimeout(350);
  await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('sim_start', {
    recipeId: window.__offlineTest.pendingCycle.doc.id, scenario: 'normal', continuous: false
  }));
  const phases = [];
  let snapshot, simulator, complete = false;
  for (let attempt = 0; attempt < 180; attempt++) {
    const progress = await page.evaluate(async () => {
      const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
      const s = await api('cycle_snapshot'), sim = await api('sim_status');
      const run = window.__offlineTest.pendingCycle;
      run.maximumMeasuredFrames = Math.max(run.maximumMeasuredFrames, s.part?.measuredFrames ?? 0);
      const m = await api('cycle_part_data');
      if (m.length) run.measurements = m;
      return {snapshot: s, simulator: sim, complete: s.phase === 'IDLE' &&
        s.stats.total > run.baseline.stats.total && !sim.running};
    });
    snapshot = progress.snapshot;
    simulator = progress.simulator;
    if (phases[phases.length - 1] !== snapshot.phase) phases.push(snapshot.phase);
    if (progress.complete) { complete = true; break; }
    await page.waitForTimeout(100);
  }
  return await page.evaluate(async ({setup, phases, snapshot, simulator, complete}) => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    const test = window.__offlineTest, run = test.pendingCycle;
    run.phases = phases;
    run.finished = new Date().toISOString();
    run.elapsedMs = Date.now() - run.startWall;
    run.snapshot = snapshot;
    run.simulator = simulator;
    run.completedHandshake = complete;
    run.cameraStatus = await api('camera_rig_status');
    run.logs = await api('cycle_logs');
    const history = await api('history_query', {query: {recipeId: run.doc.id, limit: 1}});
    run.history = history.items.length ? await api('history_detail', {id: history.items[0].id}) : null;
    const m = run.measurements ?? [];
    run.nativeSummary = {measuredFrames: m.length, measurementErrors: m.filter(f => f.error).map(f => f.error),
      actualPixelPoints: m.reduce((n, f) => n + f.px.length, 0),
      measuredBeadPoints: m.reduce((n, f) => n + f.st.filter(s => s === 0).length, 0)};
    run.fullReplayFrames = run.cameraStatus.map((s, cam) => s.frames - run.cameraStatusBefore[cam].frames);
    run.passedPipeline = complete && !!run.history?.points && run.nativeSummary.actualPixelPoints > 0 &&
      run.fullReplayFrames.every(n => n === test.options[run.name].count) &&
      run.nativeSummary.measurementErrors.length === 0;
    test.cycles.push(run);
    delete test.pendingCycle;
    return {name: setup.name, experiment: run.experiment, completedHandshake: complete, passedPipeline: run.passedPipeline,
      phases, cameraStatus: run.cameraStatus, summary: run.history?.summary, nativeSummary: run.nativeSummary,
      elapsedMs: run.elapsedMs, simulator};
  }, {setup, phases, snapshot, simulator, complete});
}
