import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const scenarios = ['normal', 'gap', 'lostFrame', 'locateFail'];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
  return value;
}

export function recipeContract(recipe) {
  const version = recipe.version === undefined ? 0 : recipe.version;
  if (!Number.isInteger(version) || version < 0 || version > 0xffffffff) throw new Error('recipe.version must be an integer in 0..4294967295');
  return canonical({
    id: recipe.id, version: Math.max(1, version), productCode: recipe.productCode, schemaVersion: recipe.schemaVersion,
    triggerMode: recipe.triggerMode, spacing: recipe.spacing, filterWindow: recipe.filterWindow,
    detect: recipe.detect, limits: recipe.limits,
    shots: recipe.shots.map(shot => ({
      id: shot.id, poseId: shot.poseId, camera: shot.camera, view: shot.view,
      bead: shot.bead, calib: shot.calib ?? shot.camera, skip: shot.skip ?? false,
      path: shot.path ?? [], mmPerPx: shot.mmPerPx ?? null,
      detect: shot.detect ?? recipe.detect, limits: shot.limits ?? recipe.limits,
    })),
  });
}

export function validateTrigger(ticket, fixture, snapshot, layout, rig, settings, engine, binding) {
  if (!ticket || typeof ticket.id !== 'string' || !ticket.id ||
      !Number.isInteger(ticket.sn) || ticket.sn < 1 || ticket.sn > 0xffffffff ||
      !Number.isInteger(ticket.k) || ticket.k < 0 || !scenarios.includes(ticket.scenario)) {
    throw new Error('Invalid virtual trigger ticket');
  }
  const part = snapshot?.part;
  if (snapshot?.phase !== 'ACQUIRE' || !part || part.sn !== ticket.sn ||
      typeof part.cycleId !== 'string' || !part.cycleId || typeof part.recipeRevision !== 'string' || !part.recipeRevision ||
      typeof part.bundleId !== 'string' || !part.bundleId) {
    throw new Error('Ticket does not identify the armed cycle');
  }
  if (fixture?.schemaVersion !== 4 || layout?.schemaVersion !== 4 ||
      ticket.recipeId !== fixture.id || part.recipeId !== fixture.id || layout.id !== fixture.id ||
      layout.revisionId !== part.recipeRevision || ticket.productCode !== fixture.productCode ||
      ticket.shotCount !== fixture.shots?.length || part.n !== fixture.shots?.length ||
      JSON.stringify(recipeContract(layout)) !== JSON.stringify(recipeContract(fixture))) {
    throw new Error('Armed recipe differs from the configured schema 4 fixture');
  }
  const shot = layout.shots[ticket.k];
  if (!shot || ticket.shotId !== shot.id || ticket.poseId !== shot.poseId ||
      ticket.camera !== shot.camera || ticket.view !== shot.view ||
      ticket.ordinal !== layout.shots.slice(0, ticket.k + 1).filter(s => s.camera === shot.camera).length) {
    throw new Error('Ticket shot, Pose, device, view or device ordinal differs from the armed plan');
  }
  const frame = part.frames?.[ticket.k];
  if (!frame || frame.shotId !== shot.id || frame.camera !== shot.camera || frame.view !== shot.view || frame.status !== 'waiting') {
    throw new Error('Armed frame identity differs or the shot has already arrived');
  }
  if (!Array.isArray(rig)) throw new Error('Camera rig is unavailable');
  for (const planned of layout.shots) {
    const cameras = rig.filter(camera => camera.id === planned.camera);
    if (cameras.length !== 1 || cameras[0].source !== 'sim' || cameras[0].acquisition !== 'triggered' ||
        ![1, 3].includes(cameras[0].viewCount) || !Number.isInteger(planned.view) ||
        planned.view < 1 || planned.view > cameras[0].viewCount) {
      throw new Error('Device ' + planned.camera + ' must be a triggered simulator with the planned views');
    }
  }
  if (settings?.vision !== true || engine?.backend !== 'LyFlow' || engine.ready !== true || engine.measuring !== true ||
      typeof engine.path !== 'string' || !engine.path) {
    throw new Error('The demo requires enabled image measurement and a ready lyFlow DLL');
  }
  if (ticket.k > 0 && (!binding || binding.k + 1 !== ticket.k || binding.sn !== ticket.sn || binding.cycleId !== part.cycleId ||
      binding.recipeRevision !== part.recipeRevision || binding.bundleId !== (part.bundleId ?? null))) {
    throw new Error('Cycle or frozen resources changed between triggers; restart the part');
  }
  return {
    cycleId: part.cycleId, recipeRevision: part.recipeRevision, bundleId: part.bundleId ?? null,
    recipeId: part.recipeId, productCode: layout.productCode, shotCount: part.n,
    sn: ticket.sn, k: ticket.k, shotId: shot.id, poseId: shot.poseId,
    camera: shot.camera, view: shot.view, ordinal: ticket.ordinal,
  };
}

export async function executeTrigger(ticket, fixture, invoke, binding) {
  const snapshot = await invoke('cycle_snapshot');
  const [layout, rig, settings, engine, app] = await Promise.all([
    invoke('cycle_layout', { recipeId: snapshot?.part?.recipeId, revisionId: snapshot?.part?.recipeRevision }),
    invoke('camera_rig_config'), invoke('cycle_get_settings'), invoke('engine_status'), invoke('app_info'),
  ]);
  const identity = validateTrigger(ticket, fixture, snapshot, layout, rig, settings, engine, binding);
  const current = await invoke('cycle_snapshot');
  const confirmed = validateTrigger(ticket, fixture, current, layout, rig, settings, engine, binding);
  if (JSON.stringify(confirmed) !== JSON.stringify(identity)) throw new Error('Armed cycle changed during trigger preflight');
  await invoke('sim_robot_trigger', { sn: ticket.sn, k: ticket.k, scenario: ticket.scenario });
  return { id: ticket.id, ok: true, identity, evidence: { scope: 'demo-app', engine, app } };
}

export function triggerDispatcher(fixture, invoke) {
  const acknowledgements = new Map();
  let binding;
  return async ticket => {
    if (!ticket || typeof ticket.id !== 'string') return { id: ticket?.id ?? null, ok: false, error: 'Invalid virtual trigger ticket' };
    const signature = JSON.stringify(canonical(ticket));
    const previous = acknowledgements.get(ticket.id);
    if (previous) {
      if (previous.signature !== signature) return { id: ticket.id, ok: false, error: 'Trigger ID was reused with a different ticket' };
      return previous.ack;
    }
    let ack;
    try {
      ack = await executeTrigger(ticket, fixture, invoke, binding);
      binding = ack.identity;
    } catch (error) {
      ack = { id: ticket.id, ok: false, error: String(error) };
    }
    acknowledgements.set(ticket.id, { signature, ack });
    if (acknowledgements.size > 256) acknowledgements.delete(acknowledgements.keys().next().value);
    return ack;
  };
}

async function main() {
  const { values } = parseArgs({ options: {
    config: { type: 'string', default: fileURLToPath(new URL('demo.config.json', import.meta.url)) },
    recipe: { type: 'string' },
  } });
  const configPath = resolve(values.config);
  const config = JSON.parse((await readFile(configPath, 'utf8')).replace(/^\uFEFF/, ''));
  if (config.schemaVersion !== 1 || ![config.robot?.port, config.bridge?.debugPort].every(p => Number.isInteger(p) && p > 0 && p < 65536)) {
    throw new Error('Invalid simulator configuration');
  }
  const recipePath = values.recipe ? resolve(values.recipe) : resolve(dirname(configPath), config.robot.recipe);
  const fixture = JSON.parse((await readFile(recipePath, 'utf8')).replace(/^\uFEFF/, ''));
  if (fixture.schemaVersion !== 4 || !Array.isArray(fixture.shots)) throw new Error('Expected a schema 4 recipe fixture');
  const robotUrl = 'http://127.0.0.1:' + config.robot.port;
  const debugUrl = 'http://127.0.0.1:' + config.bridge.debugPort;
  let socket, counter = 0, validated = false;
  const requests = new Map();

  async function evaluate(expression) {
    if (!socket || socket.readyState !== WebSocket.OPEN) throw new Error('WebView disconnected');
    const id = ++counter;
    const response = await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => { requests.delete(id); reject(new Error('Camera bridge timed out')); }, 6000);
      requests.set(id, { resolve, reject, timeout });
      socket.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
    });
    if (response.exceptionDetails) throw new Error(response.exceptionDetails.exception?.description ?? 'WebView evaluation failed');
    return response.result?.value;
  }

  const invoke = (command, args = {}) => evaluate('window.__TAURI_INTERNALS__.invoke(' + JSON.stringify(command) + ', ' + JSON.stringify(args) + ')');
  const dispatch = triggerDispatcher(fixture, invoke);

  async function connect() {
    validated = false;
    if (socket) socket.close();
    const tabs = await fetch(debugUrl + '/json/list', { signal: AbortSignal.timeout(2500) }).then(r => r.json());
    const targets = tabs.filter(t => t.type === 'page' && t.url.startsWith('http://tauri.localhost/'));
    if (targets.length !== 1) throw new Error('Expected exactly one GlueSight WebView');
    const connected = socket = new WebSocket(targets[0].webSocketDebuggerUrl);
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('WebView connection timed out')), 5000);
      connected.addEventListener('open', () => { clearTimeout(timeout); resolve(); }, { once: true });
      connected.addEventListener('error', error => { clearTimeout(timeout); reject(error); }, { once: true });
    });
    connected.addEventListener('message', event => {
      if (socket !== connected) return;
      const message = JSON.parse(String(event.data));
      const request = requests.get(message.id);
      if (request) {
        requests.delete(message.id);
        clearTimeout(request.timeout);
        message.error ? request.reject(new Error(JSON.stringify(message.error))) : request.resolve(message.result);
      }
    });
    connected.addEventListener('close', () => {
      if (socket !== connected) return;
      validated = false;
      for (const request of requests.values()) {
        clearTimeout(request.timeout);
        request.reject(new Error('WebView disconnected'));
      }
      requests.clear();
    });
    const location = (await invoke('records_list')).root;
    if (!String(location).split(/[\\/]/).includes('com.xyzrobotics.gluesight.robot-plc-demo')) {
      connected.close();
      throw new Error('Refusing to attach trigger bridge to a non-demo instance');
    }
    validated = true;
    console.log(new Date().toISOString(), 'Virtual device trigger bridge connected', location);
  }

  while (true) {
    try {
      if (!validated || !socket || socket.readyState !== WebSocket.OPEN) await connect();
      const response = await fetch(robotUrl + '/trigger', { signal: AbortSignal.timeout(2500) });
      if (!response.ok) throw new Error(await response.text());
      const ticket = await response.json();
      if (ticket) {
        const ack = await dispatch(ticket);
        const reply = await fetch(robotUrl + '/trigger-ack', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(ack), signal: AbortSignal.timeout(2500) });
        if (!reply.ok) throw new Error(await reply.text());
        console.log(new Date().toISOString(), JSON.stringify(ack));
      }
      await sleep(40);
    } catch (error) {
      console.error(new Date().toISOString(), String(error));
      await sleep(1000);
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
