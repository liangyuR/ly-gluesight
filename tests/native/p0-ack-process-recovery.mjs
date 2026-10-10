import { spawn, spawnSync } from 'node:child_process';
import { readFile, writeFile, mkdir, rename } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';

const args = Object.fromEntries(process.argv.slice(2).reduce((pairs, value, i, all) => i % 2 ? pairs : [...pairs, [value.replace(/^--/, ''), all[i + 1]]], []));
for (const required of ['executable','appdata','output','template','playwright-module','fixture','instance']) if (!args[required]) throw new Error(`Missing --${required}`);
const appdata = path.resolve(args.appdata), output = path.resolve(args.output), executable = path.resolve(args.executable);
if (!appdata.startsWith('C:\\') || !output.startsWith('C:\\') || !executable.startsWith('C:\\') || !path.basename(appdata).includes('.p0-tests.recovery')) throw new Error('Only C: isolated recovery app data and executable are allowed');
const db = path.join(appdata,'inspection.db'), configPath = path.join(appdata,'plc.json'), journalPath = path.join(appdata,'plc-handshake.json');
await mkdir(output,{recursive:false});
const instance = JSON.parse(await readFile(args.instance,'utf8'));
if (path.resolve(instance.executable) !== executable || instance.identifier !== 'com.xyzrobotics.tujiaovision.p0-tests.recovery' || createHash('sha256').update(await readFile(executable)).digest('hex').toUpperCase() !== instance.sha256.toUpperCase()) throw new Error('Verified isolated executable manifest mismatch');
if (appdata !== path.resolve(process.env.APPDATA,instance.identifier)) throw new Error('AppData must match the verified recovery identifier');
try { await readFile(journalPath); throw new Error('Existing S7 transaction must be preserved; use a fresh isolated profile'); } catch(error) { if(error.code !== 'ENOENT') throw error; }
const originalConfig = await readFile(configPath), exported = JSON.parse(await readFile(args.template,'utf8'));
const { chromium } = await import(pathToFileURL(path.resolve(args['playwright-module'])).href);
const sleep = ms => new Promise(resolve => setTimeout(resolve,ms));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const python = args.python || 'python', debugPort = Number(args['debug-port'] || 9338);
try { const response=await fetch(`http://127.0.0.1:${debugPort}/json/version`); if(response.ok) throw new Error('Stop the previous owned native instance before this process-recovery test'); } catch(error) { if(error.message.includes('Stop the previous')) throw error; }
let app, browser, lock, plc, lastId = 0;
const pending = new Map(), report = {status:'failed',scope:'Actual S7 TCP -> durable ACK -> process termination before SQLite delivery update -> startup recovery',hardwareQualified:false};
function queryDb(cycleId) {
 const code = `import sqlite3,json,sys,hashlib\nc=sqlite3.connect('file:'+sys.argv[1].replace('\\\\','/')+'?mode=ro',uri=True)\nc.row_factory=sqlite3.Row\nr=c.execute('select * from parts where cycle_id=?',(sys.argv[2],)).fetchone()\nshots=c.execute('select * from part_shots where part_id=? order by k',(r['id'],)).fetchall() if r else []\npoints=c.execute('select format,data from part_points where part_id=?',(r['id'],)).fetchone() if r else None\nprint(json.dumps({'part':dict(r) if r else None,'shots':[dict(s) for s in shots],'points':{'format':points[0],'sha256':hashlib.sha256(points[1]).hexdigest(),'bytes':len(points[1])} if points else None}))`;
 const result = spawnSync(python,['-c',code,db,cycleId],{encoding:'utf8',windowsHide:true});
 if (result.status !== 0) throw new Error(result.stderr);
 return JSON.parse(result.stdout);
}
async function waitFor(fn,label,timeout=15000) {
 const end=Date.now()+timeout;
 while(Date.now()<end) {const value=await fn();if(value)return value;await sleep(50);}
 throw new Error(`Timed out waiting for ${label}`);
}
function control(command) {
 const id=++lastId;
 return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{pending.delete(id);reject(new Error('S7 fixture response timeout'));},5000);pending.set(id,{resolve:value=>{clearTimeout(timer);resolve(value);},reject});plc.stdin.write(JSON.stringify({...command,id})+'\n');});
}
async function launch() {
 const cleanPath=(process.env.PATH||'').split(';').filter(p=>!/^d:/i.test(p)).join(';');
 app=spawn(executable,[],{cwd:path.dirname(executable),env:{...process.env,PATH:cleanPath,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${debugPort}`},windowsHide:true,stdio:['ignore','pipe','pipe']});
 const startedApp=app;const stdout=[],stderr=[];app.stdout.on('data',b=>stdout.push(b));app.stderr.on('data',b=>stderr.push(b));
 app.on('exit',async()=>{await writeFile(path.join(output,`app-${startedApp.pid}-stdout.log`),Buffer.concat(stdout));await writeFile(path.join(output,`app-${startedApp.pid}-stderr.log`),Buffer.concat(stderr));});
 await waitFor(async()=>{try{return(await fetch(`http://127.0.0.1:${debugPort}/json/version`)).ok}catch{return false}},'isolated native CDP startup');
 browser=await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`);
 const page=browser.contexts()[0].pages()[0];
 await page.waitForURL('http://tauri.localhost/**');
 await page.waitForFunction(()=>!!window.__TAURI_INTERNALS__?.invoke);
 await page.getByRole('navigation',{name:'操作导航'}).waitFor();
 return page;
}
async function stopApp() {
 const owned=app;if(!owned)return;
 if(browser) {await browser.close().catch(()=>{});browser=null;}
 if(owned.exitCode===null){owned.kill();await new Promise(resolve=>owned.once('exit',resolve));}
 app=null;
}
try {
 const readyPromise=new Promise((resolve,reject)=>{
  plc=spawn(python,['-u',path.resolve(args.fixture),'--port','0'],{windowsHide:true,stdio:['pipe','pipe','pipe']});
  createInterface({input:plc.stdout}).on('line',line=>{let value;try{value=JSON.parse(line)}catch{return}if(value.event==='ready')resolve(value);else if(pending.has(value.id)){const p=pending.get(value.id);pending.delete(value.id);value.ok?p.resolve(value.result):p.reject(new Error(value.error));}});
  plc.once('error',reject);plc.stderr.on('data',b=>process.stderr.write(b));
 });
 const ready=await readyPromise;report.fixture=ready;
 await control({op:'heartbeat',enabled:true,interval_ms:100});
 const config=exported.template;
 config.connection.host='127.0.0.1';config.connection.port=ready.port;config.connection.timeoutMs=500;config.connection.pollIntervalMs=20;config.connection.reconnectIntervalMs=200;config.heartbeat.intervalMs=100;config.autoConnect=false;
 await writeFile(configPath,JSON.stringify(config,null,2));
 let page=await launch();
 const read=(command,args)=>page.evaluate(async({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
 const guard={records:await read('records_list'),config:await read('plc_get_config'),cameras:await read('camera_rig_config')};
 if(!guard.records.root.includes('.p0-tests.recovery')||guard.config.connection.host!=='127.0.0.1'||guard.config.connection.port!==ready.port||guard.config.connection.protocol!=='s7'||guard.cameras.some(c=>c.source!=='sim'))throw new Error('Isolated real-S7 guard rejected');
 await page.getByRole('navigation',{name:'操作导航'}).getByRole('link',{name:'PLC 通讯',exact:true}).click();
 await page.getByRole('button',{name:'连接',exact:true}).click();
 await page.getByRole('button',{name:'断开',exact:true}).waitFor();
 await sleep(1200);
 await control({op:'plc_values',values:{faultReset:true}});
 await waitFor(async()=>{const status=await control({op:'status'});return status.fields.visionReady&&status},'actual S7 reset and ready');
 await control({op:'plc_values',values:{faultReset:false}});
 const plan=await read('plc_recipe_plan',{recipeId:'P0-TRICAM-UI'}), seq=101, sn=70100101;
 await control({op:'plc_request',values:{protocolVersion:1,requestSeq:seq,partSn:sn,productCode:701,shotCount:plan.shotCount,planVersion:plan.planVersion,planHash:plan.planHash,camera1Shots:plan.cameraShots[0],camera2Shots:plan.cameraShots[1],camera3Shots:plan.cameraShots[2]}});
 const fields=await waitFor(async()=>{const s=await control({op:'status'});return s.fields.done&&s.fields.resultSeq===seq&&s.fields.resultSn===sn&&s.fields},'actual safe ERR result');
 if(fields.resultCode!==90||fields.armed)throw new Error(JSON.stringify(fields));
 const durable=await waitFor(async()=>{try{const j=JSON.parse(await readFile(journalPath,'utf8'));return j.pending?.cycleId&&j.pending?.result&&j}catch{return false}},'durable submitted S7 identity');
 const cycleId=durable.pending.cycleId;
 const before=await waitFor(()=>{const r=queryDb(cycleId);return r.part?.delivery_state==='submitted'&&r},'submitted SQLite history');
 report.before={journal:durable,record:before,fields};
 if(before.part.sn!==sn||before.part.plc_code!==90||before.part.verdict!=='ERR_INSPECT'||before.shots.length!==4)throw new Error('Unexpected original safe refusal history');
 const lockReady=new Promise((resolve,reject)=>{
  const code="import sqlite3,sys\nc=sqlite3.connect(sys.argv[1],timeout=5)\nc.execute('BEGIN IMMEDIATE')\nprint('LOCKED',flush=True)\nsys.stdin.readline()\nc.rollback()\nc.close()";
  lock=spawn(python,['-u','-c',code,db],{windowsHide:true,stdio:['pipe','pipe','pipe']});lock.stdout.on('data',b=>{if(b.toString().includes('LOCKED'))resolve()});lock.once('error',reject);lock.stderr.on('data',b=>reject(new Error(b.toString())));
 });
 await lockReady;
 await control({op:'plc_ack'});
 const acknowledged=await waitFor(async()=>{try{const j=JSON.parse(await readFile(journalPath,'utf8'));return j.pending?.cycleId===cycleId&&j.pending.acknowledged&&j}catch{return false}},'real ACK persisted before database writer');
 const frozen=queryDb(cycleId);
 if(frozen.part.delivery_state!=='submitted')throw new Error('SQLite ACK was already committed before termination');
 report.crashWindow={journal:acknowledged,record:frozen,appPid:app.pid,sqliteWriterLocked:true};
 await writeFile(path.join(output,'journal-at-termination.json'),JSON.stringify(acknowledged,null,2));
 await stopApp();
 lock.stdin.end('\n');await new Promise(resolve=>lock.once('exit',resolve));lock=null;
 const afterStop=queryDb(cycleId);if(afterStop.part.delivery_state!=='submitted')throw new Error('SQLite changed before restart');
 page=await launch();
 const recovered=await waitFor(()=>{const r=queryDb(cycleId);return r.part?.delivery_state==='acknowledged'&&r},'Machine.new durable ACK recovery');
 const comparable=r=>{const part={...r.part};delete part.delivery_state;delete part.delivery_updated_at;delete part.delivery_message;return{part,shots:r.shots,points:r.points}};
 if(JSON.stringify(comparable(before))!==JSON.stringify(comparable(recovered)))throw new Error('Startup recovery modified original detection evidence');
 if(!recovered.part.delivery_message?.includes('101'))throw new Error('Recovery did not identify actual request sequence');
 const history=await page.evaluate(async(id)=>window.__TAURI_INTERNALS__.invoke('history_detail',{id}),recovered.part.id);
 if(history.summary.delivery.state!=='acknowledged'||history.summary.cycleId!==cycleId||history.summary.sn!==sn)throw new Error('Native history API did not expose recovered ACK');
 report.after={record:recovered,history};report.originalDetectionEvidenceUnchanged=true;report.status='passed';report.executable={path:executable,sha256:hash(await readFile(executable))};
 report.templateAndPlan={templateSha256:hash(await readFile(args.template)),plan};
 await writeFile(path.join(output,'wire.json'),JSON.stringify(await control({op:'trace'}),null,2));
} catch(error) {report.error=String(error.stack||error);process.exitCode=1;} finally {
 await stopApp().catch(error=>{report.cleanupError=String(error);process.exitCode=1});
 if(lock){lock.stdin.end('\n');lock.kill();}
 if(plc){try{await control({op:'shutdown'})}catch{}plc.stdin.end();plc.kill();}
 await writeFile(configPath,originalConfig);
 for(const suffix of ['json','audit.jsonl']){const p=path.join(appdata,`plc-handshake.${suffix}`);try{await rename(p,path.join(output,`plc-handshake.${suffix}`))}catch(error){if(error.code!=='ENOENT')throw error}}
 await writeFile(path.join(output,'report.json'),JSON.stringify(report,null,2)+'\n');
 console.log(JSON.stringify({status:report.status,error:report.error,output,originalDetectionEvidenceUnchanged:report.originalDetectionEvidenceUnchanged}));
}
