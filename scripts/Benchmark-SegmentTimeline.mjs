import {chromium} from "playwright-core";
import {createServer} from "node:http";
import {readFile,writeFile,unlink} from "node:fs/promises";
import path from "node:path";
import {spawn} from "node:child_process";
const repo=process.cwd(),out=path.join(repo,"output/perf-thumbnails"),port=18779;
const mime={".html":"text/html",".js":"text/javascript",".css":"text/css"};
const server=createServer(async(req,res)=>{try{const name=new URL(req.url,"http://localhost").pathname;const file=path.join(out,"dist",name==="/"?"index.html":name);const bytes=await readFile(file);res.setHeader("Content-Type",mime[path.extname(file)]||"application/octet-stream");res.end(bytes);}catch{res.writeHead(404);res.end();}});
await new Promise((resolve,reject)=>{server.once("error",reject);server.listen(18778,"127.0.0.1",resolve);});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const modes=process.argv.includes("--smoke")?["after"]:["before","after","after","before","before","after"];
const all=[];
try {
for(let index=0;index<modes.length;index++){
 const mode=modes[index],name=`${index+1}-${mode}`;
 const backend=spawn(path.join(repo,"src-tauri/target/thumb-range-check/debug/examples/thumbnail_benchmark.exe"),["--root",path.join(out,"capture"),"--port",String(port)],{windowsHide:true,stdio:["ignore","pipe","pipe"]});
 let backendError;backend.on("error",error=>{backendError=error;});
 let backendLog="";backend.stdout.on("data",x=>backendLog+=x);backend.stderr.on("data",x=>backendLog+=x);
 let browserServer,browser,monitor,monitorError;
 const stop=path.join(out,`${name}.stop`);await unlink(stop).catch(()=>{});
 try{
  for(let tries=0;;tries++){
   if(backendError)throw backendError;
   if(backend.exitCode!==null)throw new Error(`Backend exited: ${backendLog}`);
   try{const response=await fetch(`http://127.0.0.1:${port}/health`,{signal:AbortSignal.timeout(1000)});const health=await response.json();if(health.pid!==backend.pid)throw new Error("Backend port belongs to another process");break;}catch(e){if(tries>100)throw e;await sleep(100);}
  }
  browserServer=await chromium.launchServer({headless:true,executablePath:process.env.CHROME_PATH||path.join(process.env.ProgramFiles,"Google/Chrome/Application/chrome.exe"),args:["--enable-precise-memory-info"],});
  browser=await chromium.connect(browserServer.wsEndpoint());
  monitor=spawn("python",[path.join(repo,"scripts/benchmark-process-memory.py"),"--pids",String(browserServer.process().pid),String(backend.pid),"--output",path.join(out,`${name}-memory.jsonl`),"--stop-file",stop,"--interval-ms","100"],{windowsHide:true,stdio:["ignore","pipe","pipe"]});
  monitor.on("error",error=>{monitorError=error;});
  monitor.on("exit",code=>{if(code!==0)monitorError=new Error(`Memory sampler exited: ${code}`);});
  monitor.stderr.on("data",x=>process.stderr.write(x));
  const page=await browser.newPage({viewport:{width:1600,height:1000},deviceScaleFactor:1});
  const errors=[];page.on("pageerror",e=>{errors.push(e.message);console.error("PAGEERROR",e.message);});
  const cdp=await page.context().newCDPSession(page);await cdp.send("Performance.enable");
  await page.goto(`http://127.0.0.1:18778/?mode=${mode}&backend=http://127.0.0.1:${port}`);
  await page.locator("#bench-idle").waitFor();
  await sleep(700);
  const metrics=async()=>{
   const perf=await cdp.send("Performance.getMetrics"),dom=await cdp.send("Memory.getDOMCounters");
   const client=await page.evaluate(()=>({canvasCount:document.querySelectorAll("canvas").length,thumbCanvasCount:document.querySelectorAll(".swb-cell canvas").length,thumbCells:document.querySelectorAll(".swb-cell").length,canvasBytes:[...document.querySelectorAll("canvas")].reduce((n,c)=>n+c.width*c.height*4,0),heap:performance.memory?{used:performance.memory.usedJSHeapSize,total:performance.memory.totalJSHeapSize}:null}));
   return {at:Date.now(),...client,dom,metrics:Object.fromEntries(perf.metrics.filter(x=>["JSHeapUsedSize","JSHeapTotalSize","TaskDuration","LayoutDuration","RecalcStyleDuration","LayoutCount","RecalcStyleCount"].includes(x.name)).map(x=>[x.name,x.value]))};
  };
  const result={name,mode,browserPid:browserServer.process().pid,rustPid:backend.pid,browserVersion:browser.version(),baseline:await metrics(),phases:[],errors};
  const phase=async(label,action,value)=>{
   const startedAt=Date.now();await writeFile(path.join(out,"active-run.json"),JSON.stringify({name,mode,browserPid:result.browserPid,rustPid:backend.pid,phase:label,startedAt}));
   const timing=await page.evaluate(async({action,value})=>{
    const start=performance.now(),logs=window.benchRequests.length,tasks=window.benchLongTasks.length;
    let uiMs=null,paintMs=null,thumbsMs=null;
    if(action==="switch")window.benchSwitch(value);
    else{const el=document.querySelector(".swb-scroll");el.scrollLeft=(el.scrollWidth-el.clientWidth)*value;}
    let frames=0,readyFrames=0;
    await new Promise((resolve,reject)=>{
     const tick=()=>{
      frames++;const now=performance.now();
      if(now-start>120000){reject(new Error("phase timeout"));return;}
      const el=document.querySelector(".swb-scroll"),body=document.querySelector(".swb-body");
      if(body&&uiMs===null&&(action!=="switch"||document.querySelector("[data-bench-piece]")?.getAttribute("data-bench-piece")===String(value)))uiMs=now-start;
      if(frames===2)paintMs=now-start;
      if(el&&(action!=="switch"||document.querySelector("[data-bench-piece]")?.getAttribute("data-bench-piece")===String(value))){const rect=el.getBoundingClientRect();const cells=[...el.querySelectorAll(".swb-cell")].filter(c=>{const r=c.getBoundingClientRect();return r.right>rect.left&&r.left<rect.right;});
       const ready=cells.length>0&&cells.every(c=>c.querySelector("canvas"));
       readyFrames=ready?readyFrames+1:0;
       if(readyFrames>=2){thumbsMs=now-start;resolve();return;}}
      requestAnimationFrame(tick);
     };requestAnimationFrame(tick);
    });
    return {uiMs,paintMs,thumbsMs,requestOffset:logs,longTaskOffset:tasks,startPerf:start,endPerf:performance.now()};
   },{action,value});
   const readyMetrics=await metrics();
   await page.waitForFunction(()=>window.benchRequests.every(r=>r.end!==undefined),null,{timeout:120000});
   await sleep(400);
   const settledMetrics=await metrics();
   const details=await page.evaluate(({requestOffset,longTaskOffset})=>({requests:window.benchRequests.slice(requestOffset),longTasks:window.benchLongTasks.slice(longTaskOffset)}),timing);
   result.phases.push({label,startedAt,endedAt:Date.now(),...timing,readyMetrics,settledMetrics,...details});
   console.log(JSON.stringify({name,label,uiMs:timing.uiMs,thumbsMs:timing.thumbsMs,reads:details.requests.reduce((n,r)=>n+(r.imageReads||0),0),cells:settledMetrics.thumbCells}));
   await writeFile(path.join(out,`${name}.json`),JSON.stringify(result,null,2));
  };
  await phase("first-open","switch",0);
  for(const fraction of [0.1,0.25,0.5,0.75,1,0.5,0])await phase(`scroll-${fraction}`,"scroll",fraction);
  await phase("switch-A-to-B","switch",1);
  await phase("switch-B-to-A","switch",0);
  for(let n=0;n<4;n++)await phase(`repeat-switch-${n+1}`,"switch",(n+1)%2);
  await page.screenshot({path:path.join(out,`${name}.png`)});
  if(monitorError)throw monitorError;
  if(errors.length)throw new Error(`Page errors: ${errors.join("; ")}`);
  result.final=await metrics();result.endedAt=Date.now();
  await writeFile(path.join(out,`${name}.json`),JSON.stringify(result,null,2));all.push(result);
 }finally{
  await writeFile(stop,"stop");
  if(monitor)await Promise.race([new Promise(r=>monitor.once("exit",r)),sleep(2000)]);
  if(monitor&&monitor.exitCode===null)monitor.kill();
  await browser?.close().catch(()=>{});await browserServer?.close().catch(()=>{});backend.kill();
  await writeFile(path.join(out,`${name}-backend.log`),backendLog);
 }
 await sleep(500);
}
await writeFile(path.join(out,process.argv.includes("--smoke")?"smoke-results.json":"browser-results.json"),JSON.stringify(all,null,2));
} finally { server.close(); }
