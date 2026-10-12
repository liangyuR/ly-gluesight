import { useEffect, useRef, useState } from "react";
import { OctagonX, Play } from "lucide-react";
import { errorText } from "../../../lib/errors";
import { usePlcStatus } from "../../plc";
import { cycleApi, useSiteStatus } from "../api";
import { pickRunnable, rejectionTarget } from "../readiness";
import type { Readiness, ReadinessTarget } from "../types";
import RecipeRunSelect from "./RecipeRunSelect";
import { TargetLink } from "./ReadinessChecklist";

interface Props {
  readiness: Readiness | null;
  refresh: () => Promise<Readiness | null>;
}

export default function OfflineControls({ readiness, refresh }: Props) {
  const status = useSiteStatus();
  const plc = usePlcStatus();
  const [recipeId, setRecipeId] = useState("");
  const [error, setError] = useState<{ text: string; target: ReadinessTarget | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const alive = useRef(true);
  const offline = status?.mode === "offline";
  const running = !!status?.running;
  const options = readiness?.recipes ?? null;

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  useEffect(() => {
    if (options) setRecipeId(current => pickRunnable(options, current));
  }, [options]);

  if (!status || !offline) return null;

  const perform = async (operation: () => Promise<unknown>, runningId?: string) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError(null);
    try { await operation(); } catch (e) {
      const text = errorText(e);
      const latest = runningId === undefined ? null : await refresh();
      if (alive.current) setError({ text, target: runningId === undefined ? null : rejectionTarget(latest ?? readiness, runningId) });
    }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  const connected = plc?.state === "connected";
  const message = !connected ? "等待连接离线 S7 服务…" : status.message;

  return (
    <div className="offline-controls compact">
      <span className="offline-label">离线产线</span>
      <RecipeRunSelect options={options} value={recipeId} onChange={setRecipeId} disabled={running || busy} />
      <button className="btn primary" onClick={() => void perform(() => cycleApi.offlineRun(recipeId, "normal", false, null), recipeId)} disabled={running || busy || !connected || !recipeId}>
        <Play size={15} />
        运行一件
      </button>
      <button className="btn danger" onClick={() => void perform(() => cycleApi.offlineStop(true))} disabled={busy || !running}>
        <OctagonX size={15} />
        立即停止
      </button>
      {error ? <span className="offline-msg c-ng" role="alert">{error.text} <TargetLink target={error.target} /></span>
        : <span className="offline-msg" role="status">{message}</span>}
    </div>
  );
}
