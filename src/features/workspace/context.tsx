import { storageApi } from "../../lib/storage";
import { errorText } from "../../lib/errors";
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { cameraApi, type CameraConfig } from "../camera";
import { recipeApi, type UnavailableRecipe } from "../cycle/api";
import type { Recipe, RecipeDoc, RecipeSummary } from "../cycle/types";
import { subscribe } from "../plc";
import { desktopAvailable } from "../../lib/desktop";
import { workspaceApi } from "./api";
import type { CameraTeachDraft } from "../plan/types";
import type { Workspace, WorkspaceView } from "./types";
import { corridorDraft, sameCorridor, withCorridor } from "./teach";
import { useSnackbar, type SnackbarMessage } from "qing-steel";
import "./workspace.css";

export type SnackMessage = string | SnackbarMessage;

interface WorkspaceContextValue {
  list: RecipeSummary[]; drafts: Workspace[]; cameras: CameraConfig[]; unavailable: UnavailableRecipe[];
  selectedId: string | null; data: WorkspaceView | null; doc: RecipeDoc | null; preview: Recipe | null;
  previewError: string; error: string; busy: boolean; dirty: boolean; frameDirty: boolean;
  /** 走廊示教里尚未保存的草稿（走廊、像素当量、检测参数），按相机 id */
  frameDrafts: Record<string, CameraTeachDraft>;
  rememberPosition: (k: number, view: number) => void;
  select: (id: string) => Promise<WorkspaceView | null>; clearSelection:()=>void; reloadList: () => Promise<void>;
  setDoc: (doc: RecipeDoc) => void; setFrameDraft: (camera: string, draft: CameraTeachDraft) => void;
  act: (request: () => Promise<WorkspaceView>, message?: SnackMessage | ((next: WorkspaceView) => SnackMessage)) => Promise<WorkspaceView | null>;
  saveDoc: (doc?: RecipeDoc) => Promise<WorkspaceView | null>;
  saveState: "saved" | "saving" | "failed" | "pending"; retrySave: () => void;
  setError: (message: string) => void;
}
const Context = createContext<WorkspaceContextValue | null>(null);
const equal = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [list, setList] = useState<RecipeSummary[]>([]);
  const [unavailable, setUnavailable] = useState<UnavailableRecipe[]>([]);
  const [recipeErrors, setRecipeErrors] = useState("");
  const [drafts, setDrafts] = useState<Workspace[]>([]);
  const [cameras, setCameras] = useState<CameraConfig[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [data, setData] = useState<WorkspaceView | null>(null);
  const [doc, setDoc] = useState<RecipeDoc | null>(null);
  const [preview, setPreview] = useState<Recipe | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [frameDrafts, setFrameDrafts] = useState<Record<string, CameraTeachDraft>>({});
  const [error, setError] = useState("");
  const [preferenceError, setPreferenceError] = useState("");
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
  const { show: snack, clear: clearSnack } = useSnackbar();
  const cameraList=useRef("");
  const cameraSources=useRef<Record<string, string>>({});
  const dirty = !!doc && !!data && !equal(doc, data.workspace.doc);
  // 草稿与候选里已保存的走廊比较（按 f32），不同才算未保存
  const savedCorridor = (camera: string) => corridorDraft(data?.workspace.doc.inspect?.find(item => item.camera === camera));
  const frameDirty = Object.keys(frameDrafts).some(camera => !sameCorridor(frameDrafts[camera], savedCorridor(camera)));
  dirtyRef.current = dirty || frameDirty;

  const accept = useCallback((next: WorkspaceView) => {
    // 草稿按相机 id 存：换了配方或相机组变化时清空，免得走廊落到别的相机上
    const inspect=next.workspace.doc.inspect??[];
    const key=JSON.stringify([next.workspace.doc.id,inspect.map(item=>item.camera)]);
    const changed=key!==cameraList.current;cameraList.current=key;
    const previousSources=cameraSources.current;
    // 这台相机换了示教帧或标定：旧草稿是在旧帧上画的，丢弃
    const sources=Object.fromEntries(inspect.map(item=>[item.camera,JSON.stringify([item.calib,next.plan?.cameraTeaching.find(t=>t.camera===item.camera)?.frame?.id??null])]));
    cameraSources.current=sources;
    setData(next);
    setDoc(next.workspace.doc);
    setPreview(next.layout);
    setPreviewError("");
    setFrameDrafts(previous => {
      if (changed) return {};
      const kept: Record<string, CameraTeachDraft> = {};
      for (const [camera, draft] of Object.entries(previous)) {
        if(previousSources[camera]!==sources[camera])continue;
        const saved = corridorDraft(inspect.find(item => item.camera === camera));
        // 取样后后端补上了像素当量：草稿还没有时跟着用
        const merged = !Number.isFinite(draft.mmPerPx) && Number.isFinite(saved.mmPerPx) ? { ...draft, mmPerPx: saved.mmPerPx } : draft;
        if (!sameCorridor(merged, saved)) kept[camera] = merged;
      }
      return kept;
    });
  }, []);

  const reloadList = useCallback(async () => {
    if (!desktopAvailable()) return;
    const [recipes, workspaces, configs] = await Promise.all([recipeApi.list(), workspaceApi.list(), cameraApi.rigConfig()]);
    setList(recipes.recipes); setDrafts(workspaces); setCameras(configs);
    setUnavailable(recipes.unavailable ?? []);
    setRecipeErrors(recipes.errors.join("；"));
  }, []);

  const select = useCallback(async (id: string) => {
    if (dirtyRef.current && selected.current !== id) { setError("当前草稿尚未保存，请等待保存完成或重试后再切换配方"); return null; }
    pendingRefresh.current = null;
    const serial = ++requestSerial.current;
    selected.current = id;
    setSelectedId(id);
    setError(""); clearSnack(); setData(null); setDoc(null); setPreview(null); setFrameDrafts({});
    try {
      const next = await workspaceApi.get(id);
      if (serial === requestSerial.current) {
        if(next.workspace.doc.id!==id)throw new Error("候选响应与所选配方不一致，请重试选择");
        accept(next);
        void storageApi.updatePreferences({ lastWorkspace: id }).then(() => { if (serial === requestSerial.current) setPreferenceError(""); }).catch(e => { if (serial === requestSerial.current) setPreferenceError(`工作区偏好未保存：${errorText(e)}`); });
        return next;
      }
    } catch (e) { if (serial === requestSerial.current) setError(errorText(e)); }
    return null;
  }, [accept, clearSnack]);

  useEffect(() => {
    if (!desktopAvailable()) return;
    let alive = true;
    reloadList().then(async () => {
      const records = await recipeApi.list();
      const drafts = await workspaceApi.list();
      let preferred: string | null = null;
      try { preferred = (await storageApi.preferences()).lastWorkspace ?? null; }
      catch (e) { if (alive) setPreferenceError(`工作区偏好读取失败：${errorText(e)}`); }
      const ids = [...records.recipes.map(r => r.id), ...drafts.map(w => w.doc.id)];
      if (alive && !selected.current && ids.length) await select(preferred && ids.includes(preferred) ? preferred : ids[0]);
    }).catch(e => setError(errorText(e)));
    const refresh = (id: string) => {
      void reloadList().catch(e => setError(errorText(e)));
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
    }).catch(e => { if (current()) setError(errorText(e)); });
    return () => { alive = false; };
  }, [refreshRevision, selectedId, dirty, frameDirty, busy, accept]);

  useEffect(() => {
    if (!doc || !desktopAvailable() || equal(doc, data?.workspace.doc)) return;
    let alive = true;
    const timer = setTimeout(() => recipeApi.preview(doc).then(r => {
      if (alive) { setPreview(r); setPreviewError(""); }
    }).catch(e => { if (alive) setPreviewError(errorText(e)); }), 250);
    return () => { alive = false; clearTimeout(timer); };
  }, [doc, data?.workspace.doc]);

  const act = useCallback(async (request: () => Promise<WorkspaceView>, message: SnackMessage | ((next: WorkspaceView) => SnackMessage) = "") => {
    if (acting.current) return null;
    acting.current = true; setBusy(true); setError(""); clearSnack();
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
      catch (e) { if (current()) setError(`候选操作已完成，但配方列表刷新失败：${errorText(e)}`); }
      if (!current()) return null;
      snack(typeof message === "function" ? message(next) : message);
      return next;
    } catch (e) { if (current()) setError(errorText(e)); return null; }
    finally { acting.current = false; setBusy(false); }
  }, [accept, reloadList, snack, clearSnack]);

  const rememberPosition = useCallback((k: number, view: number) => {
    const id = selected.current;
    if (!id) return;
    void workspaceApi.progress(id,k,view).then(next => {
      if (selected.current !== id) return;
      setData(previous => previous?.workspace.doc.id === id ? {...previous,workspace:{...previous.workspace,lastPosition:next.workspace.lastPosition}} : previous);
    }).catch(e => { if (selected.current === id) setError(errorText(e)); });
  }, []);

  const saveDoc = useCallback((draft?: RecipeDoc) => {
    const current = draft ?? doc;
    if (!current || !data) return Promise.resolve(null);
    return act(() => workspaceApi.saveDoc(current.id, data.workspace.revision, current), "候选配置已保存，生产版本保持不变");
  }, [doc, data, act]);

  useEffect(() => {
    if (!data || !doc || busy || !desktopAvailable()) return;
    const entry = Object.entries(frameDrafts).find(([camera, draft]) => !sameCorridor(draft, savedCorridor(camera)));
    if (!dirty && !entry) { setSaveState("saved"); return; }
    const signature = JSON.stringify([doc, entry, saveRetry]);
    if (failedSave.current === signature) return;
    setSaveState("pending");
    const timer = setTimeout(async () => {
      setSaveState("saving");
      // 走廊草稿并入候选配方的 inspect，整份经 workspace_save_doc 提交（段配方没有单独的草稿命令）
      const result = dirty ? await act(() => workspaceApi.saveDoc(doc.id, data.workspace.revision, doc)) : await act(() => workspaceApi.saveDoc(doc.id, data.workspace.revision, withCorridor(data.workspace.doc, entry![0], entry![1])));
      if (!result) { failedSave.current = signature; setSaveState("failed"); }
      else { failedSave.current = ""; setSaveState("saved"); }
    }, 900);
    return () => clearTimeout(timer);
  }, [doc, data, frameDrafts, dirty, busy, saveRetry, act]);

  return <Context.Provider value={{ list, drafts, cameras, unavailable, selectedId, data, doc, preview, previewError, error: error || preferenceError || recipeErrors, busy, dirty, frameDirty,
    clearSelection:()=>{pendingRefresh.current=null;requestSerial.current++;selected.current=null;setSelectedId(null);setData(null);setDoc(null);setPreview(null);setFrameDrafts({});setError("");clearSnack();},
    frameDrafts, rememberPosition, select, reloadList, setDoc, setFrameDraft:(camera, draft) => setFrameDrafts(previous => ({ ...previous, [camera]:draft })),
    act, saveDoc, saveState, retrySave: () => setSaveRetry(v => v + 1), setError }}>{children}</Context.Provider>;
}

export function useWorkspace() {
  const value = useContext(Context);
  if (!value) throw new Error("WorkspaceProvider missing");
  return value;
}
