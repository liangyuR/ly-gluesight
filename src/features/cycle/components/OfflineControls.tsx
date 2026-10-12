import { useEffect, useRef, useState } from "react";
import { OctagonX, Play } from "lucide-react";
import { usePlcStatus } from "../../plc";
import { cycleApi, useRecipes, useSiteStatus } from "../api";

export default function OfflineControls() {
  const recipes = useRecipes();
  const status = useSiteStatus();
  const plc = usePlcStatus();
  const [recipeId, setRecipeId] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const alive = useRef(true);
  const offline = status?.mode === "offline";
  const running = !!status?.running;

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  useEffect(() => {
    if (!recipes.some(r => r.id === recipeId)) setRecipeId(recipes[0]?.id ?? "");
  }, [recipes, recipeId]);

  if (!status || !offline) return null;

  const perform = async (operation: () => Promise<unknown>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError("");
    try { await operation(); } catch (e) { if (alive.current) setError(String(e)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  const connected = plc?.state === "connected";
  const message = error || (!connected ? "等待连接离线 S7 服务…" : status.message);

  return (
    <div className="offline-controls compact">
      <span className="offline-label">离线产线</span>
      <select aria-label="离线配方" className="input" value={recipeId} onChange={(e) => setRecipeId(e.target.value)} disabled={running || busy}>
        {recipes.map((r) => (
          <option key={r.id} value={r.id}>
            {r.id} · 代码 {r.productCode} · N={r.shotCount}
          </option>
        ))}
      </select>
      <button className="btn primary" onClick={() => void perform(() => cycleApi.offlineRun(recipeId, "normal", false, null))} disabled={running || busy || !connected || !recipeId}>
        <Play size={15} />
        运行一件
      </button>
      <button className="btn danger" onClick={() => void perform(() => cycleApi.offlineStop(true))} disabled={busy || !running}>
        <OctagonX size={15} />
        立即停止
      </button>
      <span className={`offline-msg${error ? " c-ng" : ""}`} role="status">{message}</span>
    </div>
  );
}
