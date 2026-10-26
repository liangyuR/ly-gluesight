import { useCallback, useEffect, useRef, useState } from "react";
import { recipeApi } from "../cycle";
import { workspaceApi } from "../workspace/api";
import { desktopAvailable } from "../../lib/desktop";
import { errorText } from "../../lib/errors";
import { cameraApi } from "./api";
import type { CameraConfig } from "./types";

/** 相机组配置与当前选中相机；withReferences 时一并读出各相机被哪些配方引用（移除相机的前置条件）。 */
export function useRigConfig({ withReferences, preferredId, reloadKey }: { withReferences: boolean; preferredId?: string; reloadKey?: unknown }) {
  const [configs,setConfigs]=useState<CameraConfig[]>([]);
  const [cam,setCam]=useState(0);
  const [error,setError]=useState("");
  const [loading,setLoading]=useState(false);
  const [references,setReferences]=useState<Record<string,string[]>>({});
  const [referencesReady,setReferencesReady]=useState(false);
  const [referenceError,setReferenceError]=useState("");
  const mounted=useRef(true),loadSerial=useRef(0);
  const current=useRef({cam,configs});current.current={cam,configs};
  const chosen=useRef<string|undefined>(undefined);
  const select=useCallback((i:number)=>{chosen.current=current.current.configs[i]?.id;setCam(i);},[]);
  useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;loadSerial.current++;};},[]);
  const reload=useCallback(async()=>{
    const serial=++loadSerial.current;
    setLoading(true);setReferencesReady(false);
    const referenceRequest=withReferences&&desktopAvailable()?Promise.all([recipeApi.list(),workspaceApi.list()]).then(([production,drafts])=>{
      if(production.errors.length)throw new Error(production.errors.join("；"));
      const found:Record<string,string[]>={};
      const remember=(ids:string[],name:string)=>ids.forEach(id=>{const values=found[id]??=[];if(!values.includes(name))values.push(name);});
      production.recipes.forEach(r=>remember(r.cameras,`生产配方 ${r.name}（${r.id}）`));
      drafts.forEach(w=>{
        for(const [doc,label] of [[w.doc,"候选配方"],[w.pending?.doc,"待发布配方"]] as const){
          if(doc)remember(doc.inspect?.map(item=>item.camera)??[],`${label} ${doc.name}（${doc.id}）`);
        }
      });
      return {found,error:""};
    }).catch(e=>({found:{},error:`相机引用尚未读取：${errorText(e)}。在配方库处理后刷新再移除相机。`})):Promise.resolve({found:{},error:""});
    try{
      const [next,refs]=await Promise.all([cameraApi.rigConfig(),referenceRequest]);
      if(!mounted.current||serial!==loadSerial.current)return null;
      const selected=chosen.current ?? current.current.configs[current.current.cam]?.id ?? preferredId;
      setConfigs(next);
      if(selected){const i=Math.max(0,next.findIndex(c=>c.id===selected));chosen.current=next[i]?.id;current.current={cam:i,configs:next};setCam(i);}
      setReferences(refs.found);setReferencesReady(!refs.error);setReferenceError(refs.error);setError("");
      return next;
    }catch(e){if(mounted.current&&serial===loadSerial.current)setError(errorText(e));return null;}
    finally{if(mounted.current&&serial===loadSerial.current)setLoading(false);}
  },[withReferences,preferredId]);
  const reloadRef=useRef(reload);reloadRef.current=reload;
  useEffect(()=>{void reloadRef.current();},[reloadKey]);
  return {configs,cam,setCam:select,error,setError,loading,references,referencesReady,referenceError,reload};
}
