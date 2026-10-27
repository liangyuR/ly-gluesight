import { invoke } from "@tauri-apps/api/core";
import { desktopAvailable } from "./desktop";

export interface StorageStatus {
  environment: string;
  root: string;
  logRoot: string | null;
  runtimeLogDir: string | null;
  visionLogDir: string | null;
  migrationStatus: string;
  migrationMessage?: string | null;
}
export interface StoragePreferences {
  flyshotSpeedMmS?: number | null;
  flyshotSpacingMm?: number | null;
  lastWorkspace?: string | null;
  legacyImported?: boolean;
}
const keys = ["gluesight.feasibility.manual", "tujiao-last-workspace"];
let queue = Promise.resolve();
function serial<T>(operation: () => Promise<T>): Promise<T> {
  const next = queue.then(operation);
  queue = next.then(() => undefined, () => undefined);
  return next;
}
async function readPreferences(): Promise<StoragePreferences> {
  if (!desktopAvailable()) return {};
  let saved = await invoke<StoragePreferences>("storage_preferences_get");
  if (!saved.legacyImported) {
    const raw = localStorage.getItem(keys[0]);
    const lastWorkspace = localStorage.getItem(keys[1]);
    let manual: Record<string, unknown> = {};
    if (raw) {
      try { manual = JSON.parse(raw) ?? {}; } catch { throw new Error("旧飞拍参数格式无效，尚未迁移，请检查旧浏览器资料"); }
    }
    const patch: StoragePreferences = { legacyImported: true };
    if (saved.flyshotSpeedMmS == null && typeof manual.speed === "number" && Number.isFinite(manual.speed) && manual.speed > 0) patch.flyshotSpeedMmS = manual.speed;
    if (saved.flyshotSpacingMm == null && typeof manual.spacing === "number" && Number.isFinite(manual.spacing) && manual.spacing > 0) patch.flyshotSpacingMm = manual.spacing;
    if (saved.lastWorkspace == null && lastWorkspace) patch.lastWorkspace = lastWorkspace;
    saved = await invoke<StoragePreferences>("storage_preferences_update", { patch });
  }
  for (const key of keys) localStorage.removeItem(key);
  return saved;
}
export const storageApi = {
  status: () => invoke<StorageStatus>("storage_status"),
  openDirectory: (target: "data" | "logs" | "runtime" | "vision" | "lyflow" = "data") => invoke<void>("storage_open_directory", { target }),
  preferences: () => serial(readPreferences),
  updatePreferences: (patch: StoragePreferences) => serial(async () => {
    if (!desktopAvailable()) throw new Error("浏览器预览不保存资料，请使用桌面软件");
    await readPreferences();
    return invoke<StoragePreferences>("storage_preferences_update", { patch });
  }),
};
