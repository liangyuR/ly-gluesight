import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { errorText } from "../../lib/errors";
import { useOperation, type Operation } from "../workspace/operation";
import { historyApi } from "./api";
import type { HistoryListView, HistoryQuery, RejudgeRequest, RejudgeResult } from "./types";

export type RejudgeScope = { query: HistoryQuery; view: HistoryListView; total: number };
export type RejudgeOptions = Pick<RejudgeRequest, "useCurrentRecipe" | "overrides">;

type RejudgeTask = { scope: RejudgeScope; options: RejudgeOptions } & (
  | { status: "running" }
  | { status: "completed"; taskId: string; result: RejudgeResult }
  | { status: "cancelled" }
  | { status: "failed"; error: string }
);

const RejudgeTaskContext = createContext<{
  task: RejudgeTask | null;
  operation: Operation;
  run: (scope: RejudgeScope, options: RejudgeOptions) => Promise<void>;
} | null>(null);

export function RejudgeTaskProvider({ children }: { children: ReactNode }) {
  const [task, setTask] = useState<RejudgeTask | null>(null);
  const operation = useOperation();
  const pending = useRef(false);
  const run = useCallback(async (scope: RejudgeScope, options: RejudgeOptions) => {
    if (pending.current) return;
    pending.current = true;
    setTask({ scope, options, status: "running" });
    try {
      const completed = await operation.run(async taskId => ({
        taskId,
        result: await historyApi.rejudge({ query: scope.query, ids: [], ...options }, taskId),
      }));
      setTask({ scope, options, status: "completed", ...completed });
    } catch (cause) {
      if (operation.snapshot()?.progress.stage === "cancelled") setTask({ scope, options, status: "cancelled" });
      else setTask({ scope, options, status: "failed", error: errorText(cause) });
    } finally {
      pending.current = false;
    }
  }, [operation]);
  return <RejudgeTaskContext.Provider value={{ task, operation, run }}>{children}</RejudgeTaskContext.Provider>;
}

export function useRejudgeTask() {
  const context = useContext(RejudgeTaskContext);
  if (!context) throw new Error("RejudgeTaskProvider is required");
  return context;
}
