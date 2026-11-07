import { useEffect, useState } from "react";
import { Button, Table, type TableColumn } from "qing-steel";
import { errorText } from "../../lib/errors";
import type { PassSkipSuggestView } from "../plan/types";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel } from "./components";

type PassLead = PassSkipSuggestView["passes"][number];

const columns: TableColumn<PassLead>[] = [
  { key: "pass", header: "笔", render: row => `第 ${row.pass} 笔` },
  { key: "firstSeq", header: "起始帧组", render: row => row.firstSeq },
  { key: "framesInPass", header: "帧数", render: row => row.framesInPass },
  { key: "frames", header: "起涂帧组数", render: row => row.frames ?? <span className="c-warn">{row.note}</span> },
];

/**
 * 每笔起涂跳过的建议（D-SB-62，试测页）：按示教件测量的每帧特征，取各笔起涂未成形帧组数的最大值；
 * 操作员“采用建议”后才写进配方，结尾跳过保持不变。
 */
export function PassSkipSuggest({ locked }: { locked: boolean }) {
  const { data, doc, act } = useWorkspace();
  const [view, setView] = useState<PassSkipSuggestView | null>(null);
  const [error, setError] = useState("");
  const revision = data?.workspace.revision ?? 0;
  useEffect(() => {
    if (!doc) return;
    let alive = true;
    workspaceApi.passSkipSuggest(doc.id)
      .then(next => { if (alive) { setView(next); setError(""); } })
      .catch(e => { if (alive) setError(errorText(e)); });
    return () => { alive = false; };
  }, [doc?.id, revision]);
  if (!doc || !data) return null;
  const tail = data.workspace.doc.passSkip?.tail ?? 0;
  const short = view?.suggested != null && view.current < view.suggested;
  const adopt = () => {
    if (view?.suggested == null) return;
    const head = view.suggested;
    void act(() => workspaceApi.confirmPassSkip(doc.id, data.workspace.revision, head, tail), `每笔开头已改为跳过 ${head} 个帧组`);
  };
  return <Panel title="每笔起涂" help="机械臂每次起涂打开、收胶关闭拍照，一笔一次门控；每笔开头胶条还没铺满。按示教件测量的每帧特征，取各笔胶条成形前的帧组数，建议取最大值">
    {error && <Notice title="无法给出每笔起涂建议" tone="warn">{error}</Notice>}
    {view && <>
      <div className="wp-actions">
        <span className="muted">当前每笔开头跳过 {view.current} 个帧组</span>
        {view.suggested != null
          ? <Badge tone={short ? "warn" : "ok"}>建议 {view.suggested} 个帧组</Badge>
          : <span className="muted">{view.pending}</span>}
        <Button size="sm" aria-label="每笔起涂采用建议" disabled={locked || view.suggested == null || view.suggested === view.current} onClick={adopt}>采用建议</Button>
      </div>
      {short && <p className="muted hint">当前跳过少于建议：正常件每笔起涂的帧会判断胶。</p>}
      {view.passes.length > 0 && <Table caption="示教件各笔起涂" columns={columns} rows={view.passes} rowKey={row => String(row.pass)} />}
    </>}
  </Panel>;
}
