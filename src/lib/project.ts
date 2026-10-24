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
