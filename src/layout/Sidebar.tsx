import { useEffect, useState } from "react";
import { NavLink } from "react-router-dom";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { isNavGroup, navEntries } from "./nav";
import { getEngineStatus, type EngineStatus } from "../lib/api";
import { linkStateLabels, usePlcStatus } from "../features/plc";

export default function Sidebar() {
  const [collapsed, setCollapsed] = useState(false);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const plc = usePlcStatus();
  const plcDot = { connected: "ok", connecting: "warn", error: "err", disconnected: "off" }[plc?.state ?? "disconnected"];

  useEffect(() => {
    getEngineStatus().then(setEngine).catch(() => setEngine(null));
  }, []);

  return (
    <aside className={`sidebar${collapsed ? " collapsed" : ""}`}>
      <div className="sidebar-brand">
        <img src="/app-icon.png" alt="" className="brand-logo" />
        {!collapsed && (
          <div className="brand-text">
            <strong>GlueSight</strong>
            <span>胶路智检</span>
          </div>
        )}
      </div>

      <nav className="sidebar-nav">
        {navEntries.map((entry) =>
          isNavGroup(entry) ? (
            <div className="nav-group" key={entry.label}>
              <div className="nav-group-label" title={collapsed ? entry.label : undefined}>
                <entry.icon size={18} />
                {!collapsed && <span>{entry.label}</span>}
              </div>
              {entry.children.map(({ path, label }) => (
                <NavLink
                  key={path}
                  to={path}
                  title={collapsed ? label : undefined}
                  className={({ isActive }) => `nav-item nav-subitem${isActive ? " active" : ""}`}
                >
                  {!collapsed && <span>{label}</span>}
                </NavLink>
              ))}
            </div>
          ) : (
            <NavLink
              key={entry.path}
              to={entry.path}
              title={collapsed ? entry.label : undefined}
              className={({ isActive }) => `nav-item${isActive ? " active" : ""}`}
            >
              <entry.icon size={18} />
              {!collapsed && <span>{entry.label}</span>}
            </NavLink>
          )
        )}
      </nav>

      <div className="sidebar-footer">
        <div className="status-list">
          <div className="engine-status" title={plc?.message ?? "PLC 未连接"}>
            <span className={`dot ${plcDot}`} />
            {!collapsed && <span>PLC · {linkStateLabels[plc?.state ?? "disconnected"]}</span>}
          </div>
          <div className="engine-status" title={engine?.message ?? "未连接后端"}>
            <span className={`dot ${engine?.ready ? "ok" : "warn"}`} />
            {!collapsed && <span>{engine ? `${engine.backend} · ${engine.ready ? "就绪" : "未就绪"}` : "后端未连接"}</span>}
          </div>
        </div>
        <button className="icon-btn" onClick={() => setCollapsed((v) => !v)} aria-label="折叠侧边栏">
          {collapsed ? <PanelLeftOpen size={18} /> : <PanelLeftClose size={18} />}
        </button>
      </div>
    </aside>
  );
}
