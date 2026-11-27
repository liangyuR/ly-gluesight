import { invoke, isTauri } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { subscribe } from "../plc";
import type { CycleSettings, Readiness, Recipe, RecipeDoc, RecipeSummary, Snapshot } from "./types";

function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!isTauri() && ["cycle_save_settings","cycle_set_maintenance","cycle_select_recipe"].includes(cmd))
    return Promise.reject(new Error("生产配置与检测操作需要 GlueSight · 胶路智检 桌面后端"));
  if (!isTauri()) return Promise.resolve(fallback());
  return invoke<T>(cmd, args);
}

const defaultSettings: CycleSettings = {
  productSource: "plc",
  manualRecipeId: null,
  timeouts: { motionMs: 1_200_000, drainMs: 1000, procMs: 3000, gateGuardMs: 80 },
  historyDays: 180,
  record: "all",
  recordKeep: null,
  recordMaxGb: null,
  idleCompress: false,
};

export const cycleApi = {
  snapshot: () => call<Snapshot | null>("cycle_snapshot", undefined, () => null),
  recipes: () => call<RecipeSummary[]>("cycle_recipes", undefined, () => []),
  layout: (recipeId: string, revisionId?: string) => call<Recipe | null>("cycle_layout", { recipeId, revisionId: revisionId ?? null }, () => null),
  getSettings: () => call<CycleSettings>("cycle_get_settings", undefined, () => structuredClone(defaultSettings)),
  saveSettings: (settings: CycleSettings) => call<void>("cycle_save_settings", { settings }, () => undefined),
  /** 开启须填原因；从下一件起生效 */
  setMaintenance: (enabled: boolean, reason: string) => call<void>("cycle_set_maintenance", { enabled, reason }, () => undefined),
  selectRecipe: (recipeId: string) => call<void>("cycle_select_recipe", { recipeId }, () => undefined),
  readiness: () => call<Readiness | null>("cycle_readiness", undefined, () => null),
};

export interface UnavailableRecipe {
  fileName: string;
  id: string | null;
  name: string | null;
  schemaVersion: number | null;
  currentSchemaVersion: number;
  reason: string;
}

export const recipeApi = {
  list: () => call<{ recipes: RecipeSummary[]; errors: string[]; unavailable: UnavailableRecipe[] }>("recipe_list", undefined, () => ({ recipes: [], errors: [], unavailable: [] })),
  removeUnavailable: (fileName: string) => call<void>("recipe_delete_unavailable", { fileName }, () => Promise.reject("配方操作需要桌面后端") as never),
  doc: (id: string) => call<RecipeDoc>("recipe_doc", { id }, () => Promise.reject("非桌面环境") as never),
  preview: (doc: RecipeDoc) => call<Recipe>("recipe_preview", { doc }, () => Promise.reject("非桌面环境") as never),
  template: () => call<RecipeDoc>("recipe_template", undefined, () => Promise.reject("非桌面环境") as never),
};

const layoutCache = new Map<string, Promise<Recipe | null>>();
const recipeListeners = new Set<() => void>();
let stopRecipeEvents: (() => void) | null = null;
function notifyRecipes() {
  layoutCache.clear();
  recipeListeners.forEach((f) => f());
}
function listenRecipes(f: () => void) {
  recipeListeners.add(f);
  stopRecipeEvents ??= subscribe<string>("workspace://changed", notifyRecipes);
  return () => {
    recipeListeners.delete(f);
    if (recipeListeners.size === 0 && stopRecipeEvents) {
      stopRecipeEvents();
      stopRecipeEvents = null;
    }
  };
}

/**
 * 配方的运行数据。给了 revisionId 就要那一版（工件用的配方刚改过，这一件仍按开工时的样子画）。
 * 换了配方还没取回来时返回 null，不拿上一个配方的数据去对新工件的测量点；keepPrevious 时同一配方的新版取回来之前先给旧版。
 */
export function useLayout(recipeId: string | null | undefined, revisionId?: string, keepPrevious = false) {
  const [state, setState] = useState<{ key: string; id: string; layout: Recipe | null } | null>(null);
  const [gen, setGen] = useState(0);
  const key = recipeId ? `${recipeId}:${revisionId ?? ""}` : "";
  useEffect(() => {
    return listenRecipes(() => setGen((g) => g + 1));
  }, []);
  useEffect(() => {
    if (!recipeId) return;
    if (!layoutCache.has(key))
      layoutCache.set(
        key,
        cycleApi.layout(recipeId, revisionId).catch(() => {
          layoutCache.delete(key);
          return null;
        }),
      );
    let alive = true;
    layoutCache.get(key)!.then(
      (layout) => alive && setState((prev) => (!layout && prev?.key === key && prev.layout ? prev : { key, id: recipeId, layout })),
    );
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, gen]);
  if (!state) return null;
  return state.key === key || (keepPrevious && state.id === recipeId) ? state.layout : null;
}

export function useRecipes() {
  const [recipes, setRecipes] = useState<RecipeSummary[]>([]);
  useEffect(() => {
    const load = () => cycleApi.recipes().then(setRecipes).catch(() => setRecipes([]));
    load();
    return listenRecipes(load);
  }, []);
  return recipes;
}

export function useCycle() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const latest = useRef<Snapshot | null>(null);

  useEffect(() => {
    let alive = true, receivedSnapshot = false;
    const retired = new Set<string>();
    const update = (next: Snapshot) => {
      if (!alive || next.since < (latest.current?.since ?? -Infinity) || (next.part && retired.has(next.part.cycleId))) return;
      const previous = latest.current?.part?.cycleId;
      if (previous && previous !== next.part?.cycleId) retired.add(previous);
      if (retired.size > 128) retired.delete(retired.values().next().value!);
      latest.current = next;
      setSnapshot(next);
    };
    const off = subscribe<Snapshot>("cycle://snapshot", value => { receivedSnapshot = true; update(value); });
    void cycleApi.snapshot().then(value => { if (value && !receivedSnapshot) update(value); }).catch(() => {});
    return () => { alive = false; off(); };
  }, []);

  return { snapshot };
}

/** 开工就绪报告：定时刷新，配方库变化或 refreshKey 变化时立即重查。 */
export type ReadinessLoadStatus = "loading" | "success" | "error";

export function useReadiness(refreshKey?: string) {
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const [status, setStatus] = useState<ReadinessLoadStatus>("loading");
  const [error, setError] = useState<string | null>(null);
  const inflight = useRef<Promise<Readiness | null> | null>(null);
  const queued = useRef<Promise<Readiness | null> | null>(null);
  const alive = useRef(true);
  const start = useCallback((): Promise<Readiness | null> => {
    const run: Promise<Readiness | null> = cycleApi.readiness()
      .then(next => {
        if (alive.current) {
          setReadiness(next);
          setStatus("success");
          setError(null);
        }
        return next;
      }, reason => {
        if (alive.current) {
          setStatus("error");
          setError(reason instanceof Error ? reason.message : String(reason));
        }
        return null;
      })
      .finally(() => { if (inflight.current === run) inflight.current = null; });
    inflight.current = run;
    return run;
  }, []);
  /** 上次查询未返回时不再并发；显式刷新排在其后补查一次。 */
  const refresh = useCallback((): Promise<Readiness | null> => {
    if (!inflight.current) return start();
    queued.current ??= inflight.current.then(() => { queued.current = null; return start(); });
    return queued.current;
  }, [start]);
  useEffect(() => {
    alive.current = true;
    const timer = setInterval(() => { if (!inflight.current && !queued.current) void start(); }, 2000);
    const offRecipes = listenRecipes(() => void refresh());
    return () => { alive.current = false; clearInterval(timer); offRecipes(); };
  }, [refresh, start]);
  useEffect(() => { void refresh(); }, [refresh, refreshKey]);
  return { readiness, refresh, status, error };
}
