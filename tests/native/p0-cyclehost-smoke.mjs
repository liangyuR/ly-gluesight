import assert from 'node:assert/strict';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { configureReplay } from './p0-clean-cyclehost-setup.mjs';
import { persistFailedAttempt, recordedArtifact, sampleProcess, scanReplayInputs, validateCameraSource, validatePart, validateReplayOutputs } from './p0-cyclehost-performance.mjs';

process.env.PATH = ['C:\\Users\\11601\\AppData\\Local\\Temp\\gluesight-p0-recovery-20261010\\native', 'C:\\vcpkg\\installed\\x64-windows\\bin', ...(process.env.PATH ?? '').split(';').filter(value => value && !/^D:/i.test(value))].join(';');
const cPath = value => { const path = resolve(value); assert(/^C:\\/i.test(path), `C: required: ${path}`); return path; };
const { values } = parseArgs({ options: { instance: {type:'string'}, baseline: {type:'string'}, output: {type:'string'}, 'playwright-module': {type:'string'}, cdp: {type:'string',default:'http://127.0.0.1:9338'} } });
for (const key of ['instance','baseline','output','playwright-module']) assert(values[key], `Missing --${key}`);
const artifact = async path => { path=cPath(path);const bytes=await readFile(path);return {path,bytes:bytes.length}; };
const json = async path => JSON.parse((await readFile(cPath(path),'utf8')).replace(/^\uFEFF/,''));
const tree = async directory => {
 const files=[];
 for(const entry of (await readdir(cPath(directory),{withFileTypes:true})).sort((a,b)=>a.name.localeCompare(b.name))) {
  assert(!entry.isSymbolicLink(),'Linked frozen resource');
  const path=join(directory,entry.name);
  if(entry.isDirectory())files.push(...await tree(path));else if(entry.isFile())files.push(await artifact(path));
 }
 return files;
};
const instance=await json(values.instance),baseline=await json(values.baseline),output=cPath(values.output);
assert.equal(instance.identifier,'com.xyzrobotics.tujiaovision.p0-tests.performance');
assert(Number.isSafeInteger(instance.pid)&&instance.pid>0&&instance.processStartTime&&instance.runtimeBase);
assert(baseline.passed&&baseline.runs.length===4&&baseline.runs.every(run=>run.passed&&run.completedParts===100&&run.independentValidatorPassed));
await mkdir(output,{recursive:false});
const report={startedAt:new Date().toISOString(),passed:false,scope:'Four actual native UI cycles after audit retry fix; one per combination; not a hundred-part performance benchmark',benchmark:false,partsPerCombination:1,instance,baseline:await artifact(values.baseline),software:await Promise.all(['p0-cyclehost-smoke.mjs','p0-cyclehost-performance.mjs','p0-clean-cyclehost-setup.mjs','../../scripts/robot-plc-demo/camera-bridge.mjs'].map(name=>artifact(join(dirname(fileURLToPath(import.meta.url)),name)))),runs:[],physicalValidation:false,s7HardwareValidation:false};
const {chromium}=await import(pathToFileURL(cPath(values['playwright-module'])).href);
const browser=await chromium.connectOverCDP(values.cdp);
let attempt={stage:'guard',accepted:false,lastPoll:null},recordsRoot;
try {
 const pages=[];
 for(const page of browser.contexts().flatMap(context=>context.pages()))if(await page.evaluate(()=>!!window.__TAURI_INTERNALS__).catch(()=>false))pages.push(page);
 assert.equal(pages.length,1);
 const page=pages[0],read=(command,args)=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
 recordsRoot=(await read('records_list')).root;
 assert.equal(cPath(recordsRoot).toLowerCase(),'c:\\users\\11601\\appdata\\roaming\\com.xyzrobotics.tujiaovision.p0-tests.performance\\records');
 const settings=await read('cycle_get_settings'),engine=await read('engine_status');
 assert(settings.vision&&settings.record==='all'&&settings.recordKeep===1000&&settings.recordMaxGb===20&&settings.timeouts.armMs===200);
 assert(engine.backend==='LyFlow'&&engine.ready&&engine.measuring&&engine.version);
 const plc=await read('plc_get_config');assert.equal(plc.connection.protocol,'simulator');assert.equal((await read('plc_get_status')).state,'connected');
 report.guard={recordsRoot,settings,engine,dll:await artifact(engine.path),process:await sampleProcess(instance.pid,instance.executable,instance.processStartTime)};
 for(const views of [3,1])for(const scenario of ['normal','gap']) {
  const mode=views===3?'tricam':'single',previousRun=baseline.runs.find(run=>run.mode===mode&&run.scenario===scenario);
  assert(previousRun);
  const reference=await json(join(cPath(previousRun.output),'cyclehost-report.json'));
  assert(reference.completed&&reference.passed&&reference.completedParts===100);
  const directory=cPath(reference.replayInputs.directory),fixture=cPath(reference.provenance.fixture.path),release=dirname(fixture);
  const inputs=await scanReplayInputs(directory,mode);assert.deepEqual(inputs,reference.replayInputs);
  const frozenBefore=await tree(release);assert.deepEqual(frozenBefore,reference.provenance.release);
  await configureReplay(page,{views,directory});
  const cameras=await read('camera_rig_config');validateCameraSource(cameras.find(camera=>camera.id==='cam1'),'replay',mode,directory);
  const fixtureDoc=await json(fixture),layout=await read('cycle_layout',{recipeId:fixtureDoc.id});
  assert.equal(layout.revisionId,reference.provenance.layout.revisionId);
  assert.deepEqual(layout.shots.map(shot=>[shot.id,shot.camera,shot.view]),reference.provenance.layout.shots.map(shot=>[shot.id,shot.camera,shot.view]));
  assert.equal((await json(join(release,'manifest.json'))).bundleId,reference.provenance.bundleId);
  await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'在线检测',exact:true}).click();
  await page.getByRole('combobox',{name:'模拟配方',exact:true}).selectOption(fixtureDoc.id);
  await page.getByRole('combobox',{name:'模拟工况',exact:true}).selectOption(scenario);
  assert.equal((await read('cycle_snapshot')).phase,'IDLE');assert(!(await read('sim_status')).running);
  const previous=(await read('history_query',{query:{recipeId:fixtureDoc.id,limit:1}})).items[0]?.id??-1,began=Date.now();
  attempt={mode,scenario,stage:'clickRun',accepted:false,startedAt:began,lastPoll:null};
  await page.getByRole('button',{name:'运行一件',exact:true}).click();
  let row;
  for(const deadline=Date.now()+30000;Date.now()<deadline;) {
   const snapshot=await read('cycle_snapshot'),simulator=await read('sim_status'),summary=(await read('history_query',{query:{recipeId:fixtureDoc.id,limit:1}})).items[0];
   attempt.lastPoll={snapshot,simulator,summary};
   if(summary?.id>previous) {
    const detail=await read('history_detail',{id:summary.id}),originals=await read('workspace_record_images',{historyId:summary.id}),measurements=await read('cycle_part_data');
    Object.assign(attempt.lastPoll,{detail,originals,measurements});
    if(detail.summary.delivery.state==='acknowledged'&&detail.recording.state==='complete'&&originals.complete&&!simulator.running&&snapshot.phase==='IDLE') {row={part:1,scenario,uiObservedCycleMs:Date.now()-began,detail,originals,measurements};break;}
   }
   await page.waitForTimeout(50);
  }
  assert(row,'No settled acknowledged and complete smoke cycle');attempt.row=row;attempt.stage='validate';
  row.armingLog=(await read('cycle_logs')).filter(line=>line.ts>=began).find(line=>line.ev==='armed↑ busy↑');
  row.armMs=Number(row.armingLog?.msg.match(/布防耗时 (\d+) ms/)?.[1]);
  assert.equal(row.detail.summary.bundleId,reference.provenance.bundleId);
  assert.deepEqual(validatePart(row,layout,mode,scenario),[]);
  row.recordedArtifacts=[];
  for(const frame of row.originals.frames)row.recordedArtifacts.push({k:frame.k,view:frame.view,...await recordedArtifact(recordsRoot,frame.file)});
  const directories=new Set(row.originals.frames.map(frame=>dirname(frame.file)));assert.equal(directories.size,1);
  const metadata=await artifact(join(recordsRoot,[...directories][0],'part.json'));
  row.recordingMetadata={...metadata,document:await json(metadata.path)};
  row.replayComparisons=validateReplayOutputs(row,inputs,layout);
  assert.deepEqual(await scanReplayInputs(directory,mode),inputs);assert.deepEqual(await tree(release),frozenBefore);
  await sampleProcess(instance.pid,instance.executable,instance.processStartTime);
  report.runs.push({mode,scenario,passed:true,reference:await artifact(join(previousRun.output,'cyclehost-report.json')),row});attempt.accepted=true;
  await writeFile(join(output,'report.json'),JSON.stringify(report,null,2)+'\n');
  console.log(JSON.stringify({mode,scenario,passed:true,cycleId:row.detail.summary.cycleId,armMs:row.armMs,rawPGMs:row.recordedArtifacts.length}));
 }
 report.passed=report.runs.length===4;
 report.executableAfter=await artifact(instance.executable);assert.equal(report.executableAfter.sha256,instance.sha256.toLowerCase());
 report.dllAfter=await artifact(engine.path);assert.equal(report.dllAfter.sha256,report.guard.dll.sha256);
} catch(error) {
 report.passed=false;report.error=error.stack??String(error);report.failedAttempt=await persistFailedAttempt(output,attempt,error,report.runs.length);process.exitCode=1;
} finally {
 report.finishedAt=new Date().toISOString();await writeFile(join(output,'report.json'),JSON.stringify(report,null,2)+'\n');await browser.close();
}

