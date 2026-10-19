import { useEffect, useRef, useState } from "react";
import { OctagonX, Play } from "lucide-react";
import { Button } from "qing-steel";
import { errorText } from "../../../lib/errors";
import { usePlcStatus } from "../../plc";
import { cycleApi, useSiteStatus } from "../api";
import { rejectionTarget, useRunChoice } from "../readiness";
import type { Readiness, ReadinessTarget } from "../types";
import RecipeRunSelect from "./RecipeRunSelect";
import { TargetLink } from "./ReadinessChecklist";

interface Props {
  publishedSection?: boolean;
  candidateRunning?: boolean;
  readiness: Readiness | null;
  refresh: () => Promise<Readiness | null>;
}

export default function OfflineControls({ readiness, refresh, publishedSection = false, candidateRunning = false }: Props) {
  const status = useSiteStatus();
  const plc = usePlcStatus();
  const [error, setError] = useState<{ text: string; target: ReadinessTarget | null } | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const alive = useRef(true);
  const offline = status?.mode === "offline";
  const running = !!status?.running;
  const options = readiness?.recipes.filter(recipe => recipe.published) ?? null;
  const [recipeId, setRecipeId] = useRunChoice(options, running);
  const message = plc?.state !== "connected" ? "等待连接离线 S7 服务…" : status?.message ?? "";

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  useEffect(() => setError(null), [message, running]);

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

  const controls = (
    <div className="offline-controls compact">
      <span className="offline-label">{candidateRunning ? "候选调试运行中" : publishedSection ? "已发布配方回放" : "离线产线"}</span>
      {!candidateRunning && <RecipeRunSelect options={options} value={recipeId} onChange={setRecipeId} disabled={running || busy} />}
      {!candidateRunning && <Button variant="filled" size="sm" leadingIcon={<Play size={15} />} onClick={() => void perform(() => cycleApi.offlineRun(recipeId, "normal", false, null), recipeId)} disabled={running || busy || !connected || !recipeId}>
        运行一件
      </Button>}
      <Button danger size="sm" leadingIcon={<OctagonX size={15} />} onClick={() => void perform(() => cycleApi.offlineStop(true))} disabled={busy || !running}>
        立即停止
      </Button>
      <span className="offline-msg" role="status" title={message}>{message}</span>
      {error && <span className="offline-msg c-ng" role="alert">{error.text} <TargetLink target={error.target} /></span>}
    </div>
  );
  if (!publishedSection) return controls;
  if (candidateRunning) return controls;
  return <details className="offline-published-controls"><summary>已发布配方离线回放（需要发布）</summary><p className="muted">此区域只运行已发布版本；上方候选试运行无需发布。</p>{controls}</details>;
}
