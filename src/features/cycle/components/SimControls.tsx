import { useEffect, useRef, useState } from "react";
import { Play, Repeat, Square } from "lucide-react";
import { plcApi, subscribe, usePlcStatus } from "../../plc";
import { cycleApi, useRecipes, useSimStatus } from "../api";
import type { Scenario } from "../types";

const scenarios: [Scenario, string][] = [
  ["normal", "正常件"],
  ["excursion", "局部超差（允许）"],
  ["gap", "跨帧断胶"],
  ["lostFrame", "传输丢帧"],
  ["locateFail", "定位失败"],
  ["countMismatch", "拍照点数不一致"],
  ["random", "随机（连续运行用）"],
];

export default function SimControls({ compact = false }: { compact?: boolean }) {
  const recipes = useRecipes();
  const status = useSimStatus();
  const plc = usePlcStatus();
  const [isSim, setIsSim] = useState(false);
  const [recipeId, setRecipeId] = useState("");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const [error, setError] = useState("");
  const [configError, setConfigError] = useState("");
  const [configLoading, setConfigLoading] = useState(true);
  const [confirmedConfigKey, setConfirmedConfigKey] = useState<string | null>(null);
  const [configVersion, setConfigVersion] = useState(0);
  const [busy, setBusy] = useState(false);
  const [stopRequested, setStopRequested] = useState(false);
  const pending = useRef(false);
  const alive = useRef(true);
  const scope = useRef({ key: "", generation: 0 });
  const key = `${plc?.state ?? ""}:${plc?.since ?? ""}:${configVersion}`;
  if (scope.current.key !== key) scope.current = { key, generation: scope.current.generation + 1 };
  const generation = scope.current.generation;
  const configPending = configLoading || confirmedConfigKey !== key;

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  useEffect(() => {
    return subscribe("plc://config", () => setConfigVersion(v => v + 1));
  }, []);
  useEffect(() => {
    let alive = true;
    setConfigLoading(true); setConfigError(""); setError(""); setStopRequested(false);
    plcApi.getConfig().then((c) => { if (alive) { setIsSim(c.connection.protocol === "simulator"); setConfigError(""); } })
      .catch(e => { if (alive) { setIsSim(false); setConfigError(String(e)); } })
      .finally(() => { if (alive) { setConfirmedConfigKey(key); setConfigLoading(false); } });
    return () => { alive = false; };
  }, [key]);
  useEffect(() => {
    if (!status?.running) setStopRequested(false);
  }, [status?.running, stopRequested]);
  useEffect(() => {
    if (!recipes.some(r => r.id === recipeId)) setRecipeId(recipes[0]?.id ?? "");
  }, [recipes, recipeId]);

  if (!isSim) {
    if (configPending) return compact ? null : <p role="status" className="muted">正在确认模拟 PLC 配置…</p>;
    return compact && !configError ? null : <div><p className={configError ? "c-ng" : "muted"}>{configError || "模拟节拍需要把 PLC 协议设为“模拟器”并连接。"}</p>{configError && <button className="btn" onClick={() => setConfigVersion(v => v + 1)}>重新加载模拟配置</button>}</div>;
  }

  const perform = async (operation: () => Promise<void>) => {
    if (pending.current || configPending) return;
    pending.current = true; setBusy(true); setError("");
    try { await operation(); } catch (e) { if (alive.current && scope.current.generation === generation) setError(String(e)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  const running = !!status?.running;
  const connected = plc?.state === "connected";

  return (
    <div className={`sim-controls${compact ? " compact" : ""}`}>
      {compact && <span className="sim-label">模拟节拍</span>}
      <select id="sim-recipe" aria-label="模拟配方" className="input" value={recipeId} onChange={(e) => setRecipeId(e.target.value)} disabled={running || busy || configPending}>
        {recipes.map((r) => (
          <option key={r.id} value={r.id}>
            {r.id} · 代码 {r.productCode} · N={r.shotCount}
          </option>
        ))}
      </select>
      <select id="sim-scenario" aria-label="模拟工况" className="input" value={scenario} onChange={(e) => setScenario(e.target.value as Scenario)} disabled={running || busy || configPending}>
        {scenarios.map(([v, label]) => (
          <option key={v} value={v}>
            {label}
          </option>
        ))}
      </select>
      <button className="btn primary" onClick={() => void perform(() => cycleApi.simStart(recipeId, scenario, false))} disabled={running || busy || configPending || !connected || !recipeId}>
        <Play size={15} />
        运行一件
      </button>
      <button className="btn" onClick={() => void perform(() => cycleApi.simStart(recipeId, scenario, true))} disabled={running || busy || configPending || !connected || !recipeId}>
        <Repeat size={15} />
        连续运行
      </button>
      <button className="btn" onClick={() => void perform(async () => {
        await cycleApi.simStop();
        if (alive.current && scope.current.generation === generation) setStopRequested(true);
      })} disabled={busy || configPending || stopRequested || !running || !status?.continuous}>
        <Square size={15} />
        本件后停止
      </button>
      <span className={`sim-msg${error ? " c-ng" : ""}`} role="status">{error || (configPending ? "正在确认模拟 PLC 配置…" : !connected ? "请先连接模拟 PLC" : stopRequested && running ? "已请求停止，等待本件完成…" : status?.message)}</span>
    </div>
  );
}
