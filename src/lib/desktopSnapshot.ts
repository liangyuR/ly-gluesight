import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export function createSharedSnapshot<T>(
  event: string,
  read: () => Promise<T | null>,
  { readOnListenFailure = false }: { readOnListenFailure?: boolean } = {},
) {
  type Listener = (state: T | null) => void;
  const listeners = new Set<Listener>();
  let current: T | null | undefined;
  let connection: ReturnType<typeof createConnection> | undefined;

  function publish(state: T | null) {
    current = state;
    listeners.forEach(listener => listener(state));
  }

  function createConnection() {
    let active = true, pending = false, resolved = false;
    let unlisten: UnlistenFn | undefined;

    const start = async () => {
      if (!active || pending || resolved) return;
      pending = true;
      const desktop = isTauri();
      try {
        if (desktop && !unlisten) {
          let acceptEvents = true;
          try {
            const stop = await listen<T>(event, message => {
              if (!active || !acceptEvents) return;
              resolved = true;
              publish(message.payload);
            });
            if (!active) { stop(); return; }
            unlisten = stop;
          } catch {
            acceptEvents = false;
            resolved = false;
            if (!readOnListenFailure) {
              if (active) publish(null);
              return;
            }
          }
        }
        if (active && !resolved) {
          const state = await read();
          if (active && !resolved) {
            resolved = !desktop || !!unlisten;
            publish(state);
          }
        }
      } catch {
        if (active && !resolved) publish(null);
      } finally {
        pending = false;
      }
    };
    return { start, dispose: () => { active = false; unlisten?.(); } };
  }

  return (listener: Listener) => {
    listeners.add(listener);
    if (current !== undefined) listener(current);
    connection ??= createConnection();
    void connection.start();
    return () => {
      if (!listeners.delete(listener) || listeners.size) return;
      connection?.dispose();
      connection = undefined;
      current = undefined;
    };
  };
}
