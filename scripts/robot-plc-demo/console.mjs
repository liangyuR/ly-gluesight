const names = {
  idle: '就绪', waiting_ready: '等待视觉就绪', starting: '等待布防', moving: '机器人运动中',
  triggered: '拍照触发', waiting_result: '等待检测结果', acknowledging: '确认结果',
  complete: '本件完成', stopping: '正在停止', stopped: '已停止',
  reset_requested: '复位已请求', error: '需要检查',
};
const labels = { normal: '正常', gap: '断胶', lostFrame: '丢帧', locateFail: '定位失败', countMismatch: '点数不符' };
const signals = [
  ['partStart', 'C10 工件开始'], ['partEnd', 'C11 运动结束'], ['resultAck', 'C12 结果确认'],
  ['faultReset', 'C13 故障复位'], ['visionReady', 'C20 视觉就绪'], ['armed', 'C21 已布防'],
  ['busy', 'C22 检测中'], ['done', 'C23 结果有效'], ['heartbeat', 'C0 心跳'],
];

// The server owns readiness. Missing or stale state never enables an operation.
export function controlsFor(state, online, pending = false) {
  const available = online && state != null && !pending;
  return {
    one: !(available && !state.running && !state.stopping && state.canStart === true),
    continuous: !(available && !state.running && !state.stopping && state.canStart === true),
    stop: !(available && state.running && !state.stopping),
    reset: !(available && !state.running && state.canReset === true),
    scenario: !(available && !state.running),
    reason: !online ? 'Robot 服务暂不可达，操作已暂停；连接恢复后会自动更新。'
      : pending ? '操作正在提交，请稍候。'
      : state?.startBlockedReason || (state?.canStart ? '视觉、相机触发桥与握手均已就绪，可以运行。' : '正在确认服务和握手状态。'),
  };
}

function init() {
  const $ = selector => document.querySelector(selector);
  let state = null;
  let online = false;
  let pending = false;
  let refreshing = null;

  function renderControls() {
    const controls = controlsFor(state, online, pending);
    for (const id of ['one', 'continuous', 'stop', 'reset', 'scenario']) {
      const element = document.getElementById(id);
      element.disabled = controls[id];
      element.title = controls[id] ? controls.reason : '';
    }
    $('#stop').textContent = state?.stopping && online ? '停止请求已收到' : '本件后停止';
    $('#readiness').textContent = controls.reason;
  }

  function renderSignals(plc) {
    $('#signals').replaceChildren(...signals.map(([key, label]) => {
      const element = document.createElement('div');
      element.className = 'signal';
      const lamp = document.createElement('span');
      lamp.className = 'light' + (plc?.[key] ? ' on' : '');
      element.append(lamp, document.createTextNode(label));
      const value = document.createElement('span');
      value.className = 'value';
      value.textContent = plc ? (plc[key] ? '1' : '0') : '—';
      element.append(value);
      return element;
    }));
  }

  function markOffline(error) {
    online = false;
    $('#connection').textContent = 'Robot 服务暂不可达：' + error.message;
    $('#phase').textContent = '连接已中断';
    $('#message').textContent = '无法确认当前工件是否结束。请检查服务，恢复连接后再操作。';
    $('#detail').textContent = '以下结果为最近一次记录；当前 Robot、PLC 和触发桥状态未知。';
    $('#registers').textContent = 'PLC 当前值未知，等待重新连接。';
    renderSignals(null);
    renderControls();
  }

  async function readResponse(response) {
    const body = await response.json();
    if (!response.ok) throw Error(body.error || `服务返回 ${response.status}`);
    return body;
  }

  function renderPath(recipe, k) {
    const svg = $('svg');
    const ns = 'http://www.w3.org/2000/svg';
    const points = recipe.shots;
    const xs = points.map(point => point[0]);
    const ys = points.map(point => point[1]);
    const xmin = Math.min(...xs), ymin = Math.min(...ys);
    const dx = Math.max(...xs) - xmin || 1, dy = Math.max(...ys) - ymin || 1;
    const xy = points.map(point => [50 + (point[0] - xmin) / dx * 320, 38 + (point[1] - ymin) / dy * 110]);
    const make = (tag, attributes) => {
      const element = document.createElementNS(ns, tag);
      for (const [name, value] of Object.entries(attributes)) element.setAttribute(name, value);
      return element;
    };
    svg.replaceChildren(make('polyline', {
      points: xy.map(point => point.join(',')).join(' '), fill: 'none', stroke: '#4674b5', 'stroke-dasharray': '6 4',
    }));
    xy.forEach(([x, y], index) => {
      svg.append(make('circle', { cx: x, cy: y, r: 10, fill: '#30445e', stroke: '#72aaff', 'stroke-width': 2 }));
      const label = make('text', { x: x + 13, y: y + 4, fill: '#b8c9df', 'font-size': 12 });
      label.textContent = 'k' + (index + 1);
      svg.append(label);
    });
    const [x, y] = xy[k ?? 0] || xy[0];
    svg.append(make('circle', { id: 'robot', cx: x, cy: y, r: 6, fill: '#43d5a3' }));
  }

  function renderState(s) {
    $('#phase').textContent = s.stopping ? '正在停止 · 等待本件完成' : names[s.phase] || s.phase;
    $('#message').textContent = s.stopping ? `已收到停止请求，完成本件并释放握手后停止。当前：${s.message}` : s.message;
    $('#detail').textContent = `SN ${s.sn ?? '—'} · 已完成 ${s.parts} 件 · 触发桥 ${s.bridgeOnline ? '在线' : '离线'}`;
    $('#endpoint').textContent = s.plcEndpoint;
    $('#unit').textContent = `站号 ${s.unitId} · 地址从 0 开始 · 32 位值按 ABCD 顺序`;
    $('#recipe-summary').textContent = `${s.recipe.shots.length} 点飞拍 · ${s.recipe.id} · 产品代码 ${s.recipe.productCode}`;
    $('#connection').textContent = s.plcError ? 'PLC 服务未连接：' + s.plcError : 'Modbus TCP 通讯正常';
    renderSignals(s.plc);
    const plc = s.plc || {};
    $('#registers').textContent = `HR100 SN ${plc.partSn ?? '—'} · HR102 产品 ${plc.productCode ?? '—'} · HR103 拍照点 ${plc.shotCount ?? '—'} · HR112 结果 SN ${plc.resultSn ?? '—'}`;
    $('#results').replaceChildren(...s.results.slice(-8).reverse().map(result => {
      const row = document.createElement('tr');
      for (const value of [result.sn, labels[result.scenario], result.resultCode, result.faultCode]) {
        const cell = document.createElement('td');
        cell.textContent = value;
        row.append(cell);
      }
      row.className = result.resultCode === 1 ? 'good' : 'bad';
      return row;
    }));
    $('#events').textContent = s.events.slice(-12).reverse().map(event =>
      `${new Date(event.ts).toLocaleTimeString('zh-CN', { timeZone: 'Asia/Hong_Kong', hour12: false })}  ${event.message}`,
    ).join('\n');
    renderPath(s.recipe, s.k);
    renderControls();
  }

  function refresh() {
    if (refreshing) return refreshing;
    refreshing = (async () => {
      try {
        const response = await fetch('/state', { cache: 'no-store', signal: AbortSignal.timeout(4000) });
        state = await readResponse(response);
        online = true;
        renderState(state);
      } catch (error) {
        markOffline(error);
      }
    })().finally(() => { refreshing = null; });
    return refreshing;
  }

  async function action(path, data = {}) {
    if (pending || !online) return;
    pending = true;
    $('#error').textContent = '';
    renderControls();
    try {
      const response = await fetch(path, {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(data), signal: AbortSignal.timeout(4000),
      });
      await readResponse(response);
    } catch (error) {
      $('#error').textContent = error.message;
    } finally {
      // A poll started before the POST may still contain the previous state.
      // Finish it, then obtain a new snapshot before re-enabling any control.
      if (refreshing) await refreshing;
      await refresh();
      pending = false;
      renderControls();
    }
  }

  for (const [id, continuous] of [['one', false], ['continuous', true]]) {
    document.getElementById(id).addEventListener('click', () => action('/start', { scenario: $('#scenario').value, continuous }));
  }
  $('#stop').addEventListener('click', () => action('/stop'));
  $('#reset').addEventListener('click', () => action('/reset'));
  async function poll() {
    await refresh();
    setTimeout(poll, 600);
  }
  poll();
}

if (typeof document !== 'undefined') init();
