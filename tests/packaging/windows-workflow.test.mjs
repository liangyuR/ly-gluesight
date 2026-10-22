import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import test from 'node:test';

const root = new URL('../../', import.meta.url);
const workflow = readFileSync(new URL('.github/workflows/windows-package.yml', root), 'utf8');
const gate = fileURLToPath(new URL('scripts/Test-PackageVersions.ps1', root));
const setVersion = fileURLToPath(new URL('scripts/Set-PackageVersion.ps1', root));
const releaseManifest = fileURLToPath(new URL('scripts/Write-ReleaseManifest.ps1', root));

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

function withFixture(change, callback) {
  const directory = mkdtempSync(join(tmpdir(), 'gluesight-ci-version-'));
  try {
    const fixture = {
      packageJson: { version: '0.1.0', packageManager: 'pnpm@12.11.2', engines: { node: '>=24.15.0' } },
      tauri: { version: '0.1.0' },
      windows: { bundle: { windows: { webviewInstallMode: { type: 'skip' } } } },
      rustVersion: '0.1.0', lockVersion: '0.1.0',
    };
    change?.(fixture);
    mkdirSync(join(directory, 'src-tauri'));
    writeFileSync(join(directory, 'package.json'), JSON.stringify(fixture.packageJson));
    writeFileSync(join(directory, 'src-tauri', 'tauri.conf.json'), JSON.stringify(fixture.tauri));
    writeFileSync(join(directory, 'src-tauri', 'tauri.windows.conf.json'), JSON.stringify(fixture.windows));
    writeFileSync(join(directory, '.node-version'), '24.15.0\n');
    writeFileSync(join(directory, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.99.0"\n');
    writeFileSync(join(directory, 'src-tauri', 'Cargo.toml'), `[package]\nname = "gluesight"\nversion = "${fixture.rustVersion}"\n\n[dependencies]\nlyflow-client = { git = "https://github.com/liangyuR/lyFlow", rev = "fixed-revision" }\n`);
    writeFileSync(join(directory, 'src-tauri', 'Cargo.lock'), `version = 4\n\n[[package]]\nname = "gluesight"\nversion = "${fixture.lockVersion}"\ndependencies = ["lyflow-client"]\n\n[[package]]\nname = "lyflow-client"\nversion = "1.1.0"\nsource = "git+https://github.com/liangyuR/lyFlow?rev=fixed-revision#fixed-revision"\n`);
    return callback(directory);
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

function runVersionGate(change, tag = 'v0.1.0') {
  return withFixture(change, (directory) => spawnSync('pwsh', ['-NoProfile', '-File', gate, '-ProjectRoot', directory, '-Tag', tag], { encoding: 'utf8' }));
}

test('shared release gate accepts matching versions and installer policy', () => {
  const result = runVersionGate();
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout.slice(result.stdout.indexOf('{'))), { application: '0.1.0', node: '24.15.0', pnpm: '12.11.2', rust: '1.99.0' });
});

test('shared release gate rejects version mismatch including Cargo.lock', () => {
  for (const change of [
    (fixture) => { fixture.packageJson.version = '0.2.0'; },
    (fixture) => { fixture.tauri.version = '0.2.0'; },
    (fixture) => { fixture.rustVersion = '0.2.0'; },
    (fixture) => { fixture.lockVersion = '0.2.0'; },
  ]) {
    const result = runVersionGate(change);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /versions must agree/);
  }
});

test('shared release gate rejects wrong or prerelease tags', () => {
  for (const tag of ['v0.2.0', 'v0.1.0-beta.1', 'v0.1']) {
    const result = runVersionGate(undefined, tag);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Tag must be vX.Y.Z/);
  }
});

test('shared release gate rejects WebView2 download mode', () => {
  const result = runVersionGate((fixture) => { fixture.windows.bundle.windows.webviewInstallMode.type = 'downloadBootstrapper'; });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /WebView2 installation must be skipped/);
});

test('release ancestry accepts a tag on main and rejects a tag ahead of main', () => {
  withFixture(undefined, (directory) => {
    const env = { ...process.env, GIT_DIR: join(directory, 'git-metadata'), GIT_WORK_TREE: directory };
    const git = (...args) => {
      const result = spawnSync('git', ['-C', directory, '-c', 'user.name=Packaging test', '-c', 'user.email=packaging-test@example.invalid', '-c', 'core.hooksPath=NUL', ...args], { encoding: 'utf8', env });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git('init', '-b', 'main');
    git('add', 'package.json');
    git('commit', '-m', 'fixture main');
    git('update-ref', 'refs/remotes/origin/main', 'HEAD');
    git('tag', 'v0.1.0');
    const run = () => spawnSync('pwsh', ['-NoProfile', '-File', gate, '-ProjectRoot', directory, '-Tag', 'v0.1.0', '-VerifyMainAncestor'], { encoding: 'utf8', env });
    const accepted = run();
    assert.equal(accepted.status, 0, accepted.stderr);
    writeFileSync(join(directory, 'outside-main.txt'), 'new branch commit');
    git('add', 'outside-main.txt');
    git('commit', '-m', 'fixture outside main');
    git('tag', '-f', 'v0.1.0');
    const rejected = run();
    assert.notEqual(rejected.status, 0);
    assert.match(rejected.stderr, /main history/);
  });
});

test('version setter synchronizes all application manifests without modifying Git dependencies', () => {
  withFixture(undefined, (directory) => {
    const result = spawnSync('pwsh', ['-NoProfile', '-File', setVersion, '0.2.3', '-ProjectRoot', directory], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8')).version, '0.2.3');
    assert.equal(JSON.parse(readFileSync(join(directory, 'src-tauri/tauri.conf.json'), 'utf8')).version, '0.2.3');
    assert.match(readFileSync(join(directory, 'src-tauri/Cargo.toml'), 'utf8'), /rev = "fixed-revision"/);
    assert.match(readFileSync(join(directory, 'src-tauri/Cargo.lock'), 'utf8'), /name = "lyflow-client"\nversion = "1.1.0"/);
  });
});

test('version setter rejects invalid input before modifying manifests', () => {
  withFixture(undefined, (directory) => {
    const before = readFileSync(join(directory, 'package.json'), 'utf8');
    const result = spawnSync('pwsh', ['-NoProfile', '-File', setVersion, '0.2.3-beta', '-ProjectRoot', directory], { encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.equal(readFileSync(join(directory, 'package.json'), 'utf8'), before);
  });
});

test('SDK clean-environment check detects the real Common Files installation', () => {
  withFixture(undefined, (directory) => {
    const scriptPath = join(directory, 'gate.ps1');
    writeFileSync(scriptPath, `function Test-Path { param([string]$LiteralPath) return $LiteralPath -eq 'C:/Program Files (x86)/Common Files/MVS' }\n${stepScript('Require a clean SDK environment')}`);
    const result = spawnSync('pwsh', ['-NoProfile', '-File', scriptPath], { encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /runner without system MVS/);
  });
});

test('package gate executes the existing actual DLL visual checks before bundling', () => {
  stepScript('Verify actual DLL vision and application runtime');
  assert.ok(workflow.indexOf('- name: Verify actual DLL vision') < workflow.indexOf('- name: Bundle NSIS installer'));
  assert.match(readFileSync(new URL('scripts/Test-PackageApplication.ps1', root), 'utf8'), /--check-packaged-runtime/);
});

test('dependency cache identity uses the locked Tauri version and platform NSIS directory', () => {
  const directory = mkdtempSync(join(tmpdir(), 'gluesight-cache-identity-'));
  try {
    mkdirSync(join(directory, 'packaging'));
    writeFileSync(join(directory, 'packaging/runtime.json'), JSON.stringify({ repository: 'example/repo', tag: 'vendor-v2', asset: 'vendor-v2.zip', bundleVersion: 2 }));
    writeFileSync(join(directory, 'pnpm-lock.yaml'), "importers:\n  .:\n    devDependencies:\n      '@tauri-apps/cli':\n        specifier: ^2.11.5\n        version: 2.11.5\n");
    const scriptPath = join(directory, 'identity.ps1');
    writeFileSync(scriptPath, `$ErrorActionPreference = 'Stop'\n${stepScript('Read dependency cache identities')}`);
    const outputPath = join(directory, 'outputs.txt');
    const result = spawnSync('pwsh', ['-NoProfile', '-File', scriptPath], {
      cwd: directory,
      env: { ...process.env, GITHUB_OUTPUT: outputPath, LOCALAPPDATA: join(directory, 'app-data') },
      encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stderr);
    const output = readFileSync(outputPath, 'utf8');
    assert.match(output, /nsis=nsis-windows-2\.11\.5/);
    assert.ok(output.includes(`nsis-path=${join(directory, 'app-data', 'tauri', 'NSIS')}`));
    assert.match(workflow, /"pnpm":"\$\{\{ steps\.node-tools\.outputs\.cache-hit \}\}"/);
    assert.match(workflow, /"nsis":"\$\{\{ steps\.nsis-cache\.outputs\.cache-hit \}\}"/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('release manifest records the packaged lyFlow version, timings and cache outcomes', () => {
  withFixture(undefined, (directory) => {
    const env = { ...process.env, GIT_DIR: join(directory, 'git-metadata'), GIT_WORK_TREE: directory };
    const toolDirectory = join(directory, 'tools');
    mkdirSync(toolDirectory);
    for (const [name, version] of Object.entries({ node: 'v24.15.0', pnpm: '12.11.2', rustc: 'rustc 1.99.0', cargo: 'cargo 1.99.0' })) {
      writeFileSync(join(toolDirectory, `${name}.cmd`), `@echo ${version}\r\n`);
    }
    env.PATH = `${toolDirectory};${env.PATH ?? env.Path}`;
    for (const args of [['init', '-b', 'main'], ['add', 'package.json'], ['commit', '-m', 'fixture manifest']]) {
      const result = spawnSync('git', ['-C', directory, '-c', 'user.name=Packaging test', '-c', 'user.email=packaging-test@example.invalid', '-c', 'core.hooksPath=NUL', ...args], { encoding: 'utf8', env });
      assert.equal(result.status, 0, result.stderr);
    }
    mkdirSync(join(directory, 'packaging'));
    writeFileSync(join(directory, 'packaging/runtime.json'), JSON.stringify({ repository: 'example/repository', tag: 'vendor-v2', asset: 'vendor-v2.zip', bundleVersion: 2, mvsVersion: '4.8.1.2', lyflow: { repository: 'liangyuR/lyFlow', series: '1.1' } }));
    const runtimeManifest = join(directory, 'runtime/manifest.json');
    mkdirSync(join(directory, 'runtime'));
    writeFileSync(runtimeManifest, JSON.stringify({ lyflow: { repository: 'liangyuR/lyFlow', series: '1.1', version: '1.1.3', abiVersion: '1.1.0', clientTag: 'v1.1.3', operators: [] } }));
    writeFileSync(join(directory, 'timings.txt'), 'runtime=1.250\nrust=2.500\n');
    const outputPath = join(directory, 'reports/release-manifest.json');
    const result = spawnSync('pwsh', ['-NoProfile', '-File', releaseManifest, '-ProjectRoot', directory, '-OutputPath', outputPath, '-RuntimeManifestPath', runtimeManifest, '-TimingPath', join(directory, 'timings.txt'), '-CacheStatusJson', '{"enabled":true,"lyflow":"true"}', '-Status', 'success'], { encoding: 'utf8', env, cwd: directory });
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(readFileSync(outputPath, 'utf8'));
    assert.deepEqual(manifest.lyflow, { repository: 'liangyuR/lyFlow', series: '1.1', version: '1.1.3', abiVersion: '1.1.0', clientTag: 'v1.1.3' });
    assert.deepEqual(manifest.stageSeconds, { runtime: 1.25, rust: 2.5 });
    assert.equal(manifest.caches.lyflow, 'true');
    assert.equal(manifest.application.version, '0.1.0');
    assert.equal(manifest.validationStatus, 'success');
    assert.equal(manifest.tools.rustc, 'rustc 1.99.0');
  });
});
