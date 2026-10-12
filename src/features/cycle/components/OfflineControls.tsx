import { useEffect, useRef, useState } from "react";
import { FolderOpen, OctagonX, Play, Power, PowerOff, Repeat, Square, X, Zap } from "lucide-react";
import { usePlcStatus } from "../../plc";
import { cameraApi } from "../../camera/api";
import { formatTime, sampleLabelText, verdictLabel } from "../../history/meta";
import { cycleApi, useOfflineStatus, useRecipes } from "../api";
import type { OfflineFault, SampleRun, Scenario } from "../types";
import SamplePartSelect from "./SamplePartSelect";

const scenarios: [Scenario, string][] = [
  ["normal", "正常件"],
  ["excursion", "局部超差（允许）"],
  ["gap", "断胶（超过允许长度）"],
  ["lostFrame", "传输丢帧"],
  ["locateFail", "定位失败"],
  ["countMismatch", "设备触发数与计划不符"],
  ["motionTimeout", "运动超时（不发 partEnd）"],
  ["ackMismatch", "ACK 序号不匹配"],
  ["random", "随机（连续运行用）"],
];

const faults: [OfflineFault, string][] = [
  ["disconnect", "断开当前 S7 连接"],
  ["heartbeatStop", "停止 PLC 心跳"],
  ["heartbeatResume", "恢复 PLC 心跳"],
  ["rejectWrite", "拒绝下一次写"],
  ["rejectRead", "拒绝下一次读"],
  ["delay", "下一次请求超时"],
  ["clear", "清除待执行故障"],
];

const purposeText: Record<SampleRun["purpose"], string> = { production: "生产", capture: "整圈采集" };

export default function OfflineControls({ compact = false }: { compact?: boolean }) {
  const recipes = useRecipes();
  const status = useOfflineStatus();
  const plc = usePlcStatus();
  const [recipeId, setRecipeId] = useState("");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const [fault, setFault] = useState<OfflineFault>("disconnect");
  const [partId, setPartId] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [stopRequested, setStopRequested] = useState(false);
  const pending = useRef(false);
  const alive = useRef(true);
  const enabled = !!status?.enabled;
  const running = !!status?.running;
  const sampleSet = status?.sampleSet ?? null;

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  useEffect(() => {
    if (!running) setStopRequested(false);
  }, [running]);
  useEffect(() => {
    setError("");
  }, [enabled]);
  useEffect(() => {
    if (!recipes.some(r => r.id === recipeId)) setRecipeId(recipes[0]?.id ?? "");
  }, [recipes, recipeId]);
  useEffect(() => {
    if (partId && !sampleSet?.parts.some(p => p.partId === partId)) setPartId("");
  }, [sampleSet, partId]);

  const perform = async (operation: () => Promise<unknown>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError("");
    try { await operation(); } catch (e) { if (alive.current) setError(String(e)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };

  if (!status) return compact ? null : <p role="status" className="muted">正在读取离线产线状态…</p>;
  if (!enabled) {
    if (compact) return null;
    return (
      <div className="offline-controls">
        <button className="btn primary" onClick={() => void perform(cycleApi.offlineEnable)} disabled={busy}>
          <Power size={15} />
          启用离线模式
        </button>
        <span className={`offline-msg${error ? " c-ng" : ""}`} role="status">{error || "启用后 PLC 临时连接进程内 S7 服务，走生产 S7 握手；停用后恢复原 PLC 配置。"}</span>
      </div>
    );
  }

  const connected = plc?.state === "connected";
  const canRun = !running && !busy && connected && !!recipeId;
  const run = (continuous: boolean) => void perform(() => cycleApi.offlineRun(recipeId, scenario, continuous, continuous || compact || !partId ? null : partId));
  const chooseSampleSet = () => void perform(async () => {
    const dir = await cameraApi.pickReplayDir(sampleSet?.dir ?? "", "选择离线样本集目录");
    if (dir !== null) await cycleApi.offlineSelectSampleSet(dir);
  });
  const message = error || (!connected ? "等待连接离线 S7 服务…" : stopRequested && running ? "已请求停止，等待本件完成…" : status.message);

  const controls = (
    <div className={`offline-controls${compact ? " compact" : ""}`}>
      {compact ? <span className="offline-label">离线产线</span> : <span className="muted mono">127.0.0.1:{status.port ?? "—"}</span>}
      <select aria-label="离线配方" className="input" value={recipeId} onChange={(e) => setRecipeId(e.target.value)} disabled={running || busy}>
        {recipes.map((r) => (
          <option key={r.id} value={r.id}>
            {r.id} · 代码 {r.productCode} · N={r.shotCount}
          </option>
        ))}
      </select>
      <select aria-label="离线工况" className="input" value={scenario} onChange={(e) => setScenario(e.target.value as Scenario)} disabled={running || busy}>
        {scenarios.map(([v, label]) => (
          <option key={v} value={v}>
            {label}
          </option>
        ))}
      </select>
      {!compact && sampleSet && <SamplePartSelect sampleSet={sampleSet} nextPart={status.nextPart} value={partId} onChange={setPartId} disabled={running || busy} />}
      <button className="btn primary" onClick={() => run(false)} disabled={!canRun}>
        <Play size={15} />
        运行一件
      </button>
      <button className="btn" onClick={() => run(true)} disabled={!canRun}>
        <Repeat size={15} />
        连续运行
      </button>
      <button className="btn" onClick={() => void perform(async () => {
        await cycleApi.offlineStop(false);
        if (alive.current) setStopRequested(true);
      })} disabled={busy || stopRequested || !running || !status.continuous}>
        <Square size={15} />
        本件后停止
      </button>
      <button className="btn danger" onClick={() => void perform(() => cycleApi.offlineStop(true))} disabled={busy || !running}>
        <OctagonX size={15} />
        立即停止
      </button>
      {!compact && <>
        <select aria-label="故障注入" className="input" value={fault} onChange={(e) => setFault(e.target.value as OfflineFault)} disabled={busy}>
          {faults.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </select>
        <button className="btn" onClick={() => void perform(() => cycleApi.offlineInject(fault))} disabled={busy}>
          <Zap size={15} />
          注入故障
        </button>
        <button className="btn" onClick={() => void perform(cycleApi.offlineDisable)} disabled={busy || running}>
          <PowerOff size={15} />
          停用离线模式
        </button>
      </>}
      {status.heartbeatStopped && <span className="offline-label">PLC 心跳已停止</span>}
      <span className={`offline-msg${error ? " c-ng" : ""}`} role="status">{message}</span>
    </div>
  );
  if (compact) return controls;

  const missingTitle = sampleSet && sampleSet.missingCount > 0 ? sampleSet.missing.slice(0, 10).join("\n") + (sampleSet.missingCount > 10 ? "\n…" : "") : undefined;
  return (
    <div className="offline-stack">
      <div className="offline-controls">
        <span className="muted">样本集</span>
        {sampleSet ? (
          <span title={missingTitle}>
            {sampleSet.name} · {sampleSet.parts.length} 件 · {sampleSet.shotCount} 点 · <span className={sampleSet.missingCount > 0 ? "c-ng" : undefined}>缺图 {sampleSet.missingCount}</span>
          </span>
        ) : <span className="muted">未选择</span>}
        <button className="btn" onClick={chooseSampleSet} disabled={busy || running}>
          <FolderOpen size={15} />
          选择样本集
        </button>
        {sampleSet && (
          <button className="btn" onClick={() => void perform(() => cycleApi.offlineSelectSampleSet(null))} disabled={busy || running}>
            <X size={15} />
            清除样本集
          </button>
        )}
        {status.sampleError && <span className="offline-msg c-ng" role="alert">{status.sampleError}</span>}
      </div>
      {controls}
      {status.recent.length > 0 && (
        <div className="table-wrap">
          <table className="table" aria-label="最近样本件">
            <caption className="muted">最近样本件</caption>
            <thead>
              <tr><th>时间</th><th>样本件</th><th>样本 SN</th><th>PLC SN</th><th>用途</th><th>期望</th><th>实际</th></tr>
            </thead>
            <tbody>
              {status.recent.map((r) => (
                <tr key={`${r.ts}:${r.partSn}:${r.partId}`}>
                  <td className="mono nowrap">{formatTime(r.ts)}</td>
                  <td className="mono">{r.partId}</td>
                  <td className="mono">{r.sampleSn}</td>
                  <td className="mono">{r.partSn}</td>
                  <td>{purposeText[r.purpose]}</td>
                  <td>{sampleLabelText[r.label]}</td>
                  <td>{r.verdict ? verdictLabel[r.verdict] : r.message || "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
