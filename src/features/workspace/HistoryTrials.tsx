import { useEffect, useRef, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { Button, Checkbox } from "qing-steel";
import { errorText } from "../../lib/errors";
import { verdictLabel, verdictTone } from "../history";
import type { HistoryDetailState, PartMode } from "../history/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Panel } from "./components";
import { pieceTime } from "./pieces";
import type { Comparison, HistoryCandidate } from "./types";
import { OperationStatus, useOperation } from "./operation";

const modeLabels: Record<PartMode, string> = { production: "生产", maintenanceCollect: "维护采集", maintenanceShadow: "维护试判" };
type TrialResult = Pick<Comparison, "id" | "cycleId" | "candidateRevision"> &
  Pick<Comparison["judgement"], "verdict" | "reason"> & { unmeasured: string };
type HistoryTrialLocationState = { historyTrial: {
  recipeId: string;
  instanceId: number;
  all: boolean;
  recordId: number;
  results: Record<number, TrialResult>;
} };

const toneOf = (verdict: TrialResult["verdict"]) => { const tone = verdictTone(verdict); return tone === "fault" ? "ng" : tone; };

/**
 * 历史件试测（试测页）：直接列出本配方的历史件，“触发测试”用当前候选（已保存的走廊与参数）从录制原图重测并判定一件，
 * 结果另存为对照（`workspace_compare`，raw），不改历史记录，也不改示教件基准；详情在历史记录页里看。
 * 还有改动等着自动保存时，点测试先立即保存，存好再按保存后的修订测。
 */
export function HistoryTrials({ recipeId }: { recipeId: string }) {
  const location = useLocation();
  const operation = useOperation();
  const { data, acting, pendingSave, saveNow, saveState } = useWorkspace();
  const instanceId = data?.workspace.instanceId ?? 0;
  const resume = (location.state as HistoryTrialLocationState | null)?.historyTrial;
  const restored = resume?.recipeId === recipeId && resume.instanceId === instanceId ? resume : undefined;
  const restoreRecord = useRef(restored?.recordId);
  const [rows, setRows] = useState<HistoryCandidate[] | null>(null);
  const [error, setError] = useState("");
  const [running, setRunning] = useState<number | null>(null);
  const [queued, setQueued] = useState<number | null>(null);
  const [results, setResults] = useState<Record<number, TrialResult>>(() => restored?.results ?? {});
  // 任意配方都能测任意记录（D-SB-58）：打开后列本机全部历史件，不只本配方的
  const [all, setAll] = useState(restored?.all ?? false);
  useEffect(() => {
    let alive = true;
    setRows(null);
    workspaceApi.historyCandidates(recipeId, { ownRecipe: !all, limit: 200 }).then(r => { if (alive) setRows(r.items); }).catch(e => { if (alive) { setRows([]); setError(errorText(e)); } });
    return () => { alive = false; };
  }, [recipeId, all]);
  // 至少有一段示教了走廊就能试测（D-SB-57）：没示教的段不测、只算检测不完整
  const teaching = data?.plan.cameraTeaching ?? [];
  const usable = teaching.some(item => item.mmPerPx != null && item.segments.some(s => s.saved));
  const partial = teaching.filter(item => !item.ready).map(item => item.camera);
  const blocked = !data ? "候选未加载" : !usable ? "还没有任何一段示教走廊：各相机要有像素当量，至少一段要有走廊" : data.workspace.pending ? "候选版本正在等待生效" : "";
  const partialHint = !blocked && partial.length ? `走廊示教未完成（${partial.join("、")}）：没示教的段不测，只算检测不完整，结论只覆盖已示教的段` : "";
  const run = async (recordId: number) => {
    if (!data || blocked || running != null) return;
    setRunning(recordId); setError("");
    try {
      const result = await operation.run(taskId => workspaceApi.compare(data.workspace.doc.id, data.workspace.revision, recordId, true, taskId));
      const summary: TrialResult = {
        id: result.id, cycleId: result.cycleId, candidateRevision: result.candidateRevision,
        verdict: result.judgement.verdict, reason: result.judgement.reason,
        unmeasured: result.burst?.unmeasured?.map(item => item.reason).join("；") ?? "",
      };
      setResults(previous => ({ ...previous, [recordId]: summary }));
    } catch (e) { setError(errorText(e)); }
    finally { setRunning(null); }
  };
  const trigger = (recordId: number) => { if (pendingSave) { setQueued(recordId); saveNow(); } else void run(recordId); };
  useEffect(() => {
    if (queued == null) return;
    if (saveState === "failed") { setQueued(null); return; }
    if (!pendingSave && !acting) { const recordId = queued; setQueued(null); void run(recordId); }
  }, [queued, pendingSave, acting, saveState]);
  return <Panel title="历史件试测" help="用当前候选（已保存的走廊与参数）从录制原图重测历史件并判定，各段按示教件得出的预期帧数分段。结果另存为对照，不改历史记录，也不改示教件基准">
    <Checkbox label="显示全部历史件（任意配方、任意项目）" checked={all} onCheckedChange={value => { restoreRecord.current = undefined; setAll(value); }} />
    <OperationStatus operation={operation} label="历史件试测" />
    {blocked && <p className="muted hint">{blocked}</p>}
    {partialHint && <p className="muted hint">{partialHint}</p>}
    {error && <p className="c-warn" role="alert">{error}</p>}
    {rows == null ? <p className="muted">正在读取历史件…</p> : !rows.length ? <p className="wp-capture-empty">本配方还没有历史件。</p>
      : <ul className="wp-piece-list" aria-label="历史件试测">{rows.map(row => {
        const label = `${pieceTime(row.ts)} · SN ${row.sn}`;
        const summary = results[row.recordId];
        const result = summary?.cycleId === row.cycleId ? summary : undefined;
        const reason = blocked;
        const foreign = row.recipeId !== recipeId;
        const busy = running === row.recordId || queued === row.recordId;
        return <li key={row.recordId} className="wp-piece" aria-label={label}>
          <div className="wp-piece-head">
            <strong>{label}</strong>
            <Badge tone="neutral">{row.fixture ? "手动配置" : modeLabels[row.mode]}</Badge>
            {row.usage && <Badge tone="info">{row.usage === "teaching" ? "示教件" : "标定件"}</Badge>}
            {foreign && <Badge tone="neutral">{row.recipeId ?? "无配方"}{row.projectId ? ` · ${row.projectId}` : ""}</Badge>}
            {!row.complete && <span title="没保存原图的帧不测，只算检测不完整"><Badge tone="warn">原图不全</Badge></span>}
            <span className="muted">{row.groupCount} 帧组</span>
            {result && <Badge tone={toneOf(result.verdict)}>{verdictLabel[result.verdict]}</Badge>}
            {result?.reason && <span className="muted">{result.reason}</span>}
            {result?.unmeasured && <span title={result.unmeasured}><Badge tone="warn">部分段未测</Badge></span>}
            {result && result.candidateRevision !== data?.workspace.revision && <span className="muted">试测修订 {result.candidateRevision}</span>}
            <div className="wp-actions">
              {result && <Link className="wp-text-link" to={`/history/${row.recordId}?comparison=${encodeURIComponent(result.id)}`}
                ref={node => {
                  if (node && restoreRecord.current === row.recordId) { restoreRecord.current = undefined; node.focus(); }
                }}
                state={{ returnTo: { path: location.pathname + location.search, label: "返回试测", state: {
                  historyTrial: { recipeId, instanceId, all, recordId: row.recordId, results },
                } satisfies HistoryTrialLocationState } } satisfies HistoryDetailState}>看详情</Link>}
              <Button size="sm" aria-label={`触发测试 ${label}`} disabled={!!reason || (running != null && !busy)} title={reason || undefined} onClick={() => trigger(row.recordId)}>{busy ? "测试中…" : result ? "重新测试" : "触发测试"}</Button>
            </div>
          </div>
        </li>;
      })}</ul>}
  </Panel>;
}
