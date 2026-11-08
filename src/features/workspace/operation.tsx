import { useEffect, useState, useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { Button } from "qing-steel";
import { desktopAvailable, desktopCall } from "../../lib/desktop";
import { errorText } from "../../lib/errors";
import "./operation.css";

export interface OperationProgress {
  taskId: string;
  stage: "preparing" | "measuring" | "summarizing" | "saving" | "completed" | "cancelled" | "failed";
  completed: number;
  total: number;
  failed: number;
  skipped: number;
  elapsedMs: number;
}

const terminal = (stage: OperationProgress["stage"]) => ["completed", "cancelled", "failed"].includes(stage);
const labels: Record<OperationProgress["stage"], string> = {
  preparing: "正在校验数据", measuring: "正在测量", summarizing: "正在汇总判定", saving: "正在保存结果",
  completed: "已完成", cancelled: "已取消，原结果保留", failed: "未完成，原结果保留",
};
interface OperationState { progress: OperationProgress; registered: boolean; cancelling: boolean; error: string }

export class Operation {
  private state: OperationState | null = null;
  private listeners = new Set<() => void>();
  private active = false;
  private disposed = false;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  snapshot = () => this.state;
  private update(state: OperationState) { this.state = state; this.listeners.forEach(listener => listener()); }
  attach = () => { this.disposed = false; };
  dispose = () => { this.disposed = true; void this.cancel(); };
  private async finalProgress(taskId: string) {
    if (!desktopAvailable()) return;
    const progress = await desktopCall<OperationProgress | null>("operation_status", { taskId }).catch(() => null);
    if (progress?.taskId === taskId && terminal(progress.stage) && this.state?.progress.taskId === taskId) this.update({ ...this.state, progress });
  }

  async run<T>(execute: (taskId: string) => Promise<T>): Promise<T> {
    if (this.disposed) throw new Error("任务已取消，原结果保留");
    if (this.active) throw new Error("正在处理，请稍候");
    this.active = true;
    const taskId = crypto.randomUUID();
    const started = performance.now();
    this.update({ progress: { taskId, stage: "preparing", completed: 0, total: 0, failed: 0, skipped: 0, elapsedMs: 0 }, registered: false, cancelling: false, error: "" });
    let stop: (() => void) | undefined;
    let submitted = false;
    try {
      if (desktopAvailable()) stop = await listen<OperationProgress>("operation://progress", ({ payload }) => {
        if (payload.taskId !== taskId || !this.state || terminal(this.state.progress.stage)) return;
        this.update({ ...this.state, progress: payload, registered: true });
        if (this.disposed) void this.cancel();
      });
      if (this.disposed) throw new Error("OPERATION_CANCELLED: 页面已关闭");
      submitted = true;
      const result = await execute(taskId);
      await this.finalProgress(taskId);
      this.update({ ...this.state!, progress: { ...this.state!.progress, stage: "completed", completed: this.state!.progress.total, elapsedMs: performance.now() - started } });
      return result;
    } catch (error) {
      if (submitted) await this.finalProgress(taskId);
      const cancelled = this.state?.progress.stage === "cancelled" || errorText(error).startsWith("OPERATION_CANCELLED:");
      this.update({ ...this.state!, error: cancelled ? "" : errorText(error), progress: { ...this.state!.progress, stage: cancelled ? "cancelled" : "failed", elapsedMs: performance.now() - started } });
      throw cancelled ? new Error("任务已取消，原结果保留") : error;
    } finally { stop?.(); this.active = false; }
  }

  cancel = async () => {
    if (!this.active || !this.state?.registered || this.state.cancelling || terminal(this.state.progress.stage) || this.state.progress.stage === "saving") return;
    const taskId = this.state.progress.taskId;
    this.update({ ...this.state, cancelling: true, error: "" });
    try {
      const accepted = await desktopCall<boolean>("operation_cancel", { taskId });
      if (this.state?.progress.taskId === taskId && !accepted) this.update({ ...this.state, cancelling: false });
    } catch (error) {
      if (this.state?.progress.taskId === taskId) this.update({ ...this.state, cancelling: false, error: errorText(error) });
    }
  };
}

export function useOperation() {
  const [operation] = useState(() => new Operation());
  useEffect(() => { operation.attach(); return operation.dispose; }, [operation]);
  return operation;
}

export function OperationStatus({ operation, label }: { operation: Operation; label: string }) {
  const state = useSyncExternalStore(operation.subscribe, operation.snapshot);
  if (!state) return null;
  const { progress, cancelling, registered, error } = state;
  const done = terminal(progress.stage);
  const percent = progress.stage === "completed" ? 100 : progress.total > 0 ? Math.min(99, Math.floor(progress.completed / progress.total * 100)) : done ? 0 : undefined;
  return <div className="wp-operation" role="status" aria-label={`${label}进度`}>
    <div className="wp-operation-line"><strong>{label} · {cancelling && !done ? "正在取消，等待当前帧结束" : labels[progress.stage]}</strong>
      {!done && <Button size="sm" disabled={!registered || cancelling || progress.stage === "saving"} onClick={() => void operation.cancel()}>{progress.stage === "saving" ? "保存中" : "取消"}</Button>}
    </div>
    <progress aria-label={`${label}完成进度`} max={100} value={percent} />
    <span>{progress.total > 0 ? `已处理 ${progress.completed} / ${progress.total} 个相机帧 · ` : ""}已用 {(progress.elapsedMs / 1000).toFixed(1)} 秒{progress.failed > 0 ? ` · 测量失败 ${progress.failed}` : ""}{progress.skipped > 0 ? ` · 跳过 ${progress.skipped}` : ""}</span>
    {error && <span role="alert">{error}</span>}
  </div>;
}
