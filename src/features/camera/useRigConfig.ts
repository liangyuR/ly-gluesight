import { useCallback, useEffect, useRef, useState } from "react";
import { recipeApi } from "../cycle/api";
import { subscribe } from "../plc/api";
import { workspaceApi } from "../workspace/api";
import { desktopAvailable } from "../../lib/desktop";
import { errorText } from "../../lib/errors";
import { cameraApi } from "./api";
import type { CameraConfig } from "./types";

type References = { references: Record<string, string[]>; ready: boolean; error: string };

async function readCameraReferences(): Promise<References["references"]> {
  const [production, drafts] = await Promise.allSettled([recipeApi.list(), workspaceApi.list()]);
  if (production.status === "rejected") throw production.reason;
  if (drafts.status === "rejected") throw drafts.reason;
  if (production.value.errors.length) throw new Error(production.value.errors.join("；"));

  const found: References["references"] = Object.create(null);
  const remember = (ids: string[], name: string) => {
    for (const id of ids) {
      const names = found[id] ??= [];
      if (!names.includes(name)) names.push(name);
    }
  };
  for (const recipe of production.value.recipes) remember(recipe.cameras, `生产配方 ${recipe.name}（${recipe.id}）`);
  for (const draft of drafts.value) {
    remember(draft.cameras, `候选配方 ${draft.name}（${draft.id}）`);
    const pending = draft.pending;
    if (pending) remember(pending.cameras, `待发布配方 ${pending.name}（${pending.id}）`);
  }
  return found;
}

function useCameraReferences(enabled: boolean) {
  const [state, setState] = useState<References>({ references: {}, ready: false, error: "" });
  const refreshRef = useRef<(() => void) | null>(null);
  const refresh = useCallback(() => refreshRef.current?.(), []);

  useEffect(() => {
    if (!enabled || !desktopAvailable()) return;
    let alive = true, reading = false, queued = false;
    const load = async () => {
      if (!alive) return;
      if (reading) { queued = true; return; }
      reading = true;
      try {
        const references = await readCameraReferences();
        if (alive && !queued) setState({ references, ready: true, error: "" });
      } catch (e) {
        if (alive && !queued) setState({
          references: {}, ready: false,
          error: `相机引用尚未读取：${errorText(e)}。在配方库处理后刷新再移除相机。`,
        });
      } finally {
        reading = false;
        if (queued) { queued = false; void load(); }
      }
    };
    const invalidate = () => {
      if (!alive) return;
      setState(previous => previous.ready || previous.error ? { ...previous, ready: false, error: "" } : previous);
      void load();
    };
    refreshRef.current = invalidate;
    const stop = subscribe("workspace://changed", invalidate, undefined, invalidate);
    return () => { alive = false; refreshRef.current = null; stop(); };
  }, [enabled]);

  return { references: state.references, referencesReady: state.ready, referenceError: state.error, refreshReferences: refresh };
}

export function useRigConfig({ withReferences, preferredId }: { withReferences: boolean; preferredId?: string }) {
  const [configs,setConfigs]=useState<CameraConfig[]>([]);
  const [cam,setCam]=useState(0);
  const [error,setError]=useState("");
  const [loading,setLoading]=useState(false);
  const [configReady,setConfigReady]=useState(false);
  const references = useCameraReferences(withReferences);
  const mounted=useRef(true),loadSerial=useRef(0);
  const current=useRef({cam,configs});current.current={cam,configs};
  const chosen=useRef<string|undefined>(undefined);
  const select=useCallback((i:number)=>{chosen.current=current.current.configs[i]?.id;setCam(i);},[]);
  const reload=useCallback(async()=>{
    const serial=++loadSerial.current;
    setLoading(true);setConfigReady(false);
    try{
      const next=await cameraApi.rigConfig();
      if(!mounted.current||serial!==loadSerial.current)return null;
      const selected=chosen.current ?? current.current.configs[current.current.cam]?.id ?? preferredId;
      setConfigs(next);
      if(selected){const i=Math.max(0,next.findIndex(c=>c.id===selected));chosen.current=next[i]?.id;current.current={cam:i,configs:next};setCam(i);}
      setConfigReady(true);setError("");
      return next;
    }catch(e){if(mounted.current&&serial===loadSerial.current)setError(errorText(e));return null;}
    finally{if(mounted.current&&serial===loadSerial.current)setLoading(false);}
  },[preferredId]);
  const reloadRef=useRef(reload);reloadRef.current=reload;
  useEffect(()=>{
    mounted.current=true;
    void reloadRef.current();
    return()=>{mounted.current=false;loadSerial.current++;};
  },[]);
  return {configs,cam,setCam:select,error,setError,loading,configReady,...references,reload};
}
