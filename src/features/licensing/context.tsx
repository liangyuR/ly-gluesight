import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { desktopAvailable } from "../../lib/desktop";
import { errorText } from "../../lib/errors";
import { licenseApi, type LicenseStatus } from "./api";

interface LicenseContextValue {
  status: LicenseStatus | null;
  error: string;
  busy: boolean;
  refresh: () => Promise<void>;
  execute: (action: () => Promise<LicenseStatus>) => Promise<void>;
}

const LicenseContext = createContext<LicenseContextValue | null>(null);

export function LicenseProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<LicenseStatus | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const active = useRef(false);
  const pending = useRef(false);
  const refresh = useCallback(async () => {
    if (!desktopAvailable() || pending.current) return;
    const request = ++generation.current;
    try {
      const next = await licenseApi.status();
      if (active.current && request === generation.current) { setStatus(next); setError(""); }
    } catch (failure) {
      if (active.current && request === generation.current) setError(errorText(failure));
    }
  }, []);
  const execute = useCallback(async (action: () => Promise<LicenseStatus>) => {
    if (pending.current) throw new Error("授权操作正在进行，请等待完成");
    pending.current = true;
    ++generation.current;
    setBusy(true);
    try {
      const next = await action();
      if (active.current) { setStatus(next); setError(""); }
    } finally {
      pending.current = false;
      if (active.current) setBusy(false);
    }
  }, []);
  useEffect(() => {
    active.current = true;
    void refresh();
    const interval = window.setInterval(() => void refresh(), 30_000);
    const focus = () => void refresh();
    const visible = () => { if (document.visibilityState === "visible") void refresh(); };
    window.addEventListener("focus", focus);
    document.addEventListener("visibilitychange", visible);
    return () => {
      active.current = false;
      ++generation.current;
      clearInterval(interval);
      window.removeEventListener("focus", focus);
      document.removeEventListener("visibilitychange", visible);
    };
  }, [refresh]);
  useEffect(() => {
    const now = Date.now();
    const boundary = [status?.warningAt, status?.expiresAt, status?.emergencyUntil]
      .filter((value): value is string => !!value).map(Date.parse).filter(value => value > now);
    if (!boundary.length) return;
    const timer = window.setTimeout(() => void refresh(), Math.min(Math.min(...boundary) - now + 20, 2_147_483_647));
    return () => clearTimeout(timer);
  }, [status, refresh]);
  return <LicenseContext.Provider value={{ status, error, busy, refresh, execute }}>{children}</LicenseContext.Provider>;
}

export function useLicense() {
  const context = useContext(LicenseContext);
  if (!context) throw new Error("授权组件需要 LicenseProvider");
  return context;
}
