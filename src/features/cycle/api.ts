import { invoke, isTauri } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { subscribe } from "../plc";
import type { CycleSettings, ImportedPath, LogLine, Measured, Recipe, RecipeDoc, RecipeSummary, Scenario, SimStatus, Snapshot } from "./types";

function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!isTauri() && ["cycle_save_settings","cycle_select_recipe","cycle_reset","sim_start","sim_stop","recipe_save","recipe_delete"].includes(cmd))
    return Promise.reject(new Error("生产配置与检测操作需要 GlueSight · 胶路智检 桌面后端"));
  if (!isTauri()) return Promise.resolve(fallback());
  return invoke<T>(cmd, args);
}

const defaultSettings: CycleSettings = {
  productSource: "plc",
  manualRecipeId: null,
  timeouts: { armMs: 200, motionMs: 30000, drainMs: 1000, procMs: 3000, ackMs: 5000 },
  historyDays: 180,
  lyflowCore: null,
  vision: false,
  record: "off",
  recordKeep: 100,
  recordMaxGb: 20,
};

export const cycleApi = {
  snapshot: () => call<Snapshot | null>("cycle_snapshot", undefined, () => null),
  logs: () => call<LogLine[]>("cycle_logs", undefined, () => []),
  partData: () => call<Measured[]>("cycle_part_data", undefined, () => []),
  recipes: () => call<RecipeSummary[]>("cycle_recipes", undefined, () => []),
  layout: (recipeId: string, hash?: string) => call<Recipe | null>("cycle_layout", { recipeId, hash: hash ?? null }, () => null),
  getSettings: () => call<CycleSettings>("cycle_get_settings", undefined, () => structuredClone(defaultSettings)),
  saveSettings: (settings: CycleSettings) => call<void>("cycle_save_settings", { settings }, () => undefined),
  selectRecipe: (recipeId: string) => call<void>("cycle_select_recipe", { recipeId }, () => undefined),
  reset: () => call<void>("cycle_reset", undefined, () => undefined),
  simStatus: () => call<SimStatus>("sim_status", undefined, () => ({ running: false, continuous: false, parts: 0, message: "" })),
  simStart: (recipeId: string, scenario: Scenario, continuous: boolean) =>
    call<void>("sim_start", { recipeId, scenario, continuous }, () => undefined),
  simStop: () => call<void>("sim_stop", undefined, () => undefined),
};

export const recipeApi = {
  list: () => call<{ recipes: RecipeSummary[]; errors: string[] }>("recipe_list", undefined, () => ({ recipes: [], errors: [] })),
  doc: (id: string) => call<RecipeDoc>("recipe_doc", { id }, () => Promise.reject("非桌面环境") as never),
  preview: (doc: RecipeDoc) => call<Recipe>("recipe_preview", { doc }, () => Promise.reject("非桌面环境") as never),
  template: () => call<RecipeDoc>("recipe_template", undefined, () => Promise.reject("非桌面环境") as never),
  save: (doc: RecipeDoc, originalId: string | null) =>
    call<RecipeSummary>("recipe_save", { doc, originalId }, () => Promise.reject("非桌面环境") as never).then((r) => {
      layoutCache.clear();
      notifyRecipes();
      return r;
    }),
  remove: (id: string) =>
    call<void>("recipe_delete", { id }, () => undefined).then(() => {
      layoutCache.clear();
      notifyRecipes();
    }),
  parsePath: (text: string, fileName: string) =>
    call<ImportedPath>("recipe_parse_path", { text, fileName }, () => ({ points: [], bulges: [], closed: false, note: null })),
};

const layoutCache = new Map<string, Promise<Recipe | null>>();
const recipeListeners = new Set<() => void>();
function notifyRecipes() {
  recipeListeners.forEach((f) => f());
}

/**
 * 配方的运行数据。给了 hash 就要那一版（工件用的配方刚改过，这一件仍按开工时的样子画）。
 * 换了配方还没取回来时返回 null，不拿上一个配方的数据去对新工件的测量点；keepPrevious 时同一配方的新版取回来之前先给旧版。
 */
export function useLayout(recipeId: string | null | undefined, hash?: string, keepPrevious = false) {
  const [state, setState] = useState<{ key: string; id: string; layout: Recipe | null } | null>(null);
  const [gen, setGen] = useState(0);
  const key = recipeId ? `${recipeId}:${hash ?? ""}` : "";
  useEffect(() => {
    const f = () => setGen((g) => g + 1);
    recipeListeners.add(f);
    return () => void recipeListeners.delete(f);
  }, []);
  useEffect(() => {
    if (!recipeId) return;
    if (!layoutCache.has(key))
      layoutCache.set(
        key,
        cycleApi.layout(recipeId, hash).catch(() => {
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
    recipeListeners.add(load);
    return () => void recipeListeners.delete(load);
  }, []);
  return recipes;
}

export function useCycle() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [measured, setMeasured] = useState<Measured[]>([]);

  useEffect(() => {
    cycleApi.snapshot().then((s) => s && setSnapshot(s));
    cycleApi.logs().then(setLogs);
    cycleApi.partData().then(setMeasured);
    const offs = [
      subscribe<Snapshot>("cycle://snapshot", setSnapshot),
      subscribe<LogLine>("cycle://log", (line) => setLogs((prev) => [...prev.slice(-199), line])),
      subscribe<Measured>("cycle://frame", (m) =>
        setMeasured((prev) => (prev.length && prev[0].sn !== m.sn ? [m] : [...prev, m])),
      ),
    ];
    return () => offs.forEach((off) => off());
  }, []);

  const sn = snapshot?.part?.sn;
  const current = useMemo(() => (sn === undefined ? [] : measured.filter((m) => m.sn === sn)), [measured, sn]);
  return { snapshot, logs, measured: current };
}

export function useSimStatus() {
  const [status, setStatus] = useState<SimStatus | null>(null);
  useEffect(() => {
    cycleApi.simStatus().then(setStatus);
    return subscribe<SimStatus>("sim://status", setStatus);
  }, []);
  return status;
}
