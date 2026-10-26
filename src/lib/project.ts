import { invoke } from "@tauri-apps/api/core";

export interface ProjectMeta {
  schema: number;
  id: string;
  name: string;
  description: string;
  createdAt: number;
  createdBy: string;
  copiedFrom: string | null;
}

export interface ProjectEntry {
  id: string;
  meta: ProjectMeta | null;
  error: string | null;
  current: boolean;
}

export interface ProjectUsage {
  recipes: number;
  workspaces: number;
  captureRounds: number;
  records: number;
  firstRecordAt: number | null;
  lastRecordAt: number | null;
  bytes: number;
}

export interface CreateProject {
  id: string;
  name: string;
  description: string;
  copyHardware: boolean;
}

export const PROJECT_ID_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

export const projectApi = {
  current: () => invoke<ProjectMeta>("project_current"),
  list: () => invoke<ProjectEntry[]>("project_list"),
  usage: (id: string) => invoke<ProjectUsage>("project_usage", { id }),
  create: (request: CreateProject) => invoke<ProjectMeta>("project_create", { request }),
  rename: (id: string, name: string, description: string) => invoke<ProjectMeta>("project_rename", { id, name, description }),
  remove: (id: string) => invoke<void>("project_delete", { id }),
  switchTo: (id: string) => invoke<void>("project_switch", { id }),
};

export interface ProjectExported {
  path: string;
  bytes: number;
  files: number;
  includesRecords: boolean;
  notes: string[];
}

export interface ProjectImportPreview {
  path: string;
  sourceId: string;
  name: string;
  description: string;
  exportedAt: number;
  appVersion: string;
  includesRecords: boolean;
  files: number;
  bytes: number;
  notes: string[];
}

export interface ProjectImported {
  meta: ProjectMeta;
  notes: string[];
}

export interface CopyProject {
  sourceId: string;
  id: string;
  name: string;
  description: string;
  includeRecords: boolean;
}

export const projectTransferApi = {
  exportProject: (id: string, includeRecords: boolean) => invoke<ProjectExported | null>("project_export", { id, includeRecords }),
  pickImport: () => invoke<ProjectImportPreview | null>("project_import_pick"),
  importProject: (path: string, id: string, name: string) => invoke<ProjectImported>("project_import", { path, id, name }),
  copyProject: (request: CopyProject) => invoke<ProjectImported>("project_copy", { request }),
};
