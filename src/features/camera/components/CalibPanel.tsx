import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { Crosshair } from "lucide-react";

interface CalibInfo {
  path: string;
  rms: number | null;
  mmPerPx: number | null;
  maxError: number | null;
  pattern: number[] | null;
  square: number | null;
  ts: number | null;
}

/** 工位标定：标定板放在内边所在高度，软触发一帧，用 lyFlow 的 image.board_calib 求单应，存为工位标定文件。 */
export default function CalibPanel({ cam, isSim, imageId }: { cam: number; isSim: boolean; imageId?:string }) {
  const [info, setInfo] = useState<CalibInfo | null>(null);
  const [cols, setCols] = useState(11);
  const [rows, setRows] = useState(8);
  const [square, setSquare] = useState(5);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ ok: boolean; text: string } | null>(null);
  const [loading,setLoading]=useState(isTauri()),[loaded,setLoaded]=useState(false),[loadError,setLoadError]=useState("");
  const mounted=useRef(true),loadSerial=useRef(0),runSerial=useRef(0),pending=useRef(false);
  const scope=JSON.stringify([cam,imageId,isSim]),current=useRef({cam,scope});current.current={cam,scope};
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;loadSerial.current++;runSerial.current++;};},[]);

  const load=async()=>{
    if(!isTauri())return;
    const serial=++loadSerial.current;setLoading(true);setLoadError("");
    try{
      const value=await invoke<CalibInfo|null>("vision_calib_info",{cam});
      if(!mounted.current||serial!==loadSerial.current||current.current.cam!==cam)return;
      setInfo(value);setLoaded(true);
      if(value?.pattern?.length===2){setCols(value.pattern[0]);setRows(value.pattern[1]);}
      if(value?.square!=null)setSquare(value.square);
    }catch(e){if(mounted.current&&serial===loadSerial.current&&current.current.cam===cam)setLoadError(String(e));}
    finally{if(mounted.current&&serial===loadSerial.current&&current.current.cam===cam)setLoading(false);}
  };

  useEffect(() => {
    setInfo(null);setLoaded(false);setLoadError("");setCols(11);setRows(8);setSquare(5);
    void load();
    return()=>{loadSerial.current++;};
  }, [cam]);
  useEffect(()=>{runSerial.current++;pending.current=false;setBusy(false);setNotice(null);},[scope]);
  const valid=Number.isInteger(cols)&&cols>=2&&cols<=100&&Number.isInteger(rows)&&rows>=2&&rows<=100&&Number.isFinite(square)&&square>0;
  const invalid="内角点行列需为 2–100 的整数，格长必须大于 0。";
  const format=(value:number|null)=>value!=null&&Number.isFinite(value)?value.toFixed(4):"—";

  const run = async () => {
    if(pending.current||loading||isSim||!imageId||!isTauri()||!valid)return;
    const serial=++runSerial.current;pending.current=true;
    setBusy(true);
    setNotice(null);
    try {
      const r = await invoke<CalibInfo>("vision_calibrate", { pattern: [cols, rows], square, cam, imageId });
      if(!mounted.current||serial!==runSerial.current||current.current.scope!==scope)return;
      if(!r)throw new Error("标定没有返回有效结果");
      setInfo(r);
      setLoaded(true);
      setLoadError("");
      setNotice({ ok: true, text: `标定完成：残差 RMS ${format(r.rms)} mm，约 ${format(r.mmPerPx)} mm/px` });
    } catch (e) {
      if(mounted.current&&serial===runSerial.current&&current.current.scope===scope)setNotice({ ok: false, text: String(e) });
    } finally {
      if(mounted.current&&serial===runSerial.current&&current.current.scope===scope){pending.current=false;setBusy(false);}
    }
  };

  return (
    <div className="panel">
      <div className="panel-head">
        <h3 className="panel-title">工位标定（飞拍）</h3>
        <span className="muted">用 lyFlow 求单应；标定属于相机工位，换型不重标；标定板放在内边所在高度的平面上</span>
        <span className="spacer" />
        <button className="btn" onClick={()=>void load()} disabled={busy||loading||!isTauri()}>刷新标定</button>
        <button className="btn primary" onClick={()=>void run()} disabled={busy || loading || isSim || !imageId||!isTauri()||!valid}>
          <Crosshair size={15} />
          {busy ? "标定中…" : "用冻结样本标定"}
        </button>
      </div>
      {isSim ? (
        <p className="muted">模拟相机的像素当量已知（0.08 mm/px），自动使用内置标定。</p>
      ) : (
        <fieldset disabled={busy||loading||!isTauri()} style={{border:0,padding:0,margin:0,minWidth:0}}><div className="calib-row">
          <label className="field">
            <span>内角点（列）</span>
            <input id="calib-cols" className="input mono" type="number" min={2} max={100} step={1} value={Number.isFinite(cols)?cols:""} onChange={(e) => {setNotice(null);setCols(e.target.value===""?NaN:Number(e.target.value));}} />
          </label>
          <label className="field">
            <span>内角点（行）</span>
            <input id="calib-rows" className="input mono" type="number" min={2} max={100} step={1} value={Number.isFinite(rows)?rows:""} onChange={(e) => {setNotice(null);setRows(e.target.value===""?NaN:Number(e.target.value));}} />
          </label>
          <label className="field">
            <span>格长（mm）</span>
            <input id="calib-square" className="input mono" type="number" step={0.5} min={0} value={Number.isFinite(square)?square:""} onChange={(e) => {setNotice(null);setSquare(e.target.value===""?NaN:Number(e.target.value));}} />
          </label>
        </div></fieldset>
      )}
      {!isTauri()&&<p className="muted">请在桌面软件中读取并执行工位标定。</p>}
      {!isSim&&!imageId&&<p className="muted">先获取或导入冻结样本，再执行工位标定。</p>}
      {loading&&<p className="muted">正在读取标定…</p>}
      {loadError&&<div className="notice error" role="alert">读取标定失败：{loadError}。请刷新标定后重试。</div>}
      {!isSim&&!valid&&<div className="notice error" role="alert">{invalid}</div>}
      <dl className="kv">
        <dt>当前标定</dt>
        <dd>
          {info
            ? `RMS ${format(info.rms)} mm · ${format(info.mmPerPx)} mm/px${info.pattern ? ` · ${info.pattern.join("×")} @ ${info.square} mm` : ""}${info.ts ? ` · ${new Date(info.ts).toLocaleString("zh-CN", { hour12: false })}` : ""}`
            : isSim?"使用内置标定（0.08 mm/px）":loaded?"未标定":"尚未读取标定"}
        </dd>
      </dl>
      {notice && <div className={`notice ${notice.ok ? "ok" : "error"}`}>{notice.text}</div>}
    </div>
  );
}
