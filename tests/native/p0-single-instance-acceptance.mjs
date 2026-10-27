import { chromium } from 'playwright-core';
import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { lstat, mkdir, readFile, readdir, realpath, writeFile } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { isolatedDirectory, isolatedStorage, samePath } from './storage-paths.mjs';

process.env.PATH = [...(process.env.GLUESIGHT_NATIVE_PATH ?? '').split(';'), ...(process.env.PATH ?? '').split(';').filter(value => !/^D:/i.test(value))].filter(Boolean).join(';');
const execute = promisify(execFile);
const { values } = parseArgs({ options: {
  instance: { type: 'string' }, output: { type: 'string' },
  cdp: { type: 'string', default: 'http://127.0.0.1:9337' },
} });
for (const key of ['instance', 'output']) assert(values[key], `Missing --${key}`);
const cPath = value => { const path = resolve(value); assert(/^C:\\/i.test(path), `C: required: ${path}`); return path; };
const output = cPath(values.output);
const instance = JSON.parse((await readFile(cPath(values.instance), 'utf8')).replace(/^\uFEFF/, ''));
assert.equal(instance.identifier, 'com.xyzrobotics.tujiaovision.p0-tests');
assert(Number.isInteger(instance.debugPort) && instance.debugPort >= 1024 && instance.debugPort <= 65535);
assert.equal(values.cdp, 'http://127.0.0.1:' + instance.debugPort, 'CDP must match the isolated instance debug port');
assert(typeof instance.startedAt === 'string' && Number.isFinite(new Date(instance.startedAt).getTime()), 'Instance requires its process start time');
assert(instance.dataRoot, '隔离实例未声明资料目录，请重新启动测试实例');
const profile = cPath(instance.dataRoot);
assert(profile.endsWith(instance.identifier));
const projectDir = join(profile, 'projects', JSON.parse((await readFile(join(profile, 'config', 'current-project.json'), 'utf8')).replace(/^\uFEFF/, '')).current);
const executable = cPath(instance.executable);

assert(Number.isInteger(instance.pid) && instance.pid > 0);
await mkdir(output, { recursive: false });

const browser = await chromium.connectOverCDP(values.cdp);
const report = { startedAt: new Date().toISOString(), instance, profile, passed: false };
const processState = async () => JSON.parse((await execute('pwsh', ['-NoProfile', '-Command',
  `$taskProc=Get-Process -Id ${instance.pid} -ErrorAction Stop; @{pid=$taskProc.Id;path=$taskProc.Path;start=$taskProc.StartTime.ToUniversalTime().ToString('o')} | ConvertTo-Json -Compress`],
  { windowsHide: true, encoding: 'utf8' })).stdout);
const databaseState = async () => {
  const code = `import sqlite3,json,pathlib,sys\np=pathlib.Path(sys.argv[1]);c=sqlite3.connect(p.as_uri()+'?mode=ro',uri=True);r={}\nfor (n,) in c.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"):\n q='SELECT * FROM "'+n.replace('"','""')+'" ORDER BY rowid';rows=c.execute(q).fetchall();r[n]={'rows':len(rows),'values':rows}\nprint(json.dumps(r,default=lambda b:list(b)));c.close()`;
  return JSON.parse((await execute('python', ['-c', code, join(projectDir, 'inspection.db')], { windowsHide: true, encoding: 'utf8' })).stdout);
};
const rawState = async vision => {
  const files = [];
  const records = await isolatedDirectory(vision, 'records-offline');
  const canonicalRoot = cPath(await realpath(records));
  const walk = async directory => {
    assert.equal((await realpath(directory)).toLowerCase(), join(canonicalRoot, relative(records, directory)).toLowerCase());
    for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a,b) => a.name.localeCompare(b.name))) {
      const path = join(directory, entry.name);
      const info = await lstat(path);
      assert(!info.isSymbolicLink(), `Linked evidence: ${path}`);
      if (info.isDirectory()) await walk(path);
      else if (/\.pgm$/i.test(entry.name) || entry.name === 'part.json') files.push({path, bytes: info.size});
    }
  };
  await walk(records);
  assert(files.some(file => /\.pgm$/i.test(file.path)), 'Existing originals required');
  return files;
};
try {
  const native = [];
  for (const page of browser.contexts().flatMap(context => context.pages())) if (await page.evaluate(() => !!window.__TAURI_INTERNALS__).catch(() => false)) native.push(page);
  assert.equal(native.length, 1);
  const read = command => native[0].evaluate(command => window.__TAURI_INTERNALS__.invoke(command), command);
  const storage = await isolatedStorage(read, instance, instance.identifier);
  assert(samePath((await read('records_list')).root, join(storage.vision, 'records')), '录制目录不属于隔离实例');
  report.before = { process: await processState(), cycle: await read('cycle_snapshot'), database: await databaseState(), originals: await rawState(storage.vision) };
  assert.equal(resolve(report.before.process.path).toLowerCase(), executable.toLowerCase());
  assert.equal(new Date(report.before.process.start).getTime(), new Date(instance.startedAt).getTime());
  assert.equal(report.before.cycle.phase, 'IDLE');
  assert.equal(report.before.cycle.fault, null);
  const duplicate = spawn(executable, [], { cwd: resolve(executable, '..'), env: { ...process.env, GLUESIGHT_DATA_DIR: profile }, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = '', stderr = '', timedOut = false;
  duplicate.stdout.on('data', bytes => { stdout += bytes.toString(); });
  duplicate.stderr.on('data', bytes => { stderr += bytes.toString(); });
  const exited = await new Promise((accept, reject) => {
    const timer = setTimeout(() => { timedOut = true; duplicate.kill(); }, 20000);
    duplicate.once('error', error => { clearTimeout(timer); reject(error); });
    duplicate.once('exit', (code, signal) => { clearTimeout(timer); accept({ code, signal }); });
  });
  report.duplicate = { pid: duplicate.pid, ...exited, timedOut, stdout, stderr };
  await writeFile(join(output, 'duplicate.stdout.log'), stdout);
  await writeFile(join(output, 'duplicate.stderr.log'), stderr);
  report.after = { process: await processState(), cycle: await read('cycle_snapshot'), database: await databaseState(), originals: await rawState(storage.vision) };
  assert(!timedOut, 'Duplicate did not reject startup');
  assert(Number.isInteger(exited.code) && exited.code !== 0, 'Duplicate must exit nonzero');
  assert((stderr + stdout).includes('独占锁'), 'Expected exclusive AppData lock error');
  assert.deepEqual(report.after.process, report.before.process);
  assert.equal(report.after.cycle.phase, 'IDLE');
  assert.equal(report.after.cycle.fault, null);
  assert.deepEqual(report.after.database, report.before.database);
  assert.deepEqual(report.after.originals, report.before.originals);
  report.passed = true;
} catch (error) {
  report.error = error.stack ?? String(error);
  process.exitCode = 1;
} finally {
  report.finishedAt = new Date().toISOString();
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  await browser.close();
}
console.log(JSON.stringify({passed:report.passed, duplicate:report.duplicate?.code, originals:report.after?.originals.length, error:report.error}));



