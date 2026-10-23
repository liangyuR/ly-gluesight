import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { Crosshair } from "lucide-react";
import { Alert, Button, Card, Grid, Heading, NumberField, Stack, useSnackbar } from "qing-steel";

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
export default function CalibPanel({ cam, view=1, multiView=false, isSim, imageId, onMmPerPx }: { cam: number; view?:number; /** 三目拼接相机（旧配方）才标出视角；单视角相机不带“图 N”。 */ multiView?:boolean; isSim: boolean; imageId?:string; onMmPerPx?: (mmPerPx: number | null) => void }) {
  const [info, setInfo] = useState<CalibInfo | null>(null);
  const [cols, setCols] = useState(11);
  const [rows, setRows] = useState(8);
  const [square, setSquare] = useState(5);
  const [busy, setBusy] = useState(false);
  const [runError, setRunError] = useState("");
  const { show: snack } = useSnackbar();
  const [loading,setLoading]=useState(isTauri()),[loaded,setLoaded]=useState(false),[loadError,setLoadError]=useState("");
  const mounted=useRef(true),loadSerial=useRef(0),runSerial=useRef(0),pending=useRef(false);
  const scope=JSON.stringify([cam,view,imageId,isSim]),current=useRef({cam,view,scope});current.current={cam,view,scope};
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;loadSerial.current++;runSerial.current++;};},[]);

  const load=async()=>{
    if(!isTauri())return;
    const serial=++loadSerial.current;setLoading(true);setLoadError("");
    try{
      const value=await invoke<CalibInfo|null>("vision_calib_info",{cam,view});
      if(!mounted.current||serial!==loadSerial.current||(current.current.cam!==cam||current.current.view!==view))return;
      setInfo(value);setLoaded(true);
      if(value?.pattern?.length===2){setCols(value.pattern[0]);setRows(value.pattern[1]);}
      if(value?.square!=null)setSquare(value.square);
    }catch(e){if(mounted.current&&serial===loadSerial.current&&current.current.cam===cam&&current.current.view===view)setLoadError(String(e));}
    finally{if(mounted.current&&serial===loadSerial.current&&current.current.cam===cam&&current.current.view===view)setLoading(false);}
  };

  useEffect(() => {
    setInfo(null);setLoaded(false);setLoadError("");setCols(11);setRows(8);setSquare(5);
    void load();
    return()=>{loadSerial.current++;};
  }, [cam,view]);
  useEffect(()=>{runSerial.current++;pending.current=false;setBusy(false);setRunError("");},[scope]);
  const mmPerPx = isSim ? 0.08 : info?.mmPerPx ?? null;
  useEffect(()=>{onMmPerPx?.(mmPerPx);},[mmPerPx,onMmPerPx]);
  const valid=Number.isInteger(cols)&&cols>=2&&cols<=100&&Number.isInteger(rows)&&rows>=2&&rows<=100&&Number.isFinite(square)&&square>0;
  const invalid="内角点行列需为 2–100 的整数，格长必须大于 0。";
  const format=(value:number|null)=>value!=null&&Number.isFinite(value)?value.toFixed(4):"—";

  const run = async () => {
    if(pending.current||loading||isSim||!imageId||!isTauri()||!valid)return;
    const serial=++runSerial.current;pending.current=true;
    setBusy(true);
    setRunError("");
    try {
      const r = await invoke<CalibInfo>("vision_calibrate", { pattern: [cols, rows], square, cam, view, imageId });
      if(!mounted.current||serial!==runSerial.current||current.current.scope!==scope)return;
      if(!r)throw new Error("标定没有返回有效结果");
      setInfo(r);
      setLoaded(true);
      setLoadError("");
      snack(`${multiView?`图 ${view} `:""}标定完成：残差 RMS ${format(r.rms)} mm，约 ${format(r.mmPerPx)} mm/px`);
    } catch (e) {
      if(mounted.current&&serial===runSerial.current&&current.current.scope===scope)setRunError(String(e));
    } finally {
      if(mounted.current&&serial===runSerial.current&&current.current.scope===scope){pending.current=false;setBusy(false);}
    }
  };

  return (
    <Card title={<Heading level={3}>{multiView?`图 ${view} · `:""}工位标定（飞拍）</Heading>} actions={<>
      <Button onClick={()=>void load()} disabled={busy||loading||!isTauri()}>刷新标定</Button>
      <Button variant={imageId&&!isSim?"filled":"tonal"} leadingIcon={<Crosshair size={15} />} onClick={()=>void run()} disabled={busy || loading || isSim || !imageId||!isTauri()||!valid}>{busy ? "标定中…" : "用冻结样本标定"}</Button>
    </>}>
      <Stack direction="column" align="stretch" gap={10}>
      <p className="muted calib-desc">将标定板放在实际胶路测量平面；{multiView?"各图分别标定，复用前验证代表性拍照点。":"每台相机分别标定，复用前在代表性位置验证。"}</p>
      {isSim ? (
        <p className="muted">合成图相机的像素当量已知（0.08 mm/px），自动使用内置标定。</p>
      ) : (
        <fieldset disabled={busy||loading||!isTauri()} style={{border:0,padding:0,margin:0,minWidth:0}}><Grid minWidth={140} gap={10}>
          <NumberField id="calib-cols" label="内角点（列）" min={2} max={100} step={1} value={Number.isFinite(cols)?cols:""} onValueChange={value => {setRunError("");setCols(value===""?NaN:value);}} />
          <NumberField id="calib-rows" label="内角点（行）" min={2} max={100} step={1} value={Number.isFinite(rows)?rows:""} onValueChange={value => {setRunError("");setRows(value===""?NaN:value);}} />
          <NumberField id="calib-square" label="格长（mm）" min={0} step={0.5} value={Number.isFinite(square)?square:""} onValueChange={value => {setRunError("");setSquare(value===""?NaN:value);}} />
        </Grid></fieldset>
      )}
      {!isTauri()&&<p className="muted">请在桌面软件中读取并执行工位标定。</p>}
      {!isSim&&!imageId&&<p className="muted">先获取或导入冻结样本，再执行工位标定。</p>}
      {loading&&<p className="muted">正在读取标定…</p>}
      {loadError&&<Alert tone="ng">读取标定失败：{loadError}。请刷新标定后重试。</Alert>}
      {!isSim&&!valid&&<Alert tone="ng">{invalid}</Alert>}
      <dl className="kv">
        <dt>当前标定</dt>
        <dd>
          {info
            ? `RMS ${format(info.rms)} mm · ${format(info.mmPerPx)} mm/px${info.pattern ? ` · ${info.pattern.join("×")} @ ${info.square} mm` : ""}${info.ts ? ` · ${new Date(info.ts).toLocaleString("zh-CN", { hour12: false })}` : ""}`
            : isSim?"使用内置标定（0.08 mm/px）":loaded?"未标定":"尚未读取标定"}
        </dd>
      </dl>
      {runError && <Alert tone="ng">{runError}</Alert>}
      </Stack>
    </Card>
  );
}
