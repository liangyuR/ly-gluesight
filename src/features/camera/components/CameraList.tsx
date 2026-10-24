import { RefreshCw } from "lucide-react";
import { IconButton, List, ListItem } from "qing-steel";
import type { CameraConfig, CameraStatus } from "../types";

const sourceText = { mvs:"海康 MVS",sim:"合成图（离线）",sampleSet:"样本集（离线）" } as const;

export function cameraState(config: CameraConfig, status: CameraStatus | null): { tone: "ok" | "warn" | "off"; label: string; text: string } {
  if (status?.released) return { tone:"warn", label:"已释放", text:"已释放给 MVS" };
  if (status?.ready) return { tone:"ok", label:"已连接", text:status.device ? `${status.device.model} · ${status.device.serial}` : sourceText[config.source] };
  if (config.source === "mvs" && !config.serial) return { tone:"warn", label:"未绑定", text:"未绑定设备" };
  if (!status) return { tone:"off", label:"读取中", text:"正在读取相机状态" };
  return { tone:"warn", label:"未就绪", text:status.message || "相机未就绪" };
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
  return <aside className="cam-list" aria-label="相机列表">
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
