export const frameReadAbortError = () => new DOMException("帧读取已取消", "AbortError");

export function createFrameReadQueue(limit: number) {
  let active = 0;
  const pending: (() => void)[] = [];

  return function schedule<T>(read: () => Promise<T>, signal?: AbortSignal): Promise<T> {
    if (signal?.aborted) return Promise.reject(frameReadAbortError());
    return new Promise((resolve, reject) => {
      const cancel = () => {
        const index = pending.indexOf(start);
        if (index >= 0) pending.splice(index, 1);
        reject(frameReadAbortError());
      };
      const start = () => {
        if (signal?.aborted) { cancel(); return; }
        active++;
        void Promise.resolve().then(() => {
          if (signal?.aborted) throw frameReadAbortError();
          return read();
        }).then(value => { if (!signal?.aborted) resolve(value); }, reject).finally(() => {
          signal?.removeEventListener("abort", cancel);
          active--;
          while (active < limit && pending.length) pending.shift()!();
        });
      };
      signal?.addEventListener("abort", cancel, { once: true });
      if (active < limit) start(); else pending.push(start);
    });
  };
}
