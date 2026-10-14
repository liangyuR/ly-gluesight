import type { ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import type { CameraConfig, CameraStatus } from "../types";

const sourceText = { mvs:"海康 MVS",sim:"合成图（离线）",sampleSet:"样本集（离线）" } as const;

export function cameraState(config: CameraConfig, status: CameraStatus | null): { tone: "ok" | "warn" | "off"; text: string } {
  if (status?.ready) return { tone:"ok", text:status.device ? `${status.device.model} · ${status.device.serial}` : sourceText[config.source] };
  if (config.source === "mvs" && !config.serial) return { tone:"warn", text:"未绑定设备" };
  return { tone:"off", text:status?.message || "未连接" };
}

export default function CameraList({ configs, statuses, selected, filter, disabled, onSelect, onRefresh, refreshing, footer }: {
  configs: CameraConfig[];
  statuses: CameraStatus[];
  selected: number;
  filter?: (config: CameraConfig) => boolean;
  disabled?: boolean;
  onSelect: (cam: number) => void;
  onRefresh?: () => void;
  refreshing?: boolean;
  footer?: ReactNode;
}) {
  return <aside className="cam-list" aria-label="相机列表">
    <div className="cam-list-head"><span>相机 {configs.length}/8</span>{onRefresh&&<button className="icon-btn" onClick={onRefresh} disabled={refreshing||disabled} aria-label="刷新相机配置" title="刷新相机配置"><RefreshCw size={14}/></button>}</div>
    {configs.map((config,i)=>{
      if(filter&&!filter(config))return null;
      const state=cameraState(config,statuses.find(s=>s.cam===i)??null);
      return <button key={config.id} className={"cam-list-item"+(i===selected?" active":"")} aria-current={i===selected?"true":undefined} aria-label={`${config.name} ${config.id}`} disabled={disabled} onClick={()=>onSelect(i)}>
        <span className={"dot "+state.tone}/>
        <span className="cam-list-text"><span><strong>{config.name}</strong> <span className="mono muted">{config.id}</span></span><small>{state.text}</small></span>
      </button>;
    })}
    {footer}
  </aside>;
}
