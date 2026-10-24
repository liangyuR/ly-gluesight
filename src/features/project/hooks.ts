import { useEffect, useRef, useState } from "react";
import { projectApi } from "../../lib/project";
import { errorText } from "../../lib/errors";
import { useWorkspace } from "../workspace/context";
import type { UsageState } from "./rules";

const CHANGED = "gluesight:projects-changed";

export const notifyProjectsChanged = () => window.dispatchEvent(new Event(CHANGED));

export function onProjectsChanged(listener: () => void) {
  window.addEventListener(CHANGED, listener);
  return () => window.removeEventListener(CHANGED, listener);
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
  const { dirty, frameDirty, saveState } = useWorkspace();
  if (saveState === "failed") return "候选配置保存失败。请回到配方页重试保存后再切换项目。";
  if (dirty || frameDirty || saveState !== "saved") return "候选配置有尚未保存的编辑。请等待保存完成后再切换项目。";
  return "";
}

export function useProjectSwitch() {
  const [restarting, setRestarting] = useState(false);
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
      await projectApi.switchTo(id);
      return true;
    } catch (e) {
      pending.current = false;
      if (alive.current) { setRestarting(false); setError(errorText(e)); }
      return false;
    }
  };
  return { restarting, error, run };
}
