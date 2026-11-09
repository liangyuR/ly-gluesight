import { errorText } from "../../lib/errors";
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode, type Dispatch, type SetStateAction } from "react";
import { cameraApi, type CameraConfig } from "../camera";
import { recipeApi, type UnavailableRecipe } from "../cycle/api";
import type { Recipe, RecipeDoc, RecipeSummary } from "../cycle/types";
import { subscribe } from "../plc";
import { desktopAvailable } from "../../lib/desktop";
import { workspaceApi } from "./api";
import type { CameraTeachDraft } from "../plan/types";
import type { NavigationMapData } from "./navigation/types";
import type { Workspace, WorkspaceView } from "./types";
import { corridorStationError, draftKey, sameCorridor, savedDraft, withCorridor } from "./teach";
import { useSnackbar, type SnackbarMessage } from "qing-steel";
import type { WorkbenchDraft } from "./workbenchDraft";
import "./workspace.css";

export interface SegmentDraftEdit { roundId: string; draft: WorkbenchDraft }

export type SnackMessage = string | SnackbarMessage;
export interface NavigationDraft { map: NavigationMapData; revision: number }

interface WorkspaceContextValue {
  list: RecipeSummary[]; drafts: Workspace[]; cameras: CameraConfig[]; unavailable: UnavailableRecipe[];
  selectedId: string | null; data: WorkspaceView | null; doc: RecipeDoc | null; preview: Recipe | null;
  /** busy：显式操作或自动保存进行中；saving：其中的自动保存（期间可以继续编辑，改动并入下一次保存）；acting：只算显式操作 */
  previewError: string; error: string; busy: boolean; saving: boolean; acting: boolean; dirty: boolean; frameDirty: boolean;
  /** 有改动等着自动保存（含正在保存）；unsavable：有走廊草稿按站距不足三站，不会自动保存 */
  pendingSave: boolean; unsavable: boolean;
  /** 跳过防抖，马上保存待存的改动 */
  saveNow: () => void;
  segmentDraft: SegmentDraftEdit | null; segmentDirty: boolean;
  setSegmentDraft: Dispatch<SetStateAction<SegmentDraftEdit | null>>;
  navigationDraft: NavigationDraft | null; navigationDirty: boolean;
  setNavigationDraft: (draft: NavigationDraft | null) => void;
  /** 走廊示教里尚未保存的段走廊草稿，按“相机#段号”；像素当量与检测参数随相机，只读 */
  corridorDrafts: Record<string, CameraTeachDraft>;
  select: (id: string) => Promise<WorkspaceView | null>; clearSelection:()=>void; reloadList: () => Promise<void>;
  /** 放弃当前配方未保存的配置、走廊草稿与导航图草稿，回到候选已保存的内容；操作或保存进行中时不放弃 */
  discardDrafts: () => boolean;
  setDoc: (doc: RecipeDoc) => void;
  setCorridorDraft: (key: string, draft: CameraTeachDraft) => void;
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
  const [corridorDrafts, setCorridorDrafts] = useState<Record<string, CameraTeachDraft>>({});
  const [segmentDraft, setSegmentDraft] = useState<SegmentDraftEdit | null>(null);
  const segmentDirty = segmentDraft != null;
  const [navigationDraft, setNavigationDraft] = useState<NavigationDraft | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [saveState, setSaveState] = useState<"saved" | "saving" | "failed" | "pending">("saved");
  const [saveRetry, setSaveRetry] = useState(0);
  const [urgent, setUrgent] = useState(false);
  const failedSave = useRef("");
  const requestSerial = useRef(0);
  const acting = useRef(false);
  const selected = useRef<string | null>(null);
  const dirtyRef = useRef(false);
  const navigationDirtyRef = useRef(false);
  const segmentDirtyRef = useRef(false);
  const pendingRefresh = useRef<string | null>(null);
  const [refreshRevision, setRefreshRevision] = useState(0);
  const { show: snack, clear: clearSnack } = useSnackbar();
  const cameraList=useRef("");
  const cameraSources=useRef<Record<string, string>>({});
  const dirty = !!doc && !!data && !equal(doc, data.workspace.doc);
  const latestDoc = useRef(doc); latestDoc.current = doc;
  // 草稿与候选里已保存的走廊比较（按 f32），不同才算未保存
  const savedCorridor = (key: string) => savedDraft(data?.workspace.doc, key);
  const frameDirty = Object.keys(corridorDrafts).some(key => !sameCorridor(corridorDrafts[key], savedCorridor(key)));
  const navigationDirty = !!navigationDraft && !equal(navigationDraft.map, data?.navigation?.map);
  // 按站距不足三站的走廊草稿不自动保存（lyFlow 会把有胶的帧测成无胶），原因在示教页显示
  const changedDrafts = Object.entries(corridorDrafts).filter(([key, draft]) => !sameCorridor(draft, savedCorridor(key)));
  const pendingCorridor = doc ? changedDrafts.find(([, draft]) => !corridorStationError(draft, doc.spacing)) : undefined;
  const unsavable = !!doc && changedDrafts.some(([, draft]) => !!corridorStationError(draft, doc.spacing));
  const pendingSave = dirty || saving || !!pendingCorridor;
  navigationDirtyRef.current = navigationDirty;
  segmentDirtyRef.current = segmentDirty;
  dirtyRef.current = dirty || frameDirty || navigationDirty || segmentDirty;
  useEffect(() => {
    if (!navigationDirty && !segmentDirty) return;
    const preventClose = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ""; };
    window.addEventListener("beforeunload", preventClose);
    return () => window.removeEventListener("beforeunload", preventClose);
  }, [navigationDirty, segmentDirty]);

  /** keepDoc：自动保存期间本地又改了配置，只换上新修订与计划，本地配置留给下一次保存 */
  const accept = useCallback((next: WorkspaceView, keepDoc = false) => {
    // 段走廊草稿按“相机#段号”存，换了配方或相机组变化时清空，免得走廊落到别的相机上
    const inspect=next.workspace.doc.inspect??[];
    const cameraKey=JSON.stringify([next.workspace.doc.id,inspect.map(item=>item.camera)]);
    const camerasChanged=cameraKey!==cameraList.current;cameraList.current=cameraKey;
    const previousCameraSources=cameraSources.current;
    // 某段走廊换了示教帧：旧草稿是在旧帧上画的，丢弃
    const cameraSourceKeys=Object.fromEntries(inspect.flatMap(item=>{
      const teaching=next.plan.cameraTeaching.find(t=>t.camera===item.camera);
      return (teaching?.segments??[]).map(s=>[draftKey(item.camera,s.segment),s.frame.id]);
    }));
    cameraSources.current=cameraSourceKeys;
    setData(next);
    if (!keepDoc) {
      setDoc(next.workspace.doc);
      setPreview(next.layout);
      setPreviewError("");
    }
    setCorridorDrafts(previous => {
      if (camerasChanged) return {};
      const kept: Record<string, CameraTeachDraft> = {};
      for (const [key, draft] of Object.entries(previous)) {
        if(previousCameraSources[key]!==cameraSourceKeys[key])continue;
        const saved = savedDraft(next.workspace.doc, key);
        // 像素当量与测量区间跟着相机（含选帧后后端补上的设备标定当量）
        const merged = { ...draft, mmPerPx: saved.mmPerPx, window: saved.window };
        if (!sameCorridor(merged, saved)) kept[key] = merged;
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
    if (dirtyRef.current && (selected.current !== id || navigationDirtyRef.current || segmentDirtyRef.current)) { setError("当前草稿尚未保存，请先保存或撤销修改后再选择配方"); return null; }
    pendingRefresh.current = null;
    const serial = ++requestSerial.current;
    selected.current = id;
    setSelectedId(id); setNavigationDraft(null); setSegmentDraft(null);
    setError(""); clearSnack(); setData(null); setDoc(null); setPreview(null); setCorridorDrafts({});
    try {
      const next = await workspaceApi.get(id);
      if (serial === requestSerial.current) {
        if(next.workspace.doc.id!==id)throw new Error("候选响应与所选配方不一致，请重试选择");
        accept(next);
        return next;
      }
    } catch (e) { if (serial === requestSerial.current) setError(errorText(e)); }
    return null;
  }, [accept, clearSnack]);

  useEffect(() => {
    if (!desktopAvailable()) return;
    let alive = true;
    // 不自动打开任何配方：打开哪个由配方页的地址决定（/recipe/<编号>/…）
    reloadList().catch(e => { if (alive) setError(errorText(e)); });
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
    if (!id || id !== selectedId || dirty || frameDirty || navigationDirty || segmentDirty || busy || saving || !desktopAvailable()) return;
    let alive = true;
    const serial = ++requestSerial.current;
    const current = () => alive && serial === requestSerial.current && id === selected.current && !dirtyRef.current && !acting.current && !savingRef.current;
    void workspaceApi.get(id).then(next => {
      if (!current()) return;
      if (next.workspace.doc.id !== id) throw new Error("候选响应与所选配方不一致，请重试选择");
      pendingRefresh.current = null;
      accept(next);
    }).catch(e => { if (current()) setError(errorText(e)); });
    return () => { alive = false; };
  }, [refreshRevision, selectedId, dirty, frameDirty, navigationDirty, segmentDirty, busy, saving, accept]);

  useEffect(() => {
    if (!doc || !desktopAvailable() || equal(doc, data?.workspace.doc)) return;
    let alive = true;
    const timer = setTimeout(() => recipeApi.preview(doc).then(r => {
      if (alive) { setPreview(r); setPreviewError(""); }
    }).catch(e => { if (alive) setPreviewError(errorText(e)); }), 250);
    return () => { alive = false; clearTimeout(timer); };
  }, [doc, data?.workspace.doc]);

  const act = useCallback(async (request: () => Promise<WorkspaceView>, message: SnackMessage | ((next: WorkspaceView) => SnackMessage) = "") => {
    if (acting.current || savingRef.current) return null;
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

  const saveDoc = useCallback((draft?: RecipeDoc) => {
    const current = draft ?? doc;
    if (!current || !data) return Promise.resolve(null);
    return act(() => workspaceApi.saveDoc(current.id, data.workspace.revision, current), "候选配置已保存，生产版本保持不变");
  }, [doc, data, act]);

  // 自动保存不经 act：不锁编辑、不重读配方列表；保存期间的新改动在结果回来后保留，再保存一次
  useEffect(() => {
    if (!data || !doc || busy || saving || !desktopAvailable()) return;
    const corridor = pendingCorridor;
    if (!dirty && !corridor) { setUrgent(false); setSaveState(frameDirty ? "pending" : "saved"); return; }
    const signature = JSON.stringify([doc, corridor, saveRetry]);
    if (failedSave.current === signature) return;
    setSaveState("pending");
    const timer = setTimeout(async () => {
      if (acting.current) return;
      setUrgent(false);
      // 段走廊草稿并入候选配方这台相机的 inspect.segments，整份经 workspace_save_doc 提交（没有单独的草稿命令）
      const sent = dirty ? doc : withCorridor(data.workspace.doc, corridor![0], corridor![1]);
      const id = doc.id, serial = requestSerial.current;
      const current = () => id === selected.current && serial === requestSerial.current;
      savingRef.current = true; setSaving(true); setSaveState("saving");
      try {
        const next = await workspaceApi.saveDoc(id, data.workspace.revision, sent);
        if (!current()) return;
        if (next.workspace.doc.id !== id) throw new Error("候选响应与当前配方不一致，请重试保存");
        failedSave.current = "";
        // 保存期间又改了配方：留住本地改动；只存走廊时本地配方还没有这条走廊，补进去，下一轮一起保存
        const edited = !equal(latestDoc.current, doc);
        accept(next, edited);
        if (edited && !dirty) setDoc(previous => previous && withCorridor(previous, corridor![0], corridor![1]));
        setError("");
      } catch (e) {
        if (current()) { failedSave.current = signature; setSaveState("failed"); setError(errorText(e)); }
      } finally { savingRef.current = false; setSaving(false); }
    }, urgent ? 0 : 900);
    return () => clearTimeout(timer);
  }, [doc, data, corridorDrafts, dirty, frameDirty, busy, saving, saveRetry, urgent, accept]);

  return <Context.Provider value={{ list, drafts, cameras, unavailable, selectedId, data, doc, preview, previewError, error: error || recipeErrors, busy: busy || saving, saving, acting: busy, pendingSave, unsavable, saveNow: () => setUrgent(true), dirty, frameDirty, navigationDraft, navigationDirty, setNavigationDraft, segmentDraft, segmentDirty, setSegmentDraft,
    clearSelection:()=>{if(dirtyRef.current){setError("当前草稿尚未保存，请先保存或撤销修改");return;}pendingRefresh.current=null;requestSerial.current++;selected.current=null;setSelectedId(null);setData(null);setDoc(null);setPreview(null);setCorridorDrafts({});setNavigationDraft(null);setSegmentDraft(null);setError("");clearSnack();},
    discardDrafts:()=>{if(acting.current||savingRef.current)return false;dirtyRef.current=false;navigationDirtyRef.current=false;segmentDirtyRef.current=false;failedSave.current="";setDoc(data?.workspace.doc??null);setPreview(data?.layout??null);setPreviewError("");setCorridorDrafts({});setNavigationDraft(null);setSegmentDraft(null);setError("");return true;},
    corridorDrafts, select, reloadList, setDoc,
    setCorridorDraft:(key, draft) => setCorridorDrafts(previous => ({ ...previous, [key]:draft })),
    act, saveDoc, saveState, retrySave: () => setSaveRetry(v => v + 1), setError }}>{children}</Context.Provider>;
}

export function useWorkspace() {
  const value = useContext(Context);
  if (!value) throw new Error("WorkspaceProvider missing");
  return value;
}
