import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "qing-steel";
import { errorText } from "../../lib/errors";
import { verdictLabel, verdictTone } from "../history";
import type { PartMode } from "../history/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Panel } from "./components";
import { pieceTime } from "./pieces";
import type { Comparison, HistoryCandidate } from "./types";
import type { HistoryDetailState } from "../history/types";

const modeLabels: Record<PartMode, string> = { production: "生产", maintenanceCollect: "维护采集", maintenanceShadow: "维护试判" };
const toneOf = (comparison: Comparison) => { const tone = verdictTone(comparison.judgement.verdict); return tone === "fault" ? "ng" : tone; };

/**
 * 历史件试测（试测页）：直接列出本配方的历史件，“触发测试”用当前候选（已保存的走廊与参数）从录制原图重测并判定一件，
 * 结果另存为对照（`workspace_compare`，raw），不改历史记录，也不改示教件基准；详情在历史记录页里看。
 * 还有改动等着自动保存时，点测试先立即保存，存好再按保存后的修订测。
 */
export function HistoryTrials({ recipeId }: { recipeId: string }) {
  const { data, acting, pendingSave, saveNow, saveState } = useWorkspace();
  const [rows, setRows] = useState<HistoryCandidate[] | null>(null);
  const [error, setError] = useState("");
  const [running, setRunning] = useState<number | null>(null);
  const [queued, setQueued] = useState<number | null>(null);
  const [results, setResults] = useState<Record<number, Comparison>>({});
  useEffect(() => {
    let alive = true;
    setRows(null); setResults({});
    workspaceApi.historyCandidates(recipeId).then(r => { if (alive) setRows(r); }).catch(e => { if (alive) { setRows([]); setError(errorText(e)); } });
    return () => { alive = false; };
  }, [recipeId]);
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
      const result = await workspaceApi.compare(data.workspace.doc.id, data.workspace.revision, recordId, true);
      setResults(previous => ({ ...previous, [recordId]: result }));
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
    {blocked && <p className="muted hint">{blocked}</p>}
    {partialHint && <p className="muted hint">{partialHint}</p>}
    {error && <p className="c-warn" role="alert">{error}</p>}
    {rows == null ? <p className="muted">正在读取历史件…</p> : !rows.length ? <p className="wp-capture-empty">本配方还没有历史件。</p>
      : <ul className="wp-piece-list" aria-label="历史件试测">{rows.map(row => {
        const label = `${pieceTime(row.ts)} · SN ${row.sn}`;
        const result = results[row.recordId];
        const reason = !row.complete ? "原图不完整，不能从原图重测" : blocked;
        const busy = running === row.recordId || queued === row.recordId;
        return <li key={row.recordId} className="wp-piece" aria-label={label}>
          <div className="wp-piece-head">
            <strong>{label}</strong>
            <Badge tone="neutral">{row.fixture ? "手动配置" : modeLabels[row.mode]}</Badge>
            {row.usage && <Badge tone="info">{row.usage === "teaching" ? "示教件" : "标定件"}</Badge>}
            <span className="muted">{row.groupCount} 帧组</span>
            {result && <Badge tone={toneOf(result)}>{verdictLabel[result.judgement.verdict]}</Badge>}
            {result?.judgement.reason && <span className="muted">{result.judgement.reason}</span>}
            {!!result?.burst?.unmeasured?.length && <span title={result.burst.unmeasured.map(u => u.reason).join("；")}><Badge tone="warn">部分段未测</Badge></span>}
            <div className="wp-actions">
              {result && <Link className="wp-text-link" to={`/history/${row.recordId}?comparison=${encodeURIComponent(result.id)}`}
                state={{ returnTo: { path: "/recipe/calibration", label: "返回试测" } } satisfies HistoryDetailState}>看详情</Link>}
              <Button size="sm" aria-label={`触发测试 ${label}`} disabled={!!reason || (running != null && !busy)} title={reason || undefined} onClick={() => trigger(row.recordId)}>{busy ? "测试中…" : result ? "重新测试" : "触发测试"}</Button>
            </div>
          </div>
        </li>;
      })}</ul>}
  </Panel>;
}
