import assert from 'node:assert/strict';
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';
import { configureReplay, teachCleanFixture } from './p0-clean-cyclehost-setup.mjs';
import { sampleProcess, recordedArtifact, scanReplayInputs, validatePart, validateReplayOutputs, persistFailedAttempt } from './p0-cyclehost-performance.mjs';

const identifier = 'com.xyzrobotics.tujiaovision.p0-tests.performance.nohash';
const profile = join('C:\\Users\\11601\\AppData\\Roaming', identifier);
const recordsRoot = join(profile, 'records');
const id = 'P0-NO-HASH-3V';
const cPath = value => { const path = resolve(value); assert(/^C:\\/i.test(path)); return path; };
const json = async path => JSON.parse((await readFile(cPath(path), 'utf8')).replace(/^\uFEFF/, ''));
const { values } = parseArgs({ options: { instance:{type:'string'}, inputs:{type:'string'}, output:{type:'string'}, stage:{type:'string'}, 'playwright-module':{type:'string'}, cdp:{type:'string',default:'http://127.0.0.1:9340'} } });
for (const key of ['instance','inputs','output','stage','playwright-module']) assert(values[key], 'Missing --' + key);
assert(['setup','cycles','history','restart'].includes(values.stage));
const instance = await json(values.instance), inputs = cPath(values.inputs), output = cPath(values.output);
assert.equal(instance.identifier, identifier);
assert(instance.processStartTime && instance.runtimeBase && instance.executable);
await mkdir(output, { recursive:true });
const destination = join(output, values.stage + '-report.json');
try { await readFile(destination); throw new Error('Do not overwrite existing evidence: ' + destination); } catch(error) { if(error.code !== 'ENOENT') throw error; }
const report = { startedAt:new Date().toISOString(), stage:values.stage, instance, passed:false, physicalValidation:false, s7HardwareValidation:false, benchmark:false, scope:'Fresh native acceptance using explicit IDs, actual resource structure and business results; no content fingerprint matching' };
const { chromium } = await import(pathToFileURL(cPath(values['playwright-module'])).href);
const browser = await chromium.connectOverCDP(values.cdp);
let attempt = {stage:'guard',accepted:false};
try {
  const pages = [];
  for(const page of browser.contexts().flatMap(context=>context.pages())) if(await page.evaluate(()=>!!window.__TAURI_INTERNALS__).catch(()=>false)) pages.push(page);
  assert.equal(pages.length,1);
  const page=pages[0], read=(command,args)=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const records=await read('records_list'),settings=await read('cycle_get_settings'),engine=await read('engine_status');
  let cycle=await read('cycle_snapshot');
  assert.equal(cPath(records.root).toLowerCase(),recordsRoot.toLowerCase());
  const guardedProcess=await sampleProcess(instance.pid,cPath(instance.executable),instance.processStartTime);
  assert.equal((await read('plc_get_config')).connection.protocol,'simulator');
  if(values.stage==='setup') {
    assert.equal((await read('history_query',{query:{limit:1}})).items.length,0,'Fresh profile must have zero history');
    assert(!cycle.part && !(await read('sim_status')).running);
    if((await read('plc_get_status')).state==='disconnected') {
      await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'PLC 通讯',exact:true}).click();
      await page.getByRole('button',{name:'连接',exact:true}).click();
      await page.waitForFunction(async()=> (await window.__TAURI_INTERNALS__.invoke('plc_get_status')).state==='connected');
      await page.waitForTimeout(200);cycle=await read('cycle_snapshot');
    }
    assert(cycle.phase==='IDLE' || cycle.phase==='FAULT' && !cycle.part && cycle.fault?.includes('没有一个配方开得了工') && cycle.fault.includes('没有不可变发布包'));
    report.freshSite={historyCount:0,cycle,simulator:await read('sim_status')};
  }
  else if(values.stage==='restart') {
    assert(!cycle.part && !(await read('sim_status')).running);
    report.beforeReconnect={cycle,plc:await read('plc_get_status')};
    if(report.beforeReconnect.plc.state==='disconnected') {
      assert(cycle.phase==='FAULT' && cycle.fault==='PLC 未连接');
      await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'PLC 通讯',exact:true}).click();
      await page.getByRole('button',{name:'连接',exact:true}).click();
      await page.waitForFunction(async()=> (await window.__TAURI_INTERNALS__.invoke('plc_get_status')).state==='connected');
    }
    await page.waitForFunction(async id=> !(await window.__TAURI_INTERNALS__.invoke('cycle_snapshot')).alarms.some(alarm=>alarm.includes(id)),id);
    await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'在线检测',exact:true}).click();
    if((await read('cycle_snapshot')).phase==='FAULT') await page.getByRole('button',{name:'复位故障',exact:true}).click();
    await page.waitForFunction(async()=> {const cycle=await window.__TAURI_INTERNALS__.invoke('cycle_snapshot');return cycle.phase==='IDLE' && cycle.fault===null;});
    cycle=await read('cycle_snapshot');
  }
  else { assert.equal(cycle.phase,'IDLE');assert.equal(cycle.fault,null); }
  assert(!(await read('sim_status')).running);
  assert(settings.vision && settings.record==='all' && settings.timeouts.armMs===200 && settings.recordMaxGb===20);
  assert(engine.backend==='LyFlow' && engine.ready && engine.measuring);
  assert.equal((await read('plc_get_config')).connection.protocol,'simulator');
  assert.equal((await read('plc_get_status')).state,'connected');
  report.guard={recordsRoot,settings,engine,process:guardedProcess};
  if(values.stage==='setup') {
    report.setup=await teachCleanFixture(page,{views:3,inputs,output:join(output,'teaching'),recordsRoot,id,allowUnpublished:true});
    assert.equal(report.setup.layout.revisionId,`${id}-v${report.setup.layout.version}`);
    await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'在线检测',exact:true}).click();
    if((await read('cycle_snapshot')).phase==='FAULT') await page.getByRole('button',{name:'复位故障',exact:true}).click();
    await page.waitForFunction(async()=> {const cycle=await window.__TAURI_INTERNALS__.invoke('cycle_snapshot');return cycle.phase==='IDLE' && cycle.fault===null;});
    report.readyAfterPublication=await read('cycle_snapshot');
  } else {
    const workspace=await read('workspace_get',{id}),layout=await read('cycle_layout',{recipeId:id});
    assert(!workspace.workspace.pending && workspace.productionVersion===layout.version);
    assert.equal(layout.revisionId,`${id}-v${layout.version}`);
    report.layout=layout;
    if(values.stage==='cycles') {
      report.runs=[];
      for(const scenario of ['normal','gap']) {
        const directory=join(inputs,'3-view',scenario),inputStructure=await scanReplayInputs(directory,'tricam');
        await configureReplay(page,{views:3,directory,recordsRoot});
        await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'在线检测',exact:true}).click();
        await page.getByRole('combobox',{name:'模拟配方',exact:true}).selectOption(id);
        await page.getByRole('combobox',{name:'模拟工况',exact:true}).selectOption(scenario);
        const previous=(await read('history_query',{query:{recipeId:id,limit:1}})).items[0]?.id??-1,began=Date.now();
        attempt={stage:'cycle',scenario,accepted:false,lastPoll:null};
        await page.getByRole('button',{name:'运行一件',exact:true}).click();
        let row;
        for(const deadline=Date.now()+30000;Date.now()<deadline;) {
          const snapshot=await read('cycle_snapshot'),simulator=await read('sim_status'),summary=(await read('history_query',{query:{recipeId:id,limit:1}})).items[0];
          attempt.lastPoll={snapshot,simulator,summary};
          if(summary?.id>previous) {
            const detail=await read('history_detail',{id:summary.id}),originals=await read('workspace_record_images',{historyId:summary.id}),measurements=await read('cycle_part_data');
            Object.assign(attempt.lastPoll,{detail,originals,measurements});
            if(detail.summary.delivery.state==='acknowledged' && detail.recording.state==='complete' && originals.complete && !simulator.running && snapshot.phase==='IDLE') { row={part:1,scenario,detail,originals,measurements,uiObservedCycleMs:Date.now()-began};break; }
          }
          await page.waitForTimeout(50);
        }
        assert(row,'Cycle did not settle complete and acknowledged');attempt.row=row;
        row.armingLog=(await read('cycle_logs')).filter(line=>line.ts>=began).find(line=>line.ev==='armed↑ busy↑');
        row.armMs=Number(row.armingLog?.msg.match(/布防耗时 (\d+) ms/)?.[1]);
        assert.deepEqual(validatePart(row,layout,'tricam',scenario),[]);
        assert(!Object.hasOwn(row.detail.summary,'recipeHash') && !Object.hasOwn(row.detail.summary,'bundleHash'));
        row.recordedArtifacts=[];
        for(const frame of row.originals.frames)row.recordedArtifacts.push({k:frame.k,view:frame.view,...await recordedArtifact(recordsRoot,frame.file)});
        const directories=new Set(row.originals.frames.map(frame=>dirname(frame.file)));assert.equal(directories.size,1);
        row.recordingMetadata={document:await json(join(recordsRoot,[...directories][0],'part.json'))};
        row.frameIdentities=validateReplayOutputs(row,inputStructure,layout);
        assert(row.recordingMetadata.document.frames.every(frame=>!Object.hasOwn(frame,'hash')));
        const bundleDirectory=join(profile,'vision','releases',id,row.detail.summary.bundleId),manifest=await json(join(bundleDirectory,'manifest.json'));
        assert.equal(manifest.bundleId,row.detail.summary.bundleId);assert.equal(manifest.recipeRevision,layout.revisionId);
        assert(manifest.files.every(file=>!Object.hasOwn(file,'hash')));
        report.runs.push({scenario,passed:true,row,manifest});attempt.accepted=true;
        console.log(JSON.stringify({scenario,cycleId:row.detail.summary.cycleId,verdict:row.detail.summary.verdict,armMs:row.armMs,rawPGMs:row.recordedArtifacts.length}));
      }
    } else if(values.stage==='history') {
      const cycles=await json(join(output,'cycles-report.json'));assert(cycles.passed && cycles.runs.length===2);
      report.results=[];
      report.candidateBeforeEnvironmentRestore=await read('workspace_get',{id});
      await configureReplay(page,{views:3,directory:join(inputs,'3-view','normal'),recordsRoot});
      await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'单帧示教',exact:true}).click();
      report.candidateTrialsAfterEnvironmentRestore=[];
      for(let k=0;k<4;k++) {
        await page.getByRole('button',{name:`选择帧 k${k+1}`,exact:true}).click();
        if(!(await read('workspace_get',{id})).workspace.frames[k].saved) {
          await page.getByRole('button',{name:'试测当前帧',exact:true}).click();
          await page.getByRole('button',{name:'保存本帧示教',exact:true}).click();
          await page.waitForFunction(async({id,k})=>(await window.__TAURI_INTERNALS__.invoke('workspace_get',{id})).workspace.frames[k].saved,{id,k});
        }
        const frame=(await read('workspace_get',{id})).workspace.frames[k];
        assert(frame.saved && frame.trial?.passed);
        report.candidateTrialsAfterEnvironmentRestore.push({k,imageId:frame.image.id,trial:frame.trial});
      }
      const candidateBefore=await read('workspace_get',{id});
      for(const run of cycles.runs) {
        const row=run.row.detail.summary,before=await read('history_detail',{id:row.id});
        await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'历史记录',exact:true}).click();
        await page.getByRole('button',{name:'全部',exact:true}).click();
        await page.getByRole('combobox',{name:'历史配方',exact:true}).selectOption(id);
        await page.getByRole('row',{name:`查看 SN ${row.sn} 的记录`,exact:true}).click();
        await page.getByRole('heading',{name:'逐拍照点追溯',exact:true}).waitFor();
        const comparisons=[];
        for(const [button,source] of [['按原发布包重现','original'],['从原图复测整件','raw']]) {
          if(source==='raw')await page.getByRole('button',{name:'使用该配方候选',exact:true}).click();
          const old=await read('workspace_comparisons',{id,historyId:row.id});
          await page.getByRole('button',{name:button,exact:true}).click();
          let result;
          for(const deadline=Date.now()+30000;Date.now()<deadline;) {
            const current=await read('workspace_comparisons',{id,historyId:row.id});
            result=current.find(item=>item.source===source && !old.some(prior=>prior.id===item.id));
            if(result)break;
            await page.waitForTimeout(100);
          }
          assert(result && result.cycleId===row.cycleId && result.bundleId===row.bundleId && result.judgement.verdict===row.verdict);
          assert(result.measurements.length===4 && result.measurements.every(m=>m.located && !m.error && m.idx.length>=70));
          comparisons.push(result);
        }
        const viewed=[];
        for(let k=0;k<4;k++)for(const view of [1,2,3]) {
          await page.getByRole('button',{name:`查看 k${k+1} 视角 ${view}`,exact:true}).click();
          const image=page.getByRole('img',{name:`原始 SN ${row.sn} · k${k+1} · 视角 ${view}`,exact:true});
          await image.waitFor();assert.equal(await image.getAttribute('viewBox'),'0 0 1280 1024');
          viewed.push({k,view});
        }
        await page.screenshot({path:join(output,`history-${run.scenario}.png`),fullPage:true});
        assert.deepEqual(await read('history_detail',{id:row.id}),before);
        report.results.push({scenario:run.scenario,historyId:row.id,cycleId:row.cycleId,bundleId:row.bundleId,viewed,comparisons,originalRecordUnchanged:true});
      }
      assert.deepEqual((await read('workspace_get',{id})).workspace,candidateBefore.workspace);
    } else {
      const cycles=await json(join(output,'cycles-report.json'));assert(cycles.passed);
      report.restored=[];
      for(const run of cycles.runs) {
        const detail=await read('history_detail',{id:run.row.detail.summary.id}),originals=await read('workspace_record_images',{historyId:run.row.detail.summary.id});
        assert.deepEqual(detail,run.row.detail);assert(originals.complete && originals.frames.length===12);
        report.restored.push({cycleId:detail.summary.cycleId,bundleId:detail.summary.bundleId,recipeRevision:detail.summary.recipeRevision,delivery:detail.summary.delivery.state,originals:originals.frames.length});
      }
    }
  }
  await sampleProcess(instance.pid,cPath(instance.executable),instance.processStartTime);
  report.passed=true;
} catch(error) {
  report.error=error.stack??String(error);report.failedAttempt=await persistFailedAttempt(output,{...attempt,stage:values.stage},error,report.runs?.length??0);process.exitCode=1;
} finally {
  report.finishedAt=new Date().toISOString();await writeFile(destination,JSON.stringify(report,null,2)+'\n',{flag:'wx'});await browser.close();
}
console.log(JSON.stringify({stage:values.stage,passed:report.passed,error:report.error}));
