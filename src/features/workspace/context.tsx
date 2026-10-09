import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { cameraApi, type CameraConfig } from "../camera";
import { recipeApi } from "../cycle/api";
import type { Recipe, RecipeDoc, RecipeSummary } from "../cycle/types";
import { subscribe } from "../plc";
import { desktopAvailable } from "../../lib/desktop";
import { workspaceApi } from "./api";
import type { FrameParams, Workspace, WorkspaceView } from "./types";
import "./workspace.css";

interface WorkspaceContextValue {
  list: RecipeSummary[]; drafts: Workspace[]; cameras: CameraConfig[];
  selectedId: string | null; data: WorkspaceView | null; doc: RecipeDoc | null; preview: Recipe | null;
  previewError: string; error: string; busy: boolean; dirty: boolean; frameDirty: boolean;
  frameDrafts: Record<number, FrameParams>;
  select: (id: string) => Promise<WorkspaceView | null>; clearSelection:()=>void; reloadList: () => Promise<void>;
  setDoc: (doc: RecipeDoc) => void; setFrameParams: (k: number, params: FrameParams) => void;
  act: (request: () => Promise<WorkspaceView>, message?: string) => Promise<WorkspaceView | null>;
  saveDoc: (doc?: RecipeDoc) => Promise<WorkspaceView | null>;
  notice: string; setError: (message: string) => void;
}
const Context = createContext<WorkspaceContextValue | null>(null);
const equal = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [list, setList] = useState<RecipeSummary[]>([]);
  const [drafts, setDrafts] = useState<Workspace[]>([]);
  const [cameras, setCameras] = useState<CameraConfig[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [data, setData] = useState<WorkspaceView | null>(null);
  const [doc, setDoc] = useState<RecipeDoc | null>(null);
  const [preview, setPreview] = useState<Recipe | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [frameDrafts, setFrameDrafts] = useState<Record<number, FrameParams>>({});
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const requestSerial = useRef(0);
  const acting = useRef(false);
  const selected = useRef<string | null>(null);
  const dirtyRef = useRef(false);
  const geometry=useRef("");
  const dirty = !!doc && !!data && !equal(doc, data.workspace.doc);
  dirtyRef.current = dirty || Object.keys(frameDrafts).some(k => !equal(frameDrafts[Number(k)], data?.workspace.frames[Number(k)]?.params));
  const frameDirty = Object.keys(frameDrafts).some(k => !equal(frameDrafts[Number(k)], data?.workspace.frames[Number(k)]?.params));

  const accept = useCallback((next: WorkspaceView) => {
    // 拍照点带着各自的相机、视野和标定引用
    const key=JSON.stringify([next.layout.id,next.layout.path,next.layout.part,next.layout.shots,next.layout.fov,next.layout.spacing]);
    const changed=key!==geometry.current;geometry.current=key;
    setData(next);
    setDoc(next.workspace.doc);
    setPreview(next.layout);
    setPreviewError("");
    setFrameDrafts(previous => changed?{}:Object.fromEntries(Object.entries(previous).filter(([k, p]) => !!next.workspace.frames[Number(k)]&&!equal(p, next.workspace.frames[Number(k)].params))));
  }, []);

  const reloadList = useCallback(async () => {
    if (!desktopAvailable()) return;
    const [recipes, workspaces, configs] = await Promise.all([recipeApi.list(), workspaceApi.list(), cameraApi.rigConfig()]);
    setList(recipes.recipes); setDrafts(workspaces); setCameras(configs);
    if (recipes.errors.length) setError(recipes.errors.join("；"));
  }, []);

  const select = useCallback(async (id: string) => {
    const serial = ++requestSerial.current;
    selected.current = id;
    setSelectedId(id);
    setError(""); setNotice(""); setData(null); setDoc(null); setPreview(null); setFrameDrafts({});
    try {
      const next = await workspaceApi.get(id);
      if (serial === requestSerial.current) {
        if(next.workspace.doc.id!==id)throw new Error("候选响应与所选配方不一致，请重试选择");
        accept(next);
        try { localStorage.setItem("tujiao-last-workspace", id); } catch { /* view preference only */ }
        return next;
      }
    } catch (e) { if (serial === requestSerial.current) setError(String(e)); }
    return null;
  }, [accept]);

  useEffect(() => {
    if (!desktopAvailable()) return;
    let alive = true;
    reloadList().then(async () => {
      const records = await recipeApi.list();
      const drafts = await workspaceApi.list();
      let preferred: string | null = null;
      try { preferred = localStorage.getItem("tujiao-last-workspace"); } catch { /* view preference only */ }
      const ids = [...records.recipes.map(r => r.id), ...drafts.map(w => w.doc.id)];
      if (alive && !selected.current && ids.length) await select(preferred && ids.includes(preferred) ? preferred : ids[0]);
    }).catch(e => setError(String(e)));
    const off = subscribe<string>("workspace://changed", id => {
      void reloadList().catch(e => setError(String(e)));
      if (id === selected.current && !dirtyRef.current && !acting.current) {
        const serial = ++requestSerial.current;
        const current = () => alive && serial === requestSerial.current && id === selected.current && !dirtyRef.current && !acting.current;
        void workspaceApi.get(id).then(next => { if (current()) accept(next); }).catch(e => { if (current()) setError(String(e)); });
      }
    });
    return () => { alive = false; requestSerial.current++; off(); };
  }, [reloadList, select, accept]);

  useEffect(() => {
    if (!doc || !desktopAvailable() || equal(doc, data?.workspace.doc)) return;
    let alive = true;
    const timer = setTimeout(() => recipeApi.preview(doc).then(r => {
      if (alive) { setPreview(r); setPreviewError(""); }
    }).catch(e => { if (alive) setPreviewError(String(e)); }), 250);
    return () => { alive = false; clearTimeout(timer); };
  }, [doc, data?.workspace.doc]);

  const act = useCallback(async (request: () => Promise<WorkspaceView>, message = "") => {
    if (acting.current) return null;
    acting.current = true; setBusy(true); setError(""); setNotice("");
    const originalId = selected.current;
    const serial = ++requestSerial.current;
    const current = () => originalId === selected.current && serial === requestSerial.current;
    try {
      const next = await request();
      if (current()) {
        if (next.workspace.doc.id !== originalId) throw new Error("候选响应与当前配方不一致，请重试操作");
        accept(next);
      }
      try { await reloadList(); }
      catch (e) { if (current()) setError(`候选操作已完成，但配方列表刷新失败：${String(e)}`); }
      if (!current()) return null;
      setNotice(message);
      return next;
    } catch (e) { if (current()) setError(String(e)); return null; }
    finally { acting.current = false; setBusy(false); }
  }, [accept, reloadList]);

  const saveDoc = useCallback((draft?: RecipeDoc) => {
    const current = draft ?? doc;
    if (!current || !data) return Promise.resolve(null);
    return act(() => workspaceApi.saveDoc(current.id, data.workspace.revision, current), "候选配置已保存，生产版本保持不变");
  }, [doc, data, act]);

  return <Context.Provider value={{ list, drafts, cameras, selectedId, data, doc, preview, previewError, error, busy, dirty, frameDirty,
    clearSelection:()=>{requestSerial.current++;selected.current=null;setSelectedId(null);setData(null);setDoc(null);setPreview(null);setFrameDrafts({});setError("");setNotice("");},
    frameDrafts, select, reloadList, setDoc, setFrameParams:(k, params) => setFrameDrafts(previous => ({ ...previous, [k]:params })),
    act, saveDoc, notice, setError }}>{children}</Context.Provider>;
}

export function useWorkspace() {
  const value = useContext(Context);
  if (!value) throw new Error("WorkspaceProvider missing");
  return value;
}
