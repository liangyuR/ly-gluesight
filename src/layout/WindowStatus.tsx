import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Wrench } from "lucide-react";
import { getAppInfo, getEngineStatus, type AppInfo, type EngineStatus } from "../lib/api";
import { desktopAvailable } from "../lib/desktop";
import { linkStateLabels, usePlcStatus } from "../features/plc";
import { useMaintenance, useSiteStatus } from "../features/cycle/api";
import { siteModeText } from "../features/cycle/offline";
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
  const site = useSiteStatus();
  const maintenance = useMaintenance();
  if (!desktopAvailable()) return null;
  return <div className="window-badges">
    {site && <Link to="/offline" className={"window-badge" + (site.mode === "offline" ? " offline" : "")} title="查看站点模式">{siteModeText(site)}</Link>}
    {maintenance && <Link to="/inspect" className="window-badge maintenance" title={`开启于 ${formatTime(maintenance.since)} · 原因：${maintenance.reason}`}><Wrench size={12} aria-hidden="true" />维护模式</Link>}
  </div>;
}

export function WindowConnections() {
  const plc = usePlcStatus();
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  useEffect(() => {
    let alive = true, serial = 0;
    const refresh = () => {
      const request = ++serial;
      return getEngineStatus().then(status => { if (alive && request === serial) setEngine(status); }).catch(() => { if (alive && request === serial) setEngine(null); });
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 3000);
    return () => { alive = false; clearInterval(timer); };
  }, []);
  const plcDot = { connected: "ok", connecting: "warn", error: "err", disconnected: "off" }[plc?.state ?? "disconnected"];
  const plcText = "PLC · " + linkStateLabels[plc?.state ?? "disconnected"];
  const engineText = engine ? engine.backend + " · " + (engine.ready ? "就绪" : "未就绪") : "后端未连接";
  return <div className="window-connections">
    <Link to="/plc" className="window-connection" aria-label={plcText} title={plc?.message ?? "PLC 未连接"}><span className={"dot " + plcDot} /><span className="window-connection-text">{plcText}</span></Link>
    <Link to="/settings" className="window-connection" aria-label={engineText} title={(engine?.message ?? "未连接后端") + (import.meta.env.DEV && engine && !engine.ready ? "；" + devLyflowHint : "")}><span className={"dot " + (engine?.ready ? "ok" : "warn")} /><span className="window-connection-text">{engineText}</span></Link>
  </div>;
}
