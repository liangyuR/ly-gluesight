import { useState } from "react";
import { Link } from "react-router-dom";
import { Button, Dialog } from "qing-steel";
import type { WorkspacePlanView } from "../plan/types";
import { segmentLabel } from "../plan/vis";
import { workspaceApi } from "./api";
import { corridorPath } from "./cameraTasks";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel } from "./components";
import { pieceTime } from "./pieces";
import { useRecipeHref } from "./recipeRoutes";
import { carryOf, countChanges, foldRows, link, linkable, linksOf, sameLinks, toggleChange, unlink } from "./passAlignment";
import type { PassAlignment, PassChange, PassLink, PassPair } from "./types";

const changeText: Record<PassChange, string> = { same: "不变", changed: "改动", added: "新增", removed: "删除" };
const changeTone = { same: "ok", changed: "warn", added: "info", removed: "ng" } as const;
const kindText = { straight: "直线", corner: "弯道" } as const;

const range = ([a, b]: [number, number]) => a === b ? `${a + 1}` : `${a + 1}–${b + 1}`;

/**
 * 按笔对照（RB-11，分段页工作台上方）：换示教件后列出旧件的笔与新件的笔的对照，确认前可改，每改一次后端从换件前的示教重新迁移；
 * 确认后列出各段的沿用状态。locked 非空时不能改（段列表或分段有未保存的改动）。
 */
export default function PassAlignmentPanel({ alignment, plan, locked }: { alignment: PassAlignment; plan: WorkspacePlanView; locked: string }) {
  const { data, doc, act, busy } = useWorkspace();
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(new Set());
  const [picking, setPicking] = useState<number | null>(null);
  const [confirming, setConfirming] = useState(false);
  if (!data || !doc) return null;
  const revision = data.workspace.revision;
  const frozen = !!data.workspace.pending;
  const { oldPiece, newPasses, rows, confirmed } = alignment;
  const counts = countChanges(rows);
  const disabled = busy || frozen || !!locked;

  const save = async (links: PassLink[], confirm = false) => {
    setPicking(null);
    const next = await act(() => workspaceApi.passAlignmentSave(doc.id, revision, links, confirm),
      confirm ? "逐笔对照已确认" : "逐笔对照已更新，已按新对照重新迁移示教");
    if (confirm && next) setConfirming(false);
    return next;
  };
  const toggleFold = (key: number) => setExpanded(current => {
    const next = new Set(current);
    if (!next.delete(key)) next.add(key);
    return next;
  });

  const summary = <div className="pa-summary" aria-label="按笔对照摘要">
    <span>原示教件 SN <strong className="mono">{oldPiece.sn || "—"}</strong> · {pieceTime(oldPiece.ts)} · {oldPiece.passes.length} 笔 → 新示教件 {newPasses.length} 笔</span>
    <Badge tone="ok">沿用 {counts.same} 笔</Badge>
    <Badge tone={counts.changed ? "warn" : "neutral"}>改动 {counts.changed} 笔</Badge>
    <Badge tone={counts.added ? "info" : "neutral"}>新增 {counts.added} 笔</Badge>
    <Badge tone={counts.removed ? "ng" : "neutral"}>删除 {counts.removed} 笔</Badge>
    {!!alignment.removed?.length && <span className="muted">迁移时去掉的旧段：{alignment.removed.map(item => `原段 ${item.index}（${kindText[item.kind]}）`).join("、")}</span>}
  </div>;

  const table = <AlignmentTable rows={rows} oldPasses={oldPiece.passes} newPasses={newPasses} expanded={expanded} onFold={toggleFold}
    editable={!confirmed} disabled={disabled} picking={picking} onPick={setPicking}
    onToggle={old => void save(toggleChange(rows, old))}
    onUnlink={old => void save(unlink(rows, old))}
    onLink={(old, next) => { const links = link(rows, old, next); if (links) void save(links); }} />;

  if (confirmed) {
    return <Panel title="按笔对照" help="换示教件后旧件的笔与新件的笔的对照已确认；沿用的段在新件上重新选示教帧或确认沿用，改动与新增的段在新件上重新示教，都处理完才能发布">
      {summary}
      <CarryList alignment={alignment} plan={plan} disabled={busy || frozen} revision={revision} docId={doc.id} />
      <details className="pa-details"><summary>查看逐笔对照</summary>{table}</details>
    </Panel>;
  }

  return <Panel title="按笔对照" help="换示教件时按笔（一次门控为一笔）把原示教件的笔对到新示教件的笔，并按对照迁移段、走廊与示教帧。对照确认前可改：切换不变 / 改动、断开一对、把删除的旧笔和新增的新笔连成一对；每改一次都从换件前的示教重新迁移，边界草稿随之更新"
    actions={<>
      {locked && <span className="muted">{locked}</span>}
      <Button disabled={disabled || sameLinks(rows, alignment.suggested)} onClick={() => void save(linksOf(alignment.suggested))}>恢复自动对照</Button>
      <Button variant="filled" disabled={disabled} onClick={() => setConfirming(true)}>确认对照</Button>
    </>}>
    <Notice tone="warn" title="逐笔对照还没确认">确认前不能保存分段、不能改段参数，也不能发布。不变的笔沿用走廊、示教帧与段配置；改动、新增的笔要在新件上重新示教。</Notice>
    {summary}
    {table}
    {confirming && <Dialog open title="确认逐笔对照" onOpenChange={open => { if (!open) setConfirming(false); }}
      footer={<><Button disabled={busy} onClick={() => setConfirming(false)}>取消</Button><Button variant="filled" disabled={busy} onClick={() => void save(linksOf(rows), true)}>确认对照</Button></>}>
      <div className="wp-stack">
        <p>按当前对照确认：沿用 {counts.same} 笔、改动 {counts.changed} 笔、新增 {counts.added} 笔、删除 {counts.removed} 笔。</p>
        <p><strong>确认后对照不能再改。</strong>之后到分段页确认边界；改动与新增的段要在新件上重新示教，沿用的段要在新件上重新选示教帧或确认沿用。</p>
      </div>
    </Dialog>}
  </Panel>;
}

function passText(index: number | undefined, passes: number[]) {
  return index == null ? "—" : `第 ${index + 1} 笔（${passes[index] ?? "?"} 帧）`;
}

function AlignmentTable({ rows, oldPasses, newPasses, expanded, onFold, editable, disabled, picking, onPick, onToggle, onUnlink, onLink }: {
  rows: PassPair[]; oldPasses: number[]; newPasses: number[]; expanded: ReadonlySet<number>; onFold: (key: number) => void;
  editable: boolean; disabled: boolean; picking: number | null; onPick: (old: number | null) => void;
  onToggle: (old: number) => void; onUnlink: (old: number) => void; onLink: (old: number, next: number) => void;
}) {
  const candidates = picking == null ? [] : linkable(rows, picking);
  const actions = (row: PassPair) => {
    if (!editable) return null;
    if (row.old != null && row.new != null) {
      const old = row.old;
      return <>
        <Button size="sm" disabled={disabled} onClick={() => onToggle(old)}>{row.change === "same" ? "改为改动" : "改为不变"}</Button>
        <Button size="sm" disabled={disabled} onClick={() => onUnlink(old)}>断开</Button>
      </>;
    }
    if (row.old != null) {
      const old = row.old;
      if (picking === old) return <Button size="sm" onClick={() => onPick(null)}>取消连接</Button>;
      const none = !linkable(rows, old).length;
      return <Button size="sm" disabled={disabled || none} title={none ? "前后没有能保持顺序的新增笔：先断开相邻的对应" : undefined}
        onClick={() => onPick(old)}>连到新增笔…</Button>;
    }
    if (picking == null || row.new == null) return null;
    const next = row.new;
    const ok = candidates.includes(next);
    return <Button size="sm" variant={ok ? "filled" : undefined} disabled={disabled || !ok} title={ok ? undefined : "会打乱两件笔的先后顺序"}
      onClick={() => onLink(picking, next)}>与旧第 {picking + 1} 笔连成一对</Button>;
  };
  return <>
    {picking != null && <p className="muted" role="status">选一个新增的新笔与旧第 {picking + 1} 笔连成一对：只能选在它前后两对对应之间的新笔，连上后算改动。</p>}
    <div className="pa-scroll">
      <table className="pa-table" aria-label="逐笔对照">
        <thead><tr><th>原示教件</th><th>新示教件</th><th>状态</th><th>图像距离</th>{editable && <th>操作</th>}</tr></thead>
        <tbody>
          {foldRows(rows, expanded).map(line => {
            if (line.kind === "fold") {
              const label = line.old[0] === line.new[0] && line.old[1] === line.new[1]
                ? `第 ${range(line.old)} 笔不变`
                : `原第 ${range(line.old)} 笔 → 新第 ${range(line.new)} 笔不变`;
              return <tr key={`fold-${line.key}`} className="pa-fold">
                <td colSpan={editable ? 4 : 3}>{label}（{line.old[1] - line.old[0] + 1} 笔）</td>
                <td><Button size="sm" variant="text" aria-expanded={line.open} onClick={() => onFold(line.key)}>{line.open ? "收起" : "展开"}</Button></td>
              </tr>;
            }
            const { row } = line;
            return <tr key={`${row.old ?? "-"}:${row.new ?? "-"}`} className={picking != null && picking === row.old ? "pa-picking" : undefined}>
              <td>{passText(row.old, oldPasses)}</td>
              <td>{passText(row.new, newPasses)}</td>
              <td><Badge tone={changeTone[row.change]}>{changeText[row.change]}</Badge></td>
              <td className="mono">{row.distance == null ? "—" : row.distance.toFixed(2)}</td>
              {editable && <td><div className="pa-actions">{actions(row)}</div></td>}
            </tr>;
          })}
        </tbody>
      </table>
    </div>
  </>;
}

function CarryList({ alignment, plan, disabled, revision, docId }: { alignment: PassAlignment; plan: WorkspacePlanView; disabled: boolean; revision: number; docId: string }) {
  const href = useRecipeHref();
  const { act } = useWorkspace();
  const states = plan.segments.map(segment => ({ segment, state: carryOf(alignment, segment, plan.cameras) }));
  const pending = states.filter(item => item.state && !item.state.done).length;
  const accept = (segment: number, accepted: boolean) =>
    act(() => workspaceApi.passCarryAccept(docId, revision, segment, accepted), accepted ? `已确认沿用段 ${segment}` : `已撤销段 ${segment} 的沿用确认`);
  return <>
    <p className={pending ? "pa-pending" : "muted"} role="status">{pending ? `还有 ${pending} 段待处理，处理完才能发布` : "各段都已处理"}</p>
    <ul className="pa-carry" aria-label="各段沿用状态">
      {states.map(({ segment, state }) => {
        const name = `段 ${segment.index} · ${segmentLabel(segment)}`;
        if (!state) return <li key={segment.index}><strong>{name}</strong><span className="muted">不检测，无需处理</span></li>;
        const retaughtAll = !state.missing.length;
        return <li key={segment.index} aria-label={`段 ${segment.index} 沿用状态`}>
          <strong>{name}</strong>
          <Badge tone={state.change === "same" ? "ok" : state.change === "changed" ? "warn" : "info"}>{state.change === "same" ? "沿用" : state.change === "changed" ? "改动" : "新增"}</Badge>
          {state.change === "same"
            ? <>
              <span className={state.done ? "muted" : "pa-pending"}>{retaughtAll ? "已在新件上重新示教" : state.accepted ? "已确认沿用" : "待在新件上重新选示教帧或确认沿用"}</span>
              {!retaughtAll && <Button size="sm" disabled={disabled} onClick={() => void accept(segment.index, !state.accepted)}>{state.accepted ? "撤销" : "确认沿用"}</Button>}
            </>
            : retaughtAll
              ? <span className="muted">已在新件上{state.change === "added" ? "示教" : "重新示教"}</span>
              : <span className="pa-pending">{state.change === "added" ? "待示教" : "待在新件上重新示教"}：{state.missing.map((camera, i) => <span key={camera}>{i > 0 && "、"}<Link to={href(corridorPath(camera, segment.index))}>{camera}</Link></span>)}</span>}
        </li>;
      })}
    </ul>
  </>;
}
