import { useEffect, useRef, useState } from "react";
import { projectApi, type ProjectEntry, type ProjectMeta } from "../../lib/project";
import { errorText } from "../../lib/errors";
import { useWorkspace } from "../workspace/context";
import type { UsageState } from "./rules";

const CHANGED = "gluesight:projects-changed";

export const notifyProjectsChanged = () => window.dispatchEvent(new Event(CHANGED));

export function onProjectsChanged(listener: () => void) {
  window.addEventListener(CHANGED, listener);
  return () => window.removeEventListener(CHANGED, listener);
}

export function useAlive() {
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  return alive;
}

/** 当前项目与本机项目列表；entries 为 null 表示列表没读到（不能据此判定项目已删除）。 */
export interface ProjectDirectory {
  current: ProjectMeta | null;
  entries: ProjectEntry[] | null;
  loading: boolean;
  error: string;
}

export function useProjectDirectory(): ProjectDirectory {
  const [state, setState] = useState<ProjectDirectory>({ current: null, entries: null, loading: true, error: "" });
  useEffect(() => {
    let serial = 0;
    const load = () => {
      const request = ++serial;
      void Promise.allSettled([projectApi.current(), projectApi.list()]).then(([current, list]) => {
        if (request !== serial) return;
        const failed = current.status === "rejected" ? current.reason : list.status === "rejected" ? list.reason : null;
        setState({
          current: current.status === "fulfilled" ? current.value : null,
          entries: list.status === "fulfilled" ? list.value : null,
          loading: false,
          error: failed == null ? "" : errorText(failed),
        });
      });
    };
    load();
    const stop = onProjectsChanged(load);
    return () => { serial++; stop(); };
  }, []);
  return state;
}

export function useProjectUsage(ids: string[], version = 0): Record<string, UsageState> {
  const key = ids.join("\n");
  const [states, setStates] = useState<Record<string, UsageState>>({});
  useEffect(() => {
    let alive = true;
    const list = key ? key.split("\n") : [];
    setStates(previous => Object.fromEntries(list.map(id => [id, previous[id] ?? { status: "loading" }])));
    void (async () => {
      for (const id of list) {
        if (!alive) return;
        try {
          const usage = await projectApi.usage(id);
          if (alive) setStates(previous => ({ ...previous, [id]: { status: "ok", usage } }));
        } catch (error) {
          if (alive) setStates(previous => ({ ...previous, [id]: { status: "error", error: errorText(error) } }));
        }
      }
    })();
    return () => { alive = false; };
  }, [key, version]);
  return states;
}

export function useUnsavedCandidate(): string {
  const { dirty, frameDirty, navigationDirty, saveState } = useWorkspace();
  if (navigationDirty) return "工件导航图有未保存修改，请回到分段页保存或撤销后再切换项目。";
  if (saveState === "failed") return "候选配置保存失败。请回到配方页重试保存后再切换项目。";
  if (dirty || frameDirty || saveState !== "saved") return "候选配置有尚未保存的编辑。请等待保存完成后再切换项目。";
  return "";
}

export function useProjectSwitch() {
  const [restarting, setRestarting] = useState(false);
  const [restartMessage, setRestartMessage] = useState("");
  const [error, setError] = useState("");
  const pending = useRef(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const run = async (id: string) => {
    if (pending.current) return false;
    pending.current = true;
    setError("");
    setRestarting(true);
    try {
      const result = await projectApi.switchTo(id);
      if (alive.current && result.restartRequired) {
        setRestartMessage(`已将下次启动项目设为 ${result.nextProject}，当前仍运行 ${result.currentProject}。请停止当前开发命令，再用原来的 pnpm dev 或 pnpm dev:joint 命令启动，随后核对当前项目。`);
      }
      return true;
    } catch (e) {
      pending.current = false;
      if (alive.current) { setRestarting(false); setError(errorText(e)); }
      return false;
    }
  };
  return { restarting, restartMessage, error, run };
}
