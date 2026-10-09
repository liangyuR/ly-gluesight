import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { isNavGroup, navEntries, navItems } from "./nav";
import { getEngineStatus, type EngineStatus } from "../lib/api";
import { linkStateLabels, usePlcStatus } from "../features/plc";

export default function Sidebar(){
  const {pathname}=useLocation();
  const [collapsed,setCollapsed]=useState(false);
  const [engine,setEngine]=useState<EngineStatus|null>(null);
  const plc=usePlcStatus();
  const plcDot={connected:"ok",connecting:"warn",error:"err",disconnected:"off"}[plc?.state??"disconnected"];
  const active=navItems.filter(item=>pathname===item.path||pathname.startsWith(item.path+"/")).sort((a,b)=>b.path.length-a.path.length)[0]?.path;
  useEffect(()=>{
    let alive=true,serial=0;
    const refresh=()=>{const request=++serial;return getEngineStatus().then(status=>{if(alive&&request===serial)setEngine(status);}).catch(()=>{if(alive&&request===serial)setEngine(null);});};
    void refresh();const timer=setInterval(()=>void refresh(),3000);
    return()=>{alive=false;clearInterval(timer);};
  },[]);
  const item=(entry:typeof navItems[number],sub=false)=>(
    <Link key={entry.path} to={entry.path} aria-label={entry.label} title={collapsed?entry.label:undefined} aria-current={active===entry.path?"page":undefined} className={"nav-item"+(sub?" nav-subitem":"")+(active===entry.path?" active":"")}><entry.icon size={17}/>{!collapsed&&<span>{entry.label}</span>}</Link>
  );
  return <aside className={"sidebar"+(collapsed?" collapsed":"")}><div className="sidebar-brand"><img src="/app-icon.png" alt="" className="brand-logo"/>{!collapsed&&<div className="brand-text"><strong>GlueSight</strong><span>胶路智检</span></div>}</div>
    <nav className="sidebar-nav" aria-label="操作导航">{navEntries.map(entry=>isNavGroup(entry)?<div className="nav-group" key={entry.label}><div className="nav-group-label" title={collapsed?entry.label:undefined}>{collapsed?<entry.icon size={17}/>:<span>{entry.label}</span>}</div>{entry.children.map(child=>item(child,true))}</div>:item(entry))}</nav>
    <div className="sidebar-footer"><div className="status-list"><div className="engine-status" title={plc?.message??"PLC 未连接"}><span className={"dot "+plcDot}/>{!collapsed&&<span>PLC · {linkStateLabels[plc?.state??"disconnected"]}</span>}</div><div className="engine-status" title={engine?.message??"未连接后端"}><span className={"dot "+(engine?.ready?"ok":"warn")}/>{!collapsed&&<span>{engine?engine.backend+" · "+(engine.ready?"就绪":"未就绪"):"后端未连接"}</span>}</div></div><button className="icon-btn" onClick={()=>setCollapsed(v=>!v)} aria-label={collapsed?"展开侧边栏":"折叠侧边栏"}>{collapsed?<PanelLeftOpen size={18}/>:<PanelLeftClose size={18}/>}</button></div>
  </aside>;
}
