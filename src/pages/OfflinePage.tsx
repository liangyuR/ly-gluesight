import { useEffect, useRef, useState } from "react";
import { FolderOpen, LogIn, LogOut, OctagonX, Play, Repeat, Square, Zap } from "lucide-react";
import { usePlcStatus } from "../features/plc";
import { formatTime, sampleLabelText, verdictLabel } from "../features/history/meta";
import {
  cycleApi,
  faultOptions,
  measurementLabel,
  scenariosFor,
  siteApi,
  sourceLabel,
  useRecipes,
  useSiteStatus,
  type OfflineChoice,
  type OfflineFault,
  type SampleLabel,
  type SampleRun,
  type SampleSetSummary,
  type Scenario,
  type SiteSourceKind,
  type SiteStatus,
} from "../features/cycle";
import SamplePartSelect from "../features/cycle/components/SamplePartSelect";

const purposeText: Record<SampleRun["purpose"], string> = { production: "生产", capture: "整圈采集" };
const labels: SampleLabel[] = ["ok", "ng", "unknown"];

function useOperation() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef(false);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);
  const perform = async (operation: () => Promise<unknown>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError("");
    try { await operation(); } catch (e) { if (alive.current) setError(String(e)); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  };
  return { busy, error, perform, alive };
}

function SampleSetView({ sampleSet }: { sampleSet: SampleSetSummary }) {
  const missingTitle = sampleSet.missingCount > 0 ? sampleSet.missing.slice(0, 10).join("\n") + (sampleSet.missingCount > 10 ? "\n…" : "") : undefined;
  return (
    <dl className="kv offline-summary" aria-label="样本集摘要">
      <dt>名称</dt><dd>{sampleSet.name}</dd>
      <dt>目录</dt><dd className="mono">{sampleSet.dir}</dd>
      <dt>件数</dt><dd>{sampleSet.parts.length}</dd>
      <dt>拍照点</dt><dd>{sampleSet.shotCount}</dd>
      <dt>缺图</dt><dd className={sampleSet.missingCount > 0 ? "c-ng" : undefined} title={missingTitle}>{sampleSet.missingCount}</dd>
      <dt>标签</dt><dd>{labels.map(l => `${sampleLabelText[l]} ${sampleSet.parts.filter(p => p.label === l).length}`).join(" · ")}</dd>
      <dt>相机与视角</dt><dd>{sampleSet.cameras.map(c => `${c.id}（视角 ${c.views.join("/")}）`).join("、") || "—"}</dd>
    </dl>
  );
}

function OfflineEntry({ last }: { last: OfflineChoice | null }) {
  const [kind, setKind] = useState<SiteSourceKind>(last?.kind ?? "sampleSet");
  const [dir, setDir] = useState(last?.sampleSetDir ?? "");
  const [preview, setPreview] = useState<SampleSetSummary | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [loading, setLoading] = useState(false);
  const serial = useRef(0);
  const { busy, error, perform, alive } = useOperation();

  const load = async (target: string) => {
    const request = ++serial.current;
    setDir(target); setPreview(null); setPreviewError(""); setLoading(true);
    try {
      const summary = await siteApi.previewSampleSet(target);
      if (alive.current && request === serial.current) setPreview(summary);
    } catch (e) {
      if (alive.current && request === serial.current) setPreviewError(String(e));
    } finally {
      if (alive.current && request === serial.current) setLoading(false);
    }
  };
  useEffect(() => {
    if (last?.sampleSetDir) void load(last.sampleSetDir);
    return () => { serial.current++; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const choose = () => void perform(async () => {
    const picked = await siteApi.pickDir(dir, "选择离线样本集目录");
    if (picked !== null && alive.current) await load(picked);
  });
  const enter = () => void perform(() => siteApi.enterOffline(kind === "sampleSet" ? { kind, dir } : { kind }));
  const ready = kind === "synthetic" || (!!preview && !loading);

  return (
    <section className="panel offline-console">
      <div className="panel-toolbar">
        <h3 className="panel-title">当前：真机</h3>
      </div>
      <div className="form-grid">
        <label className="field">
          <span>离线来源</span>
          <select aria-label="离线来源" className="input" value={kind} onChange={(e) => setKind(e.target.value as SiteSourceKind)} disabled={busy}>
            <option value="sampleSet">样本集（现场原图）</option>
            <option value="synthetic">合成图</option>
          </select>
        </label>
      </div>
      {kind === "sampleSet" && <>
        <div className="offline-controls">
          <button className="btn" onClick={choose} disabled={busy || loading}>
            <FolderOpen size={15} />
            选择样本集目录
          </button>
          <span className="mono muted">{dir || "未选择"}</span>
        </div>
        {loading && <p className="muted" role="status">正在读取样本集…</p>}
        {previewError && <p className="notice error" role="alert">{previewError}</p>}
        {preview && <SampleSetView sampleSet={preview} />}
      </>}
      <div className="offline-effects">
        <strong>进入离线会临时切换：</strong>
        <ul>
          <li>PLC → 进程内 S7 服务</li>
          <li>相机组 → 按{sourceLabel[kind]}生成</li>
          <li>测量 → {kind === "sampleSet" ? "lyFlow" : "模拟测量"}</li>
          <li>帧录制 → 保留全部</li>
        </ul>
        <span className="muted">退出离线后全部恢复为真机配置。离线期间 PLC 与相机组配置只读。</span>
      </div>
      <div className="offline-controls">
        <button className="btn primary" onClick={enter} disabled={busy || !ready}>
          <LogIn size={15} />
          进入离线
        </button>
      </div>
      {error && <p className="notice error" role="alert">{error}</p>}
    </section>
  );
}

function OfflineSession({ status }: { status: SiteStatus }) {
  const recipes = useRecipes();
  const plc = usePlcStatus();
  const [recipeId, setRecipeId] = useState("");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const [fault, setFault] = useState<OfflineFault>("disconnect");
  const [partId, setPartId] = useState("");
  const [stopRequested, setStopRequested] = useState(false);
  const { busy, error, perform, alive } = useOperation();
  const running = status.running;
  const sampleSet = status.source === "sampleSet" ? status.sampleSet : null;
  const scenarios = scenariosFor(status.source);
  const chosen = scenarios.some(([v]) => v === scenario) ? scenario : "normal";

  useEffect(() => {
    if (!running) setStopRequested(false);
  }, [running]);
  useEffect(() => {
    if (!recipes.some(r => r.id === recipeId)) setRecipeId(recipes[0]?.id ?? "");
  }, [recipes, recipeId]);
  useEffect(() => {
    if (partId && !sampleSet?.parts.some(p => p.partId === partId)) setPartId("");
  }, [sampleSet, partId]);

  const connected = plc?.state === "connected";
  const canRun = !running && !busy && connected && !!recipeId;
  const run = (continuous: boolean) => void perform(() => cycleApi.offlineRun(recipeId, chosen, continuous, continuous || !partId ? null : partId));
  const message = !connected ? "等待连接离线 S7 服务…" : stopRequested && running ? "已请求停止，等待本件完成…" : status.message;

  return (
    <div className="stack offline-console">
      {error && <p className="notice error" role="alert">{error}</p>}
      <section className="panel" aria-label="离线状态">
        <div className="panel-toolbar">
          <h3 className="panel-title">当前：离线 · {status.source ? sourceLabel[status.source] : "—"}</h3>
          <button className="btn" onClick={() => void perform(siteApi.leaveOffline)} disabled={busy || running}>
            <LogOut size={15} />
            退出离线
          </button>
        </div>
        <dl className="kv offline-summary">
          <dt>S7 服务</dt><dd className="mono">127.0.0.1:{status.port ?? "—"}</dd>
          <dt>测量方式</dt><dd>{measurementLabel[status.measurement]}</dd>
          <dt>已运行</dt><dd>{status.parts} 件</dd>
        </dl>
        {sampleSet && <SampleSetView sampleSet={sampleSet} />}
      </section>
      <section className="panel" aria-label="运行控制">
        <h3 className="panel-title">运行控制</h3>
        <div className="offline-controls">
          <select aria-label="离线配方" className="input" value={recipeId} onChange={(e) => setRecipeId(e.target.value)} disabled={running || busy}>
            {recipes.map((r) => (
              <option key={r.id} value={r.id}>
                {r.id} · 代码 {r.productCode} · N={r.shotCount}
              </option>
            ))}
          </select>
          <select aria-label="离线工况" className="input" value={chosen} onChange={(e) => setScenario(e.target.value as Scenario)} disabled={running || busy}>
            {scenarios.map(([v, label]) => (
              <option key={v} value={v}>
                {label}
              </option>
            ))}
          </select>
          {sampleSet && <SamplePartSelect sampleSet={sampleSet} nextPart={status.nextPart} value={partId} onChange={setPartId} disabled={running || busy} />}
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
        </div>
        <p className="offline-msg" role="status">{message}</p>
      </section>
      <section className="panel">
        <h3 className="panel-title">故障注入</h3>
        <div className="offline-controls">
          <select aria-label="故障注入" className="input" value={fault} onChange={(e) => setFault(e.target.value as OfflineFault)} disabled={busy}>
            {faultOptions.map(([v, label]) => (
              <option key={v} value={v}>
                {label}
              </option>
            ))}
          </select>
          <button className="btn" onClick={() => void perform(() => cycleApi.offlineInject(fault))} disabled={busy}>
            <Zap size={15} />
            注入故障
          </button>
          {status.heartbeatStopped && <span className="offline-label">PLC 心跳已停止</span>}
        </div>
      </section>
      {status.recent.length > 0 && (
        <div className="panel table-wrap">
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

export default function OfflinePage() {
  const status = useSiteStatus();
  if (!status) return <p role="status" className="muted">正在读取站点模式…</p>;
  return status.mode === "offline" ? <OfflineSession status={status} /> : <OfflineEntry last={status.last} />;
}
