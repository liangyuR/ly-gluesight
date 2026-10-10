import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { cameraApi, type CameraConfig } from "../camera";
import { recipeApi } from "../cycle/api";
import type { Recipe, RecipeDoc, RecipeSummary } from "../cycle/types";
import { subscribe } from "../plc";
import { desktopAvailable } from "../../lib/desktop";
import { workspaceApi } from "./api";
import type { ShotTeach, Workspace, WorkspaceView } from "./types";
import { sameTeach, shotTeach } from "./teach";
import "./workspace.css";

interface WorkspaceContextValue {
  list: RecipeSummary[]; drafts: Workspace[]; cameras: CameraConfig[];
  selectedId: string | null; data: WorkspaceView | null; doc: RecipeDoc | null; preview: Recipe | null;
  previewError: string; error: string; busy: boolean; dirty: boolean; frameDirty: boolean;
  /** 单帧示教里尚未保存的中线草稿，按拍照点下标 */
  frameDrafts: Record<number, ShotTeach>;
  rememberPosition: (k: number, view: number) => void;
  select: (id: string) => Promise<WorkspaceView | null>; clearSelection:()=>void; reloadList: () => Promise<void>;
  setDoc: (doc: RecipeDoc) => void; setFrameDraft: (k: number, teach: ShotTeach) => void;
  act: (request: () => Promise<WorkspaceView>, message?: string) => Promise<WorkspaceView | null>;
  saveDoc: (doc?: RecipeDoc) => Promise<WorkspaceView | null>;
  saveState: "saved" | "saving" | "failed" | "pending"; retrySave: () => void;
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
  const [frameDrafts, setFrameDrafts] = useState<Record<number, ShotTeach>>({});
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [saveState, setSaveState] = useState<"saved" | "saving" | "failed" | "pending">("saved");
  const [saveRetry, setSaveRetry] = useState(0);
  const failedSave = useRef("");
  const requestSerial = useRef(0);
  const acting = useRef(false);
  const selected = useRef<string | null>(null);
  const dirtyRef = useRef(false);
  const pendingRefresh = useRef<string | null>(null);
  const [refreshRevision, setRefreshRevision] = useState(0);
  const shotList=useRef("");
  const shotSources=useRef<string[]>([]);
  const dirty = !!doc && !!data && !equal(doc, data.workspace.doc);
  // 草稿与候选里已保存的中线比较（按 f32），不同才算未保存
  const savedTeach = (k: number) => { const shot = data?.workspace.doc.shots[k]; return shot ? shotTeach(shot) : undefined; };
  const frameDirty = Object.keys(frameDrafts).some(k => !sameTeach(frameDrafts[Number(k)], savedTeach(Number(k))));
  dirtyRef.current = dirty || frameDirty;

  const accept = useCallback((next: WorkspaceView) => {
    // 草稿按拍照点下标存：拍照点增删、调序或换了配方时清空，免得中线落到别的拍照点上
    const key=JSON.stringify([next.workspace.doc.id,next.workspace.doc.shots.map(s=>s.id)]);
    const changed=key!==shotList.current;shotList.current=key;
    const previousSources=shotSources.current;
    const sources=next.workspace.doc.shots.map(s=>JSON.stringify([s.id,s.poseId,s.camera,s.view,s.calib]));
    shotSources.current=sources;
    setData(next);
    setDoc(next.workspace.doc);
    setPreview(next.layout);
    setPreviewError("");
    setFrameDrafts(previous => {
      if (changed) return {};
      const kept: Record<number, ShotTeach> = {};
      for (const [key, draft] of Object.entries(previous)) {
        if(previousSources[Number(key)]!==sources[Number(key)])continue;
        const shot = next.workspace.doc.shots[Number(key)];
        if (!shot) continue;
        const saved = shotTeach(shot);
        // 取样后后端补上了像素当量：草稿还没有时跟着用
        const merged = !Number.isFinite(draft.mmPerPx) && Number.isFinite(saved.mmPerPx) ? { ...draft, mmPerPx: saved.mmPerPx } : draft;
        if (!sameTeach(merged, saved)) kept[Number(key)] = merged;
      }
      return kept;
    });
  }, []);

  const reloadList = useCallback(async () => {
    if (!desktopAvailable()) return;
    const [recipes, workspaces, configs] = await Promise.all([recipeApi.list(), workspaceApi.list(), cameraApi.rigConfig()]);
    setList(recipes.recipes); setDrafts(workspaces); setCameras(configs);
    if (recipes.errors.length) setError(recipes.errors.join("；"));
  }, []);

  const select = useCallback(async (id: string) => {
    if (dirtyRef.current && selected.current !== id) { setError("当前草稿尚未保存，请等待保存完成或重试后再切换配方"); return null; }
    pendingRefresh.current = null;
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
    const refresh = (id: string) => {
      void reloadList().catch(e => setError(String(e)));
      if (id && id === selected.current) {
        pendingRefresh.current = id;
        setRefreshRevision(revision => revision + 1);
      }
    };
    const off = subscribe<string>("workspace://changed", refresh);
    const offCameras = subscribe<unknown>("camera://changed", () => refresh(selected.current ?? ""));
    const offCalibration = subscribe<unknown>("calibration://changed", () => refresh(selected.current ?? ""));
    const offSettings = subscribe<unknown>("cycle://settings-changed", () => refresh(selected.current ?? ""));
    return () => { alive = false; requestSerial.current++; off(); offCameras(); offCalibration(); offSettings(); };
  }, [reloadList, select, accept]);

  useEffect(() => {
    const id = pendingRefresh.current;
    if (!id || id !== selectedId || dirty || frameDirty || busy || !desktopAvailable()) return;
    let alive = true;
    const serial = ++requestSerial.current;
    const current = () => alive && serial === requestSerial.current && id === selected.current && !dirtyRef.current && !acting.current;
    void workspaceApi.get(id).then(next => {
      if (!current()) return;
      if (next.workspace.doc.id !== id) throw new Error("候选响应与所选配方不一致，请重试选择");
      pendingRefresh.current = null;
      accept(next);
    }).catch(e => { if (current()) setError(String(e)); });
    return () => { alive = false; };
  }, [refreshRevision, selectedId, dirty, frameDirty, busy, accept]);

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

  const rememberPosition = useCallback((k: number, view: number) => {
    const id = selected.current;
    if (!id) return;
    void workspaceApi.progress(id,k,view).then(next => {
      if (selected.current !== id) return;
      setData(previous => previous?.workspace.doc.id === id ? {...previous,workspace:{...previous.workspace,lastPosition:next.workspace.lastPosition}} : previous);
    }).catch(e => { if (selected.current === id) setError(String(e)); });
  }, []);

  const saveDoc = useCallback((draft?: RecipeDoc) => {
    const current = draft ?? doc;
    if (!current || !data) return Promise.resolve(null);
    return act(() => workspaceApi.saveDoc(current.id, data.workspace.revision, current), "候选配置已保存，生产版本保持不变");
  }, [doc, data, act]);

  useEffect(() => {
    if (!data || !doc || busy || !desktopAvailable()) return;
    const entry = Object.entries(frameDrafts).find(([k, draft]) => !sameTeach(draft, savedTeach(Number(k))));
    if (!dirty && !entry) { setSaveState("saved"); return; }
    const signature = JSON.stringify([doc, entry, saveRetry]);
    if (failedSave.current === signature) return;
    setSaveState("pending");
    const timer = setTimeout(async () => {
      setSaveState("saving");
      const result = dirty ? await saveDoc() : await act(() => workspaceApi.saveDraft(doc.id, data.workspace.revision, Number(entry![0]), entry![1]));
      if (!result) { failedSave.current = signature; setSaveState("failed"); }
      else { failedSave.current = ""; setSaveState("saved"); }
    }, 900);
    return () => clearTimeout(timer);
  }, [doc, data, frameDrafts, dirty, busy, saveRetry, saveDoc, act]);

  return <Context.Provider value={{ list, drafts, cameras, selectedId, data, doc, preview, previewError, error, busy, dirty, frameDirty,
    clearSelection:()=>{pendingRefresh.current=null;requestSerial.current++;selected.current=null;setSelectedId(null);setData(null);setDoc(null);setPreview(null);setFrameDrafts({});setError("");setNotice("");},
    frameDrafts, rememberPosition, select, reloadList, setDoc, setFrameDraft:(k, teach) => setFrameDrafts(previous => ({ ...previous, [k]:teach })),
    act, saveDoc, saveState, retrySave: () => setSaveRetry(v => v + 1), notice, setError }}>{children}</Context.Provider>;
}

export function useWorkspace() {
  const value = useContext(Context);
  if (!value) throw new Error("WorkspaceProvider missing");
  return value;
}
