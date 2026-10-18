import { useRef, useState } from "react";
import Modal from "../plc/components/Modal";
import { fromLocalInput, toLocalInput } from "../plc/time";
import type { HistoryScope } from "../history/types";
import { errorText } from "../../lib/errors";
import { formatBytes, logsApi } from "./api";
import type { DiagnosticsResult, ImageMode } from "./types";

const presets = [["1 小时", 3_600_000], ["24 小时", 86_400_000], ["7 天", 7 * 86_400_000]] as const;
const scopes: [HistoryScope | "all", string][] = [["all", "全部记录"], ["real", "真机件"], ["offline", "离线件"], ["debug", "候选调试"]];
const imageModes: [ImageMode, string][] = [["abnormal", "仅异常件（NG / ERR / 局部超差）"], ["all", "全部工件"], ["none", "不含原图"]];

export default function DiagnosticsDialog({ from, to, historyIds, title = "导出诊断包", onClose }: {
  from?: number; to?: number; historyIds?: number[]; title?: string; onClose: () => void;
}) {
  const now = Date.now();
  const [start, setStart] = useState(() => toLocalInput(from ?? now - 3_600_000));
  const [end, setEnd] = useState(() => toLocalInput(to ?? now));
  const [scope, setScope] = useState<HistoryScope | "all">("all");
  const [images, setImages] = useState<ImageMode>(historyIds?.length ? "all" : "abnormal");
  const [maxParts, setMaxParts] = useState(20);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [result, setResult] = useState<DiagnosticsResult | null>(null);
  const alive = useRef(true);
  const named = !!historyIds?.length;
  const run = async () => {
    const a = fromLocalInput(start), b = fromLocalInput(end);
    if (a === null || b === null || a >= b) { setError("请选择有效时间段，开始时间需早于结束时间"); return; }
    if (!Number.isInteger(maxParts) || maxParts < 1 || maxParts > 200) { setError("附原图件数需为 1–200 的整数"); return; }
    setBusy(true); setError(""); setResult(null);
    try {
      const done = await logsApi.exportDiagnostics({ from: a, to: b, scope: scope === "all" ? null : scope, historyIds: historyIds ?? [], images, maxImageParts: maxParts });
      if (alive.current) setResult(done);
    } catch (e) { if (alive.current) setError(errorText(e)); }
    finally { if (alive.current) setBusy(false); }
  };
  const close = () => { alive.current = false; onClose(); };
  return <Modal title={title} width={600} onClose={close} closeDisabled={busy} footer={<>
    {error && <span className="form-error" role="alert">{error}</span>}
    <button className="btn" disabled={busy} onClick={close}>{result ? "完成" : "取消"}</button>
    <button className="btn primary" disabled={busy} aria-busy={busy} onClick={() => void run()}>{busy ? "正在打包…" : "导出"}</button>
  </>}>
    <div className="diag-form">
      <p className="muted">诊断包包含时间段内的运行日志、PLC 日志、配置与状态文件、检测记录（CSV 与完整明细）、所用配方快照{named ? "，以及所选记录的原图与对照结果" : "，可选附带原图与对照结果"}。导出到资料目录的 exports/。</p>
      <div className="filter-row">
        <span className="filter-label">日志时间</span>
        <input className="input mono" aria-label="诊断包开始时间" type="datetime-local" step={1} value={start} disabled={busy} onChange={e => setStart(e.target.value)} />
        <span className="muted">至</span>
        <input className="input mono" aria-label="诊断包结束时间" type="datetime-local" step={1} value={end} disabled={busy} onChange={e => setEnd(e.target.value)} />
      </div>
      {!named && <div className="filter-row"><span className="filter-label">最近</span>{presets.map(([label, ms]) => <button key={label} className="chip" disabled={busy} onClick={() => { const t = Date.now(); setStart(toLocalInput(t - ms)); setEnd(toLocalInput(t)); }}>{label}</button>)}</div>}
      {named ? <p className="muted">记录：{historyIds!.map(id => "#" + id).join("、")}</p> :
        <label className="field"><span>检测记录</span><select className="input" aria-label="诊断包记录范围" value={scope} disabled={busy} onChange={e => setScope(e.target.value as HistoryScope | "all")}>{scopes.map(([key, label]) => <option key={key} value={key}>{label}</option>)}</select></label>}
      <label className="field"><span>原图</span><select className="input" aria-label="诊断包原图" value={images} disabled={busy} onChange={e => setImages(e.target.value as ImageMode)}>{imageModes.filter(([key]) => !named || key !== "abnormal").map(([key, label]) => <option key={key} value={key}>{named && key === "all" ? "所选记录的原图" : label}</option>)}</select></label>
      {!named && images !== "none" && <label className="field"><span>最多附原图件数（新的优先，原图总量上限 2 GB）</span><input className="input mono" aria-label="最多附原图件数" type="number" min={1} max={200} value={maxParts} disabled={busy} onChange={e => setMaxParts(Number(e.target.value))} /></label>}
      {result && <div className="notice ok" role="status">
        <div>已导出 <span className="mono">{result.path}</span>（{formatBytes(result.bytes)}）</div>
        <div>运行日志 {result.logs} 条 · PLC 日志 {result.plcLogs} 条 · 检测记录 {result.parts} 件 · 原图 {result.images} 幅（{result.imageParts} 件）</div>
        <button className="link" onClick={() => void logsApi.reveal(result.path).catch(e => setError(errorText(e)))}>打开所在文件夹</button>
      </div>}
      {!!result?.notes.length && <details><summary>打包说明（{result.notes.length}）</summary><ul className="diag-notes">{result.notes.map((note, i) => <li key={i}>{note}</li>)}</ul></details>}
    </div>
  </Modal>;
}
