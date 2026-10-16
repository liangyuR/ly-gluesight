import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { isNavGroup, matchRoute, navEntries, navItems, settingsItem } from "./nav";
import { getEngineStatus, type EngineStatus } from "../lib/api";
import { linkStateLabels, usePlcStatus } from "../features/plc";
import { devLyflowHint } from "../features/cycle/readiness";

function motionMs(token:string){
  if(window.matchMedia?.("(prefers-reduced-motion: reduce)").matches)return 0;
  return parseFloat(getComputedStyle(document.documentElement).getPropertyValue(token))||0;
}

export default function Sidebar(){
  const {pathname}=useLocation();
  const [collapsed,setCollapsed]=useState(false);
  const [switching,setSwitching]=useState(false);
  const [indicator,setIndicator]=useState<{y:number;instant:boolean}|null>(null);
  const [engine,setEngine]=useState<EngineStatus|null>(null);
  const navRef=useRef<HTMLElement>(null);
  const placed=useRef(false);
  const timers=useRef<number[]>([]);
  const plc=usePlcStatus();
  const plcDot={connected:"ok",connecting:"warn",error:"err",disconnected:"off"}[plc?.state??"disconnected"];
  const active=matchRoute([...navItems,settingsItem],pathname)?.path;
  useEffect(()=>{
    let alive=true,serial=0;
    const refresh=()=>{const request=++serial;return getEngineStatus().then(status=>{if(alive&&request===serial)setEngine(status);}).catch(()=>{if(alive&&request===serial)setEngine(null);});};
    void refresh();const timer=setInterval(()=>void refresh(),3000);
    return()=>{alive=false;clearInterval(timer);};
  },[]);
  useEffect(()=>()=>timers.current.forEach(clearTimeout),[]);
  useLayoutEffect(()=>{
    const link=navRef.current?.querySelector<HTMLElement>(".nav-item.active");
    if(!link){placed.current=false;setIndicator(null);return;}
    setIndicator({y:link.offsetTop+(collapsed?4:0),instant:!placed.current});
    placed.current=true;
  },[active,collapsed]);
  const toggle=()=>{
    if(switching)return;
    const fade=motionMs("--dur-short"),resize=motionMs("--dur-medium");
    if(!fade){setCollapsed(v=>!v);return;}
    setSwitching(true);
    timers.current=[window.setTimeout(()=>setCollapsed(v=>!v),fade),window.setTimeout(()=>setSwitching(false),fade+resize)];
  };
  const item=(entry:typeof navItems[number])=>(
    <Link key={entry.path} to={entry.path} aria-label={entry.label} title={collapsed?entry.label:undefined} aria-current={active===entry.path?"page":undefined} className={"nav-item"+(active===entry.path?" active":"")}><entry.icon size={17}/><span className="nav-label">{entry.label}</span></Link>
  );
  const plcText="PLC · "+linkStateLabels[plc?.state??"disconnected"];
  const engineText=engine?engine.backend+" · "+(engine.ready?"就绪":"未就绪"):"后端未连接";
  return <aside className={"sidebar"+(collapsed?" collapsed":"")+(switching?" switching":"")}><div className="sidebar-brand"><img src="/app-icon.png" alt="" className="brand-logo"/>{!collapsed&&<div className="brand-text"><strong>GlueSight</strong><span>胶路智检</span></div>}</div>
    <nav ref={navRef} className={"sidebar-nav"+(indicator?" has-indicator":"")} aria-label="操作导航">{indicator&&<span aria-hidden className={"nav-indicator"+(indicator.instant?" instant":"")} style={{transform:`translateY(${indicator.y}px)`}}/>}{navEntries.map(entry=>isNavGroup(entry)?<div className="nav-group" key={entry.label} role="group" aria-label={entry.label}>{collapsed?<hr className="nav-group-divider"/>:<div className="nav-group-label">{entry.label}</div>}{entry.children.map(item)}</div>:item(entry))}</nav>
    <div className="sidebar-footer"><div className="status-list"><Link to="/plc" className="engine-status" aria-label={plcText} title={plc?.message??"PLC 未连接"}><span className={"dot "+plcDot}/>{!collapsed&&<span className="status-text">{plcText}</span>}</Link><Link to={settingsItem.path} className="engine-status" aria-label={engineText} title={(engine?.message??"未连接后端")+(import.meta.env.DEV&&engine&&!engine.ready?"；"+devLyflowHint:"")}><span className={"dot "+(engine?.ready?"ok":"warn")}/>{!collapsed&&<span className="status-text">{engineText}</span>}</Link></div><div className="sidebar-actions"><Link to={settingsItem.path} className={"icon-btn"+(active===settingsItem.path?" active":"")} aria-label={settingsItem.label} title={settingsItem.label} aria-current={active===settingsItem.path?"page":undefined}><settingsItem.icon size={18}/></Link><button className="icon-btn" onClick={toggle} aria-label={collapsed?"展开侧边栏":"折叠侧边栏"} title={collapsed?"展开侧边栏":"折叠侧边栏"}>{collapsed?<PanelLeftOpen size={18}/>:<PanelLeftClose size={18}/>}</button></div></div>
  </aside>;
}
