import {decodeThumbStrip} from "../../../src/features/workspace/workbenchDraft";
import {loadPreview} from "../../../src/features/workspace/previewCache";
const query=new URLSearchParams(location.search), base=query.get("backend")||"http://127.0.0.1:18777", mode=query.get("mode")||"after";
(window as any).benchRequests=[];
async function read(path:string, args:Record<string,string|number>) {
 const log:any={path,args,startedAt:Date.now(),start:performance.now()};(window as any).benchRequests.push(log);
 try {const response=await fetch(base+path+"?"+new URLSearchParams(Object.entries(args).map(([k,v])=>[k,String(v)])));
 if(!response.ok) throw new Error(await response.text());
 log.backendMs=Number(response.headers.get("X-Elapsed-Ms"));log.imageReads=Number(response.headers.get("X-Image-Reads"));
 const buffer=await response.arrayBuffer();log.bytes=buffer.byteLength;return buffer;
 }finally {log.end=performance.now();log.endedAt=Date.now();}
}
let frameId=0;
export const workspaceApi={
 captureRoundThumbs:(round:string,camera:string,startSeq=1,count=32)=>read("/thumbs",{mode,round,camera,start:startSeq,count}).then(decodeThumbStrip),
 captureFrameImage:(round:string,camera:string,seq:number,signal?:AbortSignal)=>loadPreview(JSON.stringify(["bench",round,camera,seq]),async()=>{
 const buffer=await read("/frame",{round,camera,seq});const header=new DataView(buffer);
 const width=header.getUint32(0,true),height=header.getUint32(4,true),channels=header.getUint32(8,true) as 1|3;
 return {url:`bench:${++frameId}`,width,height,channels,pixels:{data:new Uint8Array(buffer,12),width,height,channels}};
 },signal),
 segmentTemplatePreview:async()=>{throw new Error("benchmark is read only");},
 segmentTemplateSave:async()=>{throw new Error("benchmark is read only");}
};
