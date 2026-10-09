import { createContext, useContext, type Dispatch } from "react";
import type { Action, View, WorkflowState } from "./model";
export interface WorkflowContextValue {
  state: WorkflowState;
  dispatch: Dispatch<Action>;
  go: (view: View) => void;
  notify: (message: string) => void;
}
export const WorkflowContext = createContext<WorkflowContextValue | null>(null);
export function useWorkflow(): WorkflowContextValue {
  const value = useContext(WorkflowContext);
  if (!value) throw new Error("操作流程上下文缺失");
  return value;
}
