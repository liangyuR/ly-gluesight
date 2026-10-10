import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { lstat, mkdir, readFile, readdir, realpath, writeFile } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import { parseArgs, promisify } from 'node:util';
import { pathToFileURL } from 'node:url';

process.env.PATH = ['C:\\Users\\11601\\AppData\\Local\\Temp\\gluesight-p0-recovery-20261010\\native', 'C:\\vcpkg\\installed\\x64-windows\\bin', ...(process.env.PATH ?? '').split(';').filter(value => value && !/^D:/i.test(value))].join(';');
const execute = promisify(execFile);
const { values } = parseArgs({ options: {
  instance: { type: 'string' }, output: { type: 'string' },
  'playwright-module': { type: 'string' }, cdp: { type: 'string', default: 'http://127.0.0.1:9338' },
} });
for (const key of ['instance', 'output', 'playwright-module']) assert(values[key], `Missing --${key}`);
const cPath = value => { const path = resolve(value); assert(/^C:\\/i.test(path), `C: required: ${path}`); return path; };
const output = cPath(values.output);
const instance = JSON.parse((await readFile(cPath(values.instance), 'utf8')).replace(/^\uFEFF/, ''));
assert(['com.xyzrobotics.tujiaovision.p0-tests.performance', 'com.xyzrobotics.tujiaovision.p0-tests.performance.nohash'].includes(instance.identifier));
const profile = join('C:\\Users\\11601\\AppData\\Roaming', instance.identifier);
const executable = cPath(instance.executable);

assert(Number.isInteger(instance.pid) && instance.pid > 0);
await mkdir(output, { recursive: false });
const { chromium } = await import(pathToFileURL(cPath(values['playwright-module'])).href);
const browser = await chromium.connectOverCDP(values.cdp);
const report = { startedAt: new Date().toISOString(), instance, profile, passed: false };
const processState = async () => JSON.parse((await execute('pwsh', ['-NoProfile', '-Command',
  `$taskProc=Get-Process -Id ${instance.pid} -ErrorAction Stop; @{pid=$taskProc.Id;path=$taskProc.Path;start=$taskProc.StartTime.ToUniversalTime().ToString('o')} | ConvertTo-Json -Compress`],
  { windowsHide: true, encoding: 'utf8' })).stdout);
const databaseState = async () => {
  const code = `import sqlite3,json,pathlib,sys\np=pathlib.Path(sys.argv[1]);c=sqlite3.connect(p.as_uri()+'?mode=ro',uri=True);r={}\nfor (n,) in c.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"):\n q='SELECT * FROM "'+n.replace('"','""')+'" ORDER BY rowid';rows=c.execute(q).fetchall();r[n]={'rows':len(rows),'values':rows}\nprint(json.dumps(r,default=lambda b:list(b)));c.close()`;
  return JSON.parse((await execute('python', ['-c', code, join(profile, 'inspection.db')], { windowsHide: true, encoding: 'utf8' })).stdout);
};
const rawState = async () => {
  const files = [];
  const records = join(profile, 'records');
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
  await walk(join(profile, 'records'));
  assert(files.some(file => /\.pgm$/i.test(file.path)), 'Existing originals required');
  return files;
};
try {
  const native = [];
  for (const page of browser.contexts().flatMap(context => context.pages())) if (await page.evaluate(() => !!window.__TAURI_INTERNALS__).catch(() => false)) native.push(page);
  assert.equal(native.length, 1);
  const read = command => native[0].evaluate(command => window.__TAURI_INTERNALS__.invoke(command), command);
  assert.equal(resolve((await read('records_list')).root).toLowerCase(), join(profile, 'records').toLowerCase());
  report.before = { process: await processState(), cycle: await read('cycle_snapshot'), database: await databaseState(), originals: await rawState() };
  assert.equal(resolve(report.before.process.path).toLowerCase(), executable.toLowerCase());
  assert.equal(new Date(report.before.process.start).getTime(), new Date(instance.processStartTime).getTime());
  assert.equal(report.before.cycle.phase, 'IDLE');
  assert.equal(report.before.cycle.fault, null);
  const duplicate = spawn(executable, [], { cwd: resolve(executable, '..'), windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
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
  report.after = { process: await processState(), cycle: await read('cycle_snapshot'), database: await databaseState(), originals: await rawState() };
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



