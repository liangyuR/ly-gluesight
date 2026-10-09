import { useEffect, useRef, useState } from "react";
import { Play, Square } from "lucide-react";
import { cameraApi } from "../api";
import type { DryFrame } from "../types";

const H = 130;

export default function DryRunPanel({ cam, frameMs }: { cam: number; frameMs: number | null }) {
  const [running, setRunning] = useState(false);
  const [all, setAll] = useState<DryFrame[]>([]);
  const frames = all.filter((f) => f.cam === cam);
  const [plan, setPlan] = useState(6);
  const [error, setError] = useState("");
  const [busy,setBusy]=useState(false);
  const [finished,setFinished]=useState(false);
  const pending=useRef(false),serial=useRef(0),mounted=useRef(true);
  const session=useRef({owned:false,stopping:false});
  const release=async()=>{
    if(!session.current.owned||session.current.stopping)return;
    session.current.stopping=true;
    try{await cameraApi.dryRunStop();session.current.owned=false;}
    catch{ /* 页面关闭后的清理没有可用的结果区域。 */ }
    finally{session.current.stopping=false;}
  };
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;serial.current++;void release();};},[]);
  useEffect(()=>{
    serial.current++;setAll([]);setRunning(false);setFinished(false);setError("");
    setBusy(pending.current);
    if(session.current.owned&&!session.current.stopping){setBusy(true);void release().finally(()=>{if(mounted.current)setBusy(false);});}
  },[cam]);

  useEffect(() => {
    if (!running||busy) return;
    let alive=true,polling=false;const current=serial.current;
    const timer = setInterval(() => {
      if(polling)return;polling=true;
      cameraApi.dryRunGet().then(f=>{
        if(!alive||current!==serial.current||!mounted.current)return;
        if(f===null){session.current.owned=false;setRunning(false);setFinished(false);setError("空跑已结束，未返回最终帧数据。请重新开始。");}
        else{setAll(f);setError("");}
      }).catch(e=>{if(alive&&current===serial.current&&mounted.current)setError(String(e));}).finally(()=>{polling=false;});
    },300);
    return () => {alive=false;clearInterval(timer);};
  }, [running,busy]);

  const start = async () => {
    if(pending.current||busy||running||!Number.isSafeInteger(plan)||plan<1)return;
    const current=++serial.current;
    pending.current=true;setBusy(true);
    setError("");
    try {
      await cameraApi.dryRunStart();
      session.current.owned=true;
      if(!mounted.current||current!==serial.current){await release();return;}
      setAll([]);
      setFinished(false);
      setRunning(true);
    } catch (e) {
      if(mounted.current&&current===serial.current)setError(String(e));
    } finally {pending.current=false;if(mounted.current)setBusy(false);}
  };
  const stop = async () => {
    if(pending.current||!running)return;
    const current=++serial.current;pending.current=true;setBusy(true);session.current.stopping=true;setError("");
    try{const result=await cameraApi.dryRunStop();session.current.owned=false;if(mounted.current&&current===serial.current){setAll(result);setRunning(false);setFinished(true);}}
    catch(e){if(mounted.current&&current===serial.current)setError(String(e));}
    finally{pending.current=false;session.current.stopping=false;if(mounted.current)setBusy(false);else void release();}
  };

  const intervals = frames.slice(1).map((f, i) => f.tMs - frames[i].tMs);
  const triggers = frames.length ? frames[frames.length - 1].triggerCounter - frames[0].triggerCounter + 1 : 0;
  const jumps = frames.slice(1).filter((f, i) => f.frameCounter !== frames[i].frameCounter + 1).length;
  const triggerJumps=frames.slice(1).filter((f,i)=>f.triggerCounter!==frames[i].triggerCounter+1).length;
  const lost = frames.reduce((a, f) => a + f.lostPackets, 0);
  const minGap = intervals.length ? intervals.reduce((min,ms)=>Math.min(min,ms),Infinity) : null;
  const measuredIntervals=intervals.filter(Number.isFinite);
  const duration=frameMs!=null&&Number.isFinite(frameMs)&&frameMs>0?frameMs:null;
  const top = measuredIntervals.reduce((max,ms)=>Math.max(max,ms),Math.max(700,duration??0)) * 1.1;
  const Y = (ms: number) => H - 16 - (ms / top) * (H - 24);
  const chartIntervals=intervals.slice(-64);
  const barW = chartIntervals.length ? Math.min(46, 460 / chartIntervals.length - 6) : 0;
  const ok = finished && frames.length > 0 && frames.length === plan && triggers === plan && jumps === 0 && triggerJumps===0 && lost === 0 && intervals.every(ms=>Number.isFinite(ms)&&ms>0) && (minGap===null||duration===null||minGap>=duration);

  return (
    <div className="panel">
      <div className="panel-head">
        <h3 className="panel-title">触发空跑测试</h3>
        <span className="muted">机器人不带工件走一遍路径，只计数不检测</span>
        <span className="spacer" />
        {running ? (
          <button className="btn" onClick={stop} disabled={busy}>
            <Square size={15} />
            结束
          </button>
        ) : (
          <button className="btn primary" onClick={start} disabled={busy||!Number.isSafeInteger(plan)||plan<1}>
            <Play size={15} />
            开始空跑
          </button>
        )}
      </div>
      <div className="dry-stats">
        <label>
          计划 N <input id="dry-plan" className="input mono" type="number" min={1} step={1} disabled={running||busy} value={Number.isFinite(plan)?plan:""} onChange={(e) => {setPlan(e.target.value===""?NaN:Number(e.target.value));setFinished(false);setAll([]);}} />
        </label>
        <span>收到帧 <b className={frames.length === plan ? "c-ok" : ""}>{frames.length}</b></span>
        <span>触发计数 <b className={triggers === plan ? "c-ok" : ""}>{triggers}</b></span>
        <span>帧计数跳号 <b className={jumps ? "c-ng" : ""}>{jumps}</b></span>
        <span>触发计数跳号 <b className={triggerJumps ? "c-ng" : ""}>{triggerJumps}</b></span>
        <span>丢包 <b className={lost ? "c-warn" : ""}>{lost}</b></span>
        <span>最短间隔 <b>{minGap !== null ? `${minGap.toFixed(0)} ms` : "—"}</b></span>
        {!running && finished && <span className={ok ? "c-ok" : "c-ng"}>{ok ? "通过" : "不通过"}</span>}
      </div>
      {duration===null&&<p className="muted">单帧时间未知，结果核对数量、计数与丢包；采集容量需另行确认。</p>}
      {intervals.length>64&&<p className="muted">图表显示最近 64 个间隔，计数和结论使用全部帧。</p>}
      <svg className="dry-chart" viewBox={`0 0 500 ${H}`} preserveAspectRatio="none">
        {chartIntervals.map((raw, i) => {
          const ms=Number.isFinite(raw)?Math.max(0,raw):0;
          const x = 30 + i * (barW + 6);
          return (
            <g key={i}>
              <rect x={x} y={Y(ms)} width={barW} height={H - 16 - Y(ms)} fill={duration && ms < duration ? "var(--ng)" : "var(--accent)"} />
              {barW > 22 && (
                <text x={x + barW / 2} y={Y(ms) - 3} textAnchor="middle" fontSize={9} fill="var(--text)">
                  {ms.toFixed(0)}
                </text>
              )}
            </g>
          );
        })}
        {duration && (
          <>
            <line x1={24} x2={496} y1={Y(duration)} y2={Y(duration)} stroke="var(--warn)" strokeDasharray="5 4" />
            <text x={494} y={Y(duration) - 4} textAnchor="end" fontSize={9.5} fill="var(--warn)">
              单帧时间 {duration.toFixed(0)} ms
            </text>
          </>
        )}
        {!intervals.length && (
          <text x={250} y={H / 2} textAnchor="middle" fontSize={11} fill="var(--text-muted)">
            {running ? "等待触发…" : "相邻帧的时间间隔将显示在这里"}
          </text>
        )}
      </svg>
      {error && <span className="c-ng" style={{ fontSize: 12 }}>{error}</span>}
    </div>
  );
}
