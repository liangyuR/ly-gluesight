import { useSyncExternalStore } from "react";
import { getEngineStatus, type EngineStatus } from "./api";
import { errorText } from "./errors";

type EngineReading = { data: EngineStatus | null; error: string | null; loading: boolean };
type EngineIndicator = Pick<EngineStatus, "backend" | "ready" | "message">;

const initial: EngineReading = { data: null, error: null, loading: true };
const listeners = new Set<() => void>();
let reading = initial;
let indicator: EngineIndicator | null = null;
let inFlight: Promise<EngineStatus> | null = null;
let refreshQueued = false;
let timer: ReturnType<typeof setInterval> | undefined;

function sameStatus(a: EngineStatus | null, b: EngineStatus | null) {
  return a === b || (a?.backend === b?.backend && a?.ready === b?.ready && a?.message === b?.message
    && a?.version === b?.version && a?.path === b?.path && a?.measuring === b?.measuring);
}

function publish(next: EngineReading) {
  if (reading.error === next.error && reading.loading === next.loading && sameStatus(reading.data, next.data)) return;
  reading = next;
  const data = next.data;
  if (indicator?.backend !== data?.backend || indicator?.ready !== data?.ready || indicator?.message !== data?.message) {
    indicator = data ? { backend: data.backend, ready: data.ready, message: data.message } : null;
  }
  listeners.forEach(listener => listener());
}

function finish(request: Promise<EngineStatus>, next: EngineReading) {
  if (inFlight !== request) return;
  inFlight = null;
  if (refreshQueued) {
    refreshQueued = false;
    poll();
  } else publish(next);
}

function poll() {
  if (inFlight) return;
  const request = getEngineStatus();
  inFlight = request;
  void request.then(
    data => finish(request, { data, error: null, loading: false }),
    cause => finish(request, { data: null, error: errorText(cause), loading: false }),
  );
}

function refresh() {
  if (!listeners.size) return;
  if (inFlight) refreshQueued = true;
  publish({ ...reading, error: null, loading: true });
  poll();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    poll();
    timer = setInterval(poll, 3000);
  }
  return () => {
    if (!listeners.delete(listener) || listeners.size) return;
    clearInterval(timer);
    timer = undefined;
    inFlight = null;
    refreshQueued = false;
    reading = initial;
    indicator = null;
  };
}

const getReading = () => reading;
const getIndicator = () => indicator;

export function useEngineStatus() {
  const status = useSyncExternalStore(subscribe, getReading, getReading);
  return { ...status, refresh };
}

export function useEngineIndicator() {
  return useSyncExternalStore(subscribe, getIndicator, getIndicator);
}
