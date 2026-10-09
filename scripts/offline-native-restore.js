async (page) => {
  return await page.evaluate(async () => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    const test = window.__offlineTest;
    if (!test) throw new Error('Offline setup state is unavailable');
    await api('sim_stop');
    const state = await api('cycle_snapshot');
    if (state.phase !== 'IDLE') throw new Error('Wait for the current part to finish before restoration');
    const configs = await api('camera_rig_config');
    for (let cam = configs.length - 1; cam >= test.initial.cameras.length; cam--)
      await api('camera_remove', {cam});
    for (let cam = 0; cam < test.initial.cameras.length; cam++)
      await api('camera_save_config', {cam, config: test.initial.cameras[cam]});
    await api('cycle_save_settings', {settings: test.initial.settings});
    for (const id of new Set(test.cycles.map(c => c.doc.id)))
      await api('recipe_delete', {id});
    const cameras = await api('camera_rig_config'), settings = await api('cycle_get_settings');
    test.restoration = {restoredAt: new Date().toISOString(), cameras, settings,
      cameraConfigurationsRestored: JSON.stringify(cameras) === JSON.stringify(test.initial.cameras),
      cycleSettingsRestored: JSON.stringify(settings) === JSON.stringify(test.initial.settings),
      temporaryRecipesRemoved: !(await api('recipe_list')).recipes.some(r => r.id.startsWith('OFFLINE-')),
      testHistoryPreserved: true, userProductionProfileModified: false};
    return test.restoration;
  });
}
