import { invoke } from "@tauri-apps/api/core";

export interface StorageStatus {
  environment: string;
  root: string;
  logRoot: string | null;
  runtimeLogDir: string | null;
  visionLogDir: string | null;
}
export interface RecordingDisk {
  path: string;
  /** 所在盘剩余空间；盘离线或查询失败为 null */
  freeBytes: number | null;
  writable: boolean;
  error: string | null;
  /** 新件现在写到这块盘 */
  current: boolean;
}
export interface RecordingDisks {
  /** 每块录制盘固定保留的剩余空间 */
  reserveBytes: number;
  /** 按写入优先顺序 */
  disks: RecordingDisk[];
}
export const storageApi = {
  status: () => invoke<StorageStatus>("storage_status"),
  openDirectory: (target: "data" | "logs" | "runtime" | "vision" | "lyflow" = "data") => invoke<void>("storage_open_directory", { target }),
  recordingDisks: () => invoke<RecordingDisks>("storage_recording_disks"),
  saveRecordingDisks: (paths: string[]) => invoke<RecordingDisks>("storage_recording_disks_save", { paths }),
};
