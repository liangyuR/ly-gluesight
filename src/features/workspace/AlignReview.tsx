import { useEffect, useState, type KeyboardEvent } from "react";
import { Button } from "qing-steel";
import type { WorkspacePlanView } from "../plan/types";
import { errorText } from "../../lib/errors";
import { workspaceApi } from "./api";
import { useWorkspace } from "./context";
import { Badge, Notice, Panel } from "./components";
import { ignoreKey, stripGroups, Thumb, useThumbs } from "./thumbs";
import { moveCut, segmentCameras } from "./workbenchDraft";
import type { AlignSuggestion, CaptureSummary } from "./types";
import type { ThumbStrip } from "./workbenchDraft";

const statusText = { draft: "草稿", confirmed: "已确认", stale: "需复核" } as const;

/** 缩略图宽度按显示的相机数放大：一台 160 px（缩略图原尺寸），两台 112 px，三台 72 px。 */
const cellWidth = (cameras: number) => (cameras <= 1 ? 160 : cameras === 2 ? 112 : 72);

/** 一侧（模板或本轮）边界两侧各两帧：边界两侧段的检测相机各一行，竖线标在边界处。 */
function BoundaryFrames({ label, strips, cameras, cut }: { label: string; strips: Record<string, ThumbStrip>; cameras: string[]; cut: number }) {
  const groups = [cut - 1, cut, cut + 1, cut + 2];
  const width = cellWidth(cameras.length);
  return <figure className="alr-side" aria-label={`${label}：帧组 ${cut} 之后`} style={{ ["--alr-cell" as string]: `${width}px` }}>
    <figcaption>{label} · {cut} | {cut + 1}</figcaption>
    {cameras.map(camera => {
      const strip = strips[camera];
      return <div key={camera} className="alr-row" title={camera}>
        {groups.map(group => <span key={group} className={`alr-cell${group === cut ? " edge" : ""}`} title={`${camera} 帧组 ${group}`}>
          {group >= 1 && strip && group <= strip.frames.length
            ? <Thumb pixels={strip.frames[group - 1]} width={strip.width} height={strip.height} />
            : <span className="swb-missing">—</span>}
        </span>)}
      </div>;
    })}
  </figure>;
}

/**
 * 其余轮次的对齐复核（D-SB-44 P2，段列表页、分段标定工作台下方）：软件把分段模板的边界按图像对齐到所选轮次，
 * 每个边界一行并排显示模板与本轮边界两侧的帧；← → 微调当前行（不越过相邻边界），↑ ↓ 换行，Enter 确认当前行。
 * 各行都确认后一次保存并确认本轮边界，然后转到下一个未确认的轮次。
 */
export default function AlignReview({ plan, locked }: { plan: WorkspacePlanView; locked: string }) {
  const { data, doc, act, busy } = useWorkspace();
  const [rounds, setRounds] = useState<CaptureSummary[]>([]);
  const [roundId, setRoundId] = useState("");
  const [suggestion, setSuggestion] = useState<{ round: string; value: AlignSuggestion | null; error: string } | null>(null);
  const [cuts, setCuts] = useState<number[]>([]);
  const [checked, setChecked] = useState<boolean[]>([]);
  const [row, setRow] = useState(0);
  const review = data?.workspace.captureReview;
  const template = review?.template ?? null;
  const docId = doc?.id ?? "";
  const frozen = !!data?.workspace.pending;

  useEffect(() => {
    if (!docId) return;
    let alive = true;
    workspaceApi.captureList(docId)
      .then(list => alive && setRounds(list.filter(round => round.state === "complete").sort((a, b) => a.createdAt - b.createdAt)))
      .catch(() => alive && setRounds([]));
    return () => { alive = false; };
  }, [docId]);

  const others = rounds.filter(round => round.roundId !== template?.roundId);
  const status = (id: string) => review?.rounds[id]?.status ?? null;

  useEffect(() => {
    if (!roundId || !docId || !template) return;
    let alive = true;
    setSuggestion(null);
    workspaceApi.captureReviewSuggest(docId, roundId)
      .then(value => {
        if (!alive) return;
        setSuggestion({ round: roundId, value, error: "" });
        const saved = review?.rounds[roundId];
        const start = saved?.status === "confirmed" && saved.cuts.length === value.cuts.length ? saved.cuts : value.cuts.map(item => item.cut);
        setCuts(start);
        setChecked(start.map(() => saved?.status === "confirmed"));
        setRow(0);
      })
      .catch(e => alive && setSuggestion({ round: roundId, value: null, error: errorText(e) }));
    return () => { alive = false; };
  }, [roundId, docId, template?.version]);

  const templateThumbs = useThumbs(template?.roundId ?? "", plan.cameras);
  const roundThumbs = useThumbs(roundId, plan.cameras);
  const groupCount = stripGroups(roundThumbs.strips);

  if (!template) {
    return <Panel title="其余轮次复核" help="把分段模板的边界按图像对齐到其余采集轮次，逐个确认">
      <p className="muted">先在上方分段标定里保存一轮分段并设为模板。</p>
    </Panel>;
  }

  const current = suggestion?.round === roundId ? suggestion : null;
  const value = current?.value ?? null;
  const ready = !!value && !!templateThumbs.strips && !!roundThumbs.strips && cuts.length === value.cuts.length;
  const nudge = (at: number, delta: number) => {
    const next = moveCut({ groupCount, cuts, kinds: [], cameras: [] }, at, cuts[at] + delta);
    if (next.cuts !== cuts) { setCuts(next.cuts); setChecked(list => list.map((item, i) => (i === at ? false : item))); }
  };
  const confirmRow = (at: number) => {
    setChecked(list => list.map((item, i) => (i === at ? true : item)));
    setRow(Math.min(cuts.length - 1, at + 1));
  };
  const allChecked = checked.every(Boolean);
  const blocked = !ready || !allChecked || busy || frozen || !!locked || !doc || !data;
  const save = async () => {
    if (!doc || !data) return;
    const next = await act(() => workspaceApi.captureReviewConfirmCuts(doc.id, data.workspace.revision, roundId, cuts), "本轮边界已确认");
    if (!next) return;
    const nextReview = next.workspace.captureReview;
    const pending = others.find(round => round.roundId !== roundId && nextReview?.rounds[round.roundId]?.status !== "confirmed");
    if (pending) setRoundId(pending.roundId);
  };
  const onKey = (e: KeyboardEvent) => {
    if (!ready || ignoreKey(e)) return;
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") { e.preventDefault(); nudge(row, e.key === "ArrowLeft" ? -1 : 1); }
    else if (e.key === "ArrowUp" || e.key === "ArrowDown") { e.preventDefault(); setRow(Math.min(cuts.length - 1, Math.max(0, row + (e.key === "ArrowUp" ? -1 : 1)))); }
    else if (e.key === "Enter") { e.preventDefault(); confirmRow(row); }
  };

  return <Panel title="其余轮次复核" help="软件按图像把分段模板的边界对齐到所选轮次；逐个边界对照模板与本轮的帧，← → 微调、↑ ↓ 换行、Enter 确认，全部确认后保存本轮">
    <div className="alr-rounds" role="group" aria-label="采集轮次">
      {others.map(round => {
        const state = status(round.roundId);
        return <button key={round.roundId} type="button" aria-pressed={round.roundId === roundId}
          className={`alr-round${round.roundId === roundId ? " selected" : ""}`} onClick={() => setRoundId(round.roundId)}>
          <span className="plan-mono">{round.roundId}</span>
          <Badge tone={state === "confirmed" ? "ok" : state ? "warn" : "neutral"}>{state ? statusText[state] : "未整理"}</Badge>
        </button>;
      })}
    </div>
    {!roundId && <p className="muted">选一轮开始复核（模板轮 {template.roundId} 不在列表中）。</p>}
    {current?.error && <Notice title="无法给出对齐建议" tone="warn">{current.error}</Notice>}
    {roundId && !current && <p className="muted">正在按图像对齐…</p>}
    {ready && <div className="alr-body" tabIndex={0} onKeyDown={onKey} aria-label="边界复核">
      {value.cuts.map((item, at) => {
        const cut = cuts[at];
        // 只看边界两侧段的检测相机（D-SB-41），相机少时缩略图更大
        const shown = plan.cameras.filter(camera => [plan.segments[at], plan.segments[at + 1]].some(segment => segment && segmentCameras(segment, plan.cameras).includes(camera)));
        return <section key={at} className={`alr-boundary${at === row ? " active" : ""}`} aria-label={`段 ${at + 1} → 段 ${at + 2} 的边界`} onClick={() => setRow(at)}>
          <header>
            <strong>段 {at + 1} → 段 {at + 2}</strong>
            <span className="plan-mono muted">{shown.join(" · ")}</span>
            <span className="muted">建议 {item.cut}（按比例 {item.proportional}）</span>
            {item.review && <Badge tone="warn">需重点复核</Badge>}
            {checked[at] && <Badge tone="ok">已确认</Badge>}
          </header>
          <div className="alr-compare">
            <BoundaryFrames label="模板" strips={templateThumbs.strips!} cameras={shown} cut={value.templateCuts[at]} />
            <BoundaryFrames label="本轮" strips={roundThumbs.strips!} cameras={shown} cut={cut} />
          </div>
          <div className="wp-actions">
            <Button size="sm" aria-label={`段 ${at + 1} 边界前移`} onClick={() => nudge(at, -1)}>←</Button>
            <output aria-label={`段 ${at + 1} 边界`} className="plan-mono">{cut}</output>
            <Button size="sm" aria-label={`段 ${at + 1} 边界后移`} onClick={() => nudge(at, 1)}>→</Button>
            <Button size="sm" disabled={checked[at]} onClick={() => confirmRow(at)}>确认此边界</Button>
          </div>
        </section>;
      })}
      {locked && <p className="muted">{locked}</p>}
      <div className="wp-actions">
        <Button variant="filled" disabled={blocked} onClick={() => void save()}>确认本轮边界</Button>
        <span className="muted">已确认 {checked.filter(Boolean).length} / {checked.length}</span>
      </div>
    </div>}
  </Panel>;
}
