import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { once } from 'node:events';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const scratch = path.join(root, 'output/test-temp');
mkdirSync(scratch, { recursive: true });
const windows = process.platform === 'win32';

function fixture() {
  const directory = mkdtempSync(path.join(scratch, 'desktop-'));
  mkdirSync(path.join(directory, 'scripts'));
  for (const name of ['UiTest-Common', 'Desktop-Test', 'Build-UiTestInstance', 'Start-UiTestInstance', 'Stop-UiTestInstance', 'Test-UiProductionFlow']) {
    copyFileSync(path.join(root, `scripts/${name}.ps1`), path.join(directory, `scripts/${name}.ps1`));
  }
  return directory;
}

function remove(directory) {
  assert.equal(path.dirname(directory), scratch);
  rmSync(directory, { recursive: true, force: true });
}

function run(directory, name, args = []) {
  return spawnSync('pwsh', ['-NoProfile', '-File', path.join(directory, `scripts/${name}.ps1`), ...args], {
    cwd: directory, encoding: 'utf8', windowsHide: true, timeout: 20000,
  });
}

test('显式运行库目录无 DLL → 构建启动前失败，不误报为跳过', { skip: !windows }, () => {
  const directory = fixture();
  try {
    const result = run(directory, 'Test-UiProductionFlow', ['-Suite', 'burst', '-LyFlowRuntime', path.join(directory, 'missing-runtime')]);
    assert.notEqual(result.status, 0);
    assert.equal(existsSync(path.join(directory, 'output')), false);
  } finally { remove(directory); }
});

test('同 profile 被另一进程占用 → 拒绝构建和清空资料，释放后可重新取得锁', { skip: !windows }, async () => {
  const directory = fixture();
  const output = path.join(directory, 'output/playwright/flow-regression');
  const marker = path.join(output, 'com.xyzrobotics.tujiaovision.flow-tests', 'keep.txt');
  mkdirSync(path.dirname(marker), { recursive: true });
  writeFileSync(marker, 'preserve');
  const holder = path.join(directory, 'scripts/hold.ps1');
  writeFileSync(holder, `. (Join-Path $PSScriptRoot 'UiTest-Common.ps1')
$layout = Get-UiTestLayout -Profile flow
$lock = Enter-UiTestLock -Path (Join-Path $layout.Output '.run.lock')
try { [Console]::WriteLine('LOCKED'); [void][Console]::ReadLine() } finally { $lock.Dispose() }
`);
  const child = spawn('pwsh', ['-NoProfile', '-File', holder], { cwd: directory, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  const exited = once(child, 'exit');
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('持锁进程未就绪')), 10000);
      child.stdout.once('data', chunk => { clearTimeout(timer); assert.match(String(chunk), /LOCKED/); resolve(); });
      child.once('error', error => { clearTimeout(timer); reject(error); });
    });
    for (const [name, args] of [
      ['Build-UiTestInstance', ['-Profile', 'flow']],
      ['Start-UiTestInstance', ['-Profile', 'flow', '-SkipBuild', '-ResetData']],
    ]) {
      const result = run(directory, name, args);
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /\.run\.lock/);
      assert.equal(readFileSync(marker, 'utf8'), 'preserve');
    }
    child.stdin.end('\n');
    assert.equal((await exited)[0], 0);
    const retry = run(directory, 'hold');
    assert.equal(retry.status, 0, retry.stderr);
  } finally {
    child.stdin.end('\n');
    if (child.exitCode === null) { child.kill(); await exited; }
    remove(directory);
  }
});

test('进程清单身份不匹配 → 拒绝关闭其他进程', { skip: !windows }, () => {
  const directory = fixture();
  const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { windowsHide: true, stdio: 'ignore' });
  try {
    const result = run(directory, 'Stop-UiTestInstance', ['-Profile', 'flow', '-Instance', JSON.stringify({
      pid: child.pid, identifier: 'com.xyzrobotics.tujiaovision.flow-tests',
      executable: process.execPath, startedAt: new Date().toISOString(),
    })]);
    assert.notEqual(result.status, 0);
    assert.doesNotThrow(() => process.kill(child.pid, 0));
  } finally { child.kill(); remove(directory); }
});
