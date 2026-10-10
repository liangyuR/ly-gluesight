import { invoke, isTauri } from "@tauri-apps/api/core";
import type { Recipe } from "../cycle/types";
import type { HistoryPage, HistoryQuery, PartDetail, RejudgeRequest, RejudgeResult } from "./types";
import { desktopCall } from "../../lib/desktop";

function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!isTauri()) return Promise.resolve(fallback());
  return invoke<T>(cmd, args);
}

const recipeCache = new Map<string, Promise<Recipe | null>>();

export const historyApi = {
  query: (query: HistoryQuery) =>
    call<HistoryPage>("history_query", { query }, () => ({ total: 0, counts: { ok: 0, excursion: 0, ng: 0, err: 0 }, items: [] })),
  detail: (id: number) => call<PartDetail>("history_detail", { id }, () => Promise.reject("浏览器预览模式") as never),
  recipe: (revisionId: string | null, recipeId: string | null) => {
    const key = `${revisionId}|${recipeId}`;
    if (!recipeCache.has(key)) recipeCache.set(key, call<Recipe | null>("history_recipe", { revisionId, recipeId }, () => null));
    return recipeCache.get(key)!;
  },
  rejudge: (request: RejudgeRequest) =>
    desktopCall<RejudgeResult>("history_rejudge", { request }),
  exportCsv: (query: HistoryQuery) => desktopCall<string>("history_export", { query }),
  reveal: (path: string) => desktopCall<void>("reveal_path", { path }),
};
