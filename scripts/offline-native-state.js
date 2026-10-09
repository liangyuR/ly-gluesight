async (page) => {
  return await page.evaluate(async () => {
    const api = (name, args = {}) => window.__TAURI_INTERNALS__.invoke(name, args);
    return {
      url: location.href,
      cycle: await api('cycle_snapshot'),
      settings: await api('cycle_get_settings'),
      cameras: await api('camera_rig_config'),
      cameraStatus: await api('camera_rig_status'),
      plc: await api('plc_get_status'),
      plcConfig: await api('plc_get_config'),
      simulator: await api('sim_status'),
      recipes: await api('recipe_list'),
      template: await api('recipe_template', {mode: 'follow'})
    };
  });
}
