import { invoke, isTauri } from "@tauri-apps/api/core";
import type { HistoryDeleted, HistoryPackageExported, HistoryPackageImported, HistoryPackagePreview, HistoryPage, HistoryQuery, PartDetail, RejudgeRequest, RejudgeResult } from "./types";
import { desktopCall } from "../../lib/desktop";

function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: () => T): Promise<T> {
  if (!isTauri()) return Promise.resolve(fallback());
  return invoke<T>(cmd, args);
}

export const historyApi = {
  setTags: (ids: number[], tags: string[]) => desktopCall<string[]>("history_set_tags", { ids, tags }),
  delete: (ids: number[]) => desktopCall<HistoryDeleted>("history_delete", { ids }),
  query: (query: HistoryQuery) =>
    call<HistoryPage>("history_query", { query }, () => ({ total: 0, counts: { ok: 0, excursion: 0, ng: 0, err: 0 }, items: [] })),
  detail: (id: number) => call<PartDetail>("history_detail", { id }, () => Promise.reject("浏览器预览模式") as never),
  overview: (id: number) => desktopCall<PartDetail>("history_overview", { id }),
  frameDetail: (id: number, camera: number, seq: number) => desktopCall<PartDetail>("history_frame_detail", { id, camera, seq }),
  neighbors: (id: number, query: HistoryQuery | null) => call<{ older: number | null; newer: number | null }>("history_neighbors", { id, query }, () => ({ older: null, newer: null })),
  rejudge: (request: RejudgeRequest) =>
    desktopCall<RejudgeResult>("history_rejudge", { request }),
  exportCsv: (query: HistoryQuery) => desktopCall<string>("history_export", { query }),
  reveal: (path: string) => desktopCall<void>("reveal_path", { path }),
  packageTarget: (count: number) => desktopCall<string | null>("history_package_target", { count }),
  packageExport: (ids: number[], includeImages: boolean, path: string, taskId: string) =>
    desktopCall<HistoryPackageExported>("history_package_export", { ids, includeImages, path, taskId }),
  packagePick: () => desktopCall<HistoryPackagePreview | null>("history_package_pick"),
  packageImport: (path: string, taskId: string) => desktopCall<HistoryPackageImported>("history_package_import", { path, taskId }),
};
