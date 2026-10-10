import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { controlsFor, shotPlan } from '../console.mjs';
import { recipeContract, validateTrigger, triggerDispatcher } from '../camera-bridge.mjs';

const idle = { running: false, stopping: false, canStart: true, canReset: true, startBlockedReason: null };

test('losing the HTTP service disables previously available controls', () => {
  assert.equal(controlsFor(idle, true).one, false);
  const offline = controlsFor(idle, false);
  for (const key of ['one', 'continuous', 'stop', 'reset', 'scenario']) assert.equal(offline[key], true, key);
  assert.match(offline.reason, /服务暂不可达/);
});

test('unknown state and an in-flight operation cannot leave buttons enabled', () => {
  for (const controls of [controlsFor(null, false), controlsFor(undefined, true), controlsFor(idle, true, true)]) {
    for (const key of ['one', 'continuous', 'stop', 'reset', 'scenario']) assert.equal(controls[key], true, key);
  }
});

test('a running part can be asked to stop once and cannot be reset early', () => {
  const running = { ...idle, running: true, canStart: false, canReset: false, startBlockedReason: 'Robot 正在运行' };
  assert.equal(controlsFor(running, true).stop, false);
  const stopping = controlsFor({ ...running, stopping: true, startBlockedReason: '已收到停止请求，正在完成本件' }, true);
  for (const key of ['one', 'continuous', 'stop', 'reset', 'scenario']) assert.equal(stopping[key], true, key);
  assert.match(stopping.reason, /已收到停止请求/);
  assert.equal(controlsFor(idle, true).one, false);
});

test('a busy handshake blocks start while an idle Robot can still request fault reset', () => {
  const controls = controlsFor({ ...idle, canStart: false, startBlockedReason: '检测流程尚未结束，请等待本件完成' }, true);
  assert.equal(controls.one, true);
  assert.equal(controls.continuous, true);
  assert.equal(controls.reset, false);
  assert.match(controls.reason, /检测流程尚未结束/);
});

test('PLC loss disables start and reset without hiding a running Robot stop action', () => {
  const controls = controlsFor({ running: true, stopping: false, canStart: false, canReset: false, startBlockedReason: 'PLC 服务未连接' }, true);
  assert.equal(controls.one, true);
  assert.equal(controls.reset, true);
  assert.equal(controls.stop, false);
});

function fixture(name = 'ROBOT-DEMO-TRICAM') {
  return JSON.parse(readFileSync(new URL(`../fixtures/${name}.json`, import.meta.url), 'utf8'));
}

function context(recipe = fixture()) {
  const layout = { ...structuredClone(recipe), revisionId: 'ROBOT-DEMO-v1' };
  const rig = [...new Set(recipe.shots.map(shot => shot.camera))].map(id => ({
    id, source: 'sim', acquisition: 'triggered', viewCount: recipe.shots.some(s => s.camera === id && s.view > 1) ? 3 : 1,
  }));
  const snapshot = { phase: 'ACQUIRE', part: {
    sn: 123, cycleId: 'scripted-cycle-1', recipeId: recipe.id, recipeRevision: layout.revisionId,
    bundleId: 'release-1', n: recipe.shots.length,
    frames: recipe.shots.map(s => ({ shotId: s.id, camera: s.camera, view: s.view, status: 'waiting' })),
  } };
  return { recipe, layout, rig, snapshot, settings: { vision: true },
    engine: { backend: 'LyFlow', ready: true, measuring: true, path: 'scripted-test.dll', version: 'test' } };
}

function ticket(ctx, k = 0) {
  const point = shotPlan(ctx.recipe)[k];
  return { id: `scripted-session:123:${k}`, sn: 123, k, scenario: 'normal',
    recipeId: ctx.recipe.id, productCode: ctx.recipe.productCode, shotCount: ctx.recipe.shots.length,
    shotId: point.id, poseId: point.poseId, camera: point.camera, view: point.view, ordinal: point.ordinal };
}

function check(ctx, pulse = ticket(ctx), binding) {
  return validateTrigger(pulse, ctx.recipe, ctx.snapshot, ctx.layout, ctx.rig, ctx.settings, ctx.engine, binding);
}

function rpc(ctx, calls, snapshots = []) {
  return async (command, args) => {
    calls.push({ command, args });
    switch (command) {
      case 'cycle_snapshot': return structuredClone(snapshots.shift() ?? ctx.snapshot);
      case 'cycle_layout':
        assert.deepEqual(args, { recipeId: ctx.recipe.id, revisionId: ctx.layout.revisionId });
        return ctx.layout;
      case 'camera_rig_config': return ctx.rig;
      case 'cycle_get_settings': return ctx.settings;
      case 'engine_status': return ctx.engine;
      case 'app_info': return { name: 'scripted-test-app', version: 'test' };
      case 'sim_robot_trigger': return null;
      default: throw new Error(`Unexpected command ${command}`);
    }
  };
}

test('shot plans count devices once per pulse and keep view 1→2→3→1 separate from device ordinals', () => {
  const tricam = shotPlan(fixture());
  assert.deepEqual(tricam.map(p => p.view), [1, 2, 3, 1]);
  assert.deepEqual(tricam.map(p => p.ordinal), [1, 2, 3, 4]);
  const devices = shotPlan(fixture('ROBOT-DEMO-3CAM'));
  assert.deepEqual(devices.map(p => p.camera), ['cam1', 'cam2', 'cam3', 'cam1']);
  assert.deepEqual(devices.map(p => p.ordinal), [1, 1, 1, 2]);
});

for (const version of [undefined, 0]) test('missing/zero fixture version uses backend version1 and permits actual bridge dispatch: ' + String(version), async () => {
  const recipe = fixture();
  if (version === undefined) delete recipe.version; else recipe.version = version;
  const ctx = context(recipe), calls = [];
  ctx.layout.version = 1; ctx.layout.revisionId = `${ctx.recipe.id}-v1`;
  ctx.snapshot.part.recipeRevision = ctx.layout.revisionId;
  assert.deepEqual(recipeContract(ctx.recipe), recipeContract(ctx.layout));
  const dispatch = triggerDispatcher(ctx.recipe, rpc(ctx, calls));
  for (let k = 0; k < ctx.recipe.shots.length; k++) {
    const ack = await dispatch(ticket(ctx, k));
    assert.equal(ack.ok, true, ack.error);
  }
  assert.deepEqual(calls.filter(call => call.command === 'sim_robot_trigger').map(call => call.args),
    [0, 1, 2, 3].map(k => ({ sn: 123, k, scenario: 'normal' })));
});

test('explicit positive fixture versions are preserved and cannot match another backend version', () => {
  for (const version of [1, 7, 0xffffffff]) {
    const ctx = context(); ctx.recipe.version = version; ctx.layout.version = version;
    assert.equal(recipeContract(ctx.recipe).version, version); assert.equal(check(ctx).sn, 123);
    ctx.layout.version = version === 1 ? 2 : 1;
    assert.throws(() => check(ctx), /differs/);
  }
});

test('invalid fixture/backend versions are rejected before any bridge pulse', async () => {
  for (const version of [null, true, false, -1, 0.5, NaN, Infinity, 0x100000000, '1']) {
    for (const target of ['recipe', 'layout']) {
      const ctx = context(), calls = []; ctx[target].version = version;
      assert.throws(() => recipeContract(ctx[target]), /recipe.version/);
      const ack = await triggerDispatcher(ctx.recipe, rpc(ctx, calls))(ticket(ctx));
      assert.equal(ack.ok, false); assert.match(ack.error, /recipe.version/);
      assert.equal(calls.filter(call => call.command === 'sim_robot_trigger').length, 0);
    }
  }
});
test('release metadata may differ but explicit version, geometry and limits must match', () => {
  const ctx = context();
  ctx.layout.teachingId = 'published';
  delete ctx.layout.shots[0].detect;
  delete ctx.layout.shots[0].limits;
  assert.deepEqual(recipeContract(ctx.layout), recipeContract(ctx.recipe));
  assert.equal(check(ctx).view, 1);
  ctx.layout.version = 19;
  assert.throws(() => check(ctx), /differs/);
  ctx.layout.version = ctx.recipe.version;
  ctx.layout.shots[0].mmPerPx = 0.08;
  assert.throws(() => check(ctx), /differs/);
});

test('preflight refuses wrong SN, shot, Pose, camera, view, ordinal, revision and malformed schema', () => {
  const edits = [
    ctx => { ctx.snapshot.part.sn++; }, ctx => { ctx.layout.revisionId = 'other'; },
    ctx => { ctx.layout.schemaVersion = 3; }, ctx => { ctx.snapshot.part.cycleId = ''; },
    ctx => { ctx.snapshot.part.bundleId = null; },
    ctx => { ctx.snapshot.part.n = 12; }, ctx => { ctx.snapshot.phase = 'DRAIN'; },
    ctx => { ctx.snapshot.part.frames[0].shotId = 'P4'; }, ctx => { ctx.snapshot.part.frames[0].status = 'done'; },
  ];
  for (const edit of edits) {
    const ctx = context();
    edit(ctx);
    assert.throws(() => check(ctx));
  }
  for (const [key, value] of [['shotId', 'P4'], ['poseId', 'Pose-3'], ['camera', 'cam2'], ['view', 3], ['ordinal', 2], ['k', -1], ['scenario', 'countMismatch']]) {
    const ctx = context();
    assert.throws(() => check(ctx, { ...ticket(ctx), [key]: value }), key);
  }
});

test('all planned devices must expose the required views through triggered simulation', () => {
  const edits = [
    ctx => { ctx.rig[0].viewCount = 1; }, ctx => { ctx.rig[0].viewCount = 2; },
    ctx => { ctx.rig[0].source = 'mvs'; }, ctx => { ctx.rig[0].source = 'replay'; },
    ctx => { ctx.rig[0].acquisition = 'freeRun'; }, ctx => { ctx.rig.push({ ...ctx.rig[0] }); },
    ctx => { ctx.rig = []; }, ctx => { ctx.settings.vision = false; },
    ctx => { ctx.engine.measuring = false; }, ctx => { ctx.engine.ready = false; },
    ctx => { ctx.engine.backend = '模拟测量'; },
  ];
  for (const edit of edits) {
    const ctx = context();
    edit(ctx);
    assert.throws(() => check(ctx));
  }
});

test('bridge invokes the existing Rust argument contract once and reuses an acknowledgement after a lost HTTP reply', async () => {
  const ctx = context(), calls = [];
  const dispatch = triggerDispatcher(ctx.recipe, rpc(ctx, calls));
  for (let k = 0; k < 4; k++) {
    const pulse = ticket(ctx, k);
    const ack = await dispatch(pulse);
    assert.equal(ack.ok, true, ack.error);
    assert.equal(ack.identity.cycleId, 'scripted-cycle-1');
    assert.deepEqual(await dispatch(pulse), ack);
  }
  assert.deepEqual(calls.filter(c => c.command === 'sim_robot_trigger').map(c => c.args),
    [0, 1, 2, 3].map(k => ({ sn: 123, k, scenario: 'normal' })));
  const changed = await dispatch({ ...ticket(ctx), view: 3 });
  assert.equal(changed.ok, false);
  assert.match(changed.error, /reused/);
});

test('wrong device and view are rejected before any pulse, with negative acknowledgements cached', async () => {
  for (const key of ['camera', 'view']) {
    const ctx = context(), calls = [];
    const dispatch = triggerDispatcher(ctx.recipe, rpc(ctx, calls));
    const pulse = { ...ticket(ctx), [key]: key === 'camera' ? 'wrong-device' : 3 };
    const ack = await dispatch(pulse);
    assert.equal(ack.ok, false);
    assert.equal(calls.some(c => c.command === 'sim_robot_trigger'), false);
    assert.deepEqual(await dispatch(pulse), ack);
  }
});

test('same-SN cycle changes, bundle changes and a restarted bridge cannot resume at a later shot', async () => {
  for (const edit of [ctx => { ctx.snapshot.part.cycleId = 'scripted-cycle-2'; },
    ctx => { ctx.snapshot.part.bundleId = 'different-bundle'; }]) {
    const ctx = context(), calls = [];
    const dispatch = triggerDispatcher(ctx.recipe, rpc(ctx, calls));
    assert.equal((await dispatch(ticket(ctx))).ok, true);
    edit(ctx);
    assert.equal((await dispatch(ticket(ctx, 1))).ok, false);
    assert.equal(calls.filter(c => c.command === 'sim_robot_trigger').length, 1);
  }
  const ctx = context(), calls = [];
  const dispatch = triggerDispatcher(ctx.recipe, rpc(ctx, calls));
  assert.equal((await dispatch(ticket(ctx, 1))).ok, false);
  assert.equal(calls.some(c => c.command === 'sim_robot_trigger'), false);
});

test('a cycle change during preflight and an unavailable DLL never send a camera pulse', async () => {
  const ctx = context(), calls = [];
  const changed = structuredClone(ctx.snapshot);
  changed.part.cycleId = 'changed-before-pulse';
  const dispatch = triggerDispatcher(ctx.recipe, rpc(ctx, calls, [ctx.snapshot, changed]));
  assert.equal((await dispatch(ticket(ctx))).ok, false);
  assert.equal(calls.some(c => c.command === 'sim_robot_trigger'), false);
  const unavailable = triggerDispatcher(ctx.recipe, async command => {
    if (command === 'engine_status') throw new Error('scripted DLL unavailable');
    return rpc(ctx, calls)(command);
  });
  assert.equal((await unavailable(ticket(ctx))).ok, false);
});
