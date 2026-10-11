import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const root = new URL('../../', import.meta.url);
const workflow = readFileSync(new URL('.github/workflows/windows-package.yml', root), 'utf8');

function stepScript(name) {
  const lines = workflow.split(/\r?\n/);
  const start = lines.findIndex((line) => line.trim() === `- name: ${name}`);
  assert.ok(start >= 0, `Missing workflow step: ${name}`);
  const run = lines.findIndex((line, index) => index > start && line === '        run: |');
  assert.ok(run > start, `Missing PowerShell body: ${name}`);
  const body = [];
  for (let index = run + 1; index < lines.length; index += 1) {
    if (lines[index] && !lines[index].startsWith('          ')) break;
    body.push(lines[index].slice(10));
  }
  return body.join('\n');
}

function runVersionGate(change = () => {}, tag = 'v0.1.0') {
  const directory = mkdtempSync(join(tmpdir(), 'gluesight-ci-version-'));
  try {
    const packageJson = { version: '0.1.0', packageManager: 'pnpm@11.1.3', engines: { node: '>=24.15.0' } };
    const tauri = { version: '0.1.0' };
    const windows = { bundle: { windows: { webviewInstallMode: { type: 'skip' } } } };
    const fixture = { packageJson, tauri, windows, rustVersion: '0.1.0' };
    change(fixture);
    mkdirSync(join(directory, 'src-tauri'));
    writeFileSync(join(directory, 'package.json'), JSON.stringify(packageJson));
    writeFileSync(join(directory, 'src-tauri', 'tauri.conf.json'), JSON.stringify(tauri));
    writeFileSync(join(directory, 'src-tauri', 'tauri.windows.conf.json'), JSON.stringify(windows));
    writeFileSync(join(directory, 'src-tauri', 'Cargo.toml'), `[package]\nname = "gluesight"\nversion = "${fixture.rustVersion}"\n\n[lib]\nname = "gluesight_lib"\n`);
    const scriptPath = join(directory, 'gate.ps1');
    writeFileSync(scriptPath, stepScript('Verify versions and installer policy'));
    return spawnSync('pwsh', ['-NoProfile', '-File', scriptPath], {
      cwd: directory,
      env: { ...process.env, GITHUB_OUTPUT: join(directory, 'outputs.txt'), GITHUB_REF_TYPE: 'tag', GITHUB_REF_NAME: tag },
      encoding: 'utf8',
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('release gate accepts matching application versions and the Windows skip policy', () => {
  const result = runVersionGate();
  assert.equal(result.status, 0, result.stderr);
});

test('release gate rejects a version mismatch in each application manifest', () => {
  for (const change of [
    (fixture) => { fixture.packageJson.version = '0.2.0'; },
    (fixture) => { fixture.tauri.version = '0.2.0'; },
    (fixture) => { fixture.rustVersion = '0.2.0'; },
  ]) {
    const result = runVersionGate(change);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /versions must agree/);
  }
});

test('release gate rejects wrong or non-release tags', () => {
  for (const tag of ['v0.2.0', 'v0.1.0-beta.1', 'v0.1']) {
    const result = runVersionGate(undefined, tag);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Tag must be vX.Y.Z/);
  }
});

test('release gate reads the platform config and rejects WebView2 download mode', () => {
  const result = runVersionGate((fixture) => { fixture.windows.bundle.windows.webviewInstallMode.type = 'downloadBootstrapper'; });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /WebView2 installation must be skipped/);
});

test('SDK clean-environment check detects the real Common Files installation', () => {
  const directory = mkdtempSync(join(tmpdir(), 'gluesight-ci-sdk-'));
  try {
    const scriptPath = join(directory, 'gate.ps1');
    writeFileSync(scriptPath, `function Test-Path { param([string]$LiteralPath) return $LiteralPath -eq 'C:/Program Files (x86)/Common Files/MVS' }\n${stepScript('Require a clean SDK environment')}`);
    const result = spawnSync('pwsh', ['-NoProfile', '-File', scriptPath], { encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /runner without system MVS/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
