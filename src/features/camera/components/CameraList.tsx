import { useLayoutEffect, useRef } from "react";
import { RefreshCw } from "lucide-react";
import { Button, List, ListItem } from "qing-steel";
import type { CameraConfig, CameraStatus } from "../types";

const sourceText = { mvs:"海康 MVS",sim:"合成图（离线）",sampleSet:"样本集（离线）" } as const;

export function cameraState(config: CameraConfig, status: CameraStatus | null): { tone: "ok" | "warn" | "off"; label: string; text: string } {
  if (status?.released) return { tone:"warn", label:"已释放", text:"已释放给 MVS" };
  if (status?.ready) return { tone:"ok", label:"已连接", text:status.device ? `${status.device.model} · ${status.device.serial}` : sourceText[config.source] };
  if (config.source === "mvs" && !config.serial) return { tone:"warn", label:"未绑定", text:"未绑定设备" };
  if (!status) return { tone:"off", label:"读取中", text:"正在读取相机状态" };
  return { tone:"warn", label:"未就绪", text:status.message || "相机未就绪" };
}

export interface CalibrationSummary { tone: "ok" | "warn" | "off"; text: string }

export default function CameraList({ configs, statuses, selected, disabled, onSelect, onRefresh, refreshing, calibration }: {
  configs: CameraConfig[];
  statuses: CameraStatus[];
  selected: number;
  disabled?: boolean;
  onSelect: (cam: number) => void;
  onRefresh?: () => void;
  refreshing?: boolean;
  /** 按相机 id 给出标定状态；相机就绪后用它替换设备状态那一行。 */
  calibration?: Record<string, CalibrationSummary>;
}) {
  const root=useRef<HTMLElement>(null);
  useLayoutEffect(()=>{
    const list=root.current?.querySelector<HTMLElement>(".qs-list");
    if(!list)return;
    const reveal=()=>{
      const selectedItem=list.querySelector<HTMLElement>(".qs-selected");
      if(!selectedItem)return;
      const item=selectedItem.getBoundingClientRect(),viewport=list.getBoundingClientRect();
      if(item.left<viewport.left)list.scrollLeft+=item.left-viewport.left;
      else if(item.right>viewport.right)list.scrollLeft+=item.right-viewport.right;
    };
    reveal();
    const observer=new ResizeObserver(reveal);observer.observe(list);
    return()=>observer.disconnect();
  },[selected,configs]);
  const calibrated=calibration&&configs.length>0&&configs.every(config=>calibration[config.id])
    ?configs.filter(config=>calibration[config.id]!.tone==="ok").length:null;
  return <aside ref={root} className="cam-list" aria-label="相机列表">
    <div className="cam-list-head">{calibrated===null
      ?<span>相机 {configs.length} 台</span>
      :<span className={calibrated===configs.length?"ok":undefined}>标定 {calibrated} / {configs.length}</span>}</div>
    <List>
      {configs.map((config,i)=>{
        const state=cameraState(config,statuses.find(s=>s.cam===i)??null);
        const calib=state.tone==="ok"?calibration?.[config.id]:undefined;
        return <ListItem key={config.id} selected={i===selected} disabled={disabled} onClick={()=>onSelect(i)}
          leading={<span className={"dot "+(calib?.tone??state.tone)}/>}
          title={<><strong>{config.name}</strong> <span className="mono muted">{config.id}</span></>}
          description={calib?<span className={"cam-calib "+calib.tone} title={state.text}>{calib.text}</span>:<span title={state.text}>{state.text}</span>}/>;
      })}
    </List>
    {onRefresh&&<Button className="cam-list-refresh" variant="tonal" size="sm" leadingIcon={<RefreshCw size={14}/>} loading={refreshing} disabled={refreshing||disabled} onClick={onRefresh}>刷新</Button>}
  </aside>;
}
