import { useEffect, useState } from "react";
import { Accordion, Button } from "qing-steel";
import { errorText } from "../../lib/errors";
import type { SkipSuggestView } from "../plan/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel } from "./components";
import { pieceLabel } from "./pieces";

const endLabel = { head: "段首（起涂）", tail: "段尾（收尾）" } as const;

/**
 * 起涂、收尾跳过帧的建议（D-SB-50，多轮标定页）：第 1 段段首、末段段尾按各标定轮次的每帧特征给出起涂（收尾）帧组数，
 * 建议取最大值；操作员“采用建议”后才写进配方。当前值小于建议值时，正常件的起涂（收尾）帧会判断胶。
 */
export function SkipSuggest({ locked }: { locked: boolean }) {
  const { data, doc, act } = useWorkspace();
  const [views, setViews] = useState<SkipSuggestView[] | null>(null);
  const [error, setError] = useState("");
  const revision = data?.workspace.revision ?? 0;
  useEffect(() => {
    if (!doc) return;
    let alive = true;
    workspaceApi.calibrationSkipSuggest(doc.id)
      .then(list => { if (alive) { setViews(list); setError(""); } })
      .catch(e => { if (alive) setError(errorText(e)); });
    return () => { alive = false; };
  }, [doc?.id, revision]);
  if (!doc || !data) return null;
  const label = (roundId: string) => { const piece = data.plan.pieces.find(item => item.roundId === roundId); return piece ? pieceLabel(piece) : "来源未记录"; };
  const adopt = (view: SkipSuggestView) => {
    if (view.suggested == null) return;
    void act(() => workspaceApi.confirmSkip(doc.id, data.workspace.revision, view.segment, view.end, view.suggested!), `段 ${view.segment} ${endLabel[view.end]}已改为跳过 ${view.suggested} 帧`);
  };
  return <Panel title="起涂与收尾跳过" help="整件开头出胶有延迟、胶条要走满测量区间，末尾同理：这些帧组按本段检测项会判断胶。按各标定件的每帧特征，连续两个帧组都测成且无 NG 才算胶条成形，之前的帧组数取各件最大值作建议">
    {error && <Notice title="无法给出跳过建议" tone="warn">{error}</Notice>}
    {views?.length === 0 && <p className="muted">还没有标定件。</p>}
    {views && views.length > 0 && <ul className="wp-skip-list" aria-label="跳过帧建议">{views.map(view => {
      const short = view.suggested != null && view.current < view.suggested;
      return <li key={`${view.segment}-${view.end}`} className="wp-skip-item">
        <div className="wp-actions">
          <strong>段 {view.segment} · {endLabel[view.end]}</strong>
          <span className="muted">当前跳过 {view.current} 帧</span>
          {view.suggested != null
            ? <Badge tone={short ? "warn" : "ok"}>建议 {view.suggested} 帧</Badge>
            : <span className="muted">{view.pending}</span>}
          {view.min != null && <span className="muted">各件 {view.min} – {view.max}</span>}
          <Button size="sm" aria-label={`段 ${view.segment} ${endLabel[view.end]}采用建议`} disabled={locked || view.suggested == null || view.suggested === view.current} onClick={() => adopt(view)}>采用建议</Button>
        </div>
        {short && <p className="muted hint">当前跳过帧数少于建议：正常件的{view.end === "head" ? "起涂" : "收尾"}帧会判断胶。</p>}
        <Accordion collapsible items={[{ value: "rounds", title: "各件明细", content:
          <ul className="wp-skip-rounds">{view.rounds.map(round => <li key={round.roundId}>
            <span>{label(round.roundId)}</span>：{round.frames != null ? `${round.frames} 帧组` : <span className="c-warn">不参与（{round.note}）</span>}
          </li>)}</ul>
        }]} />
      </li>;
    })}</ul>}
  </Panel>;
}
