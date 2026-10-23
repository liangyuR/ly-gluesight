import { useLayoutEffect, useRef } from "react";
import { RefreshCw } from "lucide-react";
import { IconButton, List, ListItem } from "qing-steel";
import type { CameraConfig, CameraStatus } from "../types";

const sourceText = { mvs:"海康 MVS",sim:"合成图（离线）",sampleSet:"样本集（离线）" } as const;

export function cameraState(config: CameraConfig, status: CameraStatus | null): { tone: "ok" | "warn" | "off"; label: string; text: string } {
  if (status?.released) return { tone:"warn", label:"已释放", text:"已释放给 MVS" };
  if (status?.ready) return { tone:"ok", label:"已连接", text:status.device ? `${status.device.model} · ${status.device.serial}` : sourceText[config.source] };
  if (config.source === "mvs" && !config.serial) return { tone:"warn", label:"未绑定", text:"未绑定设备" };
  return { tone:"off", label:"未连接", text:status?.message || "未连接" };
}

export default function CameraList({ configs, statuses, selected, disabled, onSelect, onRefresh, refreshing }: {
  configs: CameraConfig[];
  statuses: CameraStatus[];
  selected: number;
  disabled?: boolean;
  onSelect: (cam: number) => void;
  onRefresh?: () => void;
  refreshing?: boolean;
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
  return <aside ref={root} className="cam-list" aria-label="相机列表">
    <div className="cam-list-head"><span>相机 {configs.length}/8</span>{onRefresh&&<IconButton size="sm" onClick={onRefresh} disabled={refreshing||disabled} aria-label="刷新相机配置" title="刷新相机配置"><RefreshCw size={14}/></IconButton>}</div>
    <List>
      {configs.map((config,i)=>{
        const state=cameraState(config,statuses.find(s=>s.cam===i)??null);
        return <ListItem key={config.id} selected={i===selected} disabled={disabled} onClick={()=>onSelect(i)}
          leading={<span className={"dot "+state.tone}/>}
          title={<><strong>{config.name}</strong> <span className="mono muted">{config.id}</span></>}
          description={state.text}/>;
      })}
    </List>
  </aside>;
}
