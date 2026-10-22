import { desktopCall } from "./desktop";
import { errorText } from "./errors";

/**
 * 段配方前端已接入、后端工作包还没注册的命令（归属见 docs/decisions/segment-burst.md 5.4）。
 * Tauri 对未注册的命令拒绝为 “Command <命令名> not found”；转成这个错误后，页面显示“该功能待后端 <包> 实现”的占位，不当作故障报错。
 */
export class PendingBackendError extends Error {
  constructor(readonly command: string, readonly owner: string) {
    super(`该功能待后端 ${owner} 实现`);
    this.name = "PendingBackendError";
  }
}

export const isPendingBackend = (error: unknown): error is PendingBackendError => error instanceof PendingBackendError;

/** 调用 owner 包负责的新命令；后端还没注册时改抛 PendingBackendError，其他错误原样抛出。 */
export function pendingCall<T>(command: string, owner: string, args?: Record<string, unknown>): Promise<T> {
  return desktopCall<T>(command, args).catch((error: unknown) => {
    throw errorText(error) === `Command ${command} not found` ? new PendingBackendError(command, owner) : error;
  });
}

/** 读图失败的展示：待后端实现的给占位文字（pending），其余照常作为错误（error）。 */
export function imageFailure(error: unknown): { error: string; pending: string } {
  return isPendingBackend(error) ? { error: "", pending: error.message } : { error: errorText(error), pending: "" };
}
