import { invoke } from "@tauri-apps/api/core";
import { desktopAvailable } from "./desktop";

export interface StorageStatus {
  environment: string;
  root: string;
  logRoot: string | null;
  runtimeLogDir: string | null;
  visionLogDir: string | null;
}
export interface StoragePreferences {
  flyshotSpeedMmS?: number | null;
  flyshotSpacingMm?: number | null;
  lastWorkspace?: string | null;
}
let queue = Promise.resolve();
function serial<T>(operation: () => Promise<T>): Promise<T> {
  const next = queue.then(operation);
  queue = next.then(() => undefined, () => undefined);
  return next;
}
async function readPreferences(): Promise<StoragePreferences> {
  if (!desktopAvailable()) return {};
  return invoke<StoragePreferences>("storage_preferences_get");
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
