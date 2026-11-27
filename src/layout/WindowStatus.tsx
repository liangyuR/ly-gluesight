import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Wrench } from "lucide-react";
import { Alert, Button, Popover } from "qing-steel";
import { getAppInfo, type AppInfo } from "../lib/api";
import { useEngineIndicator } from "../lib/engineStatus";
import { desktopAvailable } from "../lib/desktop";
import { linkStateLabels, usePlcLinkStatus } from "../features/plc";
import { useMaintenance } from "../features/cycle/control";
import { devLyflowHint } from "../features/cycle/readiness";
import { formatTime } from "../features/history/meta";

export function WindowVersion() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    let alive = true;
    void getAppInfo().then(value => { if (alive) setInfo(value); }).catch(() => {});
    return () => { alive = false; };
  }, []);
  if (!info) return null;
  const detail = [info.gitSha, info.buildTime].filter(Boolean).join(" · ");
  return <span className="window-version" title={detail ? `版本 ${info.version} · ${detail}` : `版本 ${info.version}`}>{info.version === "dev" ? "dev" : "v" + info.version}</span>;
}

export function WindowModeBadges() {
  const maintenance = useMaintenance();
  if (!desktopAvailable()) return null;
  return <div className="window-badges">
    {maintenance && <Popover side="bottom" align="start" label="维护模式详情" title="维护模式" trigger={<button type="button" className="window-badge maintenance"><Wrench size={12} aria-hidden="true" />维护模式</button>}>
      <Alert tone="warn" title="不向 PLC 输出 NG">每件向 PLC 回结果码 0，按未检放行。</Alert>
      <p>开启于 {formatTime(maintenance.since)}</p>
      <p>原因：{maintenance.reason}</p>
      <Button asChild><Link to="/inspect">前往在线检测</Link></Button>
    </Popover>}
  </div>;
}

export function WindowConnections() {
  const plc = usePlcLinkStatus();
  const engine = useEngineIndicator();
  const plcDot = { connected: "ok", connecting: "warn", error: "err", disconnected: "off" }[plc?.state ?? "disconnected"];
  const plcText = "PLC · " + linkStateLabels[plc?.state ?? "disconnected"];
  const engineText = engine ? engine.backend + " · " + (engine.ready ? "就绪" : "未就绪") : "后端未连接";
  return <div className="window-connections">
    <Link to="/plc" className="window-connection" aria-label={plcText} title={plc?.message ?? "PLC 未连接"}><span className={"dot " + plcDot} /><span className="window-connection-text">{plcText}</span></Link>
    <Link to="/settings" className="window-connection" aria-label={engineText} title={(engine?.message ?? "未连接后端") + (import.meta.env.DEV && engine && !engine.ready ? "；" + devLyflowHint : "")}><span className={"dot " + (engine?.ready ? "ok" : "warn")} /><span className="window-connection-text">{engineText}</span></Link>
  </div>;
}
