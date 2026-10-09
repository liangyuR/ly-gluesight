import assert from 'node:assert/strict';
import test from 'node:test';
import { controlsFor } from '../console.mjs';

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
