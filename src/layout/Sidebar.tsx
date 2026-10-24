import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { Link, useLocation } from "react-router-dom";
import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { IconButton } from "qing-steel";
import { isNavGroup, matchRoute, navEntries, navItems, projectsItem, settingsItem } from "./nav";
import { getEngineStatus, type EngineStatus } from "../lib/api";
import { linkStateLabels, usePlcStatus } from "../features/plc";
import { devLyflowHint } from "../features/cycle/readiness";
import ProjectSwitcher from "../features/project/ProjectSwitcher";

function token(name:string){
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function motionMs(name:string){
  if(window.matchMedia?.("(prefers-reduced-motion: reduce)").matches)return 0;
  return parseFloat(token(name))||0;
}

export default function Sidebar({main}:{main?:RefObject<HTMLElement|null>}){
  const {pathname}=useLocation();
  const [collapsed,setCollapsed]=useState(false);
  const [switching,setSwitching]=useState(false);
  const [indicator,setIndicator]=useState<{y:number;instant:boolean}|null>(null);
  const [engine,setEngine]=useState<EngineStatus|null>(null);
  const navRef=useRef<HTMLElement>(null);
  const placed=useRef(false);
  const placedRail=useRef(collapsed);
  const mainLeft=useRef<number|null>(null);
  const timers=useRef<number[]>([]);
  const plc=usePlcStatus();
  const plcDot={connected:"ok",connecting:"warn",error:"err",disconnected:"off"}[plc?.state??"disconnected"];
  const active=matchRoute([...navItems,projectsItem,settingsItem],pathname)?.path;
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
    setIndicator({y:link.offsetTop+(collapsed?4:0),instant:!placed.current||placedRail.current!==collapsed});
    placed.current=true;placedRail.current=collapsed;
  },[active,collapsed]);
  useLayoutEffect(()=>{
    const from=mainLeft.current;mainLeft.current=null;
    const target=main?.current,duration=motionMs("--dur-medium");
    if(from===null||!target?.animate||!duration)return;
    const dx=from-target.getBoundingClientRect().left;
    if(Math.abs(dx)<0.5)return;
    target.animate([{transform:`translateX(${dx}px)`},{transform:"none"}],{duration,easing:token("--ease-standard")||"ease-out"});
  },[collapsed,main]);
  const toggle=()=>{
    if(switching)return;
    const fade=motionMs("--dur-short"),move=motionMs("--dur-medium");
    if(!fade){setCollapsed(v=>!v);return;}
    setSwitching(true);
    timers.current=[
      window.setTimeout(()=>{mainLeft.current=main?.current?.getBoundingClientRect().left??null;setCollapsed(v=>!v);},fade),
      window.setTimeout(()=>setSwitching(false),fade+move),
    ];
  };
  const item=(entry:typeof navItems[number])=>(
    <Link key={entry.path} to={entry.path} aria-label={entry.label} title={collapsed?entry.label:undefined} aria-current={active===entry.path?"page":undefined} className={"nav-item"+(active===entry.path?" active":"")}><entry.icon size={17}/><span className="nav-label">{entry.label}</span></Link>
  );
  const plcText="PLC · "+linkStateLabels[plc?.state??"disconnected"];
  const engineText=engine?engine.backend+" · "+(engine.ready?"就绪":"未就绪"):"后端未连接";
  return <aside className={"sidebar"+(collapsed?" collapsed":"")+(switching?" switching":"")}><span className="sidebar-surface" aria-hidden="true"><i/></span><div className="sidebar-brand"><img src="/app-icon.png" alt="" className="brand-logo"/>{!collapsed&&<div className="brand-text"><strong>GlueSight</strong><span>胶路智检</span></div>}</div>
    <ProjectSwitcher collapsed={collapsed}/>
    <nav ref={navRef} className={"sidebar-nav"+(indicator?" has-indicator":"")} aria-label="操作导航">{indicator&&<span aria-hidden className={"nav-indicator"+(indicator.instant?" instant":"")} style={{transform:`translateY(${indicator.y}px)`}}/>}{navEntries.map(entry=>isNavGroup(entry)?<div className="nav-group" key={entry.label} role="group" aria-label={entry.label}>{collapsed?<hr className="nav-group-divider"/>:<div className="nav-group-label">{entry.label}</div>}{entry.children.map(item)}</div>:item(entry))}</nav>
    <div className="sidebar-footer">{item(projectsItem)}{item(settingsItem)}<div className="status-list"><Link to="/plc" className="engine-status" aria-label={plcText} title={plc?.message??"PLC 未连接"}><span className={"dot "+plcDot}/>{!collapsed&&<span className="status-text">{plcText}</span>}</Link><Link to={settingsItem.path} className="engine-status" aria-label={engineText} title={(engine?.message??"未连接后端")+(import.meta.env.DEV&&engine&&!engine.ready?"；"+devLyflowHint:"")}><span className={"dot "+(engine?.ready?"ok":"warn")}/>{!collapsed&&<span className="status-text">{engineText}</span>}</Link></div><div className="sidebar-actions"><IconButton onClick={toggle} aria-expanded={!collapsed} aria-label={collapsed?"展开侧边栏":"折叠侧边栏"} title={collapsed?"展开侧边栏":"折叠侧边栏"}>{collapsed?<PanelLeftOpen size={18}/>:<PanelLeftClose size={18}/>}</IconButton></div></div>
  </aside>;
}
